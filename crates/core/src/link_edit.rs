//! Modification des liens d'une fiche (création, type, note, suppression),
//! qu'ils soient écrits dans l'en-tête (`links:`) ou dans le corps
//! (`[[Titre|type]]`).
//!
//! Un lien affiché (voir [`crate::links::merge`]) peut provenir des deux
//! endroits à la fois ; ces fonctions agissent sur toutes ses sources :
//! - **création** : toujours dans l'en-tête (le texte n'est pas touché) ;
//! - **changement de type** : en-tête et `[[Titre|type]]` du corps ;
//! - **note** : seulement dans l'en-tête (le corps n'a pas de note) ; un
//!   lien du corps qui reçoit une note gagne une entrée d'en-tête ;
//! - **suppression** : entrées d'en-tête retirées, liens du corps
//!   remplacés par leur texte (`[[Coffre|ouvre]]` → `Coffre`), pour ne pas
//!   perdre la phrase qui les contenait.

use crate::id::NoteId;
use crate::markdown;
use crate::note::{MetaLink, NoteMeta};

fn clean_note(note: Option<String>) -> Option<String> {
    note.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty())
}

/// Ajoute un lien dans l'en-tête. Si un lien de même cible et même type y
/// existe déjà, sa note est mise à jour ; un lien sans type vers la même
/// cible est précisé plutôt que dupliqué.
pub fn add_link(meta: &mut NoteMeta, to: &NoteId, kind: Option<String>, note: Option<String>) {
    let note = clean_note(note);
    if let Some(l) = meta
        .links
        .iter_mut()
        .find(|l| l.to == *to && l.kind == kind)
    {
        if note.is_some() {
            l.note = note;
        }
        return;
    }
    if kind.is_some()
        && let Some(l) = meta
            .links
            .iter_mut()
            .find(|l| l.to == *to && l.kind.is_none())
    {
        l.kind = kind;
        if note.is_some() {
            l.note = note;
        }
        return;
    }
    meta.links.push(MetaLink {
        to: to.clone(),
        kind,
        note,
    });
}

/// Réécrit les liens du corps vers la cible (`resolves(titre)`) dont le
/// type vaut `kind` ; `replace` reçoit le titre écrit et rend le texte de
/// remplacement. `None` si rien n'a changé.
fn rewrite_body(
    body: &str,
    resolves: &impl Fn(&str) -> bool,
    kind: Option<&str>,
    replace: impl Fn(&str) -> String,
) -> Option<String> {
    let mut hits: Vec<_> = markdown::wiki_links(body)
        .into_iter()
        .filter(|l| l.kind.as_deref() == kind && resolves(&l.target))
        .collect();
    if hits.is_empty() {
        return None;
    }
    hits.sort_by_key(|l| std::cmp::Reverse(l.range.start));
    let mut out = body.to_owned();
    for l in hits {
        // Titre tel qu'écrit (casse d'origine).
        let written = body[l.target_range.clone()].to_owned();
        out.replace_range(l.range, &replace(&written));
    }
    Some(out)
}

fn wiki(title: &str, kind: Option<&str>) -> String {
    match kind {
        Some(k) => format!("[[{title}|{k}]]"),
        None => format!("[[{title}]]"),
    }
}

/// Change le type et la note du lien `(cible, old_kind)`. Rend le nouveau
/// corps s'il a changé. `resolves(titre)` dit si un titre du corps désigne
/// la cible.
pub fn update_link(
    meta: &mut NoteMeta,
    body: &str,
    to: &NoteId,
    resolves: impl Fn(&str) -> bool,
    old_kind: Option<&str>,
    new_kind: Option<String>,
    note: Option<String>,
) -> Option<String> {
    let note = clean_note(note);
    let new_body = if old_kind == new_kind.as_deref() {
        None
    } else {
        rewrite_body(body, &resolves, old_kind, |t| wiki(t, new_kind.as_deref()))
    };
    let mut found = false;
    for l in meta
        .links
        .iter_mut()
        .filter(|l| l.to == *to && l.kind.as_deref() == old_kind)
    {
        l.kind = new_kind.clone();
        l.note = note.clone();
        found = true;
    }
    if !found && note.is_some() {
        meta.links.push(MetaLink {
            to: to.clone(),
            kind: new_kind,
            note,
        });
    }
    dedup_meta_links(meta);
    new_body
}

/// Supprime le lien `(cible, kind)`. Les liens sans type vers la même cible
/// sont aussi supprimés quand plus aucun lien typé ne les absorbe (sinon le
/// lien resterait affiché). Rend le nouveau corps s'il a changé.
pub fn remove_link(
    meta: &mut NoteMeta,
    body: &str,
    to: &NoteId,
    resolves: impl Fn(&str) -> bool,
    kind: Option<&str>,
) -> Option<String> {
    meta.links
        .retain(|l| !(l.to == *to && l.kind.as_deref() == kind));
    let mut body_out = rewrite_body(body, &resolves, kind, str::to_owned);
    if kind.is_some() {
        let current = body_out.as_deref().unwrap_or(body);
        let other_typed = meta.links.iter().any(|l| l.to == *to && l.kind.is_some())
            || markdown::wiki_links(current)
                .iter()
                .any(|l| l.kind.is_some() && resolves(&l.target));
        if !other_typed {
            meta.links.retain(|l| !(l.to == *to && l.kind.is_none()));
            if let Some(b) = rewrite_body(current, &resolves, None, str::to_owned) {
                body_out = Some(b);
            }
        }
    }
    body_out
}

