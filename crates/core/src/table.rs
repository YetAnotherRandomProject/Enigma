//! Tableaux Markdown (GFM) : analyse avec la position de chaque cellule,
//! réécriture alignée (colonnes complétées d'espaces, comme dans les IDE
//! JetBrains) en gardant le curseur dans sa cellule, et opérations
//! (ajouter/retirer des lignes ou des colonnes, redimensionner).
//!
//! Lignes du texte : 0 = en-tête, 1 = séparateur (`| --- |`), 2… = données.
//! Un `|` dans du code (`` ` ``), dans un lien `[[Titre|type]]` ou échappé
//! (`\|`) ne sépare pas les cellules. Les largeurs se comptent en
//! caractères.

use std::ops::Range;

/// Alignement d'une colonne (ligne de séparation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// Contenu d'un tableau. `rows[0]` est l'en-tête.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub aligns: Vec<Align>,
    pub rows: Vec<Vec<String>>,
}

/// Cellule repérée dans le texte analysé.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Entre les deux `|`.
    pub raw: Range<usize>,
    /// Contenu sans les espaces autour (vide : position dans `raw`).
    pub content: Range<usize>,
}

/// Tableau analysé avec ses positions (octets du texte donné).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    pub table: Table,
    /// Lignes du texte (séparateur compris).
    pub lines: Vec<Range<usize>>,
    /// Cellules par ligne de texte (séparateur compris).
    pub cells: Vec<Vec<Cell>>,
}

/// Opération sur un tableau, relative à la cellule du curseur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Réaligner seulement.
    Format,
    RowAbove,
    RowBelow,
    DeleteRow,
    ColLeft,
    ColRight,
    DeleteCol,
    Align(Align),
    /// `rows` lignes de données, `cols` colonnes.
    Resize {
        rows: usize,
        cols: usize,
    },
    /// Cellule suivante (Tab) ; ajoute une ligne après la dernière.
    NextCell,
    /// Cellule précédente (Maj+Tab).
    PrevCell,
    /// Même colonne, ligne suivante (Entrée) ; ajoute une ligne au besoin.
    NextRow,
}

fn line_ranges(text: &str) -> Vec<Range<usize>> {
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

/// Plages (relatives à `line`) des cellules d'une ligne, entre les `|`.
pub fn split_cells(line: &str) -> Vec<Range<usize>> {
    let bytes = line.as_bytes();
    let mut pipes = Vec::new();
    let mut i = 0;
    let mut code = false;
    let mut wiki = false;
    // Légende d'image `![légende|300](…)` : sa largeur contient un `|`.
    let mut alt = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'`' => code = !code,
            b'!' if !code && !wiki && bytes.get(i + 1) == Some(&b'[') => {
                alt = true;
                i += 2;
                continue;
            }
            b']' if alt => alt = false,
            b'|' if alt => {}
            b'[' if !code && bytes.get(i + 1) == Some(&b'[') => {
                wiki = true;
                i += 2;
                continue;
            }
            b']' if wiki && bytes.get(i + 1) == Some(&b']') => {
                wiki = false;
                i += 2;
                continue;
            }
            b'|' if !code && !wiki => pipes.push(i),
            _ => {}
        }
        i += 1;
    }
    let t_start = line.len() - line.trim_start().len();
    let t_end = line.trim_end().len();
    if t_start >= t_end {
        return Vec::new();
    }
    let mut begin = t_start;
    let mut end = t_end;
    if pipes.first() == Some(&t_start) {
        pipes.remove(0);
        begin = t_start + 1;
    }
    if end > begin && pipes.last() == Some(&(t_end - 1)) {
        pipes.pop();
        end = t_end - 1;
    }
    let mut out = Vec::with_capacity(pipes.len() + 1);
    let mut s = begin;
    for p in pipes {
        out.push(s..p);
        s = p + 1;
    }
    out.push(s..end.max(s));
    out
}

fn is_delimiter_cell(c: &str) -> bool {
    let c = c.trim();
    let inner = c.strip_prefix(':').unwrap_or(c);
    let inner = inner.strip_suffix(':').unwrap_or(inner);
    !inner.is_empty() && inner.bytes().all(|b| b == b'-')
}

/// Ligne de séparation `| --- | :-: |`.
pub fn is_delimiter_row(line: &str) -> bool {
    line.contains('|') && {
        let cells = split_cells(line);
        !cells.is_empty() && cells.iter().all(|r| is_delimiter_cell(&line[r.clone()]))
    }
}

