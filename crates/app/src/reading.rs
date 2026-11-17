//! Mode Lecture d'une fiche : texte mis en forme (titres, emphase, listes,
//! citations, code, liens cliquables) et images redimensionnables.
//!
//! Le découpage en blocs vient de `enigma_core::render` et n'est refait que
//! si le texte change. Tirer le coin d'une image change sa largeur ; au
//! relâchement, la largeur est écrite dans le texte (`![légende|300](…)`).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use eframe::egui::{
    self, Color32, CursorIcon, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2,
    text::{LayoutJob, TextFormat},
};
use enigma_core::Project;
use enigma_core::import::{is_remote, resolve_image};
use enigma_core::render::{Block, ImageRef, Inline, InlineLink, blocks};

/// Largeur minimale d'une image redimensionnée.
const MIN_WIDTH: f32 = 32.0;
/// Côté de la poignée de redimensionnement.
const HANDLE: f32 = 12.0;

/// Ce que l'utilisateur a demandé dans la vue Lecture.
#[derive(Debug, PartialEq)]
pub enum ReadingAction {
    /// Clic sur `[[Titre]]`.
    OpenTitle(String),
    /// Nouvelle largeur d'une image (fin du glisser).
    Resize { image: ImageRef, width: u32 },
}

#[derive(Default)]
pub struct ReadingState {
    key: Option<u64>,
    blocks: Vec<Block>,
    /// Image en cours de redimensionnement : (début de sa syntaxe, largeur).
    drag: Option<(usize, f32)>,
}

/// Adresse `file://` d'un fichier local, comprise par les chargeurs d'egui.
pub fn file_uri(path: &Path) -> String {
    let p = path.display().to_string();
    if cfg!(windows) {
        // Les chargeurs attendent `file:///C:/…` sous Windows.
        format!("file:///{}", p.replace('\\', "/"))
    } else {
        format!("file://{p}")
    }
}

fn hash_text(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl ReadingState {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        body: &str,
        project: &Project,
        note_path: &Path,
    ) -> Option<ReadingAction> {
        let key = hash_text(body);
        if self.key != Some(key) {
            self.blocks = blocks(body);
            self.key = Some(key);
        }
        let mut action = None;
        let blocks = std::mem::take(&mut self.blocks);
        egui::ScrollArea::vertical()
            .id_salt("reading_scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                if blocks.is_empty() {
                    ui.weak("Fiche vide. Passez en mode Édition pour écrire.");
                }
                for block in &blocks {
                    if let Some(a) = self.block_ui(ui, block, project, note_path) {
                        action = Some(a);
                    }
                }
            });
        self.blocks = blocks;
        action
    }

    fn block_ui(
        &mut self,
        ui: &mut egui::Ui,
        block: &Block,
        project: &Project,
        note_path: &Path,
    ) -> Option<ReadingAction> {
        let visuals = ui.visuals().clone();
        match block {
            Block::Heading { level, spans } => {
                let size = match level {
                    1 => 24.0,
                    2 => 20.0,
                    3 => 17.0,
                    _ => 15.0,
                };
                ui.add_space(4.0);
                rich_text(ui, spans, size, Some(visuals.strong_text_color()), project)
            }
            Block::Paragraph(spans) => rich_text(ui, spans, 14.0, None, project),
            Block::ListItem {
                depth,
                marker,
                spans,
            } => {
                let mut out = None;
                ui.horizontal_top(|ui| {
                    ui.add_space(8.0 + 18.0 * *depth as f32);
                    ui.label(RichText::new(marker).size(14.0).weak());
                    out = rich_text(ui, spans, 14.0, None, project);
                });
                out
            }
            Block::Quote(spans) => {
                let mut out = None;
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 4,
                        top: 2,
                        bottom: 2,
                    })
                    .stroke(Stroke::NONE)
                    .show(ui, |ui| {
                        let r = ui.max_rect();
                        ui.painter().line_segment(
                            [
                                Pos2::new(r.left() - 8.0, r.top()),
                                Pos2::new(r.left() - 8.0, r.bottom()),
                            ],
                            Stroke::new(3.0, visuals.weak_text_color()),
                        );
                        out = rich_text(ui, spans, 14.0, Some(visuals.weak_text_color()), project);
                    });
                out
            }
            Block::Code(code) => {
                egui::Frame::new()
                    .fill(visuals.code_bg_color)
                    .corner_radius(4.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(code).monospace());
                    });
                None
            }
            Block::Rule => {
                ui.separator();
                None
            }
            Block::Image(img) => self.image_ui(ui, img, project, note_path),
        }
    }

    fn image_ui(
        &mut self,
        ui: &mut egui::Ui,
        img: &ImageRef,
        project: &Project,
        note_path: &Path,
    ) -> Option<ReadingAction> {
        let Some(path) = resolve_image(project.root(), note_path, &img.src) else {
            let msg = if is_remote(&img.src) {
                format!("Image en ligne non affichée : {}", img.src)
            } else {
                format!("Image introuvable : {}", img.src)
            };
            ui.colored_label(ui.visuals().warn_fg_color, msg);
            return None;
        };
        let uri = file_uri(&path);
        let available = ui.available_width().max(MIN_WIDTH);
        let image = egui::Image::new(uri.clone());
        // Taille d'origine (connue une fois l'image chargée).
        let natural = match image.load_for_size(ui.ctx(), Vec2::splat(available)) {
            Ok(poll) => poll.size(),
            Err(e) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Image illisible ({}) : {e}", img.src),
                );
                return None;
            }
        };
        let Some(natural) = natural.filter(|s| s.x > 0.0 && s.y > 0.0) else {
            ui.add(egui::Spinner::new());
            return None;
        };
        let wanted = match self.drag {
            Some((start, w)) if start == img.range.start => w,
            _ => img.width.map_or(natural.x, |w| w as f32),
        };
        let width = wanted.clamp(MIN_WIDTH, available);
        let size = Vec2::new(width, width * natural.y / natural.x);
        let resp = ui.add(
            egui::Image::new(uri)
                .fit_to_exact_size(size)
                .alt_text(img.alt.clone()),
        );
        if !img.alt.is_empty() {
            resp.clone().on_hover_text(&img.alt);
        }

        // Poignée de redimensionnement (coin bas-droit).
        let handle_rect = Rect::from_min_size(
            resp.rect.right_bottom() - Vec2::splat(HANDLE),
            Vec2::splat(HANDLE),
        );
        let id = ui.scope_id().with(("resize", img.range.start));
        let handle = ui
            .interact(handle_rect, id, Sense::drag())
            .on_hover_cursor(CursorIcon::ResizeNwSe)
            .on_hover_text("Glisser pour redimensionner");
        let hot = handle.hovered() || handle.dragged() || resp.hovered();
        if hot {
            let painter = ui.painter();
            painter.rect_stroke(
                resp.rect,
                0.0,
                Stroke::new(1.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
            painter.rect_filled(handle_rect, 2.0, ui.visuals().selection.stroke.color);
        }
        if handle.dragged() {
            let new_w = (width + handle.drag_delta().x).clamp(MIN_WIDTH, available);
            self.drag = Some((img.range.start, new_w));
            ui.ctx().request_repaint();
        }
        if handle.drag_stopped()
            && let Some((start, w)) = self.drag.take()
            && start == img.range.start
        {
            return Some(ReadingAction::Resize {
                image: img.clone(),
                width: w.round() as u32,
            });
        }
        if hot {
            ui.label(
                RichText::new(format!("{} × {} px", size.x.round(), size.y.round()))
                    .small()
                    .weak(),
            );
        }
        None
    }
}

