//! Analyse du corps Markdown d'une fiche.
//!
//! Un seul passage de `pulldown-cmark` (avec positions) fournit :
//! - des segments stylés couvrant **tout** le texte source, pour la mise en
//!   forme pendant la frappe (le texte reste éditable tel quel) ;
//! - les liens `[[Titre]]` / `[[Titre|type]]`, hors blocs et spans de code.

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag};

/// Style d'un morceau de texte source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    /// Niveau de titre (1 à 6), 0 si ce n'est pas un titre.
    pub heading: u8,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    /// Code en ligne (`` `x` ``).
    pub code: bool,
    /// Bloc de code (indenté ou délimité).
    pub code_block: bool,
    /// Lien Markdown classique `[texte](url)`.
    pub link: bool,
    /// Lien de fiche `[[...]]`.
    pub wiki_link: bool,
    /// Puce ou numéro de liste, case à cocher.
    pub list_marker: bool,
    pub quote: bool,
    /// Caractère de syntaxe (`#`, `**`, `[[`…), à afficher discrètement.
    pub markup: bool,
}

/// Un segment de texte et son style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
}

/// Lien `[[...]]` trouvé dans le corps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiLink {
    /// Tout le lien, crochets compris.
    pub range: Range<usize>,
    /// Le titre visé (sans espaces superflus).
    pub target: String,
    /// Position du titre dans le texte source.
    pub target_range: Range<usize>,
    /// Type de lien après `|`, s'il y en a un.
    pub kind: Option<String>,
    /// Section visée après `#` : titre de section, ou `^id` de paragraphe.
    pub section: Option<String>,
}

/// Résultat complet de l'analyse.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    /// Segments contigus couvrant tout le texte, dans l'ordre.
    pub spans: Vec<Span>,
    pub links: Vec<WikiLink>,
}

pub(crate) fn options() -> Options {
    Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
}

/// Analyse complète : styles et liens.
pub fn analyze(text: &str) -> Analysis {
    let n = text.len();
    let mut styles = vec![Style::default(); n];
    let mut content = vec![false; n];
    let mut in_code = vec![false; n];

    let apply = |styles: &mut [Style], r: &Range<usize>, f: &dyn Fn(&mut Style)| {
        for s in &mut styles[r.start.min(n)..r.end.min(n)] {
            f(s);
        }
    };

    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    let lvl = level as u8;
                    apply(&mut styles, &range, &|s| s.heading = lvl);
                }
                Tag::Emphasis => apply(&mut styles, &range, &|s| s.italic = true),
                Tag::Strong => apply(&mut styles, &range, &|s| s.bold = true),
                Tag::Strikethrough => apply(&mut styles, &range, &|s| s.strike = true),
                Tag::CodeBlock(_) => {
                    apply(&mut styles, &range, &|s| s.code_block = true);
                    mark(&mut in_code, &range);
                }
                Tag::Link { .. } | Tag::Image { .. } => {
                    apply(&mut styles, &range, &|s| s.link = true);
                }
                Tag::BlockQuote(_) => apply(&mut styles, &range, &|s| s.quote = true),
                Tag::Item => {
                    let marker = list_marker_range(text, range.start);
                    apply(&mut styles, &marker, &|s| s.list_marker = true);
                }
                _ => {}
            },
            Event::Text(_) | Event::Html(_) | Event::InlineHtml(_) => {
                mark(&mut content, &range);
            }
            Event::Code(_) => {
                apply(&mut styles, &range, &|s| s.code = true);
                mark(&mut in_code, &range);
                let inner = code_inner_range(text, &range);
                mark(&mut content, &inner);
            }
            Event::TaskListMarker(_) => {
                apply(&mut styles, &range, &|s| s.list_marker = true);
                mark(&mut content, &range);
            }
            _ => {}
        }
    }

    let links = scan_wiki_links(text, &in_code);
    for link in &links {
        apply(&mut styles, &link.range, &|s| {
            s.wiki_link = true;
            s.link = false;
        });
        // Seul le titre visé est du contenu ; crochets et `|type` sont de la syntaxe.
        for c in &mut content[link.range.clone()] {
            *c = false;
        }
        mark(&mut content, &link.target_range);
    }

    for i in 0..n {
        if !content[i] {
            styles[i].markup = true;
        }
    }

    Analysis {
        spans: build_spans(text, &styles),
        links,
    }
}

