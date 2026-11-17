//! Accès disque : écriture atomique et noms de fichiers sûrs sur tous les OS.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Écrit `bytes` dans `path` de façon atomique : fichier temporaire dans le
/// même dossier, synchronisé sur disque, puis renommé par-dessus la cible.
/// En cas de plantage, l'ancien fichier reste intact.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".enigma-")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Noms réservés par Windows (insensibles à la casse).
const WINDOWS_RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

const MAX_STEM_CHARS: usize = 80;

/// Nom de fichier (sans extension) dérivé d'un titre, valable sur tous les OS.
/// Les lettres accentuées sont conservées.
pub fn file_stem_for_title(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            let bad =
                c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*');
            if bad { '-' } else { c }
        })
        .collect();
    let mut stem: String = cleaned
        .trim()
        .trim_matches('.')
        .trim()
        .chars()
        .take(MAX_STEM_CHARS)
        .collect();
    stem = stem.trim_end_matches(['.', ' ']).to_owned();
    if stem.is_empty() {
        stem = "fiche".to_owned();
    }
    if WINDOWS_RESERVED.contains(&stem.to_lowercase().as_str()) {
        stem.push('_');
    }
    stem
}

/// Premier chemin `dir/stem.md`, `dir/stem 2.md`… pour lequel `taken` est faux.
pub fn unique_note_path(dir: &Path, stem: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(format!("{stem}.md"));
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|i| dir.join(format!("{stem} {i}.md")))
        .find(|p| !taken(p))
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_creates_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        write_atomic(&p, b"un").unwrap();
        write_atomic(&p, b"deux").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "deux");
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn stems_are_safe() {
        assert_eq!(
            file_stem_for_title("Salle des Archives"),
            "Salle des Archives"
        );
        assert_eq!(file_stem_for_title("Pièce: 5/9 ?"), "Pièce- 5-9 -");
        assert_eq!(file_stem_for_title("  ..  "), "fiche");
        assert_eq!(file_stem_for_title("CON"), "CON_");
        assert_eq!(file_stem_for_title("fin."), "fin");
        assert_eq!(
            file_stem_for_title(&"x".repeat(200)).chars().count(),
            MAX_STEM_CHARS
        );
    }

    #[test]
    fn unique_path_appends_number() {
        let dir = Path::new("d");
        let taken = |p: &Path| p == dir.join("a.md") || p == dir.join("a 2.md");
        assert_eq!(unique_note_path(dir, "a", taken), dir.join("a 3.md"));
        assert_eq!(unique_note_path(dir, "b", taken), dir.join("b.md"));
    }
}
