//! Raccourcis clavier configurables, indépendants de l'interface.
//!
//! Un raccourci s'écrit `Ctrl+Maj+Z` (ou `Ctrl+Shift+Z`) ; la touche est un
//! nom de touche d'egui (`A`, `F2`, `Space`, `Enter`…), que l'application
//! convertit. Le fichier de raccourcis ne contient que les **changements**
//! par rapport aux valeurs par défaut :
//!
//! ```json
//! { "version": 1, "bindings": { "palette": ["Ctrl+K"], "toggle_graph": [] } }
//! ```
//!
//! Une liste vide désactive la commande au clavier. Les identifiants de
//! commandes inconnus sont conservés à la réécriture.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::storage::write_atomic;

pub const KEYMAP_FORMAT_VERSION: u32 = 1;

/// Combinaison de touches. `ctrl` désigne Cmd sur macOS.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Nom de la touche (ex. `Z`, `F2`, `Space`).
    pub key: String,
}

impl Shortcut {
    /// Nombre de modificateurs : les raccourcis les plus précis doivent être
    /// testés en premier (`Ctrl+Maj+Z` avant `Ctrl+Z`).
    pub fn modifier_count(&self) -> usize {
        usize::from(self.ctrl) + usize::from(self.shift) + usize::from(self.alt)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ShortcutError {
    #[error("raccourci vide")]
    Empty,
    #[error("modificateur inconnu « {0} » (Ctrl, Maj/Shift, Alt)")]
    UnknownModifier(String),
}

impl FromStr for Shortcut {
    type Err = ShortcutError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let Some((key, mods)) = parts.split_last() else {
            return Err(ShortcutError::Empty);
        };
        if key.is_empty() {
            return Err(ShortcutError::Empty);
        }
        let mut sc = Shortcut {
            ctrl: false,
            shift: false,
            alt: false,
            key: (*key).to_owned(),
        };
        for m in mods {
            match m.to_lowercase().as_str() {
                "ctrl" | "cmd" | "control" => sc.ctrl = true,
                "maj" | "shift" => sc.shift = true,
                "alt" | "option" => sc.alt = true,
                other => return Err(ShortcutError::UnknownModifier(other.to_owned())),
            }
        }
        Ok(sc)
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.shift {
            f.write_str("Maj+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        f.write_str(&self.key)
    }
}

impl Serialize for Shortcut {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Shortcut {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeymapError {
    #[error("lecture de {path} : {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("{path} : {message}")]
    Format { path: String, message: String },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct KeymapFile {
    version: u32,
    #[serde(default)]
    bindings: BTreeMap<String, Vec<Shortcut>>,
}

/// Raccourcis modifiés par l'utilisateur (le reste vient des valeurs par
/// défaut des commandes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keymap {
    overrides: BTreeMap<String, Vec<Shortcut>>,
}

impl Keymap {
    /// Charge un fichier de raccourcis ; absent = aucun changement.
    pub fn load(path: &Path) -> Result<Keymap, KeymapError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Keymap::default()),
            Err(source) => {
                return Err(KeymapError::Io {
                    path: path.display().to_string(),
                    source,
                });
            }
        };
        let file: KeymapFile = serde_json::from_str(&text).map_err(|e| KeymapError::Format {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        if file.version == 0 || file.version > KEYMAP_FORMAT_VERSION {
            return Err(KeymapError::Format {
                path: path.display().to_string(),
                message: format!("version {} non prise en charge", file.version),
            });
        }
        Ok(Keymap {
            overrides: file.bindings,
        })
    }

    /// Enregistre (atomiquement) les changements.
    pub fn save(&self, path: &Path) -> Result<(), KeymapError> {
        let io = |source| KeymapError::Io {
            path: path.display().to_string(),
            source,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let file = KeymapFile {
            version: KEYMAP_FORMAT_VERSION,
            bindings: self.overrides.clone(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(|e| KeymapError::Format {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        write_atomic(path, text.as_bytes()).map_err(io)
    }

    /// Raccourcis effectifs d'une commande.
    pub fn bindings(&self, command: &str, defaults: &[Shortcut]) -> Vec<Shortcut> {
        self.overrides
            .get(command)
            .cloned()
            .unwrap_or_else(|| defaults.to_vec())
    }

    pub fn is_overridden(&self, command: &str) -> bool {
        self.overrides.contains_key(command)
    }

    /// Remplace les raccourcis d'une commande (liste vide : désactivée).
    pub fn set(&mut self, command: &str, shortcuts: Vec<Shortcut>) {
        self.overrides.insert(command.to_owned(), shortcuts);
    }

    /// Revient aux raccourcis par défaut.
    pub fn reset(&mut self, command: &str) {
        self.overrides.remove(command);
    }

    pub fn reset_all(&mut self) {
        self.overrides.clear();
    }
}

/// Commandes (identifiants) partageant un même raccourci.
pub fn conflicts<'a>(
    effective: impl IntoIterator<Item = (&'a str, Vec<Shortcut>)>,
) -> Vec<(Shortcut, Vec<&'a str>)> {
    let mut by_shortcut: BTreeMap<Shortcut, Vec<&'a str>> = BTreeMap::new();
    for (cmd, shortcuts) in effective {
        for s in shortcuts {
            let cmds = by_shortcut.entry(s).or_default();
            if !cmds.contains(&cmd) {
                cmds.push(cmd);
            }
        }
    }
    by_shortcut
        .into_iter()
        .filter(|(_, cmds)| cmds.len() > 1)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(s: &str) -> Shortcut {
        s.parse().unwrap()
    }

    #[test]
    fn parse_and_display() {
        let s = sc("ctrl + shift + Z");
        assert!(s.ctrl && s.shift && !s.alt);
        assert_eq!(s.key, "Z");
        assert_eq!(s.to_string(), "Ctrl+Maj+Z");
        assert_eq!(sc("Maj+Alt+F2").to_string(), "Maj+Alt+F2");
        assert_eq!(sc(&s.to_string()), s, "aller-retour");
        assert_eq!(sc("Space").modifier_count(), 0);
        assert_eq!(
            "Hyper+K".parse::<Shortcut>(),
            Err(ShortcutError::UnknownModifier("hyper".into()))
        );
        assert_eq!("Ctrl+".parse::<Shortcut>(), Err(ShortcutError::Empty));
    }

    #[test]
    fn overrides_round_trip_and_defaults() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("cfg/keymap.json");
        let mut km = Keymap::load(&path).unwrap();
        assert_eq!(km, Keymap::default(), "fichier absent");
        let defaults = [sc("Ctrl+P")];
        assert_eq!(km.bindings("palette", &defaults), defaults);
        km.set("palette", vec![sc("Ctrl+K")]);
        km.set("toggle_graph", vec![]);
        km.save(&path).unwrap();
        let back = Keymap::load(&path).unwrap();
        assert_eq!(back.bindings("palette", &defaults), [sc("Ctrl+K")]);
        assert!(back.bindings("toggle_graph", &[sc("Ctrl+G")]).is_empty());
        assert!(back.is_overridden("palette"));
        let mut back = back;
        back.reset("palette");
        assert_eq!(back.bindings("palette", &defaults), defaults);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"Ctrl+K\""), "{text}");
    }

    #[test]
    fn bad_files_are_reported() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("k.json");
        std::fs::write(&path, r#"{"version": 1, "bindings": {"x": ["Turbo+A"]}}"#).unwrap();
        let err = Keymap::load(&path).unwrap_err().to_string();
        assert!(err.contains("turbo"), "{err}");
        std::fs::write(&path, r#"{"version": 9}"#).unwrap();
        assert!(Keymap::load(&path).is_err());
    }

    #[test]
    fn detects_conflicts() {
        let found = conflicts([
            ("a", vec![sc("Ctrl+K")]),
            ("b", vec![sc("Ctrl+K"), sc("Ctrl+J")]),
            ("c", vec![sc("Ctrl+J")]),
            ("d", vec![sc("Ctrl+L")]),
        ]);
        assert_eq!(
            found,
            [
                (sc("Ctrl+J"), vec!["b", "c"]),
                (sc("Ctrl+K"), vec!["a", "b"])
            ]
        );
    }
}
