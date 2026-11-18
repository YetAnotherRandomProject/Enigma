//! Mentions non liées : fiches dont le texte cite le titre d'une autre
//! fiche sans en faire un lien `[[...]]`. Utile pour relier des indices
//! après coup.

use std::ops::Range;

use crate::search::fold;
use crate::{NoteId, Project};

/// Titres plus courts ignorés (trop de fausses mentions).
const MIN_TITLE: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mention {
    /// Fiche qui cite le titre.
    pub from: NoteId,
    pub from_title: String,
    /// Position du texte cité dans le corps de `from` (octets).
    pub range: Range<usize>,
    /// Ligne où il apparaît.
    pub line: String,
    /// Le texte cité, dans `line`.
    pub in_line: Range<usize>,
}

/// Plages de code (blocs et en ligne) du texte.
fn code_ranges(text: &str) -> Vec<Range<usize>> {
    use pulldown_cmark::{Event, Parser, Tag};
    Parser::new_ext(text, crate::markdown::options())
        .into_offset_iter()
        .filter_map(|(e, r)| match e {
            Event::Start(Tag::CodeBlock(_)) | Event::Code(_) => Some(r),
            _ => None,
        })
        .collect()
}

/// Occurrences de `needle` (replié) en mot entier dans `text`.
fn word_matches(text: &str, needle: &[char]) -> Vec<Range<usize>> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let folded: Vec<char> = chars
        .iter()
        .map(|(_, c)| crate::search::fold_char(*c))
        .collect();
    let mut out = Vec::new();
    let n = needle.len();
    let mut i = 0;
    while n > 0 && i + n <= folded.len() {
        let before_ok = i == 0 || !folded[i - 1].is_alphanumeric();
        let after_ok = i + n == folded.len() || !folded[i + n].is_alphanumeric();
        if before_ok && after_ok && folded[i..i + n] == *needle {
            let start = chars[i].0;
            let end = chars.get(i + n).map_or(text.len(), |(b, _)| *b);
            out.push(start..end);
            i += n;
        } else {
            i += 1;
        }
    }
    out
}

impl Project {
    /// Mentions du titre de `id` dans les autres fiches, hors liens et
    /// code.
    pub fn unlinked_mentions(&self, id: &NoteId) -> Vec<Mention> {
        let Some(target) = self.note(id) else {
            return Vec::new();
        };
        let title = fold(target.meta.title.trim());
        if title.len() < MIN_TITLE {
            return Vec::new();
        }
        let mut out = Vec::new();
        for other in self.ids() {
            if other == id {
                continue;
            }
            let Some(note) = self.note(other) else {
                continue;
            };
            let body = &note.body;
            let hits = word_matches(body, &title);
            if hits.is_empty() {
                continue;
            }
            let mut excluded: Vec<Range<usize>> = crate::markdown::wiki_links(body)
                .into_iter()
                .map(|l| l.range)
                .collect();
            excluded.extend(code_ranges(body));
            for r in hits {
                if excluded.iter().any(|x| x.start < r.end && r.start < x.end) {
                    continue;
                }
                let ls = body[..r.start].rfind('\n').map_or(0, |i| i + 1);
                let le = body[r.end..].find('\n').map_or(body.len(), |i| r.end + i);
                out.push(Mention {
                    from: other.clone(),
                    from_title: note.meta.title.clone(),
                    line: body[ls..le].to_owned(),
                    in_line: r.start - ls..r.end - ls,
                    range: r,
                });
            }
        }
        out.sort_by(|a, b| {
            a.from_title
                .to_lowercase()
                .cmp(&b.from_title.to_lowercase())
                .then(a.range.start.cmp(&b.range.start))
        });
        out
    }

    /// Transforme une mention en lien `[[...]]` (le texte cité est gardé tel
    /// qu'écrit ; la résolution des titres ignore la casse).
    pub fn link_mention(&mut self, m: &Mention) -> Result<(), crate::project::ProjectError> {
        let Some(note) = self.note(&m.from) else {
            return Ok(());
        };
        let Some(written) = note.body.get(m.range.clone()) else {
            return Ok(());
        };
        let mut body = note.body.clone();
        let link = format!("[[{written}]]");
        body.replace_range(m.range.clone(), &link);
        self.set_body(&m.from, body)
    }

    /// Lie toutes les mentions données (de la fin vers le début de chaque
    /// fiche, pour garder les positions valables).
    pub fn link_mentions(
        &mut self,
        mentions: &[Mention],
    ) -> Result<usize, crate::project::ProjectError> {
        let mut sorted: Vec<&Mention> = mentions.iter().collect();
        sorted.sort_by_key(|m| std::cmp::Reverse(m.range.start));
        for m in &sorted {
            self.link_mention(m)?;
        }
        Ok(sorted.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn finds_and_links_mentions() {
        let d = tempfile::tempdir().unwrap();
        let mut p = Project::open(d.path(), &[Path::new("x").to_path_buf()]).unwrap();
        let coffre = p.create_note("Coffre", vec![], String::new()).unwrap();
        let salle = p
            .create_note(
                "Salle",
                vec![],
                "Le coffre est là. [[Coffre]] déjà lié.\n`coffre` en code, coffres au pluriel.\nUn COFFRE encore."
                    .into(),
            )
            .unwrap();
        let m = p.unlinked_mentions(&coffre);
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!(&m[0].line[m[0].in_line.clone()], "coffre");
        assert_eq!(m[1].line, "Un COFFRE encore.");
        assert!(p.unlinked_mentions(&salle).is_empty());
        assert_eq!(p.link_mentions(&m).unwrap(), 2);
        let body = &p.note(&salle).unwrap().body;
        assert!(body.starts_with("Le [[coffre]] est là."), "{body}");
        assert!(body.ends_with("Un [[COFFRE]] encore."), "{body}");
        assert!(p.unlinked_mentions(&coffre).is_empty());
        assert_eq!(p.backlinks(&coffre).len(), 1);
    }
}