/// Paragraphe mis en forme ; rend l'action d'un clic sur un lien.
fn rich_text(
    ui: &mut egui::Ui,
    spans: &[Inline],
    size: f32,
    color: Option<Color32>,
    project: &Project,
) -> Option<ReadingAction> {
    let visuals = ui.visuals().clone();
    let base = color.unwrap_or(visuals.text_color());
    let mut job = LayoutJob::default();
    // Plages (en caractères) des morceaux cliquables.
    let mut links: Vec<(std::ops::Range<usize>, &InlineLink)> = Vec::new();
    let mut chars = 0;
    for span in spans {
        let mut fmt = TextFormat {
            font_id: if span.style.code {
                FontId::monospace(size - 1.0)
            } else {
                FontId::proportional(size)
            },
            color: base,
            ..Default::default()
        };
        if span.style.strong {
            fmt.color = visuals.strong_text_color();
        }
        if span.style.emphasis {
            fmt.italics = true;
        }
        if span.style.strike {
            fmt.strikethrough = Stroke::new(1.0, fmt.color);
        }
        if span.style.code {
            fmt.background = visuals.code_bg_color;
        }
        if let Some(link) = &span.link {
            fmt.color = match link {
                InlineLink::Wiki { target, .. } if project.resolve_title(target).is_none() => {
                    visuals.error_fg_color
                }
                _ => visuals.hyperlink_color,
            };
            fmt.underline = Stroke::new(1.0, fmt.color);
            let n = span.text.chars().count();
            links.push((chars..chars + n, link));
        }
        chars += span.text.chars().count();
        job.append(&span.text, 0.0, fmt);
    }
    job.wrap.max_width = ui.available_width();
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let sense = if links.is_empty() {
        Sense::hover()
    } else {
        Sense::click()
    };
    let (rect, resp) = ui.allocate_exact_size(galley.size(), sense);
    ui.painter().galley(rect.min, galley.clone(), base);
    if links.is_empty() {
        return None;
    }
    let link_at = |pos: Pos2| {
        let cursor = galley.cursor_from_pos(pos - rect.min);
        let i = cursor.index.0;
        links.iter().find(|(r, _)| r.contains(&i)).map(|(_, l)| *l)
    };
    if let Some(pos) = resp.hover_pos()
        && let Some(link) = link_at(pos)
    {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        let tip = match link {
            InlineLink::Wiki { target, .. } if project.resolve_title(target).is_none() => {
                format!("Créer la fiche « {target} »")
            }
            InlineLink::Wiki { target, .. } => format!("Ouvrir « {target} »"),
            InlineLink::Url(u) => u.clone(),
        };
        resp.clone().on_hover_text_at_pointer(tip);
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(link) = link_at(pos)
    {
        match link {
            InlineLink::Wiki { target, .. } => {
                return Some(ReadingAction::OpenTitle(target.clone()));
            }
            InlineLink::Url(u) => ui.ctx().open_url(egui::OpenUrl::new_tab(u)),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris() {
        let uri = file_uri(Path::new(if cfg!(windows) {
            r"C:\a b\img.svg"
        } else {
            "/a b/img.svg"
        }));
        if cfg!(windows) {
            assert_eq!(uri, "file:///C:/a b/img.svg");
        } else {
            assert_eq!(uri, "file:///a b/img.svg");
        }
    }
}
