//! Identifiant stable d'une fiche.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Identifiant stable et unique d'une fiche.
///
/// Les nouveaux identifiants sont des ULID, mais toute chaîne non vide écrite
/// à la main dans un fichier est acceptée. Le clonage est bon marché.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NoteId(Arc<str>);

impl NoteId {
    /// Génère un nouvel identifiant (ULID).
    pub fn generate() -> Self {
        Self(Arc::from(ulid::Ulid::generate().to_string()))
    }

    /// Construit un identifiant depuis une chaîne ; `None` si elle est vide.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        (!s.is_empty()).then(|| Self(Arc::from(s)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NoteId({})", self.0)
    }
}

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for NoteId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NoteId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        NoteId::parse(&s).ok_or_else(|| serde::de::Error::custom("identifiant vide"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_unique_and_26_chars() {
        let a = NoteId::generate();
        let b = NoteId::generate();
        assert_ne!(a, b);
        assert_eq!(a.as_str().len(), 26);
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(NoteId::parse("  ").is_none());
        assert_eq!(
            NoteId::parse(" abc ").map(|i| i.to_string()),
            Some("abc".into())
        );
    }
}
