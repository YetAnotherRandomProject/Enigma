//! Tableau d'enquête : cartes de fiches posées librement à la main, reliées
//! par des « fils » selon leurs liens. Les positions sont celles du
//! tableau (`.enigma/board.json`), indépendantes du graphe.

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    text::LayoutJob,
};
use enigma_core::board::Board;
use enigma_core::links::LinkEnd;
use enigma_core::{NoteId, Project, fuzzy};
use enigma_graph::{Camera2D, Vec2};

use crate::colors::c32;

/// Taille d'une carte (unités du tableau ≈ pixels au zoom 1).
pub const CARD_W: f32 = 200.0;
pub const CARD_H: f32 = 92.0;
/// Pas de la grille quand on ajoute plusieurs fiches d'un coup.
const GRID_X: f32 = CARD_W + 30.0;
const GRID_Y: f32 = CARD_H + 30.0;
const GRID_COLS: usize = 5;

#[derive(Default)]
pub struct BoardOutcome {
    pub select: Option<NoteId>,
    pub open_editor: Option<NoteId>,
    pub deselect: bool,
    /// Le tableau a changé et doit être enregistré.
    pub changed: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    /// Carte saisie, avec le décalage pointeur → coin de la carte.
    Card(NoteId, Vec2),
    Pan,
}

#[derive(Default)]
pub struct BoardView {
    camera: Camera2D,
    fitted: bool,
    drag: Option<Drag>,
    menu: Option<NoteId>,
    add_query: String,
    last_rect: Option<Rect>,
}

fn v(p: Pos2) -> Vec2 {
    Vec2::new(p.x, p.y)
}

fn p(v: Vec2) -> Pos2 {
    Pos2::new(v.x, v.y)
}

/// Fiches du tableau qui existent encore, dans l'ordre d'empilement.
fn visible_cards<'a>(board: &'a Board, project: &Project) -> Vec<&'a enigma_core::board::Card> {
    board
        .cards
        .iter()
        .filter(|c| project.note(&c.id).is_some())
        .collect()
}

impl BoardView {
    /// Point du tableau au centre de la vue (pour poser une nouvelle carte).
    fn view_center(&self) -> Vec2 {
        self.camera.center - Vec2::new(CARD_W, CARD_H) * 0.5
    }

    /// Ajoute une fiche au centre de la vue (décalée si la place est prise).
    pub fn add_card(&mut self, board: &mut Board, id: &NoteId) -> bool {
        let base = self.view_center();
        let mut pos = base;
        for k in 1..50 {
            let taken = board
                .cards
                .iter()
                .any(|c| (Vec2::new(c.x, c.y) - pos).length() < 12.0);
            if !taken {
                break;
            }
            pos = base + Vec2::new(24.0, 24.0) * k as f32;
        }
        board.add(id, pos.x, pos.y)
    }

    /// Ajoute plusieurs fiches en grille, sous les cartes existantes.
    pub fn add_many(&mut self, board: &mut Board, project: &Project, ids: &[NoteId]) -> usize {
        let cards = visible_cards(board, project);
        let (left, top) = if cards.is_empty() {
            let c = self.view_center();
            (c.x, c.y)
        } else {
            let left = cards.iter().map(|c| c.x).fold(f32::MAX, f32::min);
            let bottom = cards.iter().map(|c| c.y).fold(f32::MIN, f32::max);
            (left, bottom + GRID_Y)
        };
        let mut added = 0;
        for id in ids {
            if board.contains(id) {
                continue;
            }
            let col = added % GRID_COLS;
            let row = added / GRID_COLS;
            board.add(id, left + col as f32 * GRID_X, top + row as f32 * GRID_Y);
            added += 1;
        }
        if added > 0 {
            self.fitted = false;
        }
        added
    }

    pub fn recenter(&mut self) {
        self.fitted = false;
    }

