//! Tableau d'enquête : cartes de fiches placées librement à la main, cartes
//! de texte libre, images, groupes (cadres titrés) et flèches étiquetées.
//!
//! Stocké à part (`.enigma/board.json`), car ces positions n'ont rien à voir
//! avec celles du graphe :
//!
//! ```json
//! { "version": 2,
//!   "cards": [ { "id": "01H…", "x": 120.0, "y": -40.0 } ],
//!   "texts": [ { "key": "t1", "x": 0, "y": 0, "w": 200, "h": 100, "text": "…" } ],
//!   "images": [ { "key": "i1", "x": 0, "y": 0, "w": 240, "src": "assets/plan.png" } ],
//!   "groups": [ { "key": "g1", "x": 0, "y": 0, "w": 600, "h": 400, "label": "Aile est" } ],
//!   "edges": [ { "key": "e1", "from": "01H…", "to": "t1", "label": "ouvre" } ] }
//! ```
//!
//! Chaque élément a une clé (l'identifiant de la fiche pour une carte de
//! fiche) ; les flèches relient deux clés. Une carte dont la fiche n'existe
//! plus est gardée (la fiche peut revenir de la corbeille) mais n'est pas
//! affichée. Les fichiers de la version 1 (cartes seules) se lisent tels
//! quels.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::id::NoteId;
use crate::storage::write_atomic;

pub const BOARD_FORMAT_VERSION: u32 = 2;
pub const BOARD_FILE: &str = "board.json";
/// Taille d'une carte de fiche (unités du tableau ≈ pixels au zoom 1).
pub const NOTE_W: f32 = 200.0;
pub const NOTE_H: f32 = 92.0;

/// Carte de texte libre.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextCard {
    pub key: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub text: String,
    /// Couleur `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Image posée sur le tableau (chemin relatif au projet).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageCard {
    pub key: String,
    pub x: f32,
    pub y: f32,
    /// Largeur ; la hauteur suit les proportions de l'image.
    pub w: f32,
    pub src: String,
}

/// Cadre titré qui regroupe des éléments (déplacés avec lui).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub key: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Flèche entre deux éléments (clés).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub key: String,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Nouvelle clé d'élément.
fn new_key(prefix: &str) -> String {
    let u = ulid::Ulid::generate().to_string().to_lowercase();
    format!("{prefix}{}", &u[u.len() - 10..])
}

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts: Vec<TextCard>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageCard>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
}

impl Default for Board {
    fn default() -> Self {
        Board {
            version: BOARD_FORMAT_VERSION,
            cards: Vec::new(),
            texts: Vec::new(),
            images: Vec::new(),
            groups: Vec::new(),
            edges: Vec::new(),
        }
    }
}