/// Liens `[[...]]` seulement (plus rapide que [`analyze`]).
pub fn wiki_links(text: &str) -> Vec<WikiLink> {
    let mut in_code = vec![false; text.len()];
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) | Event::Code(_) => mark(&mut in_code, &range),
            _ => {}
        }
    }
    scan_wiki_links(text, &in_code)
}

/// Tag `#nom` trouvé dans le corps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagRef {
    /// `#` compris.
    pub range: Range<usize>,
    /// Sans `#` (peut contenir `/` : `lieu/manoir`).
    pub name: String,
}

/// Tags `#nom` (lettres, chiffres, `_`, `-`, `/`, pas seulement des
/// chiffres), précédés d'un espace ou en début de ligne, hors code. Les
/// titres (`# Titre`) et les `#` d'un lien `[[Fiche#Section]]` n'en sont
/// pas.
pub fn tags(text: &str) -> Vec<TagRef> {
    let mut in_code = vec![false; text.len()];
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) | Event::Code(_) => mark(&mut in_code, &range),
            _ => {}
        }
    }
    scan_tags(text, &in_code)
}

/// Tags d'un texte sans code (morceau de paragraphe déjà analysé).
pub fn tags_in_plain(text: &str) -> Vec<TagRef> {
    scan_tags(text, &vec![false; text.len()])
}

fn scan_tags(text: &str, in_code: &[bool]) -> Vec<TagRef> {
    let mut out = Vec::new();
    for (i, c) in text.char_indices() {
        if c != '#' || in_code[i] {
            continue;
        }
        let prev_ok = text[..i]
            .chars()
            .next_back()
            .is_none_or(|p| p.is_whitespace() || p == '(');
        if !prev_ok {
            continue;
        }
        let rest = &text[i + 1..];
        let len = rest
            .char_indices()
            .take_while(|(_, ch)| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '/'))
            .last()
            .map_or(0, |(j, ch)| j + ch.len_utf8());
        let name = rest[..len].trim_end_matches(['/', '-']);
        if name.is_empty() || name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        out.push(TagRef {
            range: i..i + 1 + name.len(),
            name: name.to_owned(),
        });
    }
    out
}

/// Lien `[[...` en cours de saisie (pas encore refermé) sous le curseur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Completion {
    /// Après `[[` : titre partiel. `start` : octet juste après `[[`.
    Title { start: usize, query: String },
    /// Après `[[Titre|` : type partiel. `start` : octet juste après `|`.
    LinkType {
        start: usize,
        target: String,
        query: String,
    },
    /// Après `[[Titre#` : section partielle. `start` : octet après `#`.
    Section {
        start: usize,
        target: String,
        query: String,
    },
}

impl Completion {
    /// Début du texte que le choix remplacera (jusqu'au curseur).
    pub fn start(&self) -> usize {
        match self {
            Completion::Title { start, .. }
            | Completion::LinkType { start, .. }
            | Completion::Section { start, .. } => *start,
        }
    }

    /// Texte partiel déjà tapé.
    pub fn query(&self) -> &str {
        match self {
            Completion::Title { query, .. }
            | Completion::LinkType { query, .. }
            | Completion::Section { query, .. } => query,
        }
    }
}

/// Détecte un lien `[[...` non refermé entre le début de la ligne et le
/// curseur (`cursor` en octets), hors code.
pub fn completion_at(text: &str, cursor: usize) -> Option<Completion> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let line_start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[line_start..cursor];
    let open = before.rfind("[[")?;
    let inner = &before[open + 2..];
    if inner.contains('[') || inner.contains(']') {
        return None;
    }
    let abs_open = line_start + open;
    if in_code_at(text, abs_open) {
        return None;
    }
    match inner.find('|') {
        Some(p) => {
            let query = &inner[p + 1..];
            if query.contains('|') {
                return None;
            }
            Some(Completion::LinkType {
                start: abs_open + 2 + p + 1,
                target: inner[..p].trim().to_owned(),
                query: query.trim_start().to_owned(),
            })
        }
        None => match inner.find('#') {
            Some(h) => Some(Completion::Section {
                start: abs_open + 2 + h + 1,
                target: inner[..h].trim().to_owned(),
                query: inner[h + 1..].trim_start().to_owned(),
            }),
            None => Some(Completion::Title {
                start: abs_open + 2,
                query: inner.trim_start().to_owned(),
            }),
        },
    }
}

