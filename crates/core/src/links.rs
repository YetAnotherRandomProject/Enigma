//! Index des liens entre fiches, mis à jour de façon incrémentale.
//!
//! Pour chaque fiche source, l'index retient ses liens « bruts » : ceux du
//! corps (`[[Titre]]`, résolus par titre) et ceux de l'en-tête (`links:`,
//! résolus par identifiant). Deux index inverses (titre en minuscules →
//! sources, identifiant → sources) donnent les rétroliens sans parcourir
//! tout le projet. Modifier une fiche ne réindexe que cette fiche, et un
//! renommage n'invalide rien : la résolution titre → fiche se fait à la
//! lecture.

use std::collections::{HashMap, HashSet};

use crate::id::NoteId;
use crate::markdown;
use crate::note::NoteMeta;

/// Cible d'un lien brut.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RawTarget {
    /// Titre en minuscules (lien du corps).
    Title(String),
    /// Identifiant (lien de l'en-tête).
    Id(NoteId),
}

/// Lien tel qu'écrit dans une fiche, avant résolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawLink {
    pub target: RawTarget,
    /// Texte de la cible tel qu'écrit (titre ou identifiant).
    pub written: String,
    pub kind: Option<String>,
    pub in_meta: bool,
    pub note: Option<String>,
}

/// Index des liens de tout un projet.
#[derive(Debug, Default)]
pub struct LinkIndex {
    out: HashMap<NoteId, Vec<RawLink>>,
    by_title: HashMap<String, HashSet<NoteId>>,
    by_id: HashMap<NoteId, HashSet<NoteId>>,
}

impl LinkIndex {
    /// (Ré)indexe les liens d'une fiche.
    pub fn update(&mut self, source: &NoteId, meta: &NoteMeta, body: &str) {
        self.remove(source);
        let mut raw = Vec::new();
        for l in markdown::wiki_links(body) {
            raw.push(RawLink {
                target: RawTarget::Title(l.target.to_lowercase()),
                written: l.target,
                kind: l.kind,
                in_meta: false,
                note: None,
            });
        }
        for l in &meta.links {
            raw.push(RawLink {
                target: RawTarget::Id(l.to.clone()),
                written: l.to.to_string(),
                kind: l.kind.clone(),
                in_meta: true,
                note: l.note.clone(),
            });
        }
        for l in &raw {
            match &l.target {
                RawTarget::Title(t) => self.by_title.entry(t.clone()).or_default(),
                RawTarget::Id(id) => self.by_id.entry(id.clone()).or_default(),
            }
            .insert(source.clone());
        }
        if !raw.is_empty() {
            self.out.insert(source.clone(), raw);
        }
    }

    /// Retire une fiche de l'index (ses liens sortants).
    pub fn remove(&mut self, source: &NoteId) {
        let Some(old) = self.out.remove(source) else {
            return;
        };
        for l in old {
            match l.target {
                RawTarget::Title(t) => {
                    if let Some(set) = self.by_title.get_mut(&t) {
                        set.remove(source);
                        if set.is_empty() {
                            self.by_title.remove(&t);
                        }
                    }
                }
                RawTarget::Id(id) => {
                    if let Some(set) = self.by_id.get_mut(&id) {
                        set.remove(source);
                        if set.is_empty() {
                            self.by_id.remove(&id);
                        }
                    }
                }
            }
        }
    }

    /// Liens bruts d'une fiche.
    pub fn raw_outgoing(&self, source: &NoteId) -> &[RawLink] {
        self.out.get(source).map_or(&[], Vec::as_slice)
    }

    /// Fiches dont le corps contient un lien vers ce titre (en minuscules).
    pub fn sources_by_title(&self, title_lower: &str) -> impl Iterator<Item = &NoteId> {
        self.by_title.get(title_lower).into_iter().flatten()
    }

    /// Fiches dont l'en-tête contient un lien vers cet identifiant.
    pub fn sources_by_id(&self, id: &NoteId) -> impl Iterator<Item = &NoteId> {
        self.by_id.get(id).into_iter().flatten()
    }

    /// Nombre total de liens bruts (pour les diagnostics).
    pub fn len(&self) -> usize {
        self.out.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }
}

/// L'autre extrémité d'un lien résolu.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LinkEnd {
    Note(NoteId),
    /// Cible introuvable : titre ou identifiant tel qu'écrit.
    Broken(String),
}

/// Lien résolu, après fusion des doublons corps / en-tête.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLink {
    pub other: LinkEnd,
    pub kind: Option<String>,
    /// Présent dans le corps (`[[...]]`).
    pub in_body: bool,
    /// Présent dans l'en-tête (`links:`).
    pub in_meta: bool,
    pub note: Option<String>,
}

