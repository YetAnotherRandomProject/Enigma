//! Format d'une fiche : un en-tête YAML optionnel suivi d'un corps Markdown.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::id::NoteId;

/// Métadonnées d'une fiche (l'en-tête YAML).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteMeta {
    #[serde(default = "NoteId::generate")]
    pub id: NoteId,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, FieldValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<MetaLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<Pos>,
    /// Clés inconnues, conservées telles quelles à la réécriture.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
}

impl NoteMeta {
    /// Métadonnées par défaut avec un nouvel identifiant.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: NoteId::generate(),
            title: title.into(),
            categories: Vec::new(),
            status: None,
            fields: BTreeMap::new(),
            links: Vec::new(),
            pos: None,
            extra: BTreeMap::new(),
        }
    }

    /// Ajoute la catégorie si absente, la retire sinon.
    pub fn toggle_category(&mut self, category: &str) {
        if let Some(i) = self.categories.iter().position(|c| c == category) {
            self.categories.remove(i);
        } else {
            self.categories.push(category.to_owned());
        }
    }

    pub fn has_category(&self, category: &str) -> bool {
        self.categories.iter().any(|c| c == category)
    }
}

/// Valeur d'un champ personnalisé.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    /// Toute autre structure YAML (liste, table…), conservée telle quelle.
    Other(serde_yaml_ng::Value),
}

impl FieldValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            FieldValue::Int(i) => Some(*i as f64),
            FieldValue::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Représentation courte pour l'affichage.
    pub fn display(&self) -> String {
        match self {
            FieldValue::Bool(b) => if *b { "oui" } else { "non" }.to_owned(),
            FieldValue::Int(i) => i.to_string(),
            FieldValue::Float(f) => f.to_string(),
            FieldValue::Text(s) => s.clone(),
            FieldValue::Other(v) => serde_yaml_ng::to_string(v)
                .map(|s| s.trim().to_owned())
                .unwrap_or_default(),
        }
    }
}

/// Lien déclaré dans les métadonnées.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetaLink {
    pub to: NoteId,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Position d'une fiche dans le graphe.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub z: f32,
    #[serde(default)]
    pub pinned: bool,
}

/// Une fiche complète.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub meta: NoteMeta,
    /// Corps Markdown, fins de ligne normalisées en `\n`.
    pub body: String,
    /// Le fichier d'origine avait-il un en-tête ? Sinon il sera ajouté à la
    /// première sauvegarde.
    pub had_frontmatter: bool,
    /// Le fichier d'origine utilisait-il des fins de ligne `\r\n` ?
    pub crlf: bool,
}

/// Erreur de lecture d'une fiche.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum NoteParseError {
    #[error("l'en-tête commence par `---` mais n'est jamais refermé")]
    UnclosedFrontmatter,
    #[error("en-tête YAML invalide : {0}")]
    Yaml(String),
}

/// Erreur d'écriture d'une fiche.
#[derive(Debug, thiserror::Error)]
#[error("impossible de sérialiser l'en-tête : {0}")]
pub struct NoteSerializeError(String);

impl Note {
    /// Nouvelle fiche vide.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            meta: NoteMeta::new(title),
            body: String::new(),
            had_frontmatter: true,
            crlf: false,
        }
    }

    /// Lit une fiche. `fallback_title` (en général le nom du fichier) est
    /// utilisé si l'en-tête ne donne pas de titre.
    pub fn parse(text: &str, fallback_title: &str) -> Result<Note, NoteParseError> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let crlf = text.contains("\r\n");
        let normalized;
        let text = if crlf {
            normalized = text.replace("\r\n", "\n");
            normalized.as_str()
        } else {
            text
        };

        let (meta, body, had_frontmatter) = match split_frontmatter(text)? {
            Some((yaml, body)) => (parse_meta(yaml)?, body, true),
            None => (NoteMeta::new(String::new()), text, false),
        };
        let mut meta = meta;
        if meta.title.trim().is_empty() {
            meta.title = fallback_title.to_owned();
        }
        Ok(Note {
            meta,
            body: body.to_owned(),
            had_frontmatter,
            crlf,
        })
    }

    /// Produit le contenu complet du fichier (en-tête toujours présent).
    pub fn serialize(&self) -> Result<String, NoteSerializeError> {
        let yaml =
            serde_yaml_ng::to_string(&self.meta).map_err(|e| NoteSerializeError(e.to_string()))?;
        let mut out = String::with_capacity(yaml.len() + self.body.len() + 16);
        out.push_str("---\n");
        out.push_str(&yaml);
        if !yaml.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("---\n");
        out.push_str(&self.body);
        if self.crlf {
            out = out.replace('\n', "\r\n");
        }
        Ok(out)
    }
}

/// Sépare l'en-tête du corps. `Ok(None)` si le texte n'a pas d'en-tête.
fn split_frontmatter(text: &str) -> Result<Option<(&str, &str)>, NoteParseError> {
    let Some(rest) = text.strip_prefix("---") else {
        return Ok(None);
    };
    // La première ligne doit être exactement `---` (espaces finaux tolérés).
    let (first_line_rest, after_first) = match rest.find('\n') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    if !first_line_rest.trim().is_empty() {
        return Ok(None);
    }

    let mut offset = 0;
    for line in after_first.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            let yaml = &after_first[..offset];
            let body = &after_first[offset + line.len()..];
            return Ok(Some((yaml, body)));
        }
        offset += line.len();
    }
    Err(NoteParseError::UnclosedFrontmatter)
}

