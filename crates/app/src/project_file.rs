//! Projet ouvert depuis un fichier `.enigma`.
//!
//! Le conteneur est extrait dans un dossier de travail privé (cache de
//! l'utilisateur), sur lequel `Project` fonctionne comme sur un dossier.
//! Après chaque enregistrement, le dossier est ré-encodé et, s'il a changé,
//! écrit atomiquement dans le `.enigma`. À la fermeture, le dossier de
//! travail est supprimé.
//!
//! Sécurité :
//! - si le `.enigma` a été modifié par ailleurs depuis notre dernière
//!   lecture/écriture, on ne l'écrase pas : les modifications vont dans
//!   « Nom (conflit …).enigma » à côté, qui devient le fichier du projet ;
//! - un dossier de travail laissé par une session interrompue (plantage) et
//!   différent du fichier est sauvé dans « Nom (récupéré …).enigma ».

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use enigma_core::container::{self, ContainerError};

/// Empreinte disque d'un fichier (taille, date de modification).
type Stamp = (u64, Option<SystemTime>);

fn stamp(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()))
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Préfixe du dossier de travail d'un fichier (stable d'une session à
/// l'autre ; suivi de l'identifiant du processus).
fn work_prefix(file: &Path) -> String {
    let canon = file.canonicalize().unwrap_or_else(|_| file.to_owned());
    let mut h = DefaultHasher::new();
    canon.to_string_lossy().to_lowercase().hash(&mut h);
    format!("{:016x}-", h.finish())
}