/// Fusionne des liens vers des extrémités données :
/// - même extrémité et même type → un seul lien ;
/// - un lien sans type vers une extrémité qui a aussi un lien typé est
///   absorbé par ce dernier (c'est la même relation, précisée ailleurs).
pub fn merge(links: impl IntoIterator<Item = (LinkEnd, RawLink)>) -> Vec<ResolvedLink> {
    let mut merged: Vec<ResolvedLink> = Vec::new();
    for (end, raw) in links {
        if let Some(existing) = merged
            .iter_mut()
            .find(|m| m.other == end && m.kind == raw.kind)
        {
            existing.in_body |= !raw.in_meta;
            existing.in_meta |= raw.in_meta;
            if existing.note.is_none() {
                existing.note = raw.note;
            }
            continue;
        }
        merged.push(ResolvedLink {
            other: end,
            kind: raw.kind,
            in_body: !raw.in_meta,
            in_meta: raw.in_meta,
            note: raw.note,
        });
    }
    // Absorption des liens sans type.
    let mut i = 0;
    while i < merged.len() {
        if merged[i].kind.is_none()
            && let Some(j) = merged
                .iter()
                .position(|m| m.kind.is_some() && m.other == merged[i].other)
        {
            let untyped = merged.remove(i);
            let j = if j > i { j - 1 } else { j };
            merged[j].in_body |= untyped.in_body;
            merged[j].in_meta |= untyped.in_meta;
            continue;
        }
        i += 1;
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::MetaLink;

    fn id(s: &str) -> NoteId {
        NoteId::parse(s).unwrap()
    }

    fn meta_with_links(links: &[(&str, Option<&str>)]) -> NoteMeta {
        let mut m = NoteMeta::new("x");
        m.links = links
            .iter()
            .map(|(to, kind)| MetaLink {
                to: id(to),
                kind: kind.map(str::to_owned),
                note: None,
            })
            .collect();
        m
    }

    #[test]
    fn update_and_reverse_lookups() {
        let mut idx = LinkIndex::default();
        let a = id("A");
        idx.update(
            &a,
            &meta_with_links(&[("B", Some("ouvre"))]),
            "Voir [[Coffre]] et [[coffre|indique]]",
        );
        assert_eq!(idx.raw_outgoing(&a).len(), 3);
        assert_eq!(idx.sources_by_title("coffre").collect::<Vec<_>>(), vec![&a]);
        assert_eq!(idx.sources_by_id(&id("B")).collect::<Vec<_>>(), vec![&a]);

        // Réindexation : les anciennes entrées disparaissent.
        idx.update(&a, &NoteMeta::new("x"), "plus rien");
        assert!(idx.raw_outgoing(&a).is_empty());
        assert_eq!(idx.sources_by_title("coffre").count(), 0);
        assert_eq!(idx.sources_by_id(&id("B")).count(), 0);
        assert!(idx.is_empty());
    }

    #[test]
    fn remove_keeps_other_sources() {
        let mut idx = LinkIndex::default();
        idx.update(&id("A"), &NoteMeta::new("a"), "[[C]]");
        idx.update(&id("B"), &NoteMeta::new("b"), "[[c]]");
        idx.remove(&id("A"));
        assert_eq!(
            idx.sources_by_title("c").collect::<Vec<_>>(),
            vec![&id("B")]
        );
    }

    fn raw(kind: Option<&str>, in_meta: bool) -> RawLink {
        RawLink {
            target: RawTarget::Title("t".into()),
            written: "T".into(),
            kind: kind.map(str::to_owned),
            in_meta,
            note: in_meta.then(|| "note".to_owned()),
        }
    }

    #[test]
    fn merge_dedupes_same_kind_across_body_and_meta() {
        let end = LinkEnd::Note(id("T"));
        let merged = merge([
            (end.clone(), raw(Some("ouvre"), false)),
            (end.clone(), raw(Some("ouvre"), true)),
        ]);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].in_body && merged[0].in_meta);
        assert_eq!(merged[0].note.as_deref(), Some("note"));
    }

    #[test]
    fn merge_absorbs_untyped_into_typed() {
        let end = LinkEnd::Note(id("T"));
        let other = LinkEnd::Broken("x".into());
        let merged = merge([
            (end.clone(), raw(None, false)),
            (other.clone(), raw(None, false)),
            (end.clone(), raw(Some("indique"), true)),
            (end.clone(), raw(Some("contredit"), false)),
        ]);
        assert_eq!(merged.len(), 3);
        let typed: Vec<_> = merged.iter().filter(|m| m.other == end).collect();
        assert_eq!(typed.len(), 2);
        assert!(typed[0].in_body, "le lien sans type est absorbé");
        assert!(merged.iter().any(|m| m.other == other && m.kind.is_none()));
    }
}
