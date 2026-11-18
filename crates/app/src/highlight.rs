//! Mise en forme Markdown pendant la frappe : transforme l'analyse de
//! `enigma_core::markdown` en `LayoutJob` egui. Le texte source reste
//! intact (les caractères de syntaxe sont seulement atténués).

use std::hash::{DefaultHasher, Hash, Hasher};

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Color32, FontId, Stroke, Visuals};
use enigma_core::Project;
use enigma_core::markdown::{self, Style};

use crate::colors::WIKI_LINK;

pub const LIST_MARKER: Color32 = Color32::from_rgb(0xff, 0xb8, 0x6b);
pub const BODY_SIZE: f32 = 15.0;

/// Cache du dernier `LayoutJob` calculé : l'analyse n'est refaite que si le
/// texte, le projet (titres → liens cassés) ou le thème changent.
#[derive(Default)]
pub struct Highlighter {
    key: Option<u64>,
    job: LayoutJob,
}

impl Highlighter {
    pub fn job(&mut self, text: &str, project: &Project, visuals: &Visuals) -> LayoutJob {
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        project.generation().hash(&mut h);
        visuals.dark_mode.hash(&mut h);
        let key = h.finish();
        if self.key != Some(key) {
            self.job = build(text, project, visuals);
            self.key = Some(key);
        }
        self.job.clone()
    }
}

fn build(text: &str, project: &Project, visuals: &Visuals) -> LayoutJob {
    let analysis = markdown::analyze(text);
    let broken: Vec<std::ops::Range<usize>> = analysis
        .links
        .iter()
        .filter(|l| project.resolve_title(&l.target).is_none())
        .map(|l| l.range.clone())
        .collect();
    let mut job = LayoutJob::default();
    for span in &analysis.spans {
        let is_broken =
            span.style.wiki_link && broken.iter().any(|r| r.contains(&span.range.start));
        job.append(
            &text[span.range.clone()],
            0.0,
            format(span.style, is_broken, visuals),
        );
    }
    job
}

fn format(s: Style, broken: bool, v: &Visuals) -> TextFormat {
    let size = match s.heading {
        1 => 26.0,
        2 => 22.0,
        3 => 19.0,
        4 => 17.0,
        _ => BODY_SIZE,
    };
    let mono = s.code || s.code_block;
    let font_id = if mono {
        FontId::monospace(if s.heading > 0 { size } else { 14.0 })
    } else {
        FontId::proportional(size)
    };

    // egui n'embarque pas de police grasse : le gras et les titres utilisent
    // la couleur « forte » du thème.
    let mut color = v.text_color();
    if s.heading > 0 || s.bold {
        color = v.strong_text_color();
    }
    if s.quote {
        color = v.weak_text_color();
    }
    if s.link {
        color = v.hyperlink_color;
    }
    if s.wiki_link {
        color = if broken { v.error_fg_color } else { WIKI_LINK };
    }
    if s.markup {
        color = if s.list_marker {
            LIST_MARKER
        } else if s.wiki_link {
            color.gamma_multiply(0.55)
        } else {
            v.weak_text_color().gamma_multiply(0.8)
        };
    }

    TextFormat {
        font_id,
        color,
        background: if mono {
            v.code_bg_color
        } else {
            Color32::TRANSPARENT
        },
        italics: s.italic || s.quote,
        strikethrough: if s.strike {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        },
        underline: if s.wiki_link && !s.markup && broken {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        },
        ..Default::default()
    }
}
