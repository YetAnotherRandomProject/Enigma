//! `enigma-core` : modèle de données d'Enigma Notes, sans aucune dépendance UI.
//!
//! - [`note`] : format d'une fiche (en-tête YAML + corps Markdown).
//! - [`markdown`] : analyse du corps (styles pour la mise en forme, liens `[[...]]`).
//! - [`pack`] : packs de jeu déclaratifs (`pack.json`).
//! - [`project`] : un dossier de fiches chargé en mémoire, avec ses index.
//! - [`history`] : pile annuler/refaire générique.
//! - [`storage`] : écriture atomique et noms de fichiers sûrs.
//! - [`container`] : fichier projet `.enigma` (format maison).
//! - [`board`] : tableau d'enquête (cartes placées à la main).
//! - [`shortcut`] : raccourcis clavier configurables.
//! - [`fuzzy`] : recherche approximative (palette de commandes).
//! - [`search`] : recherche dans tout le projet (mots, expressions, regex, champs).
//! - [`table`] : tableaux Markdown alignés.
//! - [`live`] : découpage d'une fiche en unités d'édition (ligne, tableau, bloc de code).

pub mod board;
pub mod container;
pub mod filter;
pub mod fuzzy;
pub mod history;
pub mod id;
pub mod import;
pub mod link_edit;
pub mod links;
pub mod live;
pub mod markdown;
pub mod md_edit;
pub mod mentions;
pub mod note;
pub mod pack;
pub mod project;
pub mod render;
pub mod search;
pub mod shortcut;
pub mod storage;
pub mod table;

pub use id::NoteId;
pub use note::{FieldValue, MetaLink, Note, NoteMeta, Pos};
pub use pack::{Color, Pack};
pub use project::Project;
