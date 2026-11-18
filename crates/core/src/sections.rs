//! Sections et paragraphes d'une fiche, pour les liens
//! `[[Fiche#Titre de section]]` et `[[Fiche#^id]]`, l'insertion
//! `![[Fiche#…]]` et le plan de la fiche.
//!
//! Un paragraphe est repérable par un identifiant en fin de ligne :
//! `Le code est 1945. ^code`.

use std::ops::Range;

use crate::search::fold;

/// Titre de section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    /// Ligne du titre (octets).
    pub line: Range<usize>,
}

fn lines(body: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut start = 0;
    body.split('\n').map(move |l| {
        let r = start..start + l.trim_end_matches('\r').len();
        start += l.len() + 1;
        r
    })
}

/// Titres `#` de la fiche, hors blocs de code délimités.
pub fn headings(body: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    for r in lines(body) {
        let line = &body[r.clone()];
        let t = line.trim_start();
        for f in ["```", "~~~"] {
            if t.starts_with(f) {
                fence = match fence {
                    Some(open) if open == f => None,
                    None => Some(f),
                    other => other,
                };
            }
        }
        if fence.is_some() || line.len() - t.len() > 3 {
            continue;
        }
        let level = t.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&level) && t[level..].starts_with(' ') {
            let text = t[level..].trim().trim_end_matches('#').trim();
            out.push(Heading {
                level: level as u8,
                text: text.to_owned(),
                line: r,
            });
        }
    }
    out
}

/// Identifiant de paragraphe en fin de ligne (`… ^abc`), et sa plage
/// (espace compris).
pub fn block_id(line: &str) -> Option<(&str, Range<usize>)> {
    let t = line.trim_end();
    let caret = t.rfind(" ^")?;
    let id = &t[caret + 2..];
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .then_some((id, caret..t.len()))
}

/// Partie de la fiche visée : toute la fiche (`None`), une section (texte
/// du titre) ou un paragraphe (`^id`). `None` si introuvable.
pub fn locate(body: &str, section: Option<&str>) -> Option<Range<usize>> {
    let Some(section) = section.map(str::trim).filter(|s| !s.is_empty()) else {
        return Some(0..body.len());
    };
    if let Some(id) = section.strip_prefix('^') {
        let line =
            lines(body).find(|r| block_id(&body[r.clone()]).is_some_and(|(b, _)| b == id))?;
        // Le paragraphe : lignes non vides au-dessus jusqu'à une ligne vide.
        let mut start = line.start;
        while start > 0 {
            let prev_end = start - 1;
            let prev_start = body[..prev_end].rfind('\n').map_or(0, |i| i + 1);
            let prev = body[prev_start..prev_end].trim();
            if prev.is_empty() || prev.starts_with('#') {
                break;
            }
            start = prev_start;
        }
        return Some(start..line.end);
    }
    let wanted = fold(section);
    let all = headings(body);
    let i = all.iter().position(|h| fold(&h.text) == wanted)?;
    let h = &all[i];
    let end = all[i + 1..]
        .iter()
        .find(|n| n.level <= h.level)
        .map_or(body.len(), |n| n.line.start.saturating_sub(1));
    Some(h.line.start..end.max(h.line.end))
}

/// Texte d'une section ou d'un paragraphe, sans l'identifiant `^id`.
pub fn excerpt(body: &str, section: Option<&str>) -> Option<String> {
    let r = locate(body, section)?;
    let text = &body[r];
    Some(
        text.split('\n')
            .map(|l| match block_id(l) {
                Some((_, r)) => format!("{}{}", &l[..r.start], &l[r.end..]),
                None => l.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Donne un identifiant au paragraphe de la ligne contenant `byte` (garde
/// celui qui existe). Rend le nouveau texte et l'identifiant.
pub fn ensure_block_id(body: &str, byte: usize) -> (String, String) {
    let byte = byte.min(body.len());
    let start = body[..byte].rfind('\n').map_or(0, |i| i + 1);
    let end = body[byte..].find('\n').map_or(body.len(), |i| byte + i);
    let line = body[start..end].trim_end_matches('\r');
    if let Some((id, _)) = block_id(line) {
        return (body.to_owned(), id.to_owned());
    }
    let id = ulid::Ulid::generate().to_string().to_lowercase();
    let id = id[id.len() - 6..].to_owned();
    let insert_at = start + line.trim_end().len();
    let mut out = body.to_owned();
    out.insert_str(insert_at, &format!(" ^{id}"));
    (out, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "# Manoir\nIntro\n## Salle\nLe coffre.\nCode 1945. ^code\n\n### Détail\nx\n## Serre\nPlantes\n```\n# pas un titre\n```";

    #[test]
    fn finds_headings_and_sections() {
        let h = headings(BODY);
        let names: Vec<&str> = h.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(names, ["Manoir", "Salle", "Détail", "Serre"]);
        assert_eq!(
            &BODY[locate(BODY, Some("salle")).unwrap()],
            "## Salle\nLe coffre.\nCode 1945. ^code\n\n### Détail\nx"
        );
        assert_eq!(
            &BODY[locate(BODY, Some("Serre")).unwrap()],
            "## Serre\nPlantes\n```\n# pas un titre\n```"
        );
        assert!(locate(BODY, Some("Inconnue")).is_none());
        assert_eq!(locate(BODY, None), Some(0..BODY.len()));
    }

    #[test]
    fn block_references() {
        assert_eq!(
            &BODY[locate(BODY, Some("^code")).unwrap()],
            "Le coffre.\nCode 1945. ^code"
        );
        assert_eq!(
            excerpt(BODY, Some("^code")).unwrap(),
            "Le coffre.\nCode 1945."
        );
        let (t, id) = ensure_block_id("a\nUne ligne\nb", 5);
        assert_eq!(id.len(), 6);
        assert_eq!(t, format!("a\nUne ligne ^{id}\nb"));
        let (t2, id2) = ensure_block_id(&t, 4);
        assert_eq!((t2, id2), (t.clone(), id));
        assert!(block_id("prix ^ 3").is_none());
    }
}
