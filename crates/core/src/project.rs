//! Un projet : un dossier de fiches `.md` chargé en mémoire, son pack de jeu
//! et ses index (chemins, titres).
//!
//! Le projet est la seule voie d'écriture sur le disque. Il retient pour chaque
//! fiche si elle a des modifications non enregistrées (`dirty`) et l'empreinte
//! du dernier contenu lu ou écrit, ce qui permet de reconnaître l'écho de ses
//! propres écritures parmi les événements de surveillance des fichiers.

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::id::NoteId;
use crate::markdown;
use crate::note::{Note, NoteMeta, NoteSerializeError};
use crate::pack::{DiscoveredPack, Pack, discover_packs};
use crate::storage::{file_stem_for_title, unique_note_path, write_atomic};

/// Dossier des données internes d'un projet (ignoré lors du parcours des fiches).
pub const META_DIR: &str = ".enigma";
/// Fichier de configuration du projet, dans [`META_DIR`].
pub const CONFIG_FILE: &str = "project.json";
/// Corbeille, dans [`META_DIR`] : les fiches supprimées y sont déplacées.
pub const TRASH_DIR: &str = "trash";
/// Packs propres au projet, dans [`META_DIR`] (prioritaires).
pub const LOCAL_PACKS_DIR: &str = "packs";
/// Version du format de `project.json`.
pub const CONFIG_VERSION: u32 = 1;

/// Contenu de `.enigma/project.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub version: u32,
    /// Identifiant du pack de jeu utilisé.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            pack: None,
        }
    }
}

/// Erreur d'une opération sur le projet.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("« {0} » n'est pas un dossier")]
    NotADirectory(PathBuf),
    #[error("impossible de {action} {path} : {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("titre invalide : {0}")]
    InvalidTitle(String),
    #[error("une fiche s'appelle déjà « {0} »")]
    TitleTaken(String),
    #[error("fiche introuvable")]
    UnknownNote,
    #[error(transparent)]
    Serialize(#[from] NoteSerializeError),
    #[error("configuration du projet : {0}")]
    Config(String),
}

fn io_err(action: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> ProjectError {
    let path = path.to_owned();
    move |source| ProjectError::Io {
        action,
        path,
        source,
    }
}

/// État d'une fiche chargée.
#[derive(Clone, Debug)]
pub struct NoteEntry {
    pub note: Note,
    pub path: PathBuf,
    title_lower: String,
    dirty: bool,
    disk_hash: Option<u64>,
    /// Version du disque modifiée en externe alors que la fiche avait des
    /// modifications non enregistrées.
    conflict: Option<(Note, u64)>,
}

impl NoteEntry {
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn has_conflict(&self) -> bool {
        self.conflict.is_some()
    }
}

/// Résultat du traitement d'un changement de fichier signalé par la surveillance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChange {
    /// Rien à faire (écho de nos écritures, fichier hors sujet…).
    Ignored,
    /// Nouvelle fiche apparue.
    Added(NoteId),
    /// Fiche rechargée depuis le disque (elle n'avait pas de modification en cours).
    Reloaded(NoteId),
    /// Fiche modifiée sur le disque **et** en mémoire : à arbitrer avec
    /// [`Project::resolve_conflict`].
    Conflict(NoteId),
    /// Fichier supprimé : la fiche est retirée du projet.
    Removed(NoteId),
    /// Fichier supprimé alors que la fiche avait des modifications : elle est
    /// conservée et sera réécrite à la prochaine sauvegarde.
    RemovedWhileDirty(NoteId),
    /// Le fichier n'a pas pu être lu ; voir [`Project::issues`].
    Unreadable(PathBuf),
}

/// Un dossier de fiches chargé en mémoire.
#[derive(Debug)]
pub struct Project {
    root: PathBuf,
    config: ProjectConfig,
    pack_roots: Vec<PathBuf>,
    pack: Pack,
    pack_issue: Option<String>,
    notes: HashMap<NoteId, NoteEntry>,
    by_path: HashMap<PathBuf, NoteId>,
    titles: HashMap<String, Vec<NoteId>>,
    issues: BTreeMap<PathBuf, String>,
    generation: u64,
}

