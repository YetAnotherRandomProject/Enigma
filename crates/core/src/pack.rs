//! Packs de jeu déclaratifs : catégories, types de liens, statuts, champs et
//! modèles, décrits dans un `pack.json`.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Version du format `pack.json` comprise par cette version de l'application.
pub const PACK_FORMAT_VERSION: u32 = 1;

/// Nom du fichier de description d'un pack.
pub const PACK_FILE: &str = "pack.json";

/// Couleur RGBA, écrite `#rrggbb` ou `#rrggbbaa`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub [u8; 4]);

impl Color {
    pub const GREY: Color = Color([0x9c, 0xa3, 0xaf, 0xff]);

    pub fn parse(s: &str) -> Option<Color> {
        let hex = s.strip_prefix('#')?;
        if !(hex.len() == 6 || hex.len() == 8) || !hex.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        let a = if hex.len() == 8 { byte(6)? } else { 0xff };
        Some(Color([byte(0)?, byte(2)?, byte(4)?, a]))
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b, a] = self.0;
        if a == 0xff {
            write!(f, "#{r:02x}{g:02x}{b:02x}")
        } else {
            write!(f, "#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "couleur « {s} » invalide (attendu #rrggbb ou #rrggbbaa)"
            ))
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    pub id: String,
    pub label: String,
    pub color: Color,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkType {
    pub id: String,
    pub label: String,
    pub color: Color,
    #[serde(default = "default_true")]
    pub directed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub id: String,
    pub label: String,
    pub color: Color,
}

/// Type d'un champ personnalisé.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Number,
    Text,
    #[serde(alias = "boolean")]
    Bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDef {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: FieldKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub body: String,
}

/// Un pack de jeu.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pack {
    pub version: u32,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default)]
    pub link_types: Vec<LinkType>,
    #[serde(default)]
    pub statuses: Vec<Status>,
    #[serde(default)]
    pub fields: Vec<FieldDef>,
    #[serde(default)]
    pub templates: Vec<Template>,
}

/// Erreur de chargement d'un pack.
#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("impossible de lire {path} : {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} : JSON invalide : {message}")]
    Json { path: PathBuf, message: String },
    #[error("{path} : champ `version` manquant (format actuel : {PACK_FORMAT_VERSION})")]
    MissingVersion { path: PathBuf },
    #[error(
        "{path} : version {found} non prise en charge (cette application lit la version {PACK_FORMAT_VERSION})"
    )]
    UnsupportedVersion { path: PathBuf, found: u64 },
    #[error("{path} : pack incohérent :\n- {}", problems.join("\n- "))]
    Invalid {
        path: PathBuf,
        problems: Vec<String>,
    },
}

impl Pack {
    /// Pack vide, utilisé quand le projet n'en référence aucun.
    pub fn empty() -> Pack {
        Pack {
            version: PACK_FORMAT_VERSION,
            id: String::new(),
            name: "Aucun pack".to_owned(),
            description: None,
            categories: Vec::new(),
            link_types: Vec::new(),
            statuses: Vec::new(),
            fields: Vec::new(),
            templates: Vec::new(),
        }
    }

    /// Lit et valide un pack depuis son texte JSON. `path` sert aux messages.
    pub fn from_json(text: &str, path: &Path) -> Result<Pack, PackError> {
        let json_err = |e: serde_json::Error| PackError::Json {
            path: path.to_owned(),
            message: e.to_string(),
        };
        let raw: serde_json::Value = serde_json::from_str(text).map_err(json_err)?;
        match raw.get("version") {
            None => {
                return Err(PackError::MissingVersion {
                    path: path.to_owned(),
                });
            }
            Some(v) => {
                if let Some(found) = v.as_u64()
                    && found != u64::from(PACK_FORMAT_VERSION)
                {
                    return Err(PackError::UnsupportedVersion {
                        path: path.to_owned(),
                        found,
                    });
                }
            }
        }
        let pack: Pack = serde_json::from_str(text).map_err(json_err)?;
        let problems = pack.problems();
        if problems.is_empty() {
            Ok(pack)
        } else {
            Err(PackError::Invalid {
                path: path.to_owned(),
                problems,
            })
        }
    }