/// Applique un choix d'auto-complétion : remplace le texte partiel par
/// `choice` et referme le lien si besoin. Avec `then_type` (titre seulement),
/// insère `|` pour enchaîner sur le choix du type.
/// Renvoie le nouveau texte et la position du curseur (en octets).
pub fn apply_completion(
    text: &str,
    cursor: usize,
    completion: &Completion,
    choice: &str,
    then_type: bool,
) -> (String, usize) {
    let start = completion.start().min(cursor);
    let rest = &text[cursor..];
    let mut out = String::with_capacity(text.len() + choice.len() + 3);
    out.push_str(&text[..start]);
    out.push_str(choice);
    let after_choice = out.len();
    let new_cursor;
    match completion {
        Completion::Title { .. } if then_type => {
            if rest.starts_with('|') {
                new_cursor = after_choice + 1;
            } else if rest.starts_with("]]") {
                out.push('|');
                new_cursor = out.len();
            } else {
                out.push_str("|]]");
                new_cursor = after_choice + 1;
            }
        }
        Completion::Title { .. } if rest.starts_with('|') => new_cursor = after_choice,
        _ => {
            if rest.starts_with("]]") {
                new_cursor = after_choice + 2;
            } else {
                out.push_str("]]");
                new_cursor = out.len();
            }
        }
    }
    out.push_str(rest);
    (out, new_cursor)
}

fn in_code_at(text: &str, pos: usize) -> bool {
    Parser::new_ext(text, options())
        .into_offset_iter()
        .any(|(event, range)| {
            matches!(event, Event::Start(Tag::CodeBlock(_)) | Event::Code(_))
                && range.contains(&pos)
        })
}

/// Lien `[[...]]` qui contient la position `byte`, s'il y en a un.
pub fn wiki_link_at(text: &str, byte: usize) -> Option<WikiLink> {
    wiki_links(text)
        .into_iter()
        .find(|l| l.range.start <= byte && byte <= l.range.end)
}

/// Remplace la cible des liens `[[ancien]]` (insensible à la casse) par
/// `new_title`, en conservant le type éventuel. `None` si rien n'a changé.
pub fn rewrite_wiki_targets(text: &str, old_title: &str, new_title: &str) -> Option<String> {
    let old = old_title.trim().to_lowercase();
    let mut targets: Vec<Range<usize>> = wiki_links(text)
        .into_iter()
        .filter(|l| l.target.to_lowercase() == old)
        .map(|l| l.target_range)
        .collect();
    if targets.is_empty() {
        return None;
    }
    targets.sort_by_key(|r| std::cmp::Reverse(r.start));
    let mut out = text.to_owned();
    for r in targets {
        out.replace_range(r, new_title);
    }
    Some(out)
}

fn mark(mask: &mut [bool], r: &Range<usize>) {
    let n = mask.len();
    for m in &mut mask[r.start.min(n)..r.end.min(n)] {
        *m = true;
    }
}

/// Puce (`-`, `*`, `+`) ou numéro (`1.`, `1)`) au début d'un élément de liste.
fn list_marker_range(text: &str, start: usize) -> Range<usize> {
    let bytes = text.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    let begin = i;
    if i < bytes.len() && matches!(bytes[i], b'-' | b'*' | b'+') {
        i += 1;
    } else {
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i > begin && i < bytes.len() && matches!(bytes[i], b'.' | b')') {
            i += 1;
        } else {
            return begin..begin;
        }
    }
    begin..i
}

/// Intérieur d'un span de code, sans les accents graves qui l'entourent.
fn code_inner_range(text: &str, r: &Range<usize>) -> Range<usize> {
    let s = &text.as_bytes()[r.clone()];
    let lead = s.iter().take_while(|&&b| b == b'`').count();
    let trail = s.iter().rev().take_while(|&&b| b == b'`').count();
    if lead + trail >= s.len() {
        return r.start..r.start;
    }
    r.start + lead..r.end - trail
}

fn scan_wiki_links(text: &str, in_code: &[bool]) -> Vec<WikiLink> {
    let mut links = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find("[[") {
        let open = from + rel;
        let inner_start = open + 2;
        if in_code.get(open).copied().unwrap_or(false) {
            from = inner_start;
            continue;
        }
        let rest = &text[inner_start..];
        let Some(close_rel) = rest.find("]]") else {
            break;
        };
        let inner = &rest[..close_rel];
        if inner.contains('\n') || inner.contains('[') || inner.contains(']') {
            from = inner_start;
            continue;
        }
        let close = inner_start + close_rel;
        let (target_part, kind) = match inner.find('|') {
            Some(p) => {
                let k = inner[p + 1..].trim();
                (&inner[..p], (!k.is_empty()).then(|| k.to_owned()))
            }
            None => (inner, None),
        };
        let (target_part, section) = match target_part.find('#') {
            Some(h) => {
                let s = target_part[h + 1..].trim();
                (&target_part[..h], (!s.is_empty()).then(|| s.to_owned()))
            }
            None => (target_part, None),
        };
        let lead_ws = target_part.len() - target_part.trim_start().len();
        let target = target_part.trim();
        if !target.is_empty() {
            let t_start = inner_start + lead_ws;
            links.push(WikiLink {
                range: open..close + 2,
                target: target.to_owned(),
                target_range: t_start..t_start + target.len(),
                kind,
                section,
            });
        }
        from = close + 2;
    }
    links
}

