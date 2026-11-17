//! Format conteneur `.enigma` : tout un projet (fiches, images, réglages,
//! packs locaux, tableau, corbeille) dans un seul fichier.
//!
//! Format binaire simple, toutes les valeurs en petit-boutiste :
//!
//! ```text
//! magic          8 octets   "ENIGMANB"
//! version        u16        1
//! flags          u16        0 (réservé)
//! nb d'entrées   u32
//! entrée × n :
//!   longueur du chemin  u32
//!   chemin              UTF-8, séparateur « / », relatif, sans « .. »
//!   longueur données    u64
//!   données
//!   crc32               u32  (CRC-32 IEEE du chemin puis des données)
//! ```
//!
//! Chaque entrée porte sa somme de contrôle : un fichier abîmé est refusé
//! avec un message lisible plutôt que de charger des données fausses. Les
//! écritures sont atomiques (fichier temporaire puis renommage).

use std::path::{Path, PathBuf};

use crate::storage::write_atomic;

pub const MAGIC: &[u8; 8] = b"ENIGMANB";
pub const CONTAINER_VERSION: u16 = 1;
/// Extension des fichiers de projet.
pub const EXTENSION: &str = "enigma";
/// Limite de sécurité pour une entrée (évite d'allouer n'importe quoi si le
/// fichier est corrompu).
const MAX_ENTRY_LEN: u64 = 1 << 32;

#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("impossible de {action} {path} : {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("ce fichier n'est pas un projet Enigma Notes")]
    NotAContainer,
    #[error("version {0} du format non prise en charge (mettez l'application à jour)")]
    UnsupportedVersion(u16),
    #[error("fichier tronqué ou incomplet")]
    Truncated,
    #[error("contenu abîmé (somme de contrôle invalide) pour « {0} »")]
    Corrupted(String),
    #[error("chemin interdit dans le projet : « {0} »")]
    BadPath(String),
    #[error("entrée en double : « {0} »")]
    Duplicate(String),
}

fn io_err(action: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> ContainerError {
    let path = path.to_owned();
    move |source| ContainerError::Io {
        action,
        path,
        source,
    }
}

/// Fichier du conteneur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Chemin relatif, séparateur `/`.
    pub path: String,
    pub data: Vec<u8>,
}

/// Vérifie qu'un chemin d'entrée est relatif, sans `..`, sans `\` ni
/// composant vide.
pub fn validate_path(path: &str) -> Result<(), ContainerError> {
    let bad = || ContainerError::BadPath(path.to_owned());
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return Err(bad());
    }
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(bad());
        }
        if part.chars().any(|c| c.is_control()) {
            return Err(bad());
        }
    }
    Ok(())
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// CRC-32 (IEEE 802.3, comme zlib) incrémental.
#[derive(Clone, Copy)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Crc32(0xFFFF_FFFF)
    }
}

impl Crc32 {
    pub fn update(&mut self, bytes: &[u8]) {
        let mut c = self.0;
        for &b in bytes {
            c = CRC_TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
        }
        self.0 = c;
    }