/// Ligne pouvant être un en-tête de tableau.
pub fn is_header_row(line: &str) -> bool {
    line.contains('|') && !line.trim().is_empty() && !is_delimiter_row(line)
}

fn align_of(c: &str) -> Align {
    let c = c.trim();
    match (c.starts_with(':'), c.ends_with(':') && c.len() > 1) {
        (true, true) => Align::Center,
        (true, false) => Align::Left,
        (false, true) => Align::Right,
        (false, false) => Align::None,
    }
}

fn trimmed(text: &str, raw: Range<usize>) -> Range<usize> {
    let s = &text[raw.clone()];
    let lead = s.len() - s.trim_start().len();
    let body = s.trim();
    if body.is_empty() {
        // Après l'espace de marge s'il y en a un.
        let p = (raw.start + 1).min(raw.end);
        return p..p;
    }
    raw.start + lead..raw.start + lead + body.len()
}

/// Analyse un tableau (en-tête + séparateur + données).
pub fn parse(text: &str) -> Option<Parsed> {
    let lines = line_ranges(text);
    if lines.len() < 2 || !is_delimiter_row(&text[lines[1].clone()]) {
        return None;
    }
    let mut cells = Vec::with_capacity(lines.len());
    let mut rows = Vec::new();
    let mut aligns = Vec::new();
    for (n, l) in lines.iter().enumerate() {
        let line = &text[l.clone()];
        let row: Vec<Cell> = split_cells(line)
            .into_iter()
            .map(|r| {
                let raw = l.start + r.start..l.start + r.end;
                Cell {
                    content: trimmed(text, raw.clone()),
                    raw,
                }
            })
            .collect();
        if n == 1 {
            aligns = row.iter().map(|c| align_of(&text[c.raw.clone()])).collect();
        } else {
            rows.push(
                row.iter()
                    .map(|c| text[c.content.clone()].to_owned())
                    .collect(),
            );
        }
        cells.push(row);
    }
    Some(Parsed {
        table: Table { aligns, rows },
        lines,
        cells,
    })
}

impl Table {
    /// Tableau vide : en-têtes « Colonne n », `rows` lignes de données.
    pub fn empty(rows: usize, cols: usize) -> Table {
        let cols = cols.max(1);
        let mut all = vec![(1..=cols).map(|i| format!("Colonne {i}")).collect()];
        all.extend((0..rows).map(|_| vec![String::new(); cols]));
        Table {
            aligns: vec![Align::None; cols],
            rows: all,
        }
    }

    pub fn cols(&self) -> usize {
        self.rows
            .iter()
            .map(Vec::len)
            .chain([self.aligns.len(), 1])
            .max()
            .unwrap_or(1)
    }

    /// Lignes de données (sans l'en-tête).
    pub fn body_rows(&self) -> usize {
        self.rows.len().saturating_sub(1)
    }

    fn normalize(&mut self) {
        let n = self.cols();
        if self.rows.is_empty() {
            self.rows.push(Vec::new());
        }
        for r in &mut self.rows {
            r.resize(n, String::new());
        }
        self.aligns.resize(n, Align::None);
    }

    /// Texte aligné, et position (octets) du début du contenu de chaque
    /// cellule, par ligne de texte (séparateur compris).
    pub fn write(&self) -> (String, Vec<Vec<usize>>) {
        let mut t = self.clone();
        t.normalize();
        let n = t.cols();
        let width = |s: &str| s.chars().count();
        let widths: Vec<usize> = (0..n)
            .map(|c| {
                t.rows
                    .iter()
                    .map(|r| width(&r[c]))
                    .max()
                    .unwrap_or(0)
                    .max(3)
            })
            .collect();
        let mut out = String::new();
        let mut starts = Vec::with_capacity(t.rows.len() + 1);
        let line = |out: &mut String, cells: Vec<(String, usize)>| {
            // (texte complété, décalage du contenu dans la cellule)
            let mut s = Vec::with_capacity(cells.len());
            out.push('|');
            for (cell, lead) in cells {
                out.push(' ');
                s.push(out.len() + lead);
                out.push_str(&cell);
                out.push_str(" |");
            }
            out.push('\n');
            s
        };
        for (i, row) in t.rows.iter().enumerate() {
            if i == 1 {
                let delim = (0..n)
                    .map(|c| (delimiter(t.aligns[c], widths[c]), 0))
                    .collect();
                starts.push(line(&mut out, delim));
            }
            let cells = row
                .iter()
                .enumerate()
                .map(|(c, s)| pad(s, widths[c], t.aligns[c]))
                .collect();
            starts.push(line(&mut out, cells));
        }
        if t.rows.len() == 1 {
            let delim = (0..n)
                .map(|c| (delimiter(t.aligns[c], widths[c]), 0))
                .collect();
            starts.push(line(&mut out, delim));
        }
        out.pop();
        (out, starts)
    }

