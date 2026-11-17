//! `enigma-core` : modèle de données d'Enigma Notes, sans aucune dépendance UI.
//!
//! - [`note`] : format d'une fiche (en-tête YAML + corps Markdown).
//! - [`markdown`] : analyse du corps (styles pour la mise en forme, liens `[[...]]`).
//! - [`pack`] : packs de jeu déclaratifs (`pack.json`).
//! - [`project`] : un dossier de fiches chargé en mémoire, avec ses index.
//! - [`history`] : pile annuler/refaire générique.
//! - [`storage`] : écriture atomique et noms de fichiers sûrs.
//! - [`board`] : tableau d'enquête (cartes placées à la main).
//! - [`shortcut`] : raccourcis clavier configurables.
//! - [`fuzzy`] : recherche approximative (palette de commandes).

pub mod board;
pub mod filter;
pub mod fuzzy;
pub mod history;
pub mod id;
pub mod link_edit;
pub mod links;
pub mod markdown;
pub mod note;
pub mod pack;
pub mod project;
pub mod shortcut;
pub mod storage;

pub use id::NoteId;
pub use note::{FieldValue, MetaLink, Note, NoteMeta, Pos};
pub use pack::{Color, Pack};
pub use project::Project;
