//! Historique des versions d'une fiche, au-delà de l'annulation : à chaque
//! enregistrement, une copie du fichier est gardée dans
//! `.enigma/versions/<identifiant>/<secondes>.md`, au plus une toutes les
//! [`MIN_INTERVAL`] secondes et [`MAX_PER_NOTE`] par fiche (les plus
//! anciennes sont effacées). Comparaison ligne à ligne pour l'affichage.

use std::path::{Path, PathBuf};

use crate::NoteId;
use crate::project::META_DIR;
use crate::storage::write_atomic;

pub const VERSIONS_DIR: &str = "versions";
/// Écart minimal entre deux versions gardées (secondes).
pub const MIN_INTERVAL: u64 = 600;
pub const MAX_PER_NOTE: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub path: PathBuf,
    /// Secondes depuis 1970 (UTC).
    pub time: u64,
    pub size: u64,
}

fn dir_of(root: &Path, id: &NoteId) -> PathBuf {
    let safe: String = id
        .as_str()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    root.join(META_DIR).join(VERSIONS_DIR).join(safe)
}

/// Versions gardées d'une fiche, de la plus récente à la plus ancienne.
pub fn list(root: &Path, id: &NoteId) -> Vec<Version> {
    let Ok(entries) = std::fs::read_dir(dir_of(root, id)) else {
        return Vec::new();
    };
    let mut out: Vec<Version> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let time = path.file_stem()?.to_str()?.parse().ok()?;
            let size = e.metadata().ok()?.len();
            (path.extension()? == "md").then_some(Version { path, time, size })
        })
        .collect();
    out.sort_by_key(|v| std::cmp::Reverse(v.time));
    out
}

pub fn read(v: &Version) -> std::io::Result<String> {
    std::fs::read_to_string(&v.path)
}

/// Garde une copie de `content` si la dernière version est assez ancienne
/// et différente. Rend `true` si une version a été écrite.
pub fn snapshot(root: &Path, id: &NoteId, content: &str, now: u64) -> std::io::Result<bool> {
    let versions = list(root, id);
    if let Some(last) = versions.first() {
        if now.saturating_sub(last.time) < MIN_INTERVAL {
            return Ok(false);
        }
        if read(last).is_ok_and(|t| t == content) {
            return Ok(false);
        }
    }
    let dir = dir_of(root, id);
    std::fs::create_dir_all(&dir)?;
    write_atomic(&dir.join(format!("{now}.md")), content.as_bytes())?;
    for old in versions.iter().skip(MAX_PER_NOTE - 1) {
        let _ = std::fs::remove_file(&old.path);
    }
    Ok(true)
}

/// Ligne d'une comparaison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Diff<'a> {
    Same(&'a str),
    /// Seulement dans le texte d'arrivée.
    Added(&'a str),
    /// Seulement dans le texte de départ.
    Removed(&'a str),
}

/// Comparaison ligne à ligne (plus longue sous-suite commune). Au-delà de
/// 3000 lignes, tout est montré comme remplacé.
pub fn line_diff<'a>(from: &'a str, to: &'a str) -> Vec<Diff<'a>> {
    let a: Vec<&str> = from.lines().collect();
    let b: Vec<&str> = to.lines().collect();
    if a.len() * b.len() > 9_000_000 {
        return a
            .iter()
            .map(|l| Diff::Removed(l))
            .chain(b.iter().map(|l| Diff::Added(l)))
            .collect();
    }
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[at(i, j)] = if a[i] == b[j] {
                lcs[at(i + 1, j + 1)] + 1
            } else {
                lcs[at(i + 1, j)].max(lcs[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(n.max(m));
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(Diff::Same(a[i]));
            i += 1;
            j += 1;
        } else if lcs[at(i + 1, j)] >= lcs[at(i, j + 1)] {
            out.push(Diff::Removed(a[i]));
            i += 1;
        } else {
            out.push(Diff::Added(b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|l| Diff::Removed(l)));
    out.extend(b[j..].iter().map(|l| Diff::Added(l)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_are_spaced_and_pruned() {
        let d = tempfile::tempdir().unwrap();
        let id = NoteId::parse("01ABC").unwrap();
        assert!(snapshot(d.path(), &id, "v1", 1000).unwrap());
        assert!(!snapshot(d.path(), &id, "v2", 1100).unwrap(), "trop tôt");
        assert!(!snapshot(d.path(), &id, "v1", 5000).unwrap(), "identique");
        assert!(snapshot(d.path(), &id, "v2", 5000).unwrap());
        let l = list(d.path(), &id);
        assert_eq!(l.iter().map(|v| v.time).collect::<Vec<_>>(), [5000, 1000]);
        assert_eq!(read(&l[0]).unwrap(), "v2");
        for k in 0..60 {
            snapshot(d.path(), &id, &format!("x{k}"), 10_000 + k * 1000).unwrap();
        }
        assert_eq!(list(d.path(), &id).len(), MAX_PER_NOTE);
    }

    #[test]
    fn diffs_lines() {
        let d = line_diff("a\nb\nc", "a\nc\nd");
        assert_eq!(
            d,
            [
                Diff::Same("a"),
                Diff::Removed("b"),
                Diff::Same("c"),
                Diff::Added("d")
            ]
        );
    }
}