fn build_spans(text: &str, styles: &[Style]) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut start = 0;
    for i in 1..=text.len() {
        let boundary = i == text.len() || (text.is_char_boundary(i) && styles[i] != styles[start]);
        if boundary && i > start {
            spans.push(Span {
                range: start..i,
                style: styles[start],
            });
            start = i;
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style_at(a: &Analysis, byte: usize) -> Style {
        a.spans
            .iter()
            .find(|s| s.range.contains(&byte))
            .map(|s| s.style)
            .unwrap()
    }

    #[test]
    fn spans_cover_whole_text_contiguously() {
        let text =
            "# Titre é\n\nDu **gras**, de l'*italique*, `code` et [[Lien|indique]].\n- item\n";
        let a = analyze(text);
        let mut pos = 0;
        for s in &a.spans {
            assert_eq!(s.range.start, pos);
            assert!(text.is_char_boundary(s.range.end));
            pos = s.range.end;
        }
        assert_eq!(pos, text.len());
    }

    #[test]
    fn heading_bold_italic_code() {
        let text = "## Titre\n\nun **gras** et *ita* et `co`";
        let a = analyze(text);
        assert_eq!(style_at(&a, 0).heading, 2);
        assert!(style_at(&a, 0).markup);
        assert!(!style_at(&a, 3).markup);
        let g = text.find("gras").unwrap();
        assert!(style_at(&a, g).bold && !style_at(&a, g).markup);
        assert!(style_at(&a, g - 1).bold && style_at(&a, g - 1).markup);
        assert!(style_at(&a, text.find("ita").unwrap()).italic);
        let c = text.find("co").unwrap();
        assert!(style_at(&a, c).code && !style_at(&a, c).markup);
        assert!(style_at(&a, c - 1).markup);
    }

    #[test]
    fn every_list_item_marker_after_heading() {
        let text = "# T\n\nPièce **x**. Un *c* [[A|ouvre]].\n\n## Indices\n- Le portrait au mur regarde vers l'est\n- Une date : `1-9-4-5`\n- Voir [[M]] et [[N]]\n\n> x\n";
        let a = analyze(text);
        let dashes: Vec<usize> = text.match_indices("- ").map(|(i, _)| i).collect();
        assert_eq!(dashes.len(), 3);
        for d in dashes {
            assert!(
                style_at(&a, d).list_marker,
                "tiret à l'octet {d}: {:?}",
                a.spans
            );
        }
    }

    #[test]
    fn list_marker_detected() {
        let a = analyze("- un\n2. deux\n");
        assert!(style_at(&a, 0).list_marker);
        assert!(style_at(&a, 5).list_marker && style_at(&a, 6).list_marker);
        assert!(!style_at(&a, 2).list_marker);
    }

    #[test]
    fn wiki_links_parsed_with_kind() {
        let text = "Voir [[ Salle des Archives ]] et [[Coffre|ouvre]].";
        let links = wiki_links(text);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "Salle des Archives");
        assert_eq!(&text[links[0].target_range.clone()], "Salle des Archives");
        assert_eq!(links[0].kind, None);
        assert_eq!(links[1].target, "Coffre");
        assert_eq!(links[1].kind.as_deref(), Some("ouvre"));
        assert_eq!(&text[links[1].range.clone()], "[[Coffre|ouvre]]");
    }

    #[test]
    fn wiki_links_ignored_in_code() {
        let text = "`[[Pas un lien]]`\n\n```\n[[Non plus]]\n```\n\n[[Oui]]";
        let links = wiki_links(text);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "Oui");
    }

    #[test]
    fn malformed_wiki_links_ignored() {
        assert!(wiki_links("[[]] [[ ]] [[a\nb]] [[sans fin").is_empty());
        assert_eq!(wiki_links("[[[x]] [[y]]").len(), 1);
    }

    #[test]
    fn wiki_link_styling() {
        let text = "a [[Cible|indique]] b";
        let a = analyze(text);
        let t = text.find("Cible").unwrap();
        assert!(style_at(&a, t).wiki_link && !style_at(&a, t).markup);
        assert!(style_at(&a, 2).wiki_link && style_at(&a, 2).markup);
        let k = text.find("indique").unwrap();
        assert!(style_at(&a, k).markup);
    }

    #[test]
    fn rewrite_targets_case_insensitive_keeps_kind() {
        let text = "[[coffre]] et [[Coffre|ouvre]] mais pas `[[Coffre]]` ni [[Autre]]";
        let out = rewrite_wiki_targets(text, "COFFRE", "Grand coffre").unwrap();
        assert_eq!(
            out,
            "[[Grand coffre]] et [[Grand coffre|ouvre]] mais pas `[[Coffre]]` ni [[Autre]]"
        );
        assert_eq!(rewrite_wiki_targets("[[Autre]]", "Coffre", "X"), None);
    }

    #[test]
    fn empty_text() {
        let a = analyze("");
        assert!(a.spans.is_empty() && a.links.is_empty());
    }

    fn at_end(text: &str) -> Option<Completion> {
        completion_at(text, text.len())
    }

    #[test]
    fn completion_title_and_type() {
        assert_eq!(
            at_end("Voir [[Cof"),
            Some(Completion::Title {
                start: 7,
                query: "Cof".into()
            })
        );
        assert_eq!(
            at_end("[["),
            Some(Completion::Title {
                start: 2,
                query: String::new()
            })
        );
        assert_eq!(
            at_end("x [[Coffre|ou"),
            Some(Completion::LinkType {
                start: 11,
                target: "Coffre".into(),
                query: "ou".into()
            })
        );
    }

    #[test]
    fn completion_none_when_closed_or_in_code_or_other_line() {
        assert_eq!(at_end("[[Coffre]] puis"), None);
        assert_eq!(
            at_end(
                "[[Coffre
suite"
            ),
            None
        );
        // Un accent grave non refermé n'est pas du code (CommonMark).
        assert!(at_end("`[[Cof").is_some());
        assert_eq!(at_end("`x` `[[Cof`"), None);
        assert_eq!(
            at_end(
                "```
[[Cof"
            ),
            None
        );
        assert_eq!(at_end("a [[b|c|d"), None);
        assert_eq!(
            completion_at("é", 1),
            None,
            "pas une frontière de caractère"
        );
        // Le curseur au milieu d'un lien existant compte aussi.
        assert_eq!(
            completion_at("[[Cof]]", 5),
            Some(Completion::Title {
                start: 2,
                query: "Cof".into()
            })
        );
    }

    fn apply(text: &str, cursor: usize, choice: &str, then_type: bool) -> (String, usize) {
        let c = completion_at(text, cursor).unwrap();
        apply_completion(text, cursor, &c, choice, then_type)
    }

    #[test]
    fn apply_title_closes_link() {
        let (t, c) = apply("Voir [[cof", 10, "Coffre", false);
        assert_eq!(t, "Voir [[Coffre]]");
        assert_eq!(c, t.len());
        // « ]] » déjà présent : pas de doublon, curseur après.
        let (t, c) = apply("[[cof]] fin", 5, "Coffre", false);
        assert_eq!(t, "[[Coffre]] fin");
        assert_eq!(&t[c..], " fin");
    }

    #[test]
    fn apply_title_then_type() {
        let (t, c) = apply("[[cof", 5, "Coffre", true);
        assert_eq!(t, "[[Coffre|]]");
        assert_eq!(&t[..c], "[[Coffre|");
        assert!(matches!(
            completion_at(&t, c),
            Some(Completion::LinkType { .. })
        ));
        let (t, c) = apply("[[cof]]", 5, "Coffre", true);
        assert_eq!(t, "[[Coffre|]]");
        assert_eq!(&t[..c], "[[Coffre|");
    }

    #[test]
    fn apply_type() {
        let (t, c) = apply("[[Coffre|ou", 11, "ouvre", false);
        assert_eq!(t, "[[Coffre|ouvre]]");
        assert_eq!(c, t.len());
        let (t, c) = apply("[[Coffre|]] x", 9, "indique", false);
        assert_eq!(t, "[[Coffre|indique]] x");
        assert_eq!(&t[c..], " x");
    }

    #[test]
    fn link_at_position() {
        let text = "a [[B|ouvre]] c";
        assert_eq!(wiki_link_at(text, 5).unwrap().target, "B");
        assert!(wiki_link_at(text, 0).is_none());
    }
}