    pub fn to_markdown(&self) -> String {
        self.write().0
    }
}

fn delimiter(a: Align, w: usize) -> String {
    match a {
        Align::None => "-".repeat(w),
        Align::Left => format!(":{}", "-".repeat(w - 1)),
        Align::Right => format!("{}:", "-".repeat(w - 1)),
        Align::Center => format!(":{}:", "-".repeat(w - 2)),
    }
}

/// Cellule complétée à `w` caractères, et octet où commence son contenu.
fn pad(s: &str, w: usize, a: Align) -> (String, usize) {
    let extra = w.saturating_sub(s.chars().count());
    let left = match a {
        Align::Right => extra,
        Align::Center => extra / 2,
        _ => 0,
    };
    let out = format!("{}{s}{}", " ".repeat(left), " ".repeat(extra - left));
    (out, left)
}

/// Cellule (ligne de texte, colonne) contenant l'octet `b`, et décalage en
/// caractères depuis le début de son contenu.
pub fn cell_at(text: &str, p: &Parsed, b: usize) -> (usize, usize, usize) {
    let line = p
        .lines
        .iter()
        .position(|l| b <= l.end)
        .unwrap_or(p.lines.len() - 1);
    let row = &p.cells[line];
    if row.is_empty() {
        return (line, 0, 0);
    }
    let col = row
        .iter()
        .position(|c| b <= c.raw.end)
        .unwrap_or(row.len() - 1);
    let c = &row[col];
    let off = if b <= c.content.start {
        0
    } else {
        text[c.content.start..b.min(text.len())].chars().count()
    };
    (line, col, off)
}

/// Ligne de texte → index dans `Table::rows` (`None` : séparateur).
pub fn row_of_line(line: usize) -> Option<usize> {
    match line {
        0 => Some(0),
        1 => None,
        l => Some(l - 1),
    }
}

fn line_of_row(row: usize) -> usize {
    if row == 0 { 0 } else { row + 1 }
}

/// Octet (dans `text`) où commence le contenu de la case de la ligne `row`
/// (0 : en-tête) et de la colonne `col`.
pub fn cell_start(text: &str, row: usize, col: usize) -> Option<usize> {
    let p = parse(text)?;
    p.cells
        .get(line_of_row(row))?
        .get(col)
        .map(|c| c.content.start)
}

/// Protège les `|` qui couperaient la case (hors liens `[[…]]`, code et
/// légendes d'image, que l'analyse sait déjà reconnaître).
pub fn escape_cell(text: &str) -> String {
    let line = format!("|{text}|");
    let cells = split_cells(&line);
    if cells.len() <= 1 {
        return text.to_owned();
    }
    let mut out = text.to_owned();
    // Positions des séparateurs, dans `text` (décalées du `|` initial).
    let seps: Vec<usize> = cells[..cells.len() - 1].iter().map(|r| r.end - 1).collect();
    for p in seps.into_iter().rev() {
        out.insert(p, '\\');
    }
    out
}

/// Remplace le contenu de la case (`row`, `col`) et réécrit le tableau
/// aligné. `None` si `text` n'est pas un tableau.
pub fn set_cell(text: &str, row: usize, col: usize, content: &str) -> Option<String> {
    let mut t = parse(text)?.table;
    t.normalize();
    if col >= t.cols() {
        return None;
    }
    while t.rows.len() <= row {
        t.rows.push(vec![String::new(); t.cols()]);
    }
    t.rows[row][col] = escape_cell(content.trim());
    Some(t.to_markdown())
}