    /// Charge le pack d'un dossier (`<dir>/pack.json`).
    pub fn load_dir(dir: &Path) -> Result<Pack, PackError> {
        let path = dir.join(PACK_FILE);
        let text = std::fs::read_to_string(&path).map_err(|source| PackError::Io {
            path: path.clone(),
            source,
        })?;
        Pack::from_json(&text, &path)
    }

    /// Liste des incohérences (vide si le pack est valide).
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.id.trim().is_empty() {
            problems.push("`id` du pack vide".to_owned());
        }
        if self.name.trim().is_empty() {
            problems.push("`name` du pack vide".to_owned());
        }
        check_ids(
            &mut problems,
            "categories",
            self.categories.iter().map(|c| c.id.as_str()),
        );
        check_ids(
            &mut problems,
            "link_types",
            self.link_types.iter().map(|c| c.id.as_str()),
        );
        check_ids(
            &mut problems,
            "statuses",
            self.statuses.iter().map(|c| c.id.as_str()),
        );
        check_ids(
            &mut problems,
            "fields",
            self.fields.iter().map(|c| c.id.as_str()),
        );
        check_ids(
            &mut problems,
            "templates",
            self.templates.iter().map(|c| c.id.as_str()),
        );
        for t in &self.templates {
            for c in &t.categories {
                if self.category(c).is_none() {
                    problems.push(format!(
                        "le modèle « {} » utilise la catégorie inconnue « {c} »",
                        t.id
                    ));
                }
            }
            if let Some(s) = &t.status
                && self.status(s).is_none()
            {
                problems.push(format!(
                    "le modèle « {} » utilise le statut inconnu « {s} »",
                    t.id
                ));
            }
        }
        problems
    }

    pub fn category(&self, id: &str) -> Option<&Category> {
        self.categories.iter().find(|c| c.id == id)
    }

    pub fn status(&self, id: &str) -> Option<&Status> {
        self.statuses.iter().find(|s| s.id == id)
    }

    pub fn link_type(&self, id: &str) -> Option<&LinkType> {
        self.link_types.iter().find(|l| l.id == id)
    }

    pub fn field(&self, id: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.id == id)
    }
}

fn check_ids<'a>(problems: &mut Vec<String>, list: &str, ids: impl Iterator<Item = &'a str>) {
    let mut seen = HashSet::new();
    for id in ids {
        if id.trim().is_empty() {
            problems.push(format!("`{list}` : un identifiant est vide"));
        } else if id.chars().any(char::is_whitespace) {
            problems.push(format!(
                "`{list}` : l'identifiant « {id} » contient un espace"
            ));
        } else if !seen.insert(id) {
            problems.push(format!("`{list}` : identifiant « {id} » en double"));
        }
    }
}

/// Un pack trouvé sur le disque (valide ou non).
#[derive(Debug)]
pub struct DiscoveredPack {
    pub dir: PathBuf,
    pub result: Result<Pack, PackError>,
}

