//! Recherche dans tout le projet : titres, texte, catégories, statuts,
//! champs, tags et liens.
//!
//! Syntaxe (insensible à la casse et aux accents) :
//! - `coffre bureau` : les deux mots (ET) ;
//! - `"salle des archives"` : expression exacte ;
//! - `coffre OR clé` : l'un ou l'autre ;
//! - `-majordome` : exclusion ;
//! - `/\d{4}/` : expression régulière ;
//! - `titre:`, `cat:`, `statut:`, `tag:`, `lien:` et `<champ>:valeur`
//!   (identifiant d'un champ du pack) ; la valeur peut être entre
//!   guillemets.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::{NoteId, Project};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SearchError {
    #[error("expression régulière invalide : {0}")]
    Regex(String),
    #[error("guillemet ou barre oblique non fermé")]
    Unclosed,
}

#[derive(Debug)]
enum Term {
    /// Texte replié (minuscules, sans accents).
    Text(Vec<char>),
    Regex(Regex),
    Field {
        key: String,
        value: Vec<char>,
    },
}

#[derive(Debug)]
struct Clause {
    negate: bool,
    /// OU entre ces termes.
    any: Vec<Term>,
}

/// Requête analysée.
#[derive(Debug, Default)]
pub struct Query {
    clauses: Vec<Clause>,
}

/// Ligne du texte où la recherche a trouvé quelque chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineHit {
    /// Numéro de ligne (0 = première).
    pub line: usize,
    /// Octet du début de la première correspondance dans le corps.
    pub byte: usize,
    /// Extrait de la ligne.
    pub text: String,
    /// Correspondances dans `text` (octets).
    pub ranges: Vec<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub id: NoteId,
    pub title: String,
    pub in_title: bool,
    pub lines: Vec<LineHit>,
}

/// Lignes gardées par fiche.
const MAX_LINES: usize = 20;
/// Longueur maximale d'un extrait (caractères).
const SNIPPET: usize = 140;

/// Caractère replié : minuscule sans accent (un caractère pour un, pour
/// garder les positions).
pub fn fold_char(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        '’' => '\'',
        c => c,
    }
}

pub fn fold(s: &str) -> Vec<char> {
    s.chars().map(fold_char).collect()
}

