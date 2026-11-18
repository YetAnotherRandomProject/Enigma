//! Images dans le texte et découpage d'une fiche en blocs pour le mode
//! Lecture (sans dépendance à l'interface).
//!
//! Taille d'affichage d'une image : largeur en pixels après « | » dans la
//! légende, `![Plan du manoir|300](assets/plan.svg)`.

use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

use crate::markdown::{options, wiki_links};

/// Image `![légende|largeur](source)` trouvée dans le texte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    /// Toute la syntaxe de l'image dans le texte source.
    pub range: Range<usize>,
    /// Légende, sans la largeur.
    pub alt: String,
    /// Largeur d'affichage voulue (pixels).
    pub width: Option<u32>,
    /// Chemin ou adresse de l'image, tel qu'écrit (déséchappé).
    pub src: String,
}

/// Sépare `légende|300` en (`légende`, `Some(300)`).
pub fn split_alt(alt: &str) -> (String, Option<u32>) {
    if let Some((label, w)) = alt.rsplit_once('|')
        && let Ok(w) = w.trim().parse::<u32>()
        && w > 0
    {
        return (label.trim_end().to_owned(), Some(w));
    }
    (alt.to_owned(), None)
}

/// Images du texte (hors code), dans l'ordre.
pub fn images(text: &str) -> Vec<ImageRef> {
    let mut out = Vec::new();
    let mut current: Option<(Range<usize>, String, String)> = None;
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => {
                current = Some((range, dest_url.to_string(), String::new()));
            }
            Event::End(TagEnd::Image) => {
                if let Some((range, src, alt)) = current.take() {
                    let (alt, width) = split_alt(&alt);
                    out.push(ImageRef {
                        range,
                        alt,
                        width,
                        src,
                    });
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some((_, _, alt)) = &mut current {
                    alt.push_str(&t);
                }
            }
            _ => {}
        }
    }
    out
}

/// Réécrit la largeur d'une image (`None` : retire la largeur). La source
/// de l'image et sa légende sont gardées telles qu'écrites.
pub fn set_image_width(text: &str, image: &ImageRef, width: Option<u32>) -> String {
    let Some(src) = text.get(image.range.clone()) else {
        return text.to_owned();
    };
    let Some(close) = src.find("](") else {
        return text.to_owned();
    };
    let alt_written = src.get(2..close).unwrap_or("");
    let (label, _) = split_alt(alt_written);
    let new_alt = match width {
        Some(w) => format!("{label}|{w}"),
        None => label,
    };
    let mut out = String::with_capacity(text.len() + 8);
    out.push_str(&text[..image.range.start]);
    out.push_str("![");
    out.push_str(&new_alt);
    out.push_str(&src[close..]);
    out.push_str(&text[image.range.end..]);
    out
}

/// Texte d'insertion d'une image.
pub fn image_markdown(alt: &str, src: &str) -> String {
    let alt: String = alt
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '|'))
        .collect();
    let src = if src.contains([' ', '(', ')']) {
        format!("<{src}>")
    } else {
        src.to_owned()
    };
    format!("![{alt}]({src})")
}

/// Style d'un morceau de texte.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub strong: bool,
    pub emphasis: bool,
    pub code: bool,
    pub strike: bool,
}

/// Lien porté par un morceau de texte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineLink {
    /// `[[Titre]]` ou `[[Titre|type]]`.
    Wiki {
        target: String,
        kind: Option<String>,
    },
    /// Lien Markdown classique.
    Url(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub style: InlineStyle,
    pub link: Option<InlineLink>,
}

/// Bloc à afficher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        spans: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    ListItem {
        depth: usize,
        marker: String,
        /// Liste de tâches : case cochée ou non.
        task: Option<bool>,
        spans: Vec<Inline>,
    },
    Quote(Vec<Inline>),
    Code(String),
    Rule,
    Image(ImageRef),
    /// Paragraphe réduit à un lien vers un fichier local (son, vidéo,
    /// document) : affiché comme une pièce jointe.
    Attachment {
        label: String,
        src: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Container {
    Paragraph,
    Heading(u8),
    Item,
}

struct Builder<'t> {
    text: &'t str,
    wiki: Vec<crate::markdown::WikiLink>,
    blocks: Vec<Block>,
    spans: Vec<Inline>,
    style: InlineStyle,
    url: Option<String>,
    container: Option<Container>,
    quote_depth: usize,
    /// Compteurs des listes ouvertes (`None` : liste à puces).
    lists: Vec<Option<u64>>,
    item_marker: String,
    item_task: Option<bool>,
    /// Fin du dernier lien `[[...]]` émis (ses morceaux suivants sont ignorés).
    wiki_done_until: usize,
}