/// Cherche les packs (`<racine>/<pack>/pack.json`) dans chaque racine, dans
/// l'ordre. Un pack dont l'`id` a déjà été trouvé dans une racine précédente
/// est ignoré (les premières racines sont prioritaires).
pub fn discover_packs(roots: &[PathBuf]) -> Vec<DiscoveredPack> {
    let mut found = Vec::new();
    let mut ids = HashSet::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.join(PACK_FILE).is_file())
            .collect();
        dirs.sort();
        for dir in dirs {
            let result = Pack::load_dir(&dir);
            if let Ok(p) = &result
                && !ids.insert(p.id.clone())
            {
                continue;
            }
            found.push(DiscoveredPack { dir, result });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r###"{
      "version": 1,
      "id": "test",
      "name": "Test",
      "categories": [ { "id": "piece", "label": "Pièce", "color": "#4aa3ff", "icon": "🚪" } ],
      "link_types": [ { "id": "indique", "label": "indique", "color": "#ffd23f" } ],
      "statuses": [ { "id": "theorie", "label": "Théorie", "color": "#9ca3af" } ],
      "fields": [ { "id": "jour", "label": "Jour", "type": "number" } ],
      "templates": [ { "id": "piece", "label": "Nouvelle pièce", "categories": ["piece"], "body": "## D\n" } ]
    }"###;

    fn p() -> &'static Path {
        Path::new("pack.json")
    }

    #[test]
    fn parses_valid_pack() {
        let pack = Pack::from_json(VALID, p()).unwrap();
        assert_eq!(pack.categories[0].color, Color([0x4a, 0xa3, 0xff, 0xff]));
        assert!(pack.link_types[0].directed);
        assert_eq!(pack.fields[0].kind, FieldKind::Number);
        assert!(pack.category("piece").is_some());
    }

    #[test]
    fn missing_and_unsupported_version() {
        let no_version = VALID.replace("\"version\": 1,", "");
        assert!(matches!(
            Pack::from_json(&no_version, p()),
            Err(PackError::MissingVersion { .. })
        ));
        let v2 = VALID.replace("\"version\": 1", "\"version\": 2");
        assert!(matches!(
            Pack::from_json(&v2, p()),
            Err(PackError::UnsupportedVersion { found: 2, .. })
        ));
    }

    #[test]
    fn unknown_field_rejected_with_position() {
        let bad = VALID.replace("\"icon\": \"🚪\"", "\"icone\": \"🚪\"");
        let err = Pack::from_json(&bad, p()).unwrap_err().to_string();
        assert!(err.contains("icone"), "{err}");
        assert!(err.contains("line"), "{err}");
    }

    #[test]
    fn bad_color_rejected() {
        let bad = VALID.replace("#4aa3ff", "bleu");
        let err = Pack::from_json(&bad, p()).unwrap_err().to_string();
        assert!(err.contains("couleur « bleu »"), "{err}");
    }

    #[test]
    fn inconsistencies_reported() {
        let bad = VALID
            .replace(r#""categories": ["piece"]"#, r#""categories": ["inconnue"]"#)
            .replace(r#""id": "indique""#, r#""id": "theorie""#)
            .replace(
                r##"{ "id": "theorie", "label": "Théorie", "color": "#9ca3af" }"##,
                r##"{ "id": "a", "label": "A", "color": "#000000" }, { "id": "a", "label": "B", "color": "#000000" }"##,
            );
        let Err(PackError::Invalid { problems, .. }) = Pack::from_json(&bad, p()) else {
            panic!("attendu Invalid");
        };
        assert!(
            problems.iter().any(|m| m.contains("inconnue")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|m| m.contains("en double")),
            "{problems:?}"
        );
    }

    #[test]
    fn color_round_trip() {
        assert_eq!(Color::parse("#0a0B0c").unwrap().to_string(), "#0a0b0c");
        assert_eq!(Color::parse("#01020304").unwrap().0, [1, 2, 3, 4]);
        assert!(Color::parse("#12345").is_none());
        assert!(Color::parse("123456").is_none());
        assert!(Color::parse("#éé1234").is_none());
    }

    #[test]
    fn bundled_blue_prince_pack_is_valid() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/blue_prince");
        let pack = Pack::load_dir(&dir).unwrap();
        assert_eq!(pack.id, "blue_prince");
    }

    #[test]
    fn discover_skips_duplicates_and_reports_errors() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::create_dir(a.path().join("t")).unwrap();
        std::fs::write(a.path().join("t").join(PACK_FILE), VALID).unwrap();
        std::fs::create_dir(b.path().join("t2")).unwrap();
        std::fs::write(b.path().join("t2").join(PACK_FILE), VALID).unwrap();
        std::fs::create_dir(b.path().join("bad")).unwrap();
        std::fs::write(b.path().join("bad").join(PACK_FILE), "{").unwrap();
        let found = discover_packs(&[a.path().to_owned(), b.path().to_owned()]);
        assert_eq!(found.len(), 2);
        assert!(found[0].result.is_ok());
        assert!(found[1].result.is_err());
    }
}