/// Rectangle d'un élément (x, y, largeur, hauteur) ; la hauteur d'une
/// image dépend de ses proportions, inconnues ici (`h` = largeur).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl ItemRect {
    fn contains(&self, o: &ItemRect) -> bool {
        o.x >= self.x
            && o.y >= self.y
            && o.x + o.w <= self.x + self.w
            && o.y + o.h <= self.y + self.h
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
        let mut current = self.clone();
        current.version = BOARD_FORMAT_VERSION;
        let text = serde_json::to_string_pretty(&current).map_err(|e| BoardError::Format {
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
        let removed = self.cards.len() != before;
        if removed {
            self.edges
                .retain(|e| e.from != id.as_str() && e.to != id.as_str());
        }
        removed
    }

    pub fn add_text(&mut self, x: f32, y: f32, text: &str) -> String {
        let key = new_key("t");
        self.texts.push(TextCard {
            key: key.clone(),
            x,
            y,
            w: 220.0,
            h: 110.0,
            text: text.to_owned(),
            color: None,
        });
        key
    }

    pub fn add_image(&mut self, x: f32, y: f32, src: &str) -> String {
        let key = new_key("i");
        self.images.push(ImageCard {
            key: key.clone(),
            x,
            y,
            w: 260.0,
            src: src.to_owned(),
        });
        key
    }

    pub fn add_group(&mut self, x: f32, y: f32, w: f32, h: f32, label: &str) -> String {
        let key = new_key("g");
        self.groups.push(Group {
            key: key.clone(),
            x,
            y,
            w,
            h,
            label: label.to_owned(),
            color: None,
        });
        key
    }

    /// Ajoute une flèche (sans doublon dans le même sens) ; rend sa clé.
    pub fn add_edge(&mut self, from: &str, to: &str) -> Option<String> {
        if from == to || self.edges.iter().any(|e| e.from == from && e.to == to) {
            return None;
        }
        let key = new_key("e");
        self.edges.push(Edge {
            key: key.clone(),
            from: from.to_owned(),
            to: to.to_owned(),
            label: String::new(),
            color: None,
        });
        Some(key)
    }

    /// Rectangle d'un élément (carte, texte, image, groupe).
    pub fn rect_of(&self, key: &str) -> Option<ItemRect> {
        if let Some(c) = self.cards.iter().find(|c| c.id.as_str() == key) {
            return Some(ItemRect {
                x: c.x,
                y: c.y,
                w: NOTE_W,
                h: NOTE_H,
            });
        }
        if let Some(t) = self.texts.iter().find(|t| t.key == key) {
            return Some(ItemRect {
                x: t.x,
                y: t.y,
                w: t.w,
                h: t.h,
            });
        }
        if let Some(i) = self.images.iter().find(|i| i.key == key) {
            return Some(ItemRect {
                x: i.x,
                y: i.y,
                w: i.w,
                h: i.w,
            });
        }
        self.groups.iter().find(|g| g.key == key).map(|g| ItemRect {
            x: g.x,
            y: g.y,
            w: g.w,
            h: g.h,
        })
    }

    /// Déplace un élément ; un groupe emmène les éléments qu'il contient.
    pub fn move_item(&mut self, key: &str, x: f32, y: f32) {
        if let Some(g) = self.groups.iter().find(|g| g.key == key).cloned() {
            let (dx, dy) = (x - g.x, y - g.y);
            let area = ItemRect {
                x: g.x,
                y: g.y,
                w: g.w,
                h: g.h,
            };
            let inside: Vec<String> = self
                .keys()
                .into_iter()
                .filter(|k| k != key)
                .filter(|k| self.rect_of(k).is_some_and(|r| area.contains(&r)))
                .collect();
            for k in inside {
                if let Some(r) = self.rect_of(&k) {
                    self.set_pos(&k, r.x + dx, r.y + dy);
                }
            }
        }
        self.set_pos(key, x, y);
    }

    fn set_pos(&mut self, key: &str, x: f32, y: f32) {
        if let Some(c) = self.cards.iter_mut().find(|c| c.id.as_str() == key) {
            (c.x, c.y) = (x, y);
        } else if let Some(t) = self.texts.iter_mut().find(|t| t.key == key) {
            (t.x, t.y) = (x, y);
        } else if let Some(i) = self.images.iter_mut().find(|i| i.key == key) {
            (i.x, i.y) = (x, y);
        } else if let Some(g) = self.groups.iter_mut().find(|g| g.key == key) {
            (g.x, g.y) = (x, y);
        }
    }

    /// Redimensionne (texte, groupe : largeur et hauteur ; image : largeur).
    pub fn resize_item(&mut self, key: &str, w: f32, h: f32) {
        let (w, h) = (w.max(40.0), h.max(30.0));
        if let Some(t) = self.texts.iter_mut().find(|t| t.key == key) {
            (t.w, t.h) = (w, h);
        } else if let Some(i) = self.images.iter_mut().find(|i| i.key == key) {
            i.w = w;
        } else if let Some(g) = self.groups.iter_mut().find(|g| g.key == key) {
            (g.w, g.h) = (w, h);
        }
    }

    /// Retire un élément (et ses flèches). Rend `true` s'il existait.
    pub fn remove_item(&mut self, key: &str) -> bool {
        let before = self.cards.len()
            + self.texts.len()
            + self.images.len()
            + self.groups.len()
            + self.edges.len();
        self.cards.retain(|c| c.id.as_str() != key);
        self.texts.retain(|t| t.key != key);
        self.images.retain(|i| i.key != key);
        self.groups.retain(|g| g.key != key);
        self.edges
            .retain(|e| e.key != key && e.from != key && e.to != key);
        before
            != self.cards.len()
                + self.texts.len()
                + self.images.len()
                + self.groups.len()
                + self.edges.len()
    }

    /// Clés de tous les éléments posés (hors flèches).
    pub fn keys(&self) -> Vec<String> {
        self.cards
            .iter()
            .map(|c| c.id.as_str().to_owned())
            .chain(self.texts.iter().map(|t| t.key.clone()))
            .chain(self.images.iter().map(|i| i.key.clone()))
            .chain(self.groups.iter().map(|g| g.key.clone()))
            .collect()
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
    fn texts_images_groups_and_edges() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("board.json");
        let mut b = Board::default();
        b.add(&id("A"), 10.0, 10.0);
        let t = b.add_text(300.0, 10.0, "Indice : 1945");
        let i = b.add_image(10.0, 400.0, "assets/plan.png");
        let g = b.add_group(0.0, 0.0, 600.0, 200.0, "Bureau");
        let e = b.add_edge("A", &t).unwrap();
        assert!(b.add_edge("A", &t).is_none(), "pas de doublon");
        assert!(b.add_edge(&t, &t).is_none());
        b.edges[0].label = "code".into();
        // Le groupe emmène la carte et le texte qu'il contient, pas l'image.
        b.move_item(&g, 100.0, 50.0);
        assert_eq!((b.cards[0].x, b.cards[0].y), (110.0, 60.0));
        assert_eq!(b.texts[0].x, 400.0);
        assert_eq!(b.images[0].y, 400.0);
        b.resize_item(&t, 10.0, 300.0);
        assert_eq!((b.texts[0].w, b.texts[0].h), (40.0, 300.0));
        b.save(&path).unwrap();
        assert_eq!(Board::load(&path).unwrap(), b);
        // Retirer une carte retire ses flèches.
        assert!(b.remove(&id("A")));
        assert!(b.edges.is_empty());
        assert!(b.remove_item(&i));
        assert!(!b.remove_item(&e));
        // L'ancien format (version 1) se lit toujours.
        std::fs::write(&path, r#"{"version":1,"cards":[{"id":"A","x":1,"y":2}]}"#).unwrap();
        assert_eq!(Board::load(&path).unwrap().cards.len(), 1);
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
        std::fs::write(&path, r#"{"version":3}"#).unwrap();
        assert!(Board::load(&path).is_err());
    }
}