/// Fusionne les entrées d'en-tête de même cible et même type (la première
/// note non vide est gardée).
fn dedup_meta_links(meta: &mut NoteMeta) {
    let mut out: Vec<MetaLink> = Vec::with_capacity(meta.links.len());
    for l in meta.links.drain(..) {
        match out.iter_mut().find(|o| o.to == l.to && o.kind == l.kind) {
            Some(o) => {
                if o.note.is_none() {
                    o.note = l.note;
                }
            }
            None => out.push(l),
        }
    }
    meta.links = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> NoteId {
        NoteId::parse(s).unwrap()
    }

    fn meta() -> NoteMeta {
        NoteMeta::new("Salle")
    }

    fn is_coffre(t: &str) -> bool {
        t.eq_ignore_ascii_case("coffre")
    }

    #[test]
    fn add_creates_updates_and_upgrades() {
        let c = id("C");
        let mut m = meta();
        add_link(&mut m, &c, None, Some("  ".into()));
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.links[0].note, None, "note vide ignorée");
        // Un type précise le lien sans type existant.
        add_link(&mut m, &c, Some("ouvre".into()), Some("clé".into()));
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.links[0].kind.as_deref(), Some("ouvre"));
        assert_eq!(m.links[0].note.as_deref(), Some("clé"));
        // Même cible, même type : pas de doublon.
        add_link(&mut m, &c, Some("ouvre".into()), None);
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.links[0].note.as_deref(), Some("clé"));
        // Autre type : second lien.
        add_link(&mut m, &c, Some("indique".into()), None);
        assert_eq!(m.links.len(), 2);
    }

    #[test]
    fn update_changes_type_in_meta_and_body() {
        let c = id("C");
        let mut m = meta();
        add_link(&mut m, &c, Some("ouvre".into()), None);
        let body =
            "Voir [[Coffre|ouvre]] et [[coffre|ouvre]], pas [[Autre|ouvre]] ni `[[Coffre|ouvre]]`.";
        let b = update_link(
            &mut m,
            body,
            &c,
            is_coffre,
            Some("ouvre"),
            Some("indique".into()),
            Some("via la clé".into()),
        )
        .unwrap();
        assert_eq!(
            b,
            "Voir [[Coffre|indique]] et [[coffre|indique]], pas [[Autre|ouvre]] ni `[[Coffre|ouvre]]`."
        );
        assert_eq!(m.links[0].kind.as_deref(), Some("indique"));
        assert_eq!(m.links[0].note.as_deref(), Some("via la clé"));
        // Retirer le type.
        let b = update_link(&mut m, &b, &c, is_coffre, Some("indique"), None, None).unwrap();
        assert!(b.starts_with("Voir [[Coffre]] et [[coffre]]"));
        assert_eq!(m.links[0].kind, None);
        assert_eq!(m.links[0].note, None);
    }

    #[test]
    fn note_on_body_only_link_adds_meta_entry() {
        let c = id("C");
        let mut m = meta();
        let body = "[[Coffre|ouvre]]";
        let b = update_link(
            &mut m,
            body,
            &c,
            is_coffre,
            Some("ouvre"),
            Some("ouvre".into()),
            Some("note".into()),
        );
        assert_eq!(b, None, "corps inchangé");
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.links[0].kind.as_deref(), Some("ouvre"));
        assert_eq!(m.links[0].note.as_deref(), Some("note"));
    }

    #[test]
    fn update_merges_into_existing_same_type() {
        let c = id("C");
        let mut m = meta();
        add_link(&mut m, &c, Some("ouvre".into()), Some("a".into()));
        add_link(&mut m, &c, Some("indique".into()), None);
        update_link(
            &mut m,
            "",
            &c,
            is_coffre,
            Some("indique"),
            Some("ouvre".into()),
            None,
        );
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.links[0].note.as_deref(), Some("a"));
    }

    #[test]
    fn remove_unwraps_body_links_and_absorbed_untyped() {
        let c = id("C");
        let mut m = meta();
        add_link(&mut m, &c, Some("ouvre".into()), None);
        m.links.push(MetaLink {
            to: c.clone(),
            kind: None,
            note: None,
        });
        let body = "Le [[Coffre|ouvre]] puis [[coffre]] et [[Autre]].";
        let b = remove_link(&mut m, body, &c, is_coffre, Some("ouvre")).unwrap();
        assert_eq!(b, "Le Coffre puis coffre et [[Autre]].");
        assert!(m.links.is_empty());
    }

    #[test]
    fn remove_keeps_untyped_still_absorbed_by_other_type() {
        let c = id("C");
        let mut m = meta();
        add_link(&mut m, &c, Some("ouvre".into()), None);
        add_link(&mut m, &c, Some("indique".into()), None);
        let body = "[[Coffre]]";
        let b = remove_link(&mut m, body, &c, is_coffre, Some("ouvre"));
        assert_eq!(b, None);
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.links[0].kind.as_deref(), Some("indique"));
        // Lien sans type supprimé directement.
        let b = remove_link(&mut m, body, &c, is_coffre, None).unwrap();
        assert_eq!(b, "Coffre");
    }
}