/// Applique une opération au tableau `text`, le curseur étant à l'octet
/// `cursor`. Rend le nouveau texte et la nouvelle position du curseur.
/// La cellule du curseur garde ses espaces de fin (on est peut-être en
/// train de taper « a b »).
pub fn edit(text: &str, cursor: usize, op: Op) -> Option<(String, usize)> {
    let p = parse(text)?;
    let (line, col, mut off) = cell_at(text, &p, cursor);
    let mut t = p.table.clone();
    // Espaces de fin gardés dans la cellule du curseur.
    if let Some(r) = row_of_line(line)
        && let Some(cell) = p.cells[line].get(col)
        && cursor > cell.content.end
        && cursor <= cell.raw.end
        && let Some(slot) = t.rows.get_mut(r).and_then(|row| row.get_mut(col))
    {
        let start = cell.content.start.min(cursor);
        *slot = text[start..cursor].to_owned();
        if cell.content.is_empty() {
            off = text[start..cursor].chars().count();
        }
    }
    t.normalize();
    let mut row = row_of_line(line).unwrap_or(0);
    let mut col = col.min(t.cols() - 1);
    match op {
        Op::Format => {}
        Op::RowAbove => {
            let at = row.max(1);
            t.rows.insert(at, vec![String::new(); t.cols()]);
            row = at;
            off = 0;
        }
        Op::RowBelow => {
            let at = row + 1;
            t.rows.insert(at, vec![String::new(); t.cols()]);
            row = at;
            off = 0;
        }
        Op::DeleteRow => {
            if row == 0 {
                return None;
            }
            t.rows.remove(row);
            row = row.min(t.rows.len() - 1);
            off = 0;
        }
        Op::ColLeft | Op::ColRight => {
            let at = if op == Op::ColLeft { col } else { col + 1 };
            for r in &mut t.rows {
                r.insert(at, String::new());
            }
            t.aligns.insert(at, Align::None);
            col = at;
            off = 0;
        }
        Op::DeleteCol => {
            if t.cols() <= 1 {
                return None;
            }
            for r in &mut t.rows {
                r.remove(col);
            }
            t.aligns.remove(col);
            col = col.min(t.cols() - 1);
            off = 0;
        }
        Op::Align(a) => {
            if let Some(x) = t.aligns.get_mut(col) {
                *x = a;
            }
        }
        Op::Resize { rows, cols } => {
            let cols = cols.max(1);
            for r in &mut t.rows {
                r.resize(cols, String::new());
            }
            t.aligns.resize(cols, Align::None);
            t.rows.resize(rows + 1, vec![String::new(); cols]);
            row = row.min(t.rows.len() - 1);
            col = col.min(cols - 1);
            off = 0;
        }
        Op::NextCell => {
            if col + 1 < t.cols() {
                col += 1;
            } else {
                col = 0;
                row += 1;
                if row >= t.rows.len() {
                    t.rows.push(vec![String::new(); t.cols()]);
                }
            }
            off = 0;
        }
        Op::PrevCell => {
            if col > 0 {
                col -= 1;
            } else if row > 0 {
                row -= 1;
                col = t.cols() - 1;
            }
            off = 0;
        }
        Op::NextRow => {
            row += 1;
            if row >= t.rows.len() {
                t.rows.push(vec![String::new(); t.cols()]);
            }
            off = 0;
        }
    }
    let (out, starts) = t.write();
    let target = line_of_row(row);
    let start = starts
        .get(target)
        .and_then(|l| l.get(col))
        .copied()
        .unwrap_or(0);
    let content = t.rows.get(row).and_then(|r| r.get(col));
    let bytes = content.map_or(0, |c| c.char_indices().nth(off).map_or(c.len(), |(i, _)| i));
    Some((out, start + bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_cells_with_links_code_and_escapes() {
        let line = "| [[Coffre|ouvre]] | `a|b` | x\\|y |";
        let cells: Vec<&str> = split_cells(line)
            .into_iter()
            .map(|r| line[r].trim())
            .collect();
        assert_eq!(cells, ["[[Coffre|ouvre]]", "`a|b`", "x\\|y"]);
        let bare: Vec<&str> = split_cells("a | b")
            .into_iter()
            .map(|r| "a | b"[r].trim())
            .collect();
        assert_eq!(bare, ["a", "b"]);
        assert!(is_delimiter_row("|---|:-:|"));
        assert!(!is_delimiter_row("---"));
        assert!(!is_delimiter_row("| a | b |"));
    }

    #[test]
    fn writes_aligned_columns() {
        let t = Table::empty(2, 2);
        assert_eq!(
            t.to_markdown(),
            "| Colonne 1 | Colonne 2 |\n| --------- | --------- |\n|           |           |\n|           |           |"
        );
        let text = "| a | bbbbb |\n|:-:|--:|\n| ccc | d |";
        let (out, _) = edit(text, 0, Op::Format).unwrap();
        assert_eq!(out, "|  a  | bbbbb |\n| :-: | ----: |\n| ccc |     d |");
    }

    #[test]
    fn cursor_stays_in_its_cell() {
        // Frappe dans la 2e cellule de données : la colonne s'élargit.
        let text = "| a | b |\n| --- | --- |\n| xyz | 1 |";
        let cursor = text.find("xyz").unwrap() + 3;
        let (out, c) = edit(text, cursor, Op::Format).unwrap();
        assert_eq!(out, "| a   | b   |\n| --- | --- |\n| xyz | 1   |");
        assert_eq!(&out[..c], "| a   | b   |\n| --- | --- |\n| xyz");
        // Espace de fin gardé dans la cellule du curseur.
        let text = "| a | b |\n| --- | --- |\n| mot  | 1 |";
        let cursor = text.find("mot").unwrap() + 4;
        let (out, c) = edit(text, cursor, Op::Format).unwrap();
        assert_eq!(out, "| a    | b   |\n| ---- | --- |\n| mot  | 1   |");
        assert_eq!(&out[c - 4..c], "mot ");
    }

    #[test]
    fn rows_and_columns() {
        let text = Table::empty(1, 2).to_markdown();
        let (t2, c) = edit(&text, text.len(), Op::RowBelow).unwrap();
        let p = parse(&t2).unwrap();
        assert_eq!(p.table.body_rows(), 2);
        assert_eq!(cell_at(&t2, &p, c).0, 3);
        let (t3, _) = edit(&t2, 0, Op::ColRight).unwrap();
        assert_eq!(parse(&t3).unwrap().table.cols(), 3);
        let (t4, _) = edit(&t3, 0, Op::DeleteCol).unwrap();
        assert_eq!(parse(&t4).unwrap().table.rows[0], ["", "Colonne 2"]);
        assert!(edit(&t4, 0, Op::DeleteRow).is_none(), "l'en-tête reste");
        let (t5, _) = edit(&t4, 0, Op::Resize { rows: 4, cols: 1 }).unwrap();
        let p = parse(&t5).unwrap();
        assert_eq!((p.table.body_rows(), p.table.cols()), (4, 1));
        // Lignes incomplètes : complétées, rien n'est perdu.
        let (t6, _) = edit("| a |\n|---|\n| 1 | 2 | 3 |", 0, Op::Format).unwrap();
        assert_eq!(parse(&t6).unwrap().table.rows[1], ["1", "2", "3"]);
    }

    #[test]
    fn tab_and_enter_move_between_cells() {
        let text = Table::empty(1, 2).to_markdown();
        let (t, c) = edit(&text, 2, Op::NextCell).unwrap();
        let p = parse(&t).unwrap();
        assert_eq!(cell_at(&t, &p, c), (0, 1, 0));
        let (t, c) = edit(&t, c, Op::NextCell).unwrap();
        let p = parse(&t).unwrap();
        assert_eq!(cell_at(&t, &p, c), (2, 0, 0));
        // Entrée sur la dernière ligne : nouvelle ligne.
        let (t, c) = edit(&t, c, Op::NextRow).unwrap();
        let p = parse(&t).unwrap();
        assert_eq!(p.table.body_rows(), 2);
        assert_eq!(cell_at(&t, &p, c), (3, 0, 0));
        let (t, c) = edit(&t, c, Op::PrevCell).unwrap();
        let p = parse(&t).unwrap();
        assert_eq!(cell_at(&t, &p, c), (2, 1, 0));
    }

    #[test]
    fn sets_and_escapes_cells() {
        let t = "| a | b |
| --- | --- |
| 1 | 2 |";
        let out = set_cell(t, 1, 0, "x | y").unwrap();
        assert_eq!(parse(&out).unwrap().table.rows[1], ["x \\| y", "2"]);
        let out = set_cell(&out, 0, 1, "  ![p|80](a.png) ").unwrap();
        assert_eq!(parse(&out).unwrap().table.rows[0], ["a", "![p|80](a.png)"]);
        // Ligne de données absente : créée.
        let out = set_cell(t, 3, 1, "z").unwrap();
        assert_eq!(parse(&out).unwrap().table.body_rows(), 3);
        assert_eq!(cell_start(t, 1, 1), Some(t.find("2").unwrap()));
        assert!(set_cell("pas un tableau", 0, 0, "x").is_none());
    }

    #[test]
    fn images_in_cells_keep_their_syntax() {
        let text = "| ![a|80](x.png) | ![b](y.png) |\n|---|---|";
        let p = parse(text).unwrap();
        assert_eq!(p.table.rows[0], ["![a|80](x.png)", "![b](y.png)"]);
    }
}