fn parse_meta(yaml: &str) -> Result<NoteMeta, NoteParseError> {
    let is_empty = yaml
        .lines()
        .all(|l| l.trim().is_empty() || l.trim_start().starts_with('#'));
    if is_empty {
        return Ok(NoteMeta::new(String::new()));
    }
    serde_yaml_ng::from_str(yaml).map_err(|e| NoteParseError::Yaml(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---
id: 01HXYZ
title: Salle des Archives
categories: [piece, indice]
status: theorie
fields: { jour_decouverte: 3, note: \"lourd\", visite: true, ratio: 0.5 }
links:
  - { to: 01HABC, type: indique, note: \"via le code du coffre\" }
pos: { x: 120.0, y: -40.0, z: 0.0, pinned: true }
custom_key: [1, 2]
---
Texte avec [[Autre fiche]].
";

    #[test]
    fn parses_full_frontmatter() {
        let n = Note::parse(SAMPLE, "fallback").unwrap();
        assert!(n.had_frontmatter);
        assert_eq!(n.meta.id.as_str(), "01HXYZ");
        assert_eq!(n.meta.title, "Salle des Archives");
        assert_eq!(n.meta.categories, vec!["piece", "indice"]);
        assert_eq!(n.meta.status.as_deref(), Some("theorie"));
        assert_eq!(n.meta.fields["jour_decouverte"], FieldValue::Int(3));
        assert_eq!(n.meta.fields["note"], FieldValue::Text("lourd".into()));
        assert_eq!(n.meta.fields["visite"], FieldValue::Bool(true));
        assert_eq!(n.meta.fields["ratio"], FieldValue::Float(0.5));
        assert_eq!(n.meta.links.len(), 1);
        assert_eq!(n.meta.links[0].kind.as_deref(), Some("indique"));
        let pos = n.meta.pos.unwrap();
        assert!(pos.pinned);
        assert_eq!(pos.x, 120.0);
        assert!(n.meta.extra.contains_key("custom_key"));
        assert_eq!(n.body, "Texte avec [[Autre fiche]].\n");
    }

    #[test]
    fn round_trip_preserves_everything() {
        let n = Note::parse(SAMPLE, "fallback").unwrap();
        let out = n.serialize().unwrap();
        let again = Note::parse(&out, "fallback").unwrap();
        assert_eq!(n, again);
        assert!(out.contains("custom_key"));
    }

    #[test]
    fn file_without_frontmatter_gets_defaults() {
        let n = Note::parse("# Juste du texte\n", "Ma fiche").unwrap();
        assert!(!n.had_frontmatter);
        assert_eq!(n.meta.title, "Ma fiche");
        assert_eq!(n.body, "# Juste du texte\n");
        let out = n.serialize().unwrap();
        assert!(out.starts_with("---\nid: "));
        assert!(out.ends_with("---\n# Juste du texte\n"));
    }

    #[test]
    fn empty_frontmatter_is_accepted() {
        let n = Note::parse("---\n---\ncorps", "T").unwrap();
        assert!(n.had_frontmatter);
        assert_eq!(n.meta.title, "T");
        assert_eq!(n.body, "corps");
    }

    #[test]
    fn horizontal_rule_is_not_frontmatter() {
        let n = Note::parse("---- \ntexte", "T").unwrap();
        assert!(!n.had_frontmatter);
        assert_eq!(n.body, "---- \ntexte");
    }

    #[test]
    fn unclosed_frontmatter_is_an_error() {
        assert_eq!(
            Note::parse("---\ntitle: x\nbody", "T"),
            Err(NoteParseError::UnclosedFrontmatter)
        );
    }

    #[test]
    fn invalid_yaml_is_an_error() {
        let r = Note::parse("---\ntitle: [unclosed\n---\n", "T");
        assert!(matches!(r, Err(NoteParseError::Yaml(_))));
    }

    #[test]
    fn crlf_is_normalized_and_restored() {
        let src = "---\r\ntitle: A\r\nid: X\r\n---\r\nligne 1\r\nligne 2\r\n";
        let n = Note::parse(src, "T").unwrap();
        assert!(n.crlf);
        assert_eq!(n.body, "ligne 1\nligne 2\n");
        let out = n.serialize().unwrap();
        assert!(out.contains("\r\nligne 1\r\nligne 2\r\n"));
        assert!(!out.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn bom_is_stripped() {
        let n = Note::parse("\u{feff}---\ntitle: A\n---\nx", "T").unwrap();
        assert_eq!(n.meta.title, "A");
    }

    #[test]
    fn toggle_category() {
        let mut m = NoteMeta::new("x");
        m.toggle_category("piece");
        assert!(m.has_category("piece"));
        m.toggle_category("piece");
        assert!(!m.has_category("piece"));
    }
}