impl Project {
    /// Ouvre un dossier de projet. `pack_roots` : dossiers où chercher les
    /// packs de jeu (après `.enigma/packs` du projet).
    ///
    /// Les fichiers illisibles n'empêchent pas l'ouverture : ils sont listés
    /// dans [`Project::issues`] et ne sont jamais réécrits.
    pub fn open(root: &Path, pack_roots: &[PathBuf]) -> Result<Project, ProjectError> {
        if !root.is_dir() {
            return Err(ProjectError::NotADirectory(root.to_owned()));
        }
        let mut project = Project {
            root: root.to_owned(),
            config: ProjectConfig::default(),
            pack_roots: pack_roots.to_vec(),
            pack: Pack::empty(),
            pack_issue: None,
            notes: HashMap::new(),
            by_path: HashMap::new(),
            titles: HashMap::new(),
            issues: BTreeMap::new(),
            generation: 0,
        };
        project.load_config();
        project.reload_pack();

        let walker = walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !is_hidden_name(e.file_name()));
        let mut paths: Vec<PathBuf> = walker
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && is_markdown(e.path()))
            .map(|e| e.into_path())
            .collect();
        paths.sort();
        for path in paths {
            project.load_file(&path);
        }
        Ok(project)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> &ProjectConfig {
        &self.config
    }

    pub fn pack(&self) -> &Pack {
        &self.pack
    }

    /// Problème de chargement du pack, s'il y en a un.
    pub fn pack_issue(&self) -> Option<&str> {
        self.pack_issue.as_deref()
    }

    /// Fichiers qui n'ont pas pu être lus, avec la raison.
    pub fn issues(&self) -> &BTreeMap<PathBuf, String> {
        &self.issues
    }

    /// Compteur incrémenté à chaque changement de structure (ajout,
    /// suppression, renommage) : sert à invalider les caches de l'interface.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = &NoteId> {
        self.notes.keys()
    }

    pub fn note(&self, id: &NoteId) -> Option<&Note> {
        self.notes.get(id).map(|e| &e.note)
    }

    pub fn entry(&self, id: &NoteId) -> Option<&NoteEntry> {
        self.notes.get(id)
    }

    pub fn id_for_path(&self, path: &Path) -> Option<&NoteId> {
        self.by_path.get(path)
    }

    /// Fiche portant ce titre (insensible à la casse).
    pub fn resolve_title(&self, title: &str) -> Option<&NoteId> {
        self.titles
            .get(&title.trim().to_lowercase())
            .and_then(|ids| ids.first())
    }

    /// Fiches dont le titre contient `query` (insensible à la casse), celles
    /// qui commencent par `query` d'abord, puis par ordre alphabétique.
    pub fn search(&self, query: &str) -> Vec<NoteId> {
        let q = query.trim().to_lowercase();
        let mut hits: Vec<(bool, &str, &NoteId)> = self
            .notes
            .iter()
            .filter(|(_, e)| q.is_empty() || e.title_lower.contains(&q))
            .map(|(id, e)| (!e.title_lower.starts_with(&q), e.title_lower.as_str(), id))
            .collect();
        hits.sort();
        hits.into_iter().map(|(_, _, id)| id.clone()).collect()
    }

    /// Le titre est-il pris par une autre fiche que `except` ?
    pub fn is_title_taken(&self, title: &str, except: Option<&NoteId>) -> bool {
        self.titles
            .get(&title.trim().to_lowercase())
            .is_some_and(|ids| ids.iter().any(|id| Some(id) != except))
    }

    /// Vérifie et nettoie un titre. Les crochets et `|` sont interdits car ils
    /// casseraient la syntaxe `[[Titre|type]]`.
    pub fn validate_title(title: &str) -> Result<String, ProjectError> {
        let t = title.trim();
        if t.is_empty() {
            return Err(ProjectError::InvalidTitle("le titre est vide".into()));
        }
        if let Some(c) = t
            .chars()
            .find(|c| matches!(c, '[' | ']' | '|') || c.is_control())
        {
            let shown = if c.is_control() {
                "retour à la ligne".to_owned()
            } else {
                format!("« {c} »")
            };
            return Err(ProjectError::InvalidTitle(format!(
                "le caractère {shown} n'est pas autorisé"
            )));
        }
        Ok(t.to_owned())
    }

    /// Crée une fiche et l'écrit immédiatement sur le disque.
    pub fn create_note(
        &mut self,
        title: &str,
        categories: Vec<String>,
        body: String,
    ) -> Result<NoteId, ProjectError> {
        let title = Self::validate_title(title)?;
        if self.is_title_taken(&title, None) {
            return Err(ProjectError::TitleTaken(title));
        }
        let mut note = Note::new(title.clone());
        note.meta.categories = categories;
        note.body = body;
        let path = unique_note_path(&self.root, &file_stem_for_title(&title), |p| {
            self.by_path.contains_key(p) || p.exists()
        });
        let id = note.meta.id.clone();
        let bytes = note.serialize()?.into_bytes();
        write_atomic(&path, &bytes).map_err(io_err("écrire", &path))?;
        self.insert(note, path, false, Some(hash_bytes(&bytes)));
        self.generation += 1;
        Ok(id)
    }

    /// Accès en écriture au corps d'une fiche. Appeler [`Project::mark_dirty`]
    /// après une modification.
    pub fn body_mut(&mut self, id: &NoteId) -> Option<&mut String> {
        self.notes.get_mut(id).map(|e| &mut e.note.body)
    }

    /// Signale une modification non enregistrée.
    pub fn mark_dirty(&mut self, id: &NoteId) {
        if let Some(e) = self.notes.get_mut(id) {
            e.dirty = true;
        }
    }

    /// Remplace les métadonnées d'une fiche. L'identifiant et le titre sont
    /// conservés (le titre ne change que via [`Project::rename`]).
    pub fn set_meta(&mut self, id: &NoteId, mut meta: NoteMeta) -> Result<(), ProjectError> {
        let e = self.notes.get_mut(id).ok_or(ProjectError::UnknownNote)?;
        meta.id = e.note.meta.id.clone();
        meta.title = e.note.meta.title.clone();
        if e.note.meta != meta {
            e.note.meta = meta;
            e.dirty = true;
        }
        Ok(())
    }

    /// Remplace le corps d'une fiche.
    pub fn set_body(&mut self, id: &NoteId, body: String) -> Result<(), ProjectError> {
        let e = self.notes.get_mut(id).ok_or(ProjectError::UnknownNote)?;
        if e.note.body != body {
            e.note.body = body;
            e.dirty = true;
        }
        Ok(())
    }

    /// Écrit une fiche sur le disque (atomiquement), qu'elle soit modifiée ou non.
    pub fn save(&mut self, id: &NoteId) -> Result<(), ProjectError> {
        let e = self.notes.get_mut(id).ok_or(ProjectError::UnknownNote)?;
        let bytes = e.note.serialize()?.into_bytes();
        write_atomic(&e.path, &bytes).map_err(io_err("écrire", &e.path))?;
        e.disk_hash = Some(hash_bytes(&bytes));
        e.dirty = false;
        e.note.had_frontmatter = true;
        Ok(())
    }

    /// Fiches ayant des modifications non enregistrées.
    pub fn dirty_ids(&self) -> Vec<NoteId> {
        self.notes
            .iter()
            .filter(|(_, e)| e.dirty)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Enregistre toutes les fiches modifiées, sauf celles en conflit (il faut
    /// d'abord arbitrer). Renvoie les erreurs rencontrées.
    pub fn save_dirty(&mut self) -> Vec<(NoteId, ProjectError)> {
        let mut errors = Vec::new();
        for id in self.dirty_ids() {
            if self.notes.get(&id).is_some_and(|e| e.conflict.is_some()) {
                continue;
            }
            if let Err(e) = self.save(&id) {
                errors.push((id, e));
            }
        }
        errors
    }

    /// Renomme une fiche : titre, nom du fichier, et toutes les références
    /// `[[Ancien titre]]` des autres fiches. Renvoie les fiches réécrites.
    pub fn rename(&mut self, id: &NoteId, new_title: &str) -> Result<Vec<NoteId>, ProjectError> {
        let new_title = Self::validate_title(new_title)?;
        let entry = self.notes.get(id).ok_or(ProjectError::UnknownNote)?;
        let old_title = entry.note.meta.title.clone();
        if old_title == new_title {
            return Ok(Vec::new());
        }
        if self.is_title_taken(&new_title, Some(id)) {
            return Err(ProjectError::TitleTaken(new_title));
        }
        // Réécrire les références seulement si l'ancien titre désignait
        // sans ambiguïté cette fiche.
        let old_unique = self
            .titles
            .get(&old_title.to_lowercase())
            .is_some_and(|ids| ids.len() == 1);

        // 1. Nouveau titre, écrit dans le fichier actuel.
        self.unindex_title(id);
        if let Some(e) = self.notes.get_mut(id) {
            e.note.meta.title = new_title.clone();
            e.title_lower = new_title.to_lowercase();
        }
        self.index_title(id);
        self.save(id)?;

        // 2. Nouveau nom de fichier.
        let old_path = self
            .notes
            .get(id)
            .map(|e| e.path.clone())
            .ok_or(ProjectError::UnknownNote)?;
        let dir = old_path.parent().unwrap_or(&self.root).to_owned();
        let new_stem = file_stem_for_title(&new_title);
        let old_stem = old_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if new_stem != old_stem {
            let new_path = if new_stem.to_lowercase() == old_stem.to_lowercase() {
                dir.join(format!("{new_stem}.md"))
            } else {
                unique_note_path(&dir, &new_stem, |p| {
                    p != old_path && (self.by_path.contains_key(p) || p.exists())
                })
            };
            std::fs::rename(&old_path, &new_path).map_err(io_err("renommer", &old_path))?;
            self.by_path.remove(&old_path);
            self.by_path.insert(new_path.clone(), id.clone());
            if let Some(e) = self.notes.get_mut(id) {
                e.path = new_path;
            }
        }
        self.generation += 1;

        // 3. Références dans les corps.
        let mut updated = Vec::new();
        if old_unique {
            let ids: Vec<NoteId> = self.notes.keys().cloned().collect();
            for other in ids {
                let Some(e) = self.notes.get_mut(&other) else {
                    continue;
                };
                if let Some(body) =
                    markdown::rewrite_wiki_targets(&e.note.body, &old_title, &new_title)
                {
                    e.note.body = body;
                    e.dirty = true;
                    self.save(&other)?;
                    updated.push(other);
                }
            }
        }
        Ok(updated)
    }

    /// Supprime une fiche en déplaçant son fichier dans `.enigma/trash/`.
    /// Renvoie le chemin dans la corbeille.
    pub fn delete(&mut self, id: &NoteId) -> Result<PathBuf, ProjectError> {
        let path = self
            .notes
            .get(id)
            .map(|e| e.path.clone())
            .ok_or(ProjectError::UnknownNote)?;
        let trash = self.root.join(META_DIR).join(TRASH_DIR);
        std::fs::create_dir_all(&trash).map_err(io_err("créer", &trash))?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "fiche".to_owned());
        let target = unique_note_path(&trash, &stem, Path::exists);
        if path.exists() {
            std::fs::rename(&path, &target).map_err(io_err("déplacer", &path))?;
        }
        self.remove_entry(id);
        self.generation += 1;
        Ok(target)
    }

    /// Traite un changement signalé sur `path` par la surveillance des fichiers.
    pub fn on_file_changed(&mut self, path: &Path) -> FileChange {
        if !self.is_note_path(path) {
            return FileChange::Ignored;
        }
        let known = self.by_path.get(path).cloned();
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.issues.remove(path);
                let Some(id) = known else {
                    return FileChange::Ignored;
                };
                if self.notes.get(&id).is_some_and(|e| e.dirty) {
                    if let Some(e) = self.notes.get_mut(&id) {
                        e.disk_hash = None;
                    }
                    return FileChange::RemovedWhileDirty(id);
                }
                self.remove_entry(&id);
                self.generation += 1;
                return FileChange::Removed(id);
            }
            Err(e) => {
                self.issues
                    .insert(path.to_owned(), format!("lecture impossible : {e}"));
                return FileChange::Unreadable(path.to_owned());
            }
        };
        let hash = hash_bytes(&bytes);

        let Some(id) = known else {
            return match self.load_bytes(path, &bytes) {
                Some(id) => {
                    self.generation += 1;
                    FileChange::Added(id)
                }
                None => FileChange::Unreadable(path.to_owned()),
            };
        };

        let Some(entry) = self.notes.get(&id) else {
            return FileChange::Ignored;
        };
        if entry.disk_hash == Some(hash) {
            return FileChange::Ignored;
        }
        let parsed = match parse_bytes(path, &bytes) {
            Ok(n) => n,
            Err(msg) => {
                self.issues.insert(path.to_owned(), msg);
                return FileChange::Unreadable(path.to_owned());
            }
        };
        self.issues.remove(path);

        if parsed.meta.id != id {
            // L'identifiant a changé en externe : c'est une autre fiche.
            self.remove_entry(&id);
            self.generation += 1;
            return match self.load_bytes(path, &bytes) {
                Some(new_id) => FileChange::Added(new_id),
                None => FileChange::Unreadable(path.to_owned()),
            };
        }

        if entry.dirty {
            if let Some(e) = self.notes.get_mut(&id) {
                e.conflict = Some((parsed, hash));
            }
            return FileChange::Conflict(id);
        }

        let title_changed = entry.note.meta.title != parsed.meta.title;
        if title_changed {
            self.unindex_title(&id);
        }
        if let Some(e) = self.notes.get_mut(&id) {
            e.title_lower = parsed.meta.title.to_lowercase();
            e.note = parsed;
            e.disk_hash = Some(hash);
        }
        if title_changed {
            self.index_title(&id);
            self.generation += 1;
        }
        FileChange::Reloaded(id)
    }

    /// Arbitre un conflit : `keep_mine` réécrit la version en mémoire sur le
    /// disque ; sinon la version du disque remplace la version en mémoire.
    pub fn resolve_conflict(&mut self, id: &NoteId, keep_mine: bool) -> Result<(), ProjectError> {
        let e = self.notes.get_mut(id).ok_or(ProjectError::UnknownNote)?;
        let Some((theirs, hash)) = e.conflict.take() else {
            return Ok(());
        };
        if keep_mine {
            return self.save(id);
        }
        self.unindex_title(id);
        if let Some(e) = self.notes.get_mut(id) {
            e.title_lower = theirs.meta.title.to_lowercase();
            e.note = theirs;
            e.disk_hash = Some(hash);
            e.dirty = false;
        }
        self.index_title(id);
        self.generation += 1;
        Ok(())
    }

    /// Packs disponibles : ceux du projet (`.enigma/packs`) puis ceux de
    /// `pack_roots`.
    pub fn available_packs(&self) -> Vec<DiscoveredPack> {
        discover_packs(&self.all_pack_roots())
    }

    /// Change le pack du projet et l'enregistre dans `.enigma/project.json`.
    pub fn set_pack(&mut self, pack_id: Option<String>) -> Result<(), ProjectError> {
        let mut config = self.config.clone();
        config.pack = pack_id;
        let path = self.root.join(META_DIR).join(CONFIG_FILE);
        let text = serde_json::to_string_pretty(&config)
            .map_err(|e| ProjectError::Config(e.to_string()))?;
        write_atomic(&path, text.as_bytes()).map_err(io_err("écrire", &path))?;
        self.config = config;
        self.reload_pack();
        Ok(())
    }

    // --- interne ---

    fn all_pack_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.root.join(META_DIR).join(LOCAL_PACKS_DIR)];
        roots.extend(self.pack_roots.iter().cloned());
        roots
    }

    fn load_config(&mut self) {
        let path = self.root.join(META_DIR).join(CONFIG_FILE);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        match serde_json::from_str::<ProjectConfig>(&text) {
            Ok(c) if c.version == CONFIG_VERSION => self.config = c,
            Ok(c) => {
                self.issues.insert(
                    path,
                    format!(
                        "version {} non prise en charge (attendu {CONFIG_VERSION})",
                        c.version
                    ),
                );
            }
            Err(e) => {
                self.issues.insert(path, format!("JSON invalide : {e}"));
            }
        }
    }

    fn reload_pack(&mut self) {
        self.pack = Pack::empty();
        self.pack_issue = None;
        self.generation += 1;
        let Some(wanted) = self.config.pack.clone() else {
            return;
        };
        let mut failure = None;
        for found in self.available_packs() {
            match found.result {
                Ok(p) if p.id == wanted => {
                    self.pack = p;
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    if found.dir.file_name().is_some_and(|n| n == wanted.as_str()) {
                        failure = Some(e.to_string());
                    }
                }
            }
        }
        self.pack_issue = Some(failure.unwrap_or_else(|| format!("pack « {wanted} » introuvable")));
    }

    fn is_note_path(&self, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return false;
        };
        is_markdown(path) && !rel.components().any(|c| is_hidden_name(c.as_os_str()))
    }

    fn load_file(&mut self, path: &Path) -> Option<NoteId> {
        match std::fs::read(path) {
            Ok(bytes) => self.load_bytes(path, &bytes),
            Err(e) => {
                self.issues
                    .insert(path.to_owned(), format!("lecture impossible : {e}"));
                None
            }
        }
    }

    fn load_bytes(&mut self, path: &Path, bytes: &[u8]) -> Option<NoteId> {
        let mut note = match parse_bytes(path, bytes) {
            Ok(n) => n,
            Err(msg) => {
                self.issues.insert(path.to_owned(), msg);
                return None;
            }
        };
        self.issues.remove(path);
        // Un fichier sans en-tête n'est pas réécrit tant qu'on ne le modifie
        // pas : l'en-tête sera ajouté à sa première sauvegarde.
        let mut dirty = false;
        if self.notes.contains_key(&note.meta.id) {
            tracing::warn!(
                "identifiant {} en double ({}) : un nouvel identifiant est attribué",
                note.meta.id,
                path.display()
            );
            note.meta.id = NoteId::generate();
            dirty = true;
        }
        let id = note.meta.id.clone();
        self.insert(note, path.to_owned(), dirty, Some(hash_bytes(bytes)));
        Some(id)
    }

    fn insert(&mut self, note: Note, path: PathBuf, dirty: bool, disk_hash: Option<u64>) {
        let id = note.meta.id.clone();
        let entry = NoteEntry {
            title_lower: note.meta.title.to_lowercase(),
            note,
            path: path.clone(),
            dirty,
            disk_hash,
            conflict: None,
        };
        self.notes.insert(id.clone(), entry);
        self.by_path.insert(path, id.clone());
        self.index_title(&id);
    }

    fn remove_entry(&mut self, id: &NoteId) {
        self.unindex_title(id);
        if let Some(e) = self.notes.remove(id) {
            self.by_path.remove(&e.path);
        }
    }

    fn index_title(&mut self, id: &NoteId) {
        if let Some(e) = self.notes.get(id) {
            self.titles
                .entry(e.title_lower.clone())
                .or_default()
                .push(id.clone());
        }
    }

    fn unindex_title(&mut self, id: &NoteId) {
        let Some(e) = self.notes.get(id) else {
            return;
        };
        if let Some(ids) = self.titles.get_mut(&e.title_lower) {
            ids.retain(|i| i != id);
            if ids.is_empty() {
                self.titles.remove(&e.title_lower);
            }
        }
    }
}

