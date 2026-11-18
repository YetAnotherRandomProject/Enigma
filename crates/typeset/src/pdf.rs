//! Export d'une fiche en PDF : Markdown → document Typst → PDF.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::{MemWorld, TypesetError, latex_to_typst, mermaid_svg};

/// Échappe un texte ordinaire pour Typst.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '#'
                | '$'
                | '*'
                | '_'
                | '`'
                | '<'
                | '>'
                | '@'
                | '['
                | ']'
                | '~'
                | '/'
                | '='
                | '-'
                | '+'
                | '"'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn string_lit(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Prépare la syntaxe propre à l'application : `[[Titre|type]]` → `Titre`,
/// `[[Titre#Section]]` → `Titre › Section`, `![[Titre]]` → `(voir Titre)`,
/// commentaires `%%…%%` retirés, identifiants `^id` retirés.
fn prepare(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(i) = rest.find("[[") {
        let embed = rest[..i].ends_with('!');
        let before = if embed { &rest[..i - 1] } else { &rest[..i] };
        out.push_str(before);
        let after = &rest[i + 2..];
        let Some(end) = after.find("]]").filter(|e| !after[..*e].contains('\n')) else {
            out.push_str("[[");
            rest = after;
            continue;
        };
        let inner = &after[..end];
        let target = inner.split('|').next().unwrap_or(inner).trim();
        let shown = match target.split_once('#') {
            Some((t, s)) if s.starts_with('^') => t.trim().to_owned(),
            Some((t, s)) => format!("{} › {}", t.trim(), s.trim()),
            None => target.to_owned(),
        };
        if embed {
            out.push_str(&format!("(voir {shown})"));
        } else {
            out.push_str(&shown);
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    // Commentaires.
    let mut clean = String::with_capacity(out.len());
    let mut parts = out.split("%%");
    if let Some(first) = parts.next() {
        clean.push_str(first);
    }
    for (k, p) in parts.enumerate() {
        if k % 2 == 1 {
            clean.push_str(p);
        }
    }
    clean
        .lines()
        .map(escape_alt_pipes)
        .map(|l| callout_header(&l).unwrap_or(l))
        .collect::<Vec<_>>()
        .join(
            "
",
        )
        .lines()
        .map(|l| match l.trim_end().rfind(" ^") {
            Some(i)
                if l.trim_end()[i + 2..]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
                    && i + 2 < l.trim_end().len() =>
            {
                &l[..i]
            }
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// En-tête d'encadré `> [!indice]- Titre` → `> **INDICE · Titre**`.
fn callout_header(line: &str) -> Option<String> {
    let lead = line.len() - line.trim_start().len();
    let r = line[lead..]
        .strip_prefix('>')?
        .trim_start()
        .strip_prefix("[!")?;
    let close = r.find(']')?;
    let kind = r[..close].trim().to_uppercase();
    let title = r[close + 1..].trim_start_matches(['-', '+']).trim();
    let head = if title.is_empty() {
        kind
    } else {
        format!("{kind} · {title}")
    };
    Some(format!("{}> **{head}**  ", &line[..lead]))
}

/// Dans une ligne de tableau, le `|` d'une légende d'image
/// (`![plan|120](…)`) ne doit pas couper la cellule.
fn escape_alt_pipes(line: &str) -> String {
    if !line.trim_start().starts_with('|') {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len() + 4);
    let mut in_alt = false;
    let mut prev = ' ';
    for c in line.chars() {
        match c {
            '[' if prev == '!' => in_alt = true,
            ']' if in_alt => in_alt = false,
            '|' if in_alt => out.push('\\'),
            _ => {}
        }
        out.push(c);
        prev = c;
    }
    out
}

/// Fiche → source Typst et fichiers joints (images, diagrammes).
/// `load` lit une image d'après sa source écrite dans la fiche.
pub fn markdown_to_typst(
    title: &str,
    body: &str,
    load: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> (String, Vec<(String, Vec<u8>)>) {
    let mut out = String::from(
        "#set page(paper: \"a4\", margin: 2cm, numbering: \"1\")\n\
         #set text(size: 11pt, lang: \"fr\")\n\
         #set par(justify: true)\n#show link: it => underline(text(fill: rgb(\"#1473e6\"), it))\n\
         #show heading: set block(above: 1.2em, below: 0.6em)\n\
         #show quote.where(block: true): it => block(\n  stroke: (left: 2pt + luma(150)),\n  inset: (left: 10pt, y: 4pt),\n  it.body,\n)\n",
    );
    out.push_str(&format!(
        "#align(center, text(size: 20pt, weight: \"bold\")[{}])\n\n",
        escape(title)
    ));
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let text = prepare(body);
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_FOOTNOTES;
    let mut lists: Vec<bool> = Vec::new();
    let mut image: Option<(String, String)> = None;
    let mut code: Option<(String, String)> = None;
    let mut in_head = false;
    let mut highlight = false;
    let mut quote_start = false;
    for event in Parser::new_ext(&text, opts) {
        if let Some((_, alt)) = &mut image {
            match event {
                Event::Text(t) | Event::Code(t) => {
                    alt.push_str(&t);
                    continue;
                }
                Event::End(TagEnd::Image) => {}
                _ => continue,
            }
        }
        if let Some((_, buf)) = &mut code {
            match event {
                Event::Text(t) => {
                    buf.push_str(&t);
                    continue;
                }
                Event::End(TagEnd::CodeBlock) => {}
                _ => continue,
            }
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    let n = match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    };
                    out.push_str(&format!("\n{} ", "=".repeat(n)));
                }
                Tag::Paragraph => {}
                Tag::BlockQuote(_) => {
                    out.push_str("\n#quote(block: true)[");
                    quote_start = true;
                }
                Tag::CodeBlock(kind) => {
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    code = Some((lang, String::new()));
                }
                Tag::List(start) => {
                    lists.push(start.is_some());
                    out.push('\n');
                }
                Tag::Item => {
                    let depth = lists.len().saturating_sub(1);
                    let marker = if lists.last().copied().unwrap_or(false) {
                        "+"
                    } else {
                        "-"
                    };
                    out.push_str(&format!("{}{marker} ", "  ".repeat(depth)));
                }
                Tag::Emphasis => out.push_str("#emph["),
                Tag::Strong => out.push_str("#strong["),
                Tag::Strikethrough => out.push_str("#strike["),
                Tag::Link { dest_url, .. } => {
                    if dest_url.contains("://") || dest_url.starts_with("mailto:") {
                        out.push_str(&format!("#link({})[", string_lit(&dest_url)));
                    } else {
                        out.push_str("#[");
                    }
                }
                Tag::Image { dest_url, .. } => image = Some((dest_url.to_string(), String::new())),
                Tag::Table(aligns) => {
                    let table_cols = aligns.len().max(1);
                    let al: Vec<&str> = aligns
                        .iter()
                        .map(|a| match a {
                            Alignment::Center => "center",
                            Alignment::Right => "right",
                            _ => "left",
                        })
                        .collect();
                    out.push_str(&format!(
                        "\n#table(columns: {table_cols}, align: ({},), inset: 6pt,\n",
                        al.join(", ")
                    ));
                }
                Tag::TableHead => in_head = true,
                Tag::TableRow => {}
                Tag::TableCell => out.push_str(if in_head { "[*" } else { "[" }),
                Tag::FootnoteDefinition(label) => {
                    out.push_str(&format!("\n#text(size: 9pt)[#super[{}] ", escape(&label)));
                }
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Heading(_) => out.push('\n'),
                TagEnd::Paragraph => out.push_str("\n\n"),
                TagEnd::BlockQuote(_) => out.push_str("]\n"),
                TagEnd::CodeBlock => {
                    if let Some((lang, src)) = code.take() {
                        if lang.trim() == "mermaid" {
                            match mermaid_svg(&src, false) {
                                Ok(svg) => {
                                    let name = format!("/fichiers/diagramme-{}.svg", files.len());
                                    // Taille naturelle (px → pt), au plus la largeur utile.
                                    let w = svg_width(&svg).map_or(300.0, |w| (w * 0.6).min(480.0));
                                    out.push_str(&format!(
                                        "\n#align(center, image({}, width: {w}pt))\n",
                                        string_lit(&name)
                                    ));
                                    files.push((name, svg.into_bytes()));
                                }
                                Err(e) => out.push_str(&format!(
                                    "\n#text(fill: red)[{}]\n",
                                    escape(&e.to_string())
                                )),
                            }
                        } else {
                            let lang = lang.split_whitespace().next().unwrap_or("");
                            out.push_str(&format!(
                                "\n#block(fill: luma(240), inset: 8pt, width: 100%, raw({}, block: true, lang: {}))\n",
                                string_lit(src.trim_end()),
                                string_lit(lang)
                            ));
                        }
                    }
                }
                TagEnd::List(_) => {
                    lists.pop();
                    out.push('\n');
                }
                TagEnd::Item => out.push('\n'),
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                    out.push(']');
                }
                TagEnd::Image => {
                    if let Some((src, alt)) = image.take() {
                        let (label, width) = match alt.rsplit_once('|') {
                            Some((l, w)) if w.trim().parse::<u32>().is_ok() => {
                                (l.to_owned(), w.trim().parse::<f32>().ok())
                            }
                            _ => (alt.clone(), None),
                        };
                        match load(&src) {
                            Some(data) => {
                                let ext = src
                                    .rsplit('.')
                                    .next()
                                    .filter(|e| e.len() <= 4)
                                    .unwrap_or("png")
                                    .to_lowercase();
                                let name = format!("/fichiers/image-{}.{ext}", files.len());
                                let w = width.map_or("auto".to_owned(), |w| {
                                    format!("{}pt", (w * 0.75).min(480.0))
                                });
                                out.push_str(&format!(
                                    "#box(image({}, width: {w}))",
                                    string_lit(&name)
                                ));
                                files.push((name, data));
                            }
                            None => out.push_str(&escape(&format!("[image : {label}]"))),
                        }
                    }
                }
                TagEnd::Table => out.push_str(")\n"),
                TagEnd::TableHead => in_head = false,
                TagEnd::TableCell => out.push_str(if in_head { "*], " } else { "], " }),
                TagEnd::FootnoteDefinition => out.push_str("]\n"),
                _ => {}
            },
            Event::Text(t) => {
                let mut t: &str = &t;
                // Encadré : `[!type] Titre` en tête de citation.
                if quote_start {
                    quote_start = false;
                    if let Some(rest) = t.strip_prefix("[!")
                        && let Some(close) = rest.find(']')
                    {
                        let kind = rest[..close].to_uppercase();
                        out.push_str(&format!("#strong[{}] ", escape(&kind)));
                        t = rest[close + 1..].trim_start_matches(['-', '+']);
                    }
                }
                // Surlignage `==…==`.
                let mut first = true;
                for piece in t.split("==") {
                    if !first {
                        highlight = !highlight;
                        out.push_str(if highlight { "#highlight[" } else { "]" });
                    }
                    first = false;
                    out.push_str(&escape(piece));
                }
            }
            Event::Code(c) => out.push_str(&format!("#raw({})", string_lit(&c))),
            Event::InlineMath(m) => out.push_str(&format!("${}$", latex_to_typst(&m))),
            Event::DisplayMath(m) => out.push_str(&format!("\n$ {} $\n", latex_to_typst(&m))),
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push_str(" \\\n"),
            Event::Rule => out.push_str("\n#line(length: 100%)\n"),
            Event::TaskListMarker(done) => out.push_str(if done { "☑ " } else { "☐ " }),
            Event::FootnoteReference(label) => {
                out.push_str(&format!("#super[{}]", escape(&label)));
            }
            _ => {}
        }
    }
    if highlight {
        out.push(']');
    }
    (out, files)
}

/// Largeur déclarée d'un SVG (`width="…"`).
fn svg_width(svg: &str) -> Option<f32> {
    let head = &svg[..svg.find('>')?];
    let i = head.find(" width=\"")? + 8;
    let rest = &head[i..];
    let num: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse().ok()
}

/// Fiche → PDF.
pub fn note_pdf(
    title: &str,
    body: &str,
    load: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<Vec<u8>, TypesetError> {
    let (src, files) = markdown_to_typst(title, body, load);
    let world = MemWorld::new(src, files).ok_or_else(|| TypesetError::Pdf("chemin".into()))?;
    let doc = world.compile().map_err(TypesetError::Pdf)?;
    typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()).map_err(|errs| {
        TypesetError::Pdf(
            errs.iter()
                .map(|e| e.message.to_string())
                .collect::<Vec<_>>()
                .join(" ; "),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepares_app_syntax() {
        assert_eq!(
            prepare("Voir [[Coffre|ouvre]] et [[Salle#Est]].\n![[Plan]] %%secret%%fin ^abc"),
            "Voir Coffre et Salle › Est.\n(voir Plan) fin"
        );
        assert_eq!(
            prepare("> [!indice]- Le coffre\n> suite"),
            "> **INDICE · Le coffre**  \n> suite"
        );
    }

    #[test]
    fn exports_a_rich_note() {
        let body = "# Salle\n\nUn *carnet* **important**, code `1945`, ==surligné==, $x^2$.\n\n\
                    - [ ] fouiller\n- [x] lire\n\n1. un\n2. deux\n\n> [!indice] Le coffre\n> Code à 4 chiffres.\n\n\
                    | A | B |\n|---|---|\n| 1 | ![plan|120](plan.svg) |\n\n$$\\frac{a}{b}$$\n\n\
                    ```mermaid\nflowchart LR\n  A --> B\n```\n\n```rust\nlet x = 1;\n```\n\n\
                    Note[^1] et [site](https://exemple.fr). Chemin a/b, prix 5 $ + 3 = 8.\n\n[^1]: La note.";
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#4aa3ff"/></svg>"##;
        let load = |src: &str| (src == "plan.svg").then(|| svg.to_vec());
        let (typ, files) = markdown_to_typst("Salle des Archives", body, &load);
        assert_eq!(files.len(), 2, "image + diagramme");
        let pdf =
            note_pdf("Salle des Archives", body, &load).unwrap_or_else(|e| panic!("{e}\n{typ}"));
        assert!(pdf.starts_with(b"%PDF"));
        if let Ok(dir) = std::env::var("ENIGMA_SNAPSHOT_DIR") {
            std::fs::write(std::path::Path::new(&dir).join("fiche.pdf"), &pdf).unwrap();
            let (src, files) = markdown_to_typst("Salle des Archives", body, &load);
            let doc = MemWorld::new(src, files).unwrap().compile().unwrap();
            let svg = typst_svg::svg_merged(
                &doc,
                &typst_svg::SvgOptions::default(),
                typst::layout::Abs::pt(10.0),
            );
            let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
            let size = tree.size().to_int_size();
            let mut pix = resvg::tiny_skia::Pixmap::new(size.width(), size.height()).unwrap();
            pix.fill(resvg::tiny_skia::Color::WHITE);
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::default(),
                &mut pix.as_mut(),
            );
            pix.save_png(std::path::Path::new(&dir).join("fiche.png"))
                .unwrap();
        }
    }
}