    /// Position à l'écran du centre d'une carte (dernière image).
    #[cfg(test)]
    pub fn card_center(&self, board: &Board, id: &NoteId) -> Option<Pos2> {
        let rect = self.last_rect?;
        let c = board.card(id)?;
        let center = Vec2::new(c.x + CARD_W / 2.0, c.y + CARD_H / 2.0);
        Some(p(self.camera.to_screen(center, v(rect.center()))))
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        board: &mut Board,
        selected: Option<&NoteId>,
        list: &[NoteId],
    ) -> BoardOutcome {
        let mut out = BoardOutcome::default();
        self.toolbar(ui, project, board, list, &mut out);
        self.canvas(ui, project, board, selected, &mut out);
        out
    }

    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        board: &mut Board,
        list: &[NoteId],
        out: &mut BoardOutcome,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("Ajouter une fiche…", |ui| {
                ui.set_min_width(240.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.add_query)
                        .hint_text("Titre de la fiche")
                        .desired_width(240.0),
                );
                edit.request_focus();
                let mut matches: Vec<(i64, &NoteId, &str)> = project
                    .ids()
                    .filter(|id| !board.contains(id))
                    .filter_map(|id| {
                        let title = project.note(id)?.meta.title.as_str();
                        fuzzy::score(&self.add_query, title).map(|s| (s, id, title))
                    })
                    .collect();
                matches.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(b.2)));
                if matches.is_empty() {
                    ui.weak("Aucune fiche à ajouter");
                }
                let mut chosen = None;
                for (_, id, title) in matches.into_iter().take(10) {
                    if ui.button(title).clicked() {
                        chosen = Some(id.clone());
                    }
                }
                if let Some(id) = chosen {
                    if self.add_card(board, &id) {
                        out.changed = true;
                        out.select = Some(id);
                    }
                    self.add_query.clear();
                    ui.close();
                }
            });
            let missing = list.iter().filter(|id| !board.contains(id)).count();
            if missing > 0
                && ui
                    .button(format!("Ajouter les fiches de la liste ({missing})"))
                    .on_hover_text("Toutes les fiches visibles dans la liste (filtres compris)")
                    .clicked()
                && self.add_many(board, project, list) > 0
            {
                out.changed = true;
            }
            if ui.button("Recentrer").clicked() {
                self.recenter();
            }
            ui.weak(format!("{} carte(s)", visible_cards(board, project).len()));
        });
    }

    fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        board: &mut Board,
        selected: Option<&NoteId>,
        out: &mut BoardOutcome,
    ) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.last_rect = Some(rect);
        let vc = v(rect.center());
        let visuals = ui.visuals().clone();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);

        let cards: Vec<(NoteId, Vec2)> = visible_cards(board, project)
            .iter()
            .map(|c| (c.id.clone(), Vec2::new(c.x, c.y)))
            .collect();
        if !self.fitted {
            let pts = cards
                .iter()
                .flat_map(|(_, q)| [*q, *q + Vec2::new(CARD_W, CARD_H)]);
            if cards.is_empty() {
                self.camera = Camera2D::default();
                self.camera.center = Vec2::new(CARD_W, CARD_H) * 0.5;
            } else {
                self.camera.fit(pts, v(rect.size().to_pos2()), 40.0);
                self.camera.zoom = self.camera.zoom.min(1.0);
            }
            self.fitted = true;
        }
        let cam = self.camera;
        let to_s = |q: Vec2| p(cam.to_screen(q, vc));
        let card_rect = |q: Vec2| Rect::from_min_max(to_s(q), to_s(q + Vec2::new(CARD_W, CARD_H)));
        let pick = |s: Pos2| {
            cards
                .iter()
                .rev()
                .find(|(_, q)| card_rect(*q).contains(s))
                .map(|(id, q)| (id.clone(), *q))
        };
        let hovered = resp.hover_pos().and_then(pick);

        // --- Interactions ---
        if resp.drag_started()
            && let Some(start) = ui
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
        {
            let primary = ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary));
            self.drag = Some(match pick(start).filter(|_| primary) {
                Some((id, q)) => {
                    let grab = self.camera.to_world(v(start), vc) - q;
                    Drag::Card(id, grab)
                }
                None => Drag::Pan,
            });
        }
        if resp.dragged() {
            match (&self.drag, resp.interact_pointer_pos()) {
                (Some(Drag::Card(id, grab)), Some(ptr)) => {
                    let q = self.camera.to_world(v(ptr), vc) - *grab;
                    board.move_to(id, q.x, q.y);
                }
                _ => {
                    let d = resp.drag_delta();
                    self.camera.pan(Vec2::new(d.x, d.y));
                }
            }
            ui.ctx().request_repaint();
        }
        if resp.drag_stopped() {
            if let Some(Drag::Card(id, _)) = self.drag.take() {
                out.changed = true;
                out.select = Some(id);
            }
            self.drag = None;
        }
        if resp.hovered() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = pinch * (scroll * 0.002).exp();
            if (factor - 1.0).abs() > 1e-4
                && let Some(ptr) = resp.hover_pos()
            {
                self.camera.zoom_at(v(ptr), vc, factor);
            }
        }
        if resp.double_clicked() {
            if let Some((id, _)) = &hovered {
                out.open_editor = Some(id.clone());
            }
        } else if resp.clicked() {
            match &hovered {
                Some((id, _)) => out.select = Some(id.clone()),
                None => out.deselect = true,
            }
        }
        if resp.secondary_clicked() {
            self.menu = hovered.as_ref().map(|(id, _)| id.clone());
        }

        // Caméra et positions à jour (après déplacement, zoom ou glisser).
        let cam = self.camera;
        let zoom = cam.zoom;
        let to_s = |q: Vec2| p(cam.to_screen(q, vc));
        let card_rect = |q: Vec2| Rect::from_min_max(to_s(q), to_s(q + Vec2::new(CARD_W, CARD_H)));
        let cards: Vec<(NoteId, Vec2)> = visible_cards(board, project)
            .iter()
            .map(|c| (c.id.clone(), Vec2::new(c.x, c.y)))
            .collect();

        if cards.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Tableau vide : « Ajouter une fiche… », ou « Ajouter la fiche au tableau » \
                 dans la palette (Ctrl+P).",
                FontId::proportional(14.0),
                visuals.weak_text_color(),
            );
        }

        // --- Fils (liens entre cartes) ---
        let pack = project.pack();
        let center_of = |id: &NoteId| {
            cards
                .iter()
                .find(|(c, _)| c == id)
                .map(|(_, q)| to_s(*q + Vec2::new(CARD_W / 2.0, CARD_H / 2.0)))
        };
        for (id, _) in &cards {
            let Some(a) = center_of(id) else { continue };
            for link in project.outgoing_links(id) {
                let LinkEnd::Note(other) = &link.other else {
                    continue;
                };
                let Some(b) = center_of(other) else { continue };
                let color = link
                    .kind
                    .as_deref()
                    .and_then(|k| pack.link_type(k))
                    .map_or(Color32::from_rgb(0xc0, 0x39, 0x2b), |t| c32(t.color));
                let w = (2.0 * zoom).clamp(1.0, 3.0);
                painter.line_segment([a, b], Stroke::new(w, color.gamma_multiply(0.85)));
            }
        }

        // --- Cartes ---
        let radius = CornerRadius::same((6.0 * zoom).clamp(2.0, 8.0) as u8);
        for (id, q) in &cards {
            let r = card_rect(*q);
            if !r.intersects(rect) {
                continue;
            }
            let Some(note) = project.note(id) else {
                continue;
            };
            painter.rect_filled(r, radius, visuals.window_fill);
            // Bande des catégories.
            let band_h = (6.0 * zoom).max(2.0);
            let cats: Vec<Color32> = note
                .meta
                .categories
                .iter()
                .map(|c| pack.category(c).map_or(Color32::GRAY, |d| c32(d.color)))
                .collect();
            if !cats.is_empty() {
                let w = r.width() / cats.len() as f32;
                for (k, c) in cats.iter().enumerate() {
                    let band = Rect::from_min_size(
                        Pos2::new(r.left() + w * k as f32, r.top()),
                        egui::vec2(w, band_h),
                    );
                    painter.rect_filled(band, 0.0, *c);
                }
            }
            let status = note
                .meta
                .status
                .as_deref()
                .and_then(|s| pack.status(s))
                .map(|s| c32(s.color));
            let border = if selected == Some(id) {
                Stroke::new(2.5, visuals.selection.stroke.color)
            } else if hovered.as_ref().is_some_and(|(h, _)| h == id) {
                Stroke::new(1.5, visuals.strong_text_color())
            } else {
                Stroke::new(
                    1.2,
                    status.unwrap_or(visuals.widgets.noninteractive.bg_stroke.color),
                )
            };
            painter.rect_stroke(r, radius, border, StrokeKind::Inside);
            // « Punaise ».
            painter.circle_filled(
                Pos2::new(r.center().x, r.top() + band_h + 4.0 * zoom),
                (3.5 * zoom).clamp(1.5, 5.0),
                Color32::from_rgb(0xc0, 0x39, 0x2b),
            );
            if zoom < 0.3 {
                continue;
            }
            let pad = 8.0 * zoom;
            let inner_w = (r.width() - 2.0 * pad).max(10.0);
            let mut title = LayoutJob::simple(
                note.meta.title.clone(),
                FontId::proportional(14.0 * zoom),
                visuals.strong_text_color(),
                inner_w,
            );
            title.wrap.max_rows = 1;
            title.wrap.overflow_character = Some('…');
            let title_g = painter.layout_job(title);
            let title_pos = Pos2::new(r.left() + pad, r.top() + band_h + 10.0 * zoom);
            let title_h = title_g.size().y;
            painter.galley(title_pos, title_g, visuals.text_color());
            let excerpt = plain_excerpt(&note.body);
            if !excerpt.is_empty() {
                let mut job = LayoutJob::simple(
                    excerpt,
                    FontId::proportional(11.0 * zoom),
                    visuals.weak_text_color(),
                    inner_w,
                );
                job.wrap.max_rows = 3;
                job.wrap.overflow_character = Some('…');
                let g = painter.layout_job(job);
                painter.galley(
                    Pos2::new(r.left() + pad, title_pos.y + title_h + 4.0 * zoom),
                    g,
                    visuals.weak_text_color(),
                );
            }
        }

        // --- Menu contextuel ---
        let menu = self.menu.clone();
        resp.context_menu(|ui| {
            let Some(id) = &menu else {
                ui.weak("Clic droit sur une carte");
                return;
            };
            if ui.button("Ouvrir dans l'éditeur").clicked() {
                out.open_editor = Some(id.clone());
                ui.close();
            }
            if ui.button("Retirer du tableau").clicked() {
                if board.remove(id) {
                    out.changed = true;
                }
                ui.close();
            }
        });

        if self.drag.is_none()
            && let Some((id, _)) = &hovered
            && let Some(note) = project.note(id)
        {
            let title = note.meta.title.clone();
            resp.on_hover_ui_at_pointer(|ui| {
                ui.label(RichText::new(title).strong());
                ui.label(
                    RichText::new(
                        "Glisser : déplacer · clic : sélectionner · double-clic : ouvrir · clic droit : retirer",
                    )
                    .small()
                    .weak(),
                );
            });
        }
    }
}

