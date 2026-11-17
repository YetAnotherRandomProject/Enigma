//! `enigma-core` : modèle de données d'Enigma Notes, sans aucune dépendance UI.
//!
//! - [`note`] : format d'une fiche (en-tête YAML + corps Markdown).
//! - [`markdown`] : analyse du corps (styles pour la mise en forme, liens `[[...]]`).
//! - [`pack`] : packs de jeu déclaratifs (`pack.json`).
//! - [`project`] : un dossier de fiches chargé en mémoire, avec ses index.
//! - [`history`] : pile annuler/refaire générique.
//! - [`storage`] : écriture atomique et noms de fichiers sûrs.

pub mod history;
pub mod id;
pub mod markdown;
pub mod note;
pub mod pack;
pub mod project;
pub mod storage;

pub use id::NoteId;
pub use note::{FieldValue, MetaLink, Note, NoteMeta, Pos};
pub use pack::{Color, Pack};
pub use project::Project;
