//! Outils d'écriture Markdown : mise en forme d'une sélection (gras,
//! italique…), préfixes de lignes (titres, listes, citations) et modèles à
//! insérer (tableau, bloc de code…). Fonctions pures : texte + sélection
//! (octets) → nouveau texte + nouvelle sélection.

use std::ops::Range;

/// Résultat d'une transformation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub text: String,
    /// Sélection à rétablir (octets).
    pub selection: Range<usize>,
}

/// Mise en forme en ligne (entoure la sélection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inline {
    Bold,
    Italic,
    Strike,
    Code,
    /// `[[sélection]]`
    WikiLink,
}

impl Inline {
    fn markers(self) -> (&'static str, &'static str) {
        match self {
            Inline::Bold => ("**", "**"),
            Inline::Italic => ("*", "*"),
            Inline::Strike => ("~~", "~~"),
            Inline::Code => ("`", "`"),
            Inline::WikiLink => ("[[", "]]"),
        }
    }
}

/// Préfixe de ligne.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Heading(u8),
    Bullet,
    Numbered,
    Task,
    Quote,
}

fn clamp_sel(text: &str, sel: Range<usize>) -> Range<usize> {
    let fix = |mut i: usize| {
        i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let (a, b) = (fix(sel.start), fix(sel.end));
    a.min(b)..a.max(b)
}

/// Entoure la sélection de marqueurs, ou les retire s'ils y sont déjà
/// (dedans ou juste autour). Sans sélection : insère la paire et place le
/// curseur entre les deux.
pub fn toggle_inline(text: &str, sel: Range<usize>, kind: Inline) -> Edit {
    let sel = clamp_sel(text, sel);
    let (open, close) = kind.markers();
    let inner = &text[sel.clone()];
    // Marqueurs inclus dans la sélection.
    if inner.len() >= open.len() + close.len() && inner.starts_with(open) && inner.ends_with(close)
    {
        let stripped = &inner[open.len()..inner.len() - close.len()];
        let mut t = String::with_capacity(text.len());
        t.push_str(&text[..sel.start]);
        t.push_str(stripped);
        t.push_str(&text[sel.end..]);
        return Edit {
            selection: sel.start..sel.start + stripped.len(),
            text: t,
        };
    }
    // Marqueurs juste autour de la sélection.
    if sel.start >= open.len()
        && text[..sel.start].ends_with(open)
        && text[sel.end..].starts_with(close)
        // `*` seul ne doit pas défaire du gras `**`.
        && !(kind == Inline::Italic
            && text[..sel.start].ends_with("**")
            && text[sel.end..].starts_with("**"))
    {
        let a = sel.start - open.len();
        let mut t = String::with_capacity(text.len());
        t.push_str(&text[..a]);
        t.push_str(inner);
        t.push_str(&text[sel.end + close.len()..]);
        return Edit {
            selection: a..a + inner.len(),
            text: t,
        };
    }
    // Les espaces en bord de sélection restent dehors (`** gras**` est invalide).
    let lead = inner.len() - inner.trim_start().len();
    let trail = inner.len() - inner.trim_end().len();
    let (a, b) = if inner.trim().is_empty() {
        (sel.start, sel.end)
    } else {
        (sel.start + lead, sel.end - trail)
    };
    let mut t = String::with_capacity(text.len() + open.len() + close.len());
    t.push_str(&text[..a]);
    t.push_str(open);
    t.push_str(&text[a..b]);
    t.push_str(close);
    t.push_str(&text[b..]);
    let start = a + open.len();
    Edit {
        selection: start..start + (b - a),
        text: t,
    }
}

/// Début de la ligne contenant `i`.
fn line_start(text: &str, i: usize) -> usize {
    text[..i].rfind('\n').map_or(0, |p| p + 1)
}

/// Fin de la ligne contenant `i` (avant le `\n`).
fn line_end(text: &str, i: usize) -> usize {
    text[i..].find('\n').map_or(text.len(), |p| i + p)
}

/// Longueur du préfixe Markdown existant d'une ligne (titre, liste,
/// tâche, citation), espaces d'indentation exclus.
fn existing_prefix(line: &str) -> (Option<Line>, usize) {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        return (Some(Line::Heading(hashes as u8)), hashes + 1);
    }
    for (p, kind) in [
        ("- [ ] ", Line::Task),
        ("- [x] ", Line::Task),
        ("- [X] ", Line::Task),
        ("- ", Line::Bullet),
        ("* ", Line::Bullet),
        ("> ", Line::Quote),
    ] {
        if line.starts_with(p) {
            return (Some(kind), p.len());
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && line[digits..].starts_with(". ") {
        return (Some(Line::Numbered), digits + 2);
    }
    (None, 0)
}

/// Applique (ou retire, si toutes les lignes l'ont déjà) un préfixe aux
/// lignes touchées par la sélection. Les listes numérotées sont
/// renumérotées 1., 2., 3.
pub fn toggle_line(text: &str, sel: Range<usize>, kind: Line) -> Edit {
    let sel = clamp_sel(text, sel);
    let start = line_start(text, sel.start);
    let end = line_end(text, sel.end.max(sel.start));
    let block = &text[start..end];
    let lines: Vec<&str> = block.split('\n').collect();
    let all_have = lines.iter().all(|l| {
        let indent = l.len() - l.trim_start().len();
        existing_prefix(&l[indent..]).0 == Some(kind)
    });
    let mut out_lines = Vec::with_capacity(lines.len());
    let mut n = 0;
    for l in &lines {
        let indent = l.len() - l.trim_start().len();
        let (pad, rest) = l.split_at(indent);
        let (_, plen) = existing_prefix(rest);
        let content = &rest[plen..];
        if all_have {
            out_lines.push(format!("{pad}{content}"));
            continue;
        }
        if l.trim().is_empty() && lines.len() > 1 {
            out_lines.push((*l).to_owned());
            continue;
        }
        n += 1;
        let prefix = match kind {
            Line::Heading(h) => format!("{} ", "#".repeat(h.clamp(1, 6) as usize)),
            Line::Bullet => "- ".to_owned(),
            Line::Numbered => format!("{n}. "),
            Line::Task => "- [ ] ".to_owned(),
            Line::Quote => "> ".to_owned(),
        };
        out_lines.push(format!("{pad}{prefix}{content}"));
    }
    let new_block = out_lines.join("\n");
    let mut t = String::with_capacity(text.len() + 16);
    t.push_str(&text[..start]);
    t.push_str(&new_block);
    t.push_str(&text[end..]);
    // Sélection : tout le bloc transformé (ou curseur en fin de ligne).
    let selection = if sel.is_empty() && lines.len() == 1 {
        let e = start + new_block.len();
        e..e
    } else {
        start..start + new_block.len()
    };
    Edit { text: t, selection }
}

/// Entoure la sélection d'un bloc de code (ou l'insère vide).
pub fn code_block(text: &str, sel: Range<usize>) -> Edit {
    let sel = clamp_sel(text, sel);
    let inner = &text[sel.clone()];
    let body = format!("```\n{inner}\n```");
    let cursor = sel.start + 4;
    insert_block_at(text, sel, &body, cursor..cursor + inner.len())
}

/// Insère un bloc (lignes vides autour) à la place de la sélection ;
/// `inner_sel` est relative au texte final et donne la sélection voulue
/// dans le bloc (décalée automatiquement des lignes vides ajoutées).
fn insert_block_at(text: &str, sel: Range<usize>, block: &str, inner_sel: Range<usize>) -> Edit {
    let before = &text[..sel.start];
    let after = &text[sel.end..];
    let lead = if before.is_empty() || before.ends_with("\n\n") {
        ""
    } else if before.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let trail = if after.starts_with("\n\n") || after.is_empty() {
        ""
    } else if after.starts_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let t = format!("{before}{lead}{block}{trail}{after}");
    let shift = lead.len();
    Edit {
        selection: inner_sel.start + shift..inner_sel.end + shift,
        text: t,
    }
}

/// Insère un bloc au curseur (remplace la sélection), le curseur à
/// `cursor` octets du début du bloc.
pub fn insert_block(text: &str, sel: Range<usize>, block: &str, cursor: usize) -> Edit {
    let sel = clamp_sel(text, sel);
    let c = sel.start + cursor.min(block.len());
    insert_block_at(text, sel, block, c..c)
}

/// Insère du texte en ligne au curseur (remplace la sélection).
pub fn insert_inline(text: &str, sel: Range<usize>, snippet: &str) -> Edit {
    let sel = clamp_sel(text, sel);
    let t = format!("{}{snippet}{}", &text[..sel.start], &text[sel.end..]);
    let c = sel.start + snippet.len();
    Edit {
        text: t,
        selection: c..c,
    }
}

/// Tableau Markdown vide (`cols` colonnes, `rows` lignes de données).
/// Colonnes alignées (voir [`crate::table`]).
pub fn table(rows: usize, cols: usize) -> String {
    crate::table::Table::empty(rows.max(1), cols).to_markdown()
}

/// Préfixe d'une ligne de liste ou de citation : retrait, `> `, puce ou
/// numéro, case de tâche. (retrait, citations, marqueur, case, longueur).
struct Prefix<'a> {
    indent: &'a str,
    quote: &'a str,
    /// `- `, `* `, `+ `, `3. `, `3) ` (vide : simple citation).
    marker: &'a str,
    task: bool,
    len: usize,
}

fn line_prefix(line: &str) -> Option<Prefix<'_>> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let mut at = indent_len;
    let rest = &line[at..];
    let quote_len = rest.len() - rest.trim_start_matches(['>', ' ']).len();
    let quote = &rest[..quote_len];
    let quote = if quote.contains('>') { quote } else { "" };
    at += quote.len();
    let rest = &line[at..];
    let marker_len = if rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ") {
        2
    } else {
        let d = rest.bytes().take_while(u8::is_ascii_digit).count();
        if d > 0 && d < 10 && (rest[d..].starts_with(". ") || rest[d..].starts_with(") ")) {
            d + 2
        } else {
            0
        }
    };
    if marker_len == 0 && quote.is_empty() {
        return None;
    }
    let marker = &rest[..marker_len];
    at += marker_len;
    let task = marker_len > 0
        && ["[ ] ", "[x] ", "[X] "]
            .iter()
            .any(|t| line[at..].starts_with(t));
    if task {
        at += 4;
    }
    Some(Prefix {
        indent: &line[..indent_len],
        quote,
        marker,
        task,
        len: at,
    })
}