impl Builder<'_> {
    fn push_text(&mut self, s: &str, range: Range<usize>) {
        // Lien [[...]] couvrant ce morceau : un seul morceau cliquable.
        // Morceau qui commence dans un lien déjà émis : seule la fin compte.
        if range.start < self.wiki_done_until {
            let end = self.wiki_done_until;
            if range.end > end
                && let Some(tail) = s.get(s.len().saturating_sub(range.end - end)..)
            {
                let tail = tail.to_owned();
                self.push_text(&tail, end..range.end);
            }
            return;
        }
        let found = self
            .wiki
            .iter()
            .find(|w| w.range.start < range.end && range.start < w.range.end)
            .map(|w| (w.range.clone(), w.target.clone(), w.kind.clone()));
        if let Some((wrange, target, kind)) = found {
            let before = wrange.start.saturating_sub(range.start);
            if before > 0
                && let Some(head) = s.get(..before)
            {
                self.push_plain(head);
            }
            let end = wrange.end;
            self.spans.push(Inline {
                text: target.clone(),
                style: self.style,
                link: Some(InlineLink::Wiki { target, kind }),
            });
            self.wiki_done_until = end;
            if range.end > end
                && let Some(tail) = s.get(s.len() - (range.end - end)..)
            {
                let tail = tail.to_owned();
                let start = end;
                self.push_text(&tail, start..range.end);
            }
            return;
        }
        self.push_plain(s);
    }

    fn push_plain(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let link = self.url.clone().map(InlineLink::Url);
        if let Some(last) = self.spans.last_mut()
            && last.style == self.style
            && last.link == link
        {
            last.text.push_str(s);
            return;
        }
        self.spans.push(Inline {
            text: s.to_owned(),
            style: self.style,
            link,
        });
    }

    /// Termine le bloc en cours (texte accumulé).
    fn flush(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let block = match self.container {
            Some(Container::Heading(level)) => Block::Heading { level, spans },
            Some(Container::Item) => Block::ListItem {
                depth: self.lists.len().saturating_sub(1),
                marker: std::mem::take(&mut self.item_marker),
                task: self.item_task.take(),
                spans,
            },
            _ if self.quote_depth > 0 => Block::Quote(spans),
            _ => match spans.as_slice() {
                [
                    Inline {
                        text,
                        link: Some(InlineLink::Url(url)),
                        ..
                    },
                ] if !crate::import::is_remote(url) && !url.starts_with('#') => Block::Attachment {
                    label: text.clone(),
                    src: url.clone(),
                },
                _ => Block::Paragraph(spans),
            },
        };
        self.blocks.push(block);
    }
}