/// Début du texte sans la syntaxe Markdown (titres sautés, `[[Titre|type]]`
/// réduit à `Titre`, emphase et code retirés).
pub fn plain_excerpt(body: &str) -> String {
    let lines: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .take(4)
        .collect();
    let text = lines.join(" ");
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for link in enigma_core::markdown::wiki_links(&text) {
        out.push_str(&text[last..link.range.start]);
        out.push_str(&link.target);
        last = link.range.end;
    }
    out.push_str(&text[last..]);
    out.chars()
        .filter(|c| !matches!(c, '*' | '`' | '>'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> NoteId {
        NoteId::parse(s).unwrap()
    }

    #[test]
    fn excerpt_without_markdown() {
        let body = "# Titre

Un *carnet* cite le [[Coffre du bureau|ouvre]] et `1945`.
> note";
        assert_eq!(
            plain_excerpt(body),
            "Un carnet cite le Coffre du bureau et 1945.  note"
        );
    }

    #[test]
    fn add_card_avoids_exact_overlap() {
        let mut view = BoardView::default();
        let mut board = Board::default();
        assert!(view.add_card(&mut board, &id("A")));
        assert!(view.add_card(&mut board, &id("B")));
        assert!(!view.add_card(&mut board, &id("A")));
        let a = board.card(&id("A")).unwrap();
        let b = board.card(&id("B")).unwrap();
        assert!((a.x - b.x).abs() > 10.0);
    }
}
