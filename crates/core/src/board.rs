//! Tableau d'enquête : cartes de fiches placées librement à la main.
//!
//! Stocké à part (`.enigma/board.json`), car ces positions n'ont rien à voir
//! avec celles du graphe :
//!
//! ```json
//! { "version": 1, "cards": [ { "id": "01H…", "x": 120.0, "y": -40.0 } ] }
//! ```
//!
//! Une carte dont la fiche n'existe plus est gardée (la fiche peut revenir
//! de la corbeille) mais n'est pas affichée.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::id::NoteId;
use crate::storage::write_atomic;

pub const BOARD_FORMAT_VERSION: u32 = 1;
pub const BOARD_FILE: &str = "board.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Card {
    pub id: NoteId,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Board {
    pub version: u32,
    #[serde(default)]
    pub cards: Vec<Card>,
}

impl Default for Board {
    fn default() -> Self {
        Board {
            version: BOARD_FORMAT_VERSION,
            cards: Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BoardError {
    #[error("tableau d'enquête ({path}) : {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("tableau d'enquête ({path}) : {message}")]
    Format { path: String, message: String },
}

impl Board {
    /// Charge le tableau ; fichier absent = tableau vide. Les cartes en
    /// double (même fiche) sont fusionnées.
    pub fn load(path: &Path) -> Result<Board, BoardError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Board::default()),
            Err(source) => {
                return Err(BoardError::Io {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        let fmt_err = |message: String| BoardError::Format {
            path: path.display().to_string(),
            message,
        };
        let mut board: Board = serde_json::from_str(&text).map_err(|e| fmt_err(e.to_string()))?;
        if board.version == 0 || board.version > BOARD_FORMAT_VERSION {
            return Err(fmt_err(format!(
                "version {} non prise en charge",
                board.version
            )));
        }
        let mut seen = std::collections::HashSet::new();
        board.cards.retain(|c| seen.insert(c.id.clone()));
        Ok(board)
    }

    /// Enregistre atomiquement (crée le dossier au besoin).
    pub fn save(&self, path: &Path) -> Result<(), BoardError> {
        let io = |source| BoardError::Io {
            path: path.display().to_string(),
            source,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| BoardError::Format {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        write_atomic(path, text.as_bytes()).map_err(io)
    }

    pub fn contains(&self, id: &NoteId) -> bool {
        self.cards.iter().any(|c| c.id == *id)
    }

    pub fn card(&self, id: &NoteId) -> Option<&Card> {
        self.cards.iter().find(|c| c.id == *id)
    }

    /// Ajoute une carte (sans effet si la fiche y est déjà). Rend `true` si
    /// elle a été ajoutée.
    pub fn add(&mut self, id: &NoteId, x: f32, y: f32) -> bool {
        if self.contains(id) {
            return false;
        }
        self.cards.push(Card {
            id: id.clone(),
            x,
            y,
        });
        true
    }

    /// Déplace une carte et la met au premier plan.
    pub fn move_to(&mut self, id: &NoteId, x: f32, y: f32) {
        if let Some(i) = self.cards.iter().position(|c| c.id == *id) {
            let mut c = self.cards.remove(i);
            c.x = x;
            c.y = y;
            self.cards.push(c);
        }
    }

    pub fn remove(&mut self, id: &NoteId) -> bool {
        let before = self.cards.len();
        self.cards.retain(|c| c.id != *id);
        self.cards.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> NoteId {
        NoteId::parse(s).unwrap()
    }

    #[test]
    fn add_move_remove_and_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join(".enigma").join(BOARD_FILE);
        let mut b = Board::load(&path).unwrap();
        assert!(b.cards.is_empty());
        assert!(b.add(&id("A"), 1.0, 2.0));
        assert!(!b.add(&id("A"), 5.0, 5.0), "pas de doublon");
        b.add(&id("B"), 0.0, 0.0);
        b.move_to(&id("A"), 10.0, 20.0);
        assert_eq!(b.cards.last().unwrap().id, id("A"), "au premier plan");
        assert_eq!(b.card(&id("A")).unwrap().x, 10.0);
        b.save(&path).unwrap();
        let back = Board::load(&path).unwrap();
        assert_eq!(back, b);
        let mut back = back;
        assert!(back.remove(&id("B")));
        assert!(!back.remove(&id("B")));
        assert_eq!(back.cards.len(), 1);
    }

    #[test]
    fn duplicates_merged_and_errors_readable() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("board.json");
        std::fs::write(
            &path,
            r#"{"version":1,"cards":[{"id":"A","x":1,"y":2},{"id":"A","x":3,"y":4}]}"#,
        )
        .unwrap();
        let b = Board::load(&path).unwrap();
        assert_eq!(b.cards.len(), 1);
        assert_eq!(b.cards[0].x, 1.0);
        std::fs::write(&path, r#"{"version":1,"cards":[{"id":"A","x":1}]}"#).unwrap();
        let err = Board::load(&path).unwrap_err().to_string();
        assert!(err.contains("tableau d'enquête"), "{err}");
        std::fs::write(&path, r#"{"version":2}"#).unwrap();
        assert!(Board::load(&path).is_err());
    }
}
