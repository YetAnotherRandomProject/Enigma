//! Édition « en direct » : la fiche est toujours affichée mise en forme,
//! seul le morceau en cours de modification montre son texte Markdown.
//!
//! Le texte est découpé en unités d'édition : une ligne, ou un bloc entier
//! quand il n'a de sens qu'en entier (bloc de code délimité, tableau).
//! Les unités se suivent, séparées par un saut de ligne.

use std::ops::Range;

use crate::table;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind {
    Line,
    /// Bloc de code délimité par ``` ou ~~~.
    Code,
    Table,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// Sans le saut de ligne final.
    pub range: Range<usize>,
    pub kind: UnitKind,
}

/// Lignes du texte (sans `\n` ni `\r` final). Toujours au moins une.
pub fn line_ranges(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            let end = if i > start && text.as_bytes()[i - 1] == b'\r' {
                i - 1
            } else {
                i
            };
            out.push(start..end);
            start = i + 1;
        }
    }
    out.push(start..text.len());
    out
}

/// Ouverture de bloc de code : (caractère, longueur de la clôture).
fn fence_open(line: &str) -> Option<(u8, usize)> {
    let t = line.trim_start();
    if line.len() - t.len() > 3 {
        return None;
    }
    let ch = *t.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let n = t.bytes().take_while(|&b| b == ch).count();
    if n < 3 || (ch == b'`' && t[n..].contains('`')) {
        return None;
    }
    Some((ch, n))
}

fn fence_close(line: &str, ch: u8, n: usize) -> bool {
    let t = line.trim();
    t.len() >= n && t.bytes().all(|b| b == ch)
}

/// Découpe le texte en unités d'édition.
pub fn units(text: &str) -> Vec<Unit> {
    let lines = line_ranges(text);
    let line = |i: usize| &text[lines[i].clone()];
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let Some((ch, n)) = fence_open(line(i)) {
            let mut j = i + 1;
            while j < lines.len() && !fence_close(line(j), ch, n) {
                j += 1;
            }
            let last = j.min(lines.len() - 1);
            out.push(Unit {
                range: lines[i].start..lines[last].end,
                kind: UnitKind::Code,
            });
            i = last + 1;
            continue;
        }
        if i + 1 < lines.len()
            && table::is_header_row(line(i))
            && table::is_delimiter_row(line(i + 1))
        {
            let mut j = i + 2;
            while j < lines.len() && !line(j).trim().is_empty() && line(j).contains('|') {
                j += 1;
            }
            out.push(Unit {
                range: lines[i].start..lines[j - 1].end,
                kind: UnitKind::Table,
            });
            i = j;
            continue;
        }
        out.push(Unit {
            range: lines[i].clone(),
            kind: UnitKind::Line,
        });
        i += 1;
    }
    out
}

/// Unité contenant l'octet `b` (sa fin comprise).
pub fn unit_at(units: &[Unit], b: usize) -> usize {
    units
        .iter()
        .position(|u| b <= u.range.end)
        .unwrap_or(units.len().saturating_sub(1))
}

/// Position dans le texte source (octet) du `n`-ième caractère du texte
/// affiché : les caractères de syntaxe (`**`, `[[`, `# `…) sont sautés en
/// suivant les deux textes en parallèle.
pub fn source_offset(source: &str, rendered: &str, n: usize) -> usize {
    let mut want = rendered.chars().take(n).peekable();
    if want.peek().is_none() && n == 0 {
        // Début du texte affiché : après la syntaxe d'ouverture.
        let first = rendered.chars().next();
        return match first {
            Some(c) => source.find(c).unwrap_or(0),
            None => 0,
        };
    }
    let total = rendered.chars().count();
    let mut last = 0;
    for (i, c) in source.char_indices() {
        match want.peek() {
            Some(&w) if w == c => {
                want.next();
                last = i + c.len_utf8();
            }
            Some(_) => {}
            None => break,
        }
    }
    if n >= total { source.len() } else { last }
}