/// Date UTC lisible pour un nom de fichier (« 2026-11-17 22h31 »).
pub fn file_date(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Conversion jours → date civile (algorithme de H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}h{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// `dir/Nom (suffixe).enigma`, numéroté s'il existe déjà.
fn sibling(file: &Path, suffix: &str) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new("."));
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "projet".to_owned());
    let ext = container::EXTENSION;
    let first = dir.join(format!("{stem} ({suffix}).{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|k| dir.join(format!("{stem} ({suffix} {k}).{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// Résultat d'un ré-enregistrement.
#[derive(Debug, PartialEq, Eq)]
pub enum Commit {
    Unchanged,
    Written,
    /// Le fichier avait changé ailleurs : enregistré sous ce nouveau nom.
    Conflict(PathBuf),
}

/// Fichiers du dossier de travail : chemin relatif → empreinte.
type DirStamps = BTreeMap<String, Stamp>;

/// Empreintes de tous les fichiers d'un dossier (métadonnées seulement).
fn dir_stamps(dir: &Path) -> DirStamps {
    let mut out = BTreeMap::new();
    for item in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if !item.file_type().is_file() {
            continue;
        }
        let name = item.file_name().to_string_lossy();
        if name.starts_with(".tmp") || name.ends_with(".tmp") {
            continue;
        }
        let Ok(rel) = item.path().strip_prefix(dir) else {
            continue;
        };
        let key = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if let Ok(m) = item.metadata() {
            out.insert(key, (m.len(), m.modified().ok()));
        }
    }
    out
}

/// État partagé avec le fil d'écriture.
struct Inner {
    file: PathBuf,
    workdir: PathBuf,
    /// Empreinte du `.enigma` après notre dernière lecture/écriture.
    disk: Option<Stamp>,
    /// Empreintes des fichiers de travail lors du dernier enregistrement.
    saved: DirStamps,
    /// Contenu des fichiers déjà lus (relu seulement s'ils changent).
    cache: BTreeMap<String, (Stamp, Vec<u8>)>,
}

impl Inner {
    fn commit(&mut self) -> Result<Commit, ContainerError> {
        let now = dir_stamps(&self.workdir);
        let disk_now = stamp(&self.file);
        if now == self.saved && disk_now == self.disk {
            return Ok(Commit::Unchanged);
        }
        // Relit seulement les fichiers nouveaux ou modifiés.
        self.cache.retain(|k, _| now.contains_key(k));
        for (path, st) in &now {
            if self.cache.get(path).is_some_and(|(s, _)| s == st) {
                continue;
            }
            let full = path.split('/').fold(self.workdir.clone(), |p, c| p.join(c));
            let data = std::fs::read(&full).map_err(|source| ContainerError::Io {
                action: "lire",
                path: full.clone(),
                source,
            })?;
            self.cache.insert(path.clone(), (*st, data));
        }
        let bytes = container::encode(
            self.cache
                .iter()
                .map(|(p, (_, d))| (p.as_str(), d.as_slice())),
        )?;
        let changed_elsewhere = self.disk.is_some() && disk_now != self.disk;
        let outcome = if changed_elsewhere {
            let target = sibling(
                &self.file,
                &format!("conflit {}", file_date(SystemTime::now())),
            );
            container::write_file(&target, &bytes)?;
            self.file = target.clone();
            Commit::Conflict(target)
        } else {
            container::write_file(&self.file, &bytes)?;
            Commit::Written
        };
        self.disk = stamp(&self.file);
        self.saved = now;
        Ok(outcome)
    }
}

type Job = std::thread::JoinHandle<Result<Commit, String>>;

pub struct ProjectFile {
    inner: Arc<Mutex<Inner>>,
    workdir: PathBuf,
    /// Écriture en cours dans un fil.
    job: Option<Job>,
    /// Un nouvel enregistrement a été demandé pendant l'écriture.
    pending: bool,
    /// Fichiers sauvés d'une session interrompue (à signaler).
    pub recovered: Vec<PathBuf>,
}

fn lock(inner: &Mutex<Inner>) -> Result<std::sync::MutexGuard<'_, Inner>, String> {
    inner
        .lock()
        .map_err(|_| "écriture du projet interrompue".to_owned())
}

impl ProjectFile {
    fn new(file: &Path, workdir: PathBuf, recovered: Vec<PathBuf>) -> ProjectFile {
        let inner = Inner {
            file: file.to_owned(),
            workdir: workdir.clone(),
            disk: stamp(file),
            saved: dir_stamps(&workdir),
            cache: BTreeMap::new(),
        };
        ProjectFile {
            inner: Arc::new(Mutex::new(inner)),
            workdir,
            job: None,
            pending: false,
            recovered,
        }
    }

    /// Ouvre un fichier `.enigma` existant. `work_root` : dossier des
    /// dossiers de travail.
    pub fn open(file: &Path, work_root: &Path) -> Result<ProjectFile, ContainerError> {
        let bytes = std::fs::read(file).map_err(|source| ContainerError::Io {
            action: "lire",
            path: file.to_owned(),
            source,
        })?;
        let entries = container::decode(&bytes)?;
        let recovered = recover_stale(file, work_root, hash_bytes(&bytes));
        let workdir = new_workdir(file, work_root)?;
        container::extract(&entries, &workdir)?;
        Ok(ProjectFile::new(file, workdir, recovered))
    }

    /// Crée un projet dans `file` (écrasé s'il existe : l'appelant a choisi
    /// où créer) ; `init` prépare le dossier de travail (ex. import d'un
    /// ancien dossier) avant le premier enregistrement.
    pub fn create(
        file: &Path,
        work_root: &Path,
        init: impl FnOnce(&Path) -> Result<(), String>,
    ) -> Result<ProjectFile, String> {
        let workdir = new_workdir(file, work_root).map_err(|e| e.to_string())?;
        if let Err(e) = init(&workdir) {
            let _ = std::fs::remove_dir_all(&workdir);
            return Err(e);
        }
        let bytes = container::encode_dir(&workdir).map_err(|e| e.to_string())?;
        container::write_file(file, &bytes).map_err(|e| e.to_string())?;
        Ok(ProjectFile::new(file, workdir, Vec::new()))
    }

    /// Fichier `.enigma` actuel (peut changer après un conflit).
    pub fn file(&self) -> PathBuf {
        match lock(&self.inner) {
            Ok(i) => i.file.clone(),
            Err(_) => self.workdir.clone(),
        }
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Nom affiché (sans extension).
    pub fn name(&self) -> String {
        let file = self.file();
        file.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.display().to_string())
    }

    /// Enregistre maintenant (attend une écriture en cours).
    pub fn commit(&mut self) -> Result<Commit, String> {
        if let Some(job) = self.job.take() {
            let _ = job.join();
        }
        self.pending = false;
        lock(&self.inner)?.commit().map_err(|e| e.to_string())
    }

    /// Demande un enregistrement en arrière-plan (sans bloquer l'interface).
    pub fn request_commit(&mut self) {
        if self.job.is_some() {
            self.pending = true;
            return;
        }
        let inner = Arc::clone(&self.inner);
        self.job = Some(std::thread::spawn(move || {
            lock(&inner)?.commit().map_err(|e| e.to_string())
        }));
    }

    /// Résultat d'un enregistrement en arrière-plan terminé, s'il y en a un.
    pub fn poll(&mut self) -> Option<Result<Commit, String>> {
        if !self.job.as_ref().is_some_and(|j| j.is_finished()) {
            return None;
        }
        let result = self.job.take().map(|j| {
            j.join()
                .unwrap_or_else(|_| Err("écriture du projet interrompue".to_owned()))
        });
        if std::mem::take(&mut self.pending) {
            self.request_commit();
        }
        result
    }

    /// Supprime le dossier de travail (à appeler après le dernier `commit`).
    pub fn close(mut self) {
        if let Some(job) = self.job.take() {
            let _ = job.join();
        }
        if let Err(e) = std::fs::remove_dir_all(&self.workdir) {
            tracing::warn!("dossier de travail {} : {e}", self.workdir.display());
        }
    }
}

fn new_workdir(file: &Path, work_root: &Path) -> Result<PathBuf, ContainerError> {
    let dir = work_root.join(format!("{}{}", work_prefix(file), std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|source| ContainerError::Io {
            action: "vider",
            path: dir.clone(),
            source,
        })?;
    }
    std::fs::create_dir_all(&dir).map_err(|source| ContainerError::Io {
        action: "créer",
        path: dir.clone(),
        source,
    })?;
    Ok(dir)
}

/// Dossiers de travail d'une session précédente pour ce fichier : sauvés
/// à côté s'ils diffèrent du fichier, puis supprimés.
fn recover_stale(file: &Path, work_root: &Path, file_hash: u64) -> Vec<PathBuf> {
    let prefix = work_prefix(file);
    let own = format!("{prefix}{}", std::process::id());
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(work_root) else {
        return out;
    };
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&prefix) || name == own {
            continue;
        }
        let dir = entry.path();
        match container::encode_dir(&dir) {
            Ok(bytes) if hash_bytes(&bytes) != file_hash => {
                let target = sibling(file, &format!("récupéré {}", file_date(SystemTime::now())));
                match container::write_file(&target, &bytes) {
                    Ok(()) => out.push(target),
                    Err(e) => {
                        tracing::warn!("récupération de {} : {e}", dir.display());
                        continue;
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("récupération de {} : {e}", dir.display());
                continue;
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_795_473_060);
        assert_eq!(file_date(t), "2026-11-23 22h31");
        assert_eq!(file_date(SystemTime::UNIX_EPOCH), "1970-01-01 00h00");
    }

    #[test]
    fn create_edit_commit_reopen_close() {
        let d = tempfile::tempdir().unwrap();
        let work = d.path().join("work");
        let file = d.path().join("Enquête.enigma");
        let mut pf = ProjectFile::create(&file, &work, |dir| {
            std::fs::write(dir.join("Salle.md"), "# Salle").map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(pf.name(), "Enquête");
        assert!(file.is_file());
        assert_eq!(pf.commit().unwrap(), Commit::Unchanged);
        std::fs::create_dir_all(pf.workdir().join("assets")).unwrap();
        std::fs::write(pf.workdir().join("assets/a.svg"), "<svg/>").unwrap();
        assert_eq!(pf.commit().unwrap(), Commit::Written);
        let workdir = pf.workdir().to_owned();
        pf.close();
        assert!(!workdir.exists());

        let pf = ProjectFile::open(&file, &work).unwrap();
        assert_eq!(
            std::fs::read_to_string(pf.workdir().join("assets/a.svg")).unwrap(),
            "<svg/>"
        );
        assert!(pf.recovered.is_empty());
        pf.close();
    }

    #[test]
    fn external_change_goes_to_conflict_copy() {
        let d = tempfile::tempdir().unwrap();
        let work = d.path().join("work");
        let file = d.path().join("P.enigma");
        let mut pf = ProjectFile::create(&file, &work, |_| Ok(())).unwrap();
        // Quelqu'un d'autre réécrit le fichier.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let other = container::encode([("Autre.md", b"x".as_slice())]).unwrap();
        container::write_file(&file, &other).unwrap();
        std::fs::write(pf.workdir().join("Mien.md"), "y").unwrap();
        let Commit::Conflict(copy) = pf.commit().unwrap() else {
            panic!("conflit attendu")
        };
        assert!(
            copy.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("P (conflit ")
        );
        assert_eq!(
            std::fs::read(&file).unwrap(),
            other,
            "fichier d'autrui intact"
        );
        let mine = container::read_file(&copy).unwrap();
        assert!(mine.iter().any(|e| e.path == "Mien.md"));
        assert_eq!(pf.file(), copy);
        pf.close();
    }

    #[test]
    fn stale_workdir_is_recovered() {
        let d = tempfile::tempdir().unwrap();
        let work = d.path().join("work");
        let file = d.path().join("P.enigma");
        let pf = ProjectFile::create(&file, &work, |_| Ok(())).unwrap();
        // Simule un plantage : dossier de travail d'un autre processus, modifié.
        let stale = work.join(format!("{}999999", work_prefix(&file)));
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("Perdu.md"), "à sauver").unwrap();
        pf.close();
        let pf = ProjectFile::open(&file, &work).unwrap();
        assert_eq!(pf.recovered.len(), 1);
        let saved = container::read_file(&pf.recovered[0]).unwrap();
        assert!(saved.iter().any(|e| e.path == "Perdu.md"));
        assert!(!stale.exists());
        pf.close();
    }

    #[test]
    fn repack_cost_with_images() {
        // 20 Mo d'images : mesure du coût d'un ré-enregistrement.
        let d = tempfile::tempdir().unwrap();
        let work = d.path().join("work");
        let file = d.path().join("Gros.enigma");
        let mut pf = ProjectFile::create(&file, &work, |dir| {
            std::fs::create_dir_all(dir.join("assets")).map_err(|e| e.to_string())?;
            for i in 0..40 {
                let data = vec![(i % 251) as u8; 512 * 1024];
                std::fs::write(dir.join(format!("assets/img{i}.png")), data)
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        })
        .unwrap();
        std::fs::write(pf.workdir().join("Note.md"), "modifiée").unwrap();
        let t = std::time::Instant::now();
        assert_eq!(pf.commit().unwrap(), Commit::Written);
        eprintln!("ré-enregistrement de 20 Mo : {:?}", t.elapsed());
        let t = std::time::Instant::now();
        assert_eq!(pf.commit().unwrap(), Commit::Unchanged);
        eprintln!("vérification sans changement : {:?}", t.elapsed());
        // En arrière-plan : la demande rend la main tout de suite.
        std::fs::write(pf.workdir().join("Note.md"), "encore").unwrap();
        let t = std::time::Instant::now();
        pf.request_commit();
        assert!(t.elapsed() < std::time::Duration::from_millis(20));
        let result = loop {
            if let Some(r) = pf.poll() {
                break r;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        assert_eq!(result.unwrap(), Commit::Written);
        let entries = container::read_file(&pf.file()).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.path == "Note.md" && e.data == b"encore")
        );
        pf.close();
    }
}