fn line_bounds(text: &str, cursor: usize) -> Range<usize> {
    let start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let end = text[cursor..].find('\n').map_or(text.len(), |i| cursor + i);
    start..end
}

/// Entrée dans une liste ou une citation : la ligne suivante reprend la
/// puce (numéro + 1, case vide). Sur un élément vide, la puce est retirée
/// (ou l'élément remonte d'un niveau s'il est en retrait). `None` : pas
/// une liste, Entrée ordinaire.
pub fn list_enter(text: &str, cursor: usize) -> Option<Edit> {
    let cursor = clamp_sel(text, cursor..cursor).start;
    let line = line_bounds(text, cursor);
    let p = line_prefix(&text[line.clone()])?;
    if cursor < line.start + p.len {
        return None;
    }
    let content = &text[line.start + p.len..line.end];
    if content.trim().is_empty() {
        // Élément vide : remonter d'un niveau, ou fin de la liste.
        let new = if !p.indent.is_empty() && !p.marker.is_empty() {
            let cut = p
                .indent
                .len()
                .min(if p.indent.starts_with('\t') { 1 } else { 2 });
            format!(
                "{}{}{}{}",
                &p.indent[cut..],
                p.quote,
                p.marker,
                if p.task { "[ ] " } else { "" }
            )
        } else {
            String::new()
        };
        let t = format!("{}{new}{}", &text[..line.start], &text[line.end..]);
        let c = line.start + new.len();
        return Some(Edit {
            text: t,
            selection: c..c,
        });
    }
    let marker = next_marker(p.marker);
    let next = format!(
        "\n{}{}{}{}",
        p.indent,
        p.quote,
        marker,
        if p.task { "[ ] " } else { "" }
    );
    let t = format!("{}{next}{}", &text[..cursor], &text[cursor..]);
    let c = cursor + next.len();
    Some(Edit {
        text: t,
        selection: c..c,
    })
}