/// Coche ou décoche une tâche (`- [ ] …` ↔ `- [x] …`).
pub fn toggle_task(line: &str) -> Option<String> {
    let lead = line.len() - line.trim_start().len();
    let rest = &line[lead..];
    let marker = if rest.starts_with(['-', '*', '+']) {
        1
    } else {
        let d = rest.bytes().take_while(u8::is_ascii_digit).count();
        if d == 0 || !rest[d..].starts_with(['.', ')']) {
            return None;
        }
        d + 1
    };
    let after = rest.get(marker..)?;
    let spaces = after.len() - after.trim_start().len();
    if spaces == 0 {
        return None;
    }
    let at = lead + marker + spaces;
    let box_ = line.get(at..at + 3)?;
    let new = match box_ {
        "[ ]" => "[x]",
        "[x]" | "[X]" => "[ ]",
        _ => return None,
    };
    Some(format!("{}{new}{}", &line[..at], &line[at + 3..]))
}

/// Retrait d'une ligne en colonnes (tabulation = 4) et longueur en octets.
pub fn indent(line: &str) -> (usize, usize) {
    let mut cols = 0;
    let mut bytes = 0;
    for b in line.bytes() {
        match b {
            b' ' => cols += 1,
            b'\t' => cols += 4,
            _ => break,
        }
        bytes += 1;
    }
    (cols, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(UnitKind, &str)> {
        units(text)
            .into_iter()
            .map(|u| (u.kind, &text[u.range]))
            .collect()
    }

    #[test]
    fn lines_code_and_tables() {
        let text =
            "# Titre\n\n```rust\nlet a = 1;\n\nfin\n```\n| a | b |\n|---|---|\n| 1 | 2 |\nAprès";
        assert_eq!(
            kinds(text),
            [
                (UnitKind::Line, "# Titre"),
                (UnitKind::Line, ""),
                (UnitKind::Code, "```rust\nlet a = 1;\n\nfin\n```"),
                (UnitKind::Table, "| a | b |\n|---|---|\n| 1 | 2 |"),
                (UnitKind::Line, "Après"),
            ]
        );
        assert_eq!(kinds(""), [(UnitKind::Line, "")]);
        // Bloc non fermé : jusqu'à la fin.
        assert_eq!(kinds("~~~\nx"), [(UnitKind::Code, "~~~\nx")]);
        // Une ligne avec des | sans séparateur n'est pas un tableau.
        assert_eq!(
            kinds("a | b\nc"),
            [(UnitKind::Line, "a | b"), (UnitKind::Line, "c")]
        );
        let u = units("ab\r\ncd");
        assert_eq!(u[0].range, 0..2);
        assert_eq!(u[1].range, 4..6);
        assert_eq!(unit_at(&u, 2), 0);
        assert_eq!(unit_at(&u, 4), 1);
        assert_eq!(unit_at(&u, 99), 1);
    }

    #[test]
    fn maps_rendered_positions_to_source() {
        let src = "Un **mot** et [[Coffre|ouvre]] ici";
        let shown = "Un mot et Coffre ici";
        let at = |n| source_offset(src, shown, n);
        assert_eq!(&src[..at(4)], "Un **m");
        assert_eq!(&src[..at(12)], "Un **mot** et [[Co");
        assert_eq!(&src[at(17)..], "ici");
        assert_eq!(at(shown.chars().count()), src.len());
        assert_eq!(source_offset("# Titre", "Titre", 0), 2);
        assert_eq!(source_offset("", "", 0), 0);
    }

    #[test]
    fn toggles_tasks() {
        assert_eq!(toggle_task("- [ ] a").as_deref(), Some("- [x] a"));
        assert_eq!(toggle_task("  1. [X] b").as_deref(), Some("  1. [ ] b"));
        assert_eq!(toggle_task("- a"), None);
        assert_eq!(toggle_task("[ ] a"), None);
        assert_eq!(indent("\t  - x"), (6, 3));
    }
}