/// Découpe une fiche en blocs pour le mode Lecture. Les images deviennent
/// des blocs à part ; les `[[liens]]` sont des morceaux cliquables.
pub fn blocks(text: &str) -> Vec<Block> {
    let mut b = Builder {
        text,
        wiki: wiki_links(text),
        blocks: Vec::new(),
        spans: Vec::new(),
        style: InlineStyle::default(),
        url: None,
        container: None,
        quote_depth: 0,
        lists: Vec::new(),
        item_marker: String::new(),
        item_task: None,
        wiki_done_until: 0,
    };
    let mut image: Option<(Range<usize>, String, String)> = None;
    let mut code: Option<String> = None;
    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        if let Some((_, _, alt)) = &mut image {
            match event {
                Event::End(TagEnd::Image) => {
                    if let Some((range, src, alt)) = image.take() {
                        let (alt, width) = split_alt(&alt);
                        let keep = b.container;
                        b.flush();
                        b.container = keep;
                        b.blocks.push(Block::Image(ImageRef {
                            range,
                            alt,
                            width,
                            src,
                        }));
                    }
                }
                Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                _ => {}
            }
            continue;
        }
        if let Some(buf) = &mut code {
            match event {
                Event::Text(t) => buf.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let mut s = code.take().unwrap_or_default();
                    if s.ends_with('\n') {
                        s.pop();
                    }
                    b.blocks.push(Block::Code(s));
                }
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    if b.container != Some(Container::Item) {
                        b.container = Some(Container::Paragraph);
                    }
                }
                Tag::Heading { level, .. } => {
                    b.flush();
                    b.container = Some(Container::Heading(heading_level(level)));
                }
                Tag::BlockQuote(_) => {
                    b.flush();
                    b.quote_depth += 1;
                }
                Tag::CodeBlock(_) => {
                    b.flush();
                    code = Some(String::new());
                }
                Tag::List(start) => {
                    b.flush();
                    b.lists.push(start);
                }
                Tag::Item => {
                    b.flush();
                    b.container = Some(Container::Item);
                    b.item_marker = match b.lists.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}.");
                            *n += 1;
                            m
                        }
                        _ => "•".to_owned(),
                    };
                }
                Tag::Emphasis => b.style.emphasis = true,
                Tag::Strong => b.style.strong = true,
                Tag::Strikethrough => b.style.strike = true,
                Tag::Link { dest_url, .. } => b.url = Some(dest_url.to_string()),
                Tag::Image { dest_url, .. } => {
                    image = Some((range, dest_url.to_string(), String::new()));
                }
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::Paragraph => {
                    if b.container != Some(Container::Item) {
                        b.flush();
                        b.container = None;
                    }
                }
                TagEnd::Heading(_) => {
                    b.flush();
                    b.container = None;
                }
                TagEnd::BlockQuote(_) => {
                    b.flush();
                    b.quote_depth = b.quote_depth.saturating_sub(1);
                }
                TagEnd::Item => {
                    b.flush();
                    b.container = if b.lists.len() > 1 {
                        Some(Container::Item)
                    } else {
                        None
                    };
                }
                TagEnd::List(_) => {
                    b.flush();
                    b.lists.pop();
                    if b.lists.is_empty() {
                        b.container = None;
                    }
                }
                TagEnd::Emphasis => b.style.emphasis = false,
                TagEnd::Strong => b.style.strong = false,
                TagEnd::Strikethrough => b.style.strike = false,
                TagEnd::Link => b.url = None,
                _ => {}
            },
            Event::Text(t) => b.push_text(&t, range),
            Event::Code(t) => {
                let saved = b.style;
                b.style.code = true;
                b.push_plain(&t);
                b.style = saved;
            }
            Event::SoftBreak => b.push_plain(" "),
            Event::HardBreak => b.push_plain("\n"),
            Event::Rule => {
                b.flush();
                b.blocks.push(Block::Rule);
            }
            Event::TaskListMarker(done) => b.item_task = Some(done),
            Event::Html(t) | Event::InlineHtml(t) => b.push_plain(&t),
            _ => {}
        }
    }
    b.flush();
    let _ = b.text;
    b.blocks
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_images_with_width() {
        let text =
            "Voir ![Plan du manoir|300](assets/plan.svg) et ![](a.png).\n\n`![pas|1](code.png)`";
        let imgs = images(text);
        assert_eq!(imgs.len(), 2);
        assert_eq!(imgs[0].alt, "Plan du manoir");
        assert_eq!(imgs[0].width, Some(300));
        assert_eq!(imgs[0].src, "assets/plan.svg");
        assert_eq!(
            &text[imgs[0].range.clone()],
            "![Plan du manoir|300](assets/plan.svg)"
        );
        assert_eq!(imgs[1].width, None);
        assert_eq!(split_alt("a|b"), ("a|b".to_owned(), None));
        assert_eq!(split_alt("x|0"), ("x|0".to_owned(), None));
    }

    #[test]
    fn rewrites_width_only() {
        let text = "Avant ![Plan|300](<assets/mon plan.svg>) après";
        let img = &images(text)[0];
        assert_eq!(img.src, "assets/mon plan.svg");
        let t = set_image_width(text, img, Some(450));
        assert_eq!(t, "Avant ![Plan|450](<assets/mon plan.svg>) après");
        let img = &images(&t)[0];
        let t = set_image_width(&t, img, None);
        assert_eq!(t, "Avant ![Plan](<assets/mon plan.svg>) après");
        let t2 = "![](a.png)";
        assert_eq!(
            set_image_width(t2, &images(t2)[0], Some(80)),
            "![|80](a.png)"
        );
        assert_eq!(images("![|80](a.png)")[0].width, Some(80));
    }

    #[test]
    fn insertion_syntax_is_parsable() {
        let md = image_markdown("Plan [v2]", "assets/mon plan (1).png");
        let img = &images(&md)[0];
        assert_eq!(img.src, "assets/mon plan (1).png");
        assert_eq!(img.alt, "Plan v2");
    }

    fn texts(spans: &[Inline]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn blocks_cover_common_markdown() {
        let text = "# Salle\n\nUn *carnet* **important** cite [[Coffre du bureau|ouvre]] et `1945`.\n\n\
                    - un\n- deux\n  1. a\n  2. b\n\n> Les archives\n\n```\ncode\n```\n\n---\n\n\
                    ![Plan|200](assets/plan.svg)\n\nFin [web](https://x.y).";
        let b = blocks(text);
        assert!(matches!(&b[0], Block::Heading { level: 1, spans } if texts(spans) == "Salle"));
        let Block::Paragraph(p) = &b[1] else {
            panic!("{:?}", b[1])
        };
        assert_eq!(
            texts(p),
            "Un carnet important cite Coffre du bureau et 1945."
        );
        assert!(p.iter().any(|s| s.style.emphasis && s.text == "carnet"));
        assert!(p.iter().any(|s| s.style.strong && s.text == "important"));
        assert!(p.iter().any(|s| s.style.code && s.text == "1945"));
        assert!(p.iter().any(|s| s.link
            == Some(InlineLink::Wiki {
                target: "Coffre du bureau".into(),
                kind: Some("ouvre".into())
            })));
        let items: Vec<(usize, &str, String)> = b
            .iter()
            .filter_map(|x| match x {
                Block::ListItem {
                    depth,
                    marker,
                    spans,
                    ..
                } => Some((*depth, marker.as_str(), texts(spans))),
                _ => None,
            })
            .collect();
        assert_eq!(
            items,
            [
                (0, "•", "un".to_owned()),
                (0, "•", "deux".to_owned()),
                (1, "1.", "a".to_owned()),
                (1, "2.", "b".to_owned())
            ]
        );
        assert!(
            b.iter()
                .any(|x| matches!(x, Block::Quote(s) if texts(s) == "Les archives"))
        );
        assert!(b.contains(&Block::Code("code".into())));
        assert!(b.contains(&Block::Rule));
        assert!(b.iter().any(
            |x| matches!(x, Block::Image(i) if i.width == Some(200) && i.src == "assets/plan.svg")
        ));
        let Some(Block::Paragraph(last)) = b.last() else {
            panic!()
        };
        assert!(
            last.iter()
                .any(|s| s.link == Some(InlineLink::Url("https://x.y".into())) && s.text == "web")
        );
    }

    #[test]
    fn local_file_link_alone_is_an_attachment() {
        let b = blocks("[Ambiance](<assets/son du manoir.ogg>)\n\nVoir [site](https://x.y) ici.");
        assert_eq!(
            b[0],
            Block::Attachment {
                label: "Ambiance".into(),
                src: "assets/son du manoir.ogg".into()
            }
        );
        assert!(matches!(&b[1], Block::Paragraph(_)));
    }

    #[test]
    fn image_inside_paragraph_splits_it() {
        let b = blocks("Avant ![x](a.png) après");
        assert_eq!(b.len(), 3);
        assert!(matches!(&b[0], Block::Paragraph(s) if texts(s) == "Avant "));
        assert!(matches!(&b[1], Block::Image(_)));
        assert!(matches!(&b[2], Block::Paragraph(s) if texts(s) == " après"));
    }
}