    pub fn finish(self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

fn entry_crc(path: &str, data: &[u8]) -> u32 {
    let mut c = Crc32::default();
    c.update(path.as_bytes());
    c.update(data);
    c.finish()
}

/// Encode des entrées (dans l'ordre donné ; chemins vérifiés).
pub fn encode<'a>(
    entries: impl IntoIterator<Item = (&'a str, &'a [u8])>,
) -> Result<Vec<u8>, ContainerError> {
    let entries: Vec<(&str, &[u8])> = entries.into_iter().collect();
    let total: usize = entries.iter().map(|(p, d)| p.len() + d.len() + 16).sum();
    let mut out = Vec::with_capacity(16 + total);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&CONTAINER_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    let count = u32::try_from(entries.len()).map_err(|_| ContainerError::Truncated)?;
    out.extend_from_slice(&count.to_le_bytes());
    let mut seen = std::collections::HashSet::new();
    for (path, data) in entries {
        validate_path(path)?;
        if !seen.insert(path) {
            return Err(ContainerError::Duplicate(path.to_owned()));
        }
        let plen = u32::try_from(path.len()).map_err(|_| ContainerError::BadPath(path.into()))?;
        out.extend_from_slice(&plen.to_le_bytes());
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(data);
        out.extend_from_slice(&entry_crc(path, data).to_le_bytes());
    }
    Ok(out)
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ContainerError> {
        let end = self.pos.checked_add(n).ok_or(ContainerError::Truncated)?;
        let s = self
            .bytes
            .get(self.pos..end)
            .ok_or(ContainerError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ContainerError> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }
}

/// Lit des entrées depuis des octets.
pub fn decode(bytes: &[u8]) -> Result<Vec<Entry>, ContainerError> {
    let mut r = Reader { bytes, pos: 0 };
    let magic = r.take(8).map_err(|_| ContainerError::NotAContainer)?;
    if magic != MAGIC {
        return Err(ContainerError::NotAContainer);
    }
    let version = u16::from_le_bytes(r.array()?);
    if version == 0 || version > CONTAINER_VERSION {
        return Err(ContainerError::UnsupportedVersion(version));
    }
    let _flags = u16::from_le_bytes(r.array()?);
    let count = u32::from_le_bytes(r.array()?);
    let mut entries = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..count {
        let plen = u32::from_le_bytes(r.array()?) as usize;
        let path_bytes = r.take(plen)?;
        let path = String::from_utf8(path_bytes.to_vec())
            .map_err(|_| ContainerError::BadPath(String::from_utf8_lossy(path_bytes).into()))?;
        validate_path(&path)?;
        let dlen = u64::from_le_bytes(r.array()?);
        if dlen > MAX_ENTRY_LEN {
            return Err(ContainerError::Truncated);
        }
        let data = r.take(usize::try_from(dlen).map_err(|_| ContainerError::Truncated)?)?;
        let crc = u32::from_le_bytes(r.array()?);
        if crc != entry_crc(&path, data) {
            return Err(ContainerError::Corrupted(path));
        }
        if !seen.insert(path.clone()) {
            return Err(ContainerError::Duplicate(path));
        }
        entries.push(Entry {
            path,
            data: data.to_vec(),
        });
    }
    if r.pos != bytes.len() {
        return Err(ContainerError::Corrupted("fin du fichier".to_owned()));
    }
    Ok(entries)
}

/// Lit un fichier conteneur.
pub fn read_file(path: &Path) -> Result<Vec<Entry>, ContainerError> {
    let bytes = std::fs::read(path).map_err(io_err("lire", path))?;
    decode(&bytes)
}

/// Fichier temporaire laissé par une écriture atomique interrompue ?
fn is_temp_name(name: &str) -> bool {
    name.starts_with(".tmp") || name.ends_with(".tmp")
}

/// Tous les fichiers d'un dossier (récursivement, dossiers cachés compris),
/// triés par chemin, prêts à être encodés.
pub fn collect_dir(dir: &Path) -> Result<Vec<Entry>, ContainerError> {
    let mut entries = Vec::new();
    for item in walkdir::WalkDir::new(dir).follow_links(false) {
        let item = item.map_err(|e| ContainerError::Io {
            action: "parcourir",
            path: dir.to_owned(),
            source: e.into(),
        })?;
        if !item.file_type().is_file() {
            continue;
        }
        if is_temp_name(&item.file_name().to_string_lossy()) {
            continue;
        }
        let rel = item
            .path()
            .strip_prefix(dir)
            .map_err(|_| ContainerError::BadPath(item.path().display().to_string()))?;
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let path = parts.join("/");
        validate_path(&path)?;
        let data = std::fs::read(item.path()).map_err(io_err("lire", item.path()))?;
        entries.push(Entry { path, data });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// Encode le contenu d'un dossier.
pub fn encode_dir(dir: &Path) -> Result<Vec<u8>, ContainerError> {
    let entries = collect_dir(dir)?;
    encode(entries.iter().map(|e| (e.path.as_str(), e.data.as_slice())))
}

/// Écrit atomiquement des octets de conteneur dans `file`.
pub fn write_file(file: &Path, bytes: &[u8]) -> Result<(), ContainerError> {
    if let Some(parent) = file.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(io_err("créer", parent))?;
    }
    write_atomic(file, bytes).map_err(io_err("écrire", file))
}

/// Range un dossier entier dans un fichier conteneur.
pub fn pack_dir(dir: &Path, file: &Path) -> Result<(), ContainerError> {
    let bytes = encode_dir(dir)?;
    write_file(file, &bytes)
}

/// Extrait des entrées dans un dossier (créé au besoin).
pub fn extract(entries: &[Entry], dir: &Path) -> Result<(), ContainerError> {
    std::fs::create_dir_all(dir).map_err(io_err("créer", dir))?;
    for e in entries {
        validate_path(&e.path)?;
        let target = e
            .path
            .split('/')
            .fold(dir.to_owned(), |p, part| p.join(part));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(io_err("créer", parent))?;
        }
        std::fs::write(&target, &e.data).map_err(io_err("écrire", &target))?;
    }
    Ok(())
}

/// Extrait un fichier conteneur dans un dossier.
pub fn unpack(file: &Path, dir: &Path) -> Result<(), ContainerError> {
    extract(&read_file(file)?, dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_reference() {
        // Valeur de référence CRC-32 IEEE de « 123456789 ».
        let mut c = Crc32::default();
        c.update(b"123456789");
        assert_eq!(c.finish(), 0xCBF4_3926);
    }

    #[test]
    fn round_trip_bytes() {
        let img: Vec<u8> = (0..=255).collect();
        let bytes = encode([
            ("Salle.md", "# Salle\ncorps".as_bytes()),
            ("assets/plan.png", img.as_slice()),
            ("vide.txt", b"".as_slice()),
        ])
        .unwrap();
        let back = decode(&bytes).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[1].path, "assets/plan.png");
        assert_eq!(back[1].data, img);
        assert!(back[2].data.is_empty());
    }

    #[test]
    fn corruption_and_errors_detected() {
        let mut bytes = encode([("a.md", b"bonjour".as_slice())]).unwrap();
        let n = bytes.len();
        bytes[n - 6] ^= 0xFF; // un octet des données
        assert!(matches!(decode(&bytes), Err(ContainerError::Corrupted(p)) if p == "a.md"));
        let good = encode([("a.md", b"bonjour".as_slice())]).unwrap();
        assert!(matches!(
            decode(&good[..good.len() - 2]),
            Err(ContainerError::Truncated)
        ));
        assert!(matches!(
            decode(b"PK\x03\x04 pas un projet"),
            Err(ContainerError::NotAContainer)
        ));
        let mut future = good.clone();
        future[8] = 9;
        assert!(matches!(
            decode(&future),
            Err(ContainerError::UnsupportedVersion(9))
        ));
        let mut extra = good;
        extra.push(0);
        assert!(decode(&extra).is_err(), "octets en trop");
        let msg = ContainerError::NotAContainer.to_string();
        assert!(msg.contains("Enigma Notes"));
    }

    #[test]
    fn dangerous_paths_refused() {
        for bad in [
            "../x.md",
            "a/../../x",
            "/etc/passwd",
            "C:/x",
            "a\\b",
            "a//b",
            "",
            "./a",
        ] {
            assert!(validate_path(bad).is_err(), "{bad}");
            assert!(encode([(bad, b"x".as_slice())]).is_err(), "{bad}");
        }
        assert!(validate_path(".enigma/board.json").is_ok());
        assert!(validate_path("assets/Plan du manoir.svg").is_ok());
        assert!(matches!(
            encode([("a", b"1".as_slice()), ("a", b"2".as_slice())]),
            Err(ContainerError::Duplicate(_))
        ));
    }

    #[test]
    fn pack_and_unpack_directory() {
        let src = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join(".enigma/packs/p")).unwrap();
        std::fs::create_dir_all(src.path().join("assets")).unwrap();
        std::fs::write(src.path().join("Salle.md"), "x").unwrap();
        std::fs::write(src.path().join(".enigma/project.json"), "{}").unwrap();
        std::fs::write(src.path().join(".enigma/packs/p/pack.json"), "{}").unwrap();
        std::fs::write(src.path().join("assets/a.svg"), "<svg/>").unwrap();
        std::fs::write(src.path().join(".tmpAB12"), "temporaire").unwrap();
        let out = tempfile::tempdir().unwrap();
        let file = out.path().join("Projet.enigma");
        pack_dir(src.path(), &file).unwrap();
        let entries = read_file(&file).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                ".enigma/packs/p/pack.json",
                ".enigma/project.json",
                "Salle.md",
                "assets/a.svg"
            ]
        );
        let dst = out.path().join("extrait");
        unpack(&file, &dst).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("assets/a.svg")).unwrap(),
            "<svg/>"
        );
        // Contenu identique → octets identiques (ordre déterministe).
        assert_eq!(encode_dir(&dst).unwrap(), std::fs::read(&file).unwrap());
    }
}