fn next_marker(marker: &str) -> String {
    let d = marker.bytes().take_while(u8::is_ascii_digit).count();
    if d == 0 {
        return marker.to_owned();
    }
    let n: u64 = marker[..d].parse().unwrap_or(0);
    format!("{}{}", n + 1, &marker[d..])
}

/// Tab / Maj+Tab sur un élément de liste : retrait d'un niveau en plus ou
/// en moins (largeur de la puce, pour que les listes numérotées
/// s'imbriquent). `None` : pas une liste.
pub fn list_indent(text: &str, cursor: usize, outdent: bool) -> Option<Edit> {
    let cursor = clamp_sel(text, cursor..cursor).start;
    let line = line_bounds(text, cursor);
    let p = line_prefix(&text[line.clone()])?;
    if p.marker.is_empty() {
        return None;
    }
    let step = p.marker.len().min(4);
    let (new_indent, delta): (String, isize) = if outdent {
        if p.indent.is_empty() {
            return None;
        }
        let cut = if p.indent.starts_with('\t') {
            1
        } else {
            p.indent.len().min(step)
        };
        (p.indent[cut..].to_owned(), -(cut as isize))
    } else {
        (format!("{}{}", " ".repeat(step), p.indent), step as isize)
    };
    let t = format!(
        "{}{new_indent}{}",
        &text[..line.start],
        &text[line.start + p.indent.len()..]
    );
    let c = (cursor as isize + delta).max(line.start as isize) as usize;
    Some(Edit {
        text: t,
        selection: c..c,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel_of(text: &str, part: &str) -> Range<usize> {
        let a = text.find(part).unwrap();
        a..a + part.len()
    }

    #[test]
    fn bold_toggles_both_ways() {
        let t = "Un mot ici";
        let e = toggle_inline(t, sel_of(t, "mot"), Inline::Bold);
        assert_eq!(e.text, "Un **mot** ici");
        assert_eq!(&e.text[e.selection.clone()], "mot");
        // Re-basculer (sélection sur le mot) retire les marqueurs.
        let back = toggle_inline(&e.text, e.selection.clone(), Inline::Bold);
        assert_eq!(back.text, t);
        // Sélection marqueurs compris.
        let back = toggle_inline("Un **mot** ici", 3..10, Inline::Bold);
        assert_eq!(back.text, t);
        // Espaces en bord laissés dehors.
        let e = toggle_inline("a mot b", 1..6, Inline::Bold);
        assert_eq!(e.text, "a **mot** b");
        // Sans sélection : paire insérée, curseur au milieu.
        let e = toggle_inline("ab", 1..1, Inline::Code);
        assert_eq!(e.text, "a``b");
        assert_eq!(e.selection, 2..2);
    }

    #[test]
    fn italic_does_not_unwrap_bold() {
        let t = "**mot**";
        let e = toggle_inline(t, 2..5, Inline::Italic);
        assert_eq!(e.text, "***mot***");
        let e = toggle_inline("été", 0..5, Inline::WikiLink);
        assert_eq!(e.text, "[[été]]");
    }

    #[test]
    fn line_prefixes() {
        let t = "un\ndeux\n\ntrois";
        let e = toggle_line(t, 0..6, Line::Bullet);
        assert_eq!(e.text, "- un\n- deux\n\ntrois");
        let back = toggle_line(&e.text, e.selection.clone(), Line::Bullet);
        assert_eq!(back.text, t);
        let e = toggle_line(t, 0..6, Line::Numbered);
        assert_eq!(e.text, "1. un\n2. deux\n\ntrois");
        // Un titre remplace le préfixe existant.
        let e = toggle_line("- liste", 2..2, Line::Heading(2));
        assert_eq!(e.text, "## liste");
        let e = toggle_line("## titre", 0..0, Line::Heading(1));
        assert_eq!(e.text, "# titre");
        let e = toggle_line("a", 1..1, Line::Task);
        assert_eq!(e.text, "- [ ] a");
        let e = toggle_line("  > cité", 4..4, Line::Quote);
        assert_eq!(e.text, "  cité", "retirer la citation");
    }

    #[test]
    fn blocks_and_tables() {
        let e = code_block("avant x apres", 6..7);
        assert_eq!(e.text, "avant \n\n```\nx\n```\n\n apres");
        assert_eq!(&e.text[e.selection.clone()], "x");
        let tab = table(2, 3);
        assert_eq!(
            tab,
            "| Colonne 1 | Colonne 2 | Colonne 3 |\n| --------- | --------- | --------- |\n|           |           |           |\n|           |           |           |"
        );
        let e = insert_block("ligne", 5..5, "---", 3);
        assert_eq!(e.text, "ligne\n\n---");
        assert_eq!(e.selection, 10..10);
        let e = insert_inline("ab", 1..1, "X");
        assert_eq!((e.text.as_str(), e.selection), ("aXb", 2..2));
    }

    #[test]
    fn enter_continues_lists() {
        let e = list_enter("- un", 4).unwrap();
        assert_eq!(
            (e.text.as_str(), e.selection.start),
            (
                "- un
- ", 7
            )
        );
        let e = list_enter("  9. neuf", 9).unwrap();
        assert_eq!(
            e.text,
            "  9. neuf
  10. "
        );
        let e = list_enter("- [x] fait", 10).unwrap();
        assert_eq!(
            e.text,
            "- [x] fait
- [ ] "
        );
        let e = list_enter("> cité", 7).unwrap();
        assert_eq!(
            e.text,
            "> cité
> "
        );
        // Coupe la ligne au curseur.
        let e = list_enter("- ab", 3).unwrap();
        assert_eq!(
            e.text,
            "- a
- b"
        );
        // Élément vide : fin de liste, ou un niveau de moins.
        let e = list_enter(
            "- a
- ", 6,
        )
        .unwrap();
        assert_eq!(
            (e.text.as_str(), e.selection.start),
            (
                "- a
", 4
            )
        );
        let e = list_enter(
            "- a
  - ", 8,
        )
        .unwrap();
        assert_eq!(
            e.text,
            "- a
- "
        );
        assert!(list_enter("texte", 5).is_none());
        assert!(list_enter("- a", 1).is_none(), "curseur dans la puce");
    }

    #[test]
    fn tab_indents_list_items() {
        let e = list_indent(
            "- a
- b", 7, false,
        )
        .unwrap();
        assert_eq!(
            (e.text.as_str(), e.selection.start),
            (
                "- a
  - b",
                9
            )
        );
        let e = list_indent(&e.text, 9, true).unwrap();
        assert_eq!(
            e.text,
            "- a
- b"
        );
        let e = list_indent("1. a", 4, false).unwrap();
        assert_eq!(e.text, "   1. a");
        assert!(list_indent("a", 1, false).is_none());
        assert!(list_indent("- a", 3, true).is_none());
    }
}