fn find_all(hay: &[char], needle: &[char]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    if needle.is_empty() || needle.len() > hay.len() {
        return out;
    }
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == *needle {
            out.push(i..i + needle.len());
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

fn contains(hay: &str, needle: &[char]) -> bool {
    !find_all(&fold(hay), needle).is_empty()
}

/// Jeton de requête : (exclu, clé, valeur, expression régulière).
type Token = (bool, Option<String>, String, bool);

/// Découpe la requête en jetons.
fn tokens(q: &str) -> Result<Vec<Token>, SearchError> {
    let chars: Vec<char> = q.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let mut negate = false;
        if chars[i] == '-' && i + 1 < chars.len() && !chars[i + 1].is_whitespace() {
            negate = true;
            i += 1;
        }
        // Clé `mot:` (sans espace avant les deux-points).
        let mut key = None;
        let start = i;
        while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        if i < chars.len() && chars[i] == ':' && i > start && i + 1 < chars.len() {
            key = Some(chars[start..i].iter().collect::<String>().to_lowercase());
            i += 1;
        } else {
            i = start;
        }
        let (value, regex) = match chars.get(i) {
            Some('"') => {
                let end = chars[i + 1..]
                    .iter()
                    .position(|&c| c == '"')
                    .ok_or(SearchError::Unclosed)?;
                let v: String = chars[i + 1..i + 1 + end].iter().collect();
                i += end + 2;
                (v, false)
            }
            Some('/') if key.is_none() => {
                let mut j = i + 1;
                let mut v = String::new();
                loop {
                    match chars.get(j) {
                        None => return Err(SearchError::Unclosed),
                        Some('\\') if chars.get(j + 1) == Some(&'/') => {
                            v.push('/');
                            j += 2;
                        }
                        Some('/') => break,
                        Some(&c) => {
                            v.push(c);
                            j += 1;
                        }
                    }
                }
                i = j + 1;
                (v, true)
            }
            _ => {
                let s = i;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                (chars[s..i].iter().collect(), false)
            }
        };
        if value.is_empty() && key.is_none() {
            continue;
        }
        out.push((negate, key, value, regex));
    }
    Ok(out)
}

impl Query {
    pub fn parse(q: &str) -> Result<Query, SearchError> {
        let mut clauses: Vec<Clause> = Vec::new();
        let mut join = false;
        for (negate, key, value, is_regex) in tokens(q)? {
            if key.is_none() && !is_regex && value == "OR" {
                join = !clauses.is_empty();
                continue;
            }
            let term = if is_regex {
                Term::Regex(
                    RegexBuilder::new(&value)
                        .case_insensitive(true)
                        .build()
                        .map_err(|e| SearchError::Regex(e.to_string()))?,
                )
            } else if let Some(key) = key {
                Term::Field {
                    key,
                    value: fold(&value),
                }
            } else {
                Term::Text(fold(&value))
            };
            match clauses.last_mut() {
                Some(last) if join && !negate => last.any.push(term),
                _ => clauses.push(Clause {
                    negate,
                    any: vec![term],
                }),
            }
            join = false;
        }
        Ok(Query { clauses })
    }

    pub fn is_empty(&self) -> bool {
        self.clauses.is_empty()
    }
}

fn term_matches(project: &Project, id: &NoteId, term: &Term) -> bool {
    let Some(note) = project.note(id) else {
        return false;
    };
    let meta = &note.meta;
    match term {
        Term::Text(t) => contains(&meta.title, t) || contains(&note.body, t),
        Term::Regex(r) => r.is_match(&meta.title) || r.is_match(&note.body),
        Term::Field { key, value } => {
            let pack = project.pack();
            match key.as_str() {
                "titre" | "title" => contains(&meta.title, value),
                "cat" | "categorie" | "catégorie" | "category" => {
                    meta.categories.iter().any(|c| {
                        contains(c, value)
                            || pack.category(c).is_some_and(|d| contains(&d.label, value))
                    })
                }
                "statut" | "status" => meta.status.as_deref().is_some_and(|s| {
                    contains(s, value) || pack.status(s).is_some_and(|d| contains(&d.label, value))
                }),
                "tag" => crate::markdown::tags(&note.body)
                    .iter()
                    .any(|t| fold(&t.name).starts_with(value)),
                "lien" | "link" => project.outgoing_links(id).iter().any(|l| {
                    let title = match &l.other {
                        crate::links::LinkEnd::Note(other) => project
                            .note(other)
                            .map(|n| n.meta.title.clone())
                            .unwrap_or_default(),
                        crate::links::LinkEnd::Broken(t) => t.clone(),
                    };
                    contains(&title, value)
                }),
                field => meta.fields.iter().any(|(k, v)| {
                    let label = pack
                        .fields
                        .iter()
                        .find(|f| &f.id == k)
                        .map(|f| f.label.clone())
                        .unwrap_or_default();
                    (fold(k) == fold(field) || fold(&label) == fold(field))
                        && (value.is_empty() || contains(&v.display(), value))
                }),
            }
        }
    }
}

/// Correspondances d'un terme dans une ligne (octets).
fn line_ranges(line: &str, term: &Term) -> Vec<Range<usize>> {
    match term {
        Term::Text(t) => {
            let folded = fold(line);
            let starts: Vec<usize> = line
                .char_indices()
                .map(|(b, _)| b)
                .chain([line.len()])
                .collect();
            find_all(&folded, t)
                .into_iter()
                .map(|r| starts[r.start]..starts[r.end])
                .collect()
        }
        Term::Regex(r) => r
            .find_iter(line)
            .filter(|m| !m.is_empty())
            .map(|m| m.range())
            .collect(),
        Term::Field { .. } => Vec::new(),
    }
}

/// Extrait d'au plus `SNIPPET` caractères autour de la première
/// correspondance ; les plages sont décalées en conséquence.
fn snippet(line: &str, ranges: Vec<Range<usize>>) -> (String, Vec<Range<usize>>) {
    let first = ranges.first().map_or(0, |r| r.start);
    let chars_before = line[..first].chars().count();
    let skip = chars_before.saturating_sub(SNIPPET / 3);
    let start = line.char_indices().nth(skip).map_or(0, |(b, _)| b);
    let end = line[start..]
        .char_indices()
        .nth(SNIPPET)
        .map_or(line.len(), |(b, _)| start + b);
    let prefix = if start > 0 { "…" } else { "" };
    let text = format!("{prefix}{}", &line[start..end]);
    let shift = prefix.len() as isize - start as isize;
    let ranges = ranges
        .into_iter()
        .filter(|r| r.start >= start && r.end <= end)
        .map(|r| (r.start as isize + shift) as usize..(r.end as isize + shift) as usize)
        .collect();
    (text, ranges)
}

impl Project {
    /// Tous les tags du projet (en minuscules) et le nombre de fiches qui
    /// les portent. Un tag `a/b` compte aussi pour `a`.
    pub fn all_tags(&self) -> std::collections::BTreeMap<String, usize> {
        let mut out = std::collections::BTreeMap::new();
        for id in self.ids() {
            let Some(note) = self.note(id) else {
                continue;
            };
            let mut seen = std::collections::BTreeSet::new();
            for t in crate::markdown::tags(&note.body) {
                let name = t.name.to_lowercase();
                let mut prefix = String::new();
                for part in name.split('/') {
                    if !prefix.is_empty() {
                        prefix.push('/');
                    }
                    prefix.push_str(part);
                    seen.insert(prefix.clone());
                }
            }
            for t in seen {
                *out.entry(t).or_insert(0) += 1;
            }
        }
        out
    }

    /// Fiches qui répondent à la requête, avec les lignes trouvées. Tri :
    /// titre trouvé d'abord, puis nombre de lignes, puis titre.
    pub fn search_text(&self, query: &Query) -> Vec<Hit> {
        if query.is_empty() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        for id in self.ids() {
            let ok = query.clauses.iter().all(|c| {
                let any = c.any.iter().any(|t| term_matches(self, id, t));
                any != c.negate
            });
            if !ok {
                continue;
            }
            let Some(note) = self.note(id) else {
                continue;
            };
            let positive: Vec<&Term> = query
                .clauses
                .iter()
                .filter(|c| !c.negate)
                .flat_map(|c| c.any.iter())
                .collect();
            let in_title = positive.iter().any(|t| match t {
                Term::Text(x) => contains(&note.meta.title, x),
                Term::Regex(r) => r.is_match(&note.meta.title),
                Term::Field { .. } => false,
            });
            let mut lines = Vec::new();
            let mut offset = 0;
            for (n, line) in note.body.split('\n').enumerate() {
                let mut ranges: Vec<Range<usize>> =
                    positive.iter().flat_map(|t| line_ranges(line, t)).collect();
                if !ranges.is_empty() && lines.len() < MAX_LINES {
                    ranges.sort_by_key(|r| r.start);
                    let byte = offset + ranges[0].start;
                    let (text, ranges) = snippet(line.trim_end_matches('\r'), ranges);
                    lines.push(LineHit {
                        line: n,
                        byte,
                        text,
                        ranges,
                    });
                }
                offset += line.len() + 1;
            }
            hits.push(Hit {
                id: id.clone(),
                title: note.meta.title.clone(),
                in_title,
                lines,
            });
        }
        hits.sort_by(|a, b| {
            b.in_title
                .cmp(&a.in_title)
                .then(b.lines.len().cmp(&a.lines.len()))
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn project() -> (tempfile::TempDir, Project) {
        let d = tempfile::tempdir().unwrap();
        let mut p = Project::open(d.path(), &[Path::new("x").to_path_buf()]).unwrap();
        p.create_note(
            "Salle des Archives",
            vec!["piece".into()],
            "Le coffre est fermé.\nCode 1945 trouvé #indice\nVoir [[Majordome]]".into(),
        )
        .unwrap();
        p.create_note("Majordome", vec![], "Il garde la clé du coffre.".into())
            .unwrap();
        p.create_note("Serre", vec![], "Des plantes. #indice/plante".into())
            .unwrap();
        (d, p)
    }

    fn titles(p: &Project, q: &str) -> Vec<String> {
        p.search_text(&Query::parse(q).unwrap())
            .into_iter()
            .map(|h| h.title)
            .collect()
    }

    #[test]
    fn words_phrases_or_and_exclusion() {
        let (_d, p) = project();
        assert_eq!(titles(&p, "coffre"), ["Majordome", "Salle des Archives"]);
        assert_eq!(
            titles(&p, "COFFRE ferme"),
            ["Salle des Archives"],
            "accents"
        );
        assert_eq!(titles(&p, "\"garde la clé\""), ["Majordome"]);
        assert_eq!(titles(&p, "plantes OR clé").len(), 2);
        assert_eq!(titles(&p, "coffre -garde"), ["Salle des Archives"]);
        assert!(titles(&p, "").is_empty());
    }

    #[test]
    fn regex_and_fields() {
        let (_d, p) = project();
        assert_eq!(titles(&p, r"/\d{4}/"), ["Salle des Archives"]);
        assert!(matches!(Query::parse("/(/"), Err(SearchError::Regex(_))));
        assert!(matches!(Query::parse("\"abc"), Err(SearchError::Unclosed)));
        assert_eq!(titles(&p, "titre:salle"), ["Salle des Archives"]);
        assert_eq!(titles(&p, "cat:piece"), ["Salle des Archives"]);
        assert_eq!(titles(&p, "tag:indice").len(), 2);
        assert_eq!(titles(&p, "tag:indice/plante"), ["Serre"]);
        assert_eq!(titles(&p, "lien:major"), ["Salle des Archives"]);
        let tags: Vec<(String, usize)> = p.all_tags().into_iter().collect();
        assert_eq!(
            tags,
            [("indice".to_owned(), 2), ("indice/plante".to_owned(), 1)]
        );
    }

    #[test]
    fn line_hits_have_positions() {
        let (_d, p) = project();
        let hits = p.search_text(&Query::parse("code").unwrap());
        let l = &hits[0].lines[0];
        assert_eq!(l.line, 1);
        assert_eq!(&l.text[l.ranges[0].clone()], "Code");
        let body = &p.note(&hits[0].id).unwrap().body;
        assert_eq!(&body[l.byte..l.byte + 4], "Code");
        let (s, r) = snippet(
            &format!("{}fin", "x".repeat(300)),
            std::iter::once(300..303).collect(),
        );
        assert!(s.starts_with('…'));
        assert_eq!(&s[r[0].clone()], "fin");
    }
}