fn parse_bytes(path: &Path, bytes: &[u8]) -> Result<Note, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "le fichier n'est pas en UTF-8".to_owned())?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Note::parse(text, &stem).map_err(|e| e.to_string())
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

fn is_hidden_name(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, text).unwrap();
        p
    }

    fn packs_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")
    }

    fn open(dir: &Path) -> Project {
        Project::open(dir, &[packs_dir()]).unwrap()
    }

    #[test]
    fn opens_folder_and_reports_bad_files() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "a.md", "---\nid: A\ntitle: Alpha\n---\ncorps");
        write(d.path(), "sous/b.md", "pas d'en-tête");
        write(d.path(), "casse.md", "---\ntitle: [x\n---\n");
        write(d.path(), "ignore.txt", "x");
        write(d.path(), ".cache/c.md", "x");
        write(d.path(), ".enigma/trash/old.md", "x");
        let p = open(d.path());
        assert_eq!(p.len(), 2);
        assert_eq!(p.issues().len(), 1);
        assert!(p.issues().keys().next().unwrap().ends_with("casse.md"));
        let b = p.resolve_title("B").unwrap().clone();
        let entry = p.entry(&b).unwrap();
        assert!(!entry.note.had_frontmatter);
        assert!(
            !entry.is_dirty(),
            "pas réécrit tant qu'il n'est pas modifié"
        );
        assert_eq!(p.pack().id, "", "aucun pack configuré");
    }

    #[test]
    fn duplicate_ids_get_regenerated() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "a.md", "---\nid: SAME\ntitle: A\n---\n");
        write(d.path(), "b.md", "---\nid: SAME\ntitle: B\n---\n");
        let p = open(d.path());
        assert_eq!(p.len(), 2);
        assert_eq!(p.dirty_ids().len(), 1);
    }

    #[test]
    fn search_prefix_first_then_alpha() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        for t in ["Coffre", "Grand coffre", "Archives", "coffre-fort"] {
            p.create_note(t, vec![], String::new()).unwrap();
        }
        let titles: Vec<String> = p
            .search("coff")
            .iter()
            .map(|id| p.note(id).unwrap().meta.title.clone())
            .collect();
        assert_eq!(titles, ["Coffre", "coffre-fort", "Grand coffre"]);
        assert_eq!(p.search("").len(), 4);
    }

    #[test]
    fn create_writes_file_and_rejects_duplicates() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        let id = p
            .create_note("Pièce: 1", vec!["piece".into()], "x".into())
            .unwrap();
        let path = p.entry(&id).unwrap().path.clone();
        assert!(path.ends_with("Pièce- 1.md"));
        let on_disk = Note::parse(&std::fs::read_to_string(&path).unwrap(), "").unwrap();
        assert_eq!(on_disk.meta.id, id);
        assert!(matches!(
            p.create_note("pièce: 1", vec![], String::new()),
            Err(ProjectError::TitleTaken(_))
        ));
        assert!(matches!(
            p.create_note("a|b", vec![], String::new()),
            Err(ProjectError::InvalidTitle(_))
        ));
    }

    #[test]
    fn rename_moves_file_and_rewrites_links() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        let coffre = p.create_note("Coffre", vec![], String::new()).unwrap();
        let other = p
            .create_note(
                "Indice",
                vec![],
                "Voir [[coffre|ouvre]] et `[[Coffre]]`".into(),
            )
            .unwrap();
        let updated = p.rename(&coffre, "Grand coffre").unwrap();
        assert_eq!(updated, vec![other.clone()]);
        assert_eq!(
            p.note(&other).unwrap().body,
            "Voir [[Grand coffre|ouvre]] et `[[Coffre]]`"
        );
        let path = p.entry(&coffre).unwrap().path.clone();
        assert!(path.ends_with("Grand coffre.md"));
        assert!(!d.path().join("Coffre.md").exists());
        assert_eq!(p.resolve_title("grand COFFRE"), Some(&coffre));
        assert!(p.resolve_title("Coffre").is_none());
        // Relecture depuis le disque : tout est persistant.
        let again = open(d.path());
        assert_eq!(
            again.note(&other).unwrap().body,
            "Voir [[Grand coffre|ouvre]] et `[[Coffre]]`"
        );
        assert_eq!(again.note(&coffre).unwrap().meta.title, "Grand coffre");
    }

    #[test]
    fn rename_case_only() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        let id = p.create_note("coffre", vec![], String::new()).unwrap();
        p.rename(&id, "Coffre").unwrap();
        let path = p.entry(&id).unwrap().path.clone();
        assert_eq!(path.file_name().unwrap(), "Coffre.md");
        assert!(path.exists());
    }

    #[test]
    fn rename_to_taken_title_fails() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        let a = p.create_note("A", vec![], String::new()).unwrap();
        p.create_note("B", vec![], String::new()).unwrap();
        assert!(matches!(
            p.rename(&a, "b"),
            Err(ProjectError::TitleTaken(_))
        ));
    }

    #[test]
    fn delete_moves_to_trash() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        let id = p.create_note("A", vec![], String::new()).unwrap();
        let trashed = p.delete(&id).unwrap();
        assert!(trashed.exists());
        assert!(!d.path().join("A.md").exists());
        assert!(p.note(&id).is_none());
        assert_eq!(open(d.path()).len(), 0);
    }

    #[test]
    fn set_meta_keeps_id_and_title_and_save_persists() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        let id = p.create_note("A", vec![], String::new()).unwrap();
        let mut meta = p.note(&id).unwrap().meta.clone();
        meta.title = "pirate".into();
        meta.id = NoteId::generate();
        meta.categories.push("piece".into());
        p.set_meta(&id, meta).unwrap();
        assert!(p.entry(&id).unwrap().is_dirty());
        assert!(p.save_dirty().is_empty());
        let again = open(d.path());
        let n = again.note(&id).unwrap();
        assert_eq!(n.meta.title, "A");
        assert_eq!(n.meta.categories, ["piece"]);
    }

    #[test]
    fn own_writes_are_ignored_external_ones_reloaded() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        let id = p.create_note("A", vec![], "v1".into()).unwrap();
        let path = p.entry(&id).unwrap().path.clone();
        assert_eq!(p.on_file_changed(&path), FileChange::Ignored);

        let mut n = p.note(&id).unwrap().clone();
        n.body = "v2 externe".into();
        std::fs::write(&path, n.serialize().unwrap()).unwrap();
        assert_eq!(p.on_file_changed(&path), FileChange::Reloaded(id.clone()));
        assert_eq!(p.note(&id).unwrap().body, "v2 externe");
    }

    #[test]
    fn external_change_on_dirty_note_is_a_conflict() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        let id = p.create_note("A", vec![], "v1".into()).unwrap();
        let path = p.entry(&id).unwrap().path.clone();
        p.set_body(&id, "à moi".into()).unwrap();

        let mut n = p.note(&id).unwrap().clone();
        n.body = "à eux".into();
        std::fs::write(&path, n.serialize().unwrap()).unwrap();
        assert_eq!(p.on_file_changed(&path), FileChange::Conflict(id.clone()));
        assert!(p.save_dirty().is_empty());
        // La sauvegarde automatique ne doit pas écraser la version externe.
        assert!(std::fs::read_to_string(&path).unwrap().contains("à eux"));

        p.resolve_conflict(&id, false).unwrap();
        assert_eq!(p.note(&id).unwrap().body, "à eux");
        assert!(!p.entry(&id).unwrap().is_dirty());
    }

    #[test]
    fn keep_mine_overwrites_disk() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        let id = p.create_note("A", vec![], "v1".into()).unwrap();
        let path = p.entry(&id).unwrap().path.clone();
        p.set_body(&id, "à moi".into()).unwrap();
        let mut n = p.note(&id).unwrap().clone();
        n.body = "à eux".into();
        std::fs::write(&path, n.serialize().unwrap()).unwrap();
        p.on_file_changed(&path);
        p.resolve_conflict(&id, true).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("à moi"));
    }

    #[test]
    fn external_add_and_remove() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        let path = write(d.path(), "Nouvelle.md", "# hello");
        let FileChange::Added(id) = p.on_file_changed(&path) else {
            panic!("attendu Added");
        };
        assert_eq!(p.note(&id).unwrap().meta.title, "Nouvelle");
        // Modifiée en mémoire puis supprimée du disque : elle est conservée.
        p.set_body(&id, "modifiée".into()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            p.on_file_changed(&path),
            FileChange::RemovedWhileDirty(id.clone())
        );
        p.save(&id).unwrap();
        assert!(path.exists());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(p.on_file_changed(&path), FileChange::Removed(id));
        assert!(p.is_empty());
    }

    #[test]
    fn hidden_and_non_markdown_paths_ignored() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        let hidden = write(d.path(), ".enigma/x.md", "x");
        let txt = write(d.path(), "x.txt", "x");
        assert_eq!(p.on_file_changed(&hidden), FileChange::Ignored);
        assert_eq!(p.on_file_changed(&txt), FileChange::Ignored);
        assert_eq!(
            p.on_file_changed(Path::new("/ailleurs/x.md")),
            FileChange::Ignored
        );
    }

    #[test]
    fn set_pack_persists_and_loads() {
        let d = tempfile::tempdir().unwrap();
        let mut p = open(d.path());
        assert!(
            p.available_packs()
                .iter()
                .any(|f| f.result.as_ref().is_ok_and(|p| p.id == "blue_prince"))
        );
        p.set_pack(Some("blue_prince".into())).unwrap();
        assert_eq!(p.pack().id, "blue_prince");
        let again = open(d.path());
        assert_eq!(again.pack().name, "Blue Prince");
        assert!(again.pack_issue().is_none());

        let mut p = again;
        p.set_pack(Some("inexistant".into())).unwrap();
        assert!(p.pack_issue().unwrap().contains("introuvable"));
    }
}
