//! Tableau d'enquête : cartes de fiches posées librement à la main, reliées
//! par des « fils » selon leurs liens, cartes de texte libre, images,
//! groupes (cadres titrés qui emmènent leur contenu) et flèches étiquetées
//! tirées à la main. Les positions sont celles du tableau
//! (`.enigma/board.json`), indépendantes du graphe.

use std::collections::HashMap;

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind,
    text::LayoutJob,
};
use enigma_core::board::{Board, NOTE_H, NOTE_W};
use enigma_core::links::LinkEnd;
use enigma_core::{NoteId, Project, fuzzy};
use enigma_graph::{Camera2D, Vec2};

use crate::colors::c32;

/// Taille d'une carte (unités du tableau ≈ pixels au zoom 1).
pub const CARD_W: f32 = NOTE_W;
pub const CARD_H: f32 = NOTE_H;
/// Pas de la grille quand on ajoute plusieurs fiches d'un coup.
const GRID_X: f32 = CARD_W + 30.0;
const GRID_Y: f32 = CARD_H + 30.0;
const GRID_COLS: usize = 5;
/// Poignées (redimensionner, relier), en pixels d'écran.
const HANDLE: f32 = 12.0;
/// Couleurs proposées (texte, groupes, flèches).
const SWATCHES: [(&str, &str); 6] = [
    ("Rouge", "#c0392b"),
    ("Orange", "#e67e22"),
    ("Jaune", "#d4ac0d"),
    ("Vert", "#27ae60"),
    ("Bleu", "#2980b9"),
    ("Violet", "#8e44ad"),
];

#[derive(Default)]
pub struct BoardOutcome {
    pub select: Option<NoteId>,
    pub open_editor: Option<NoteId>,
    pub deselect: bool,
    /// Le tableau a changé et doit être enregistré.
    pub changed: bool,
    /// Choisir une image à poser sur le tableau.
    pub add_image: bool,
}

/// Élément sous le pointeur.
#[derive(Clone, Debug, PartialEq)]
enum Hit {
    Note(NoteId),
    Text(String),
    Image(String),
    Group(String),
    Edge(String),
}

impl Hit {
    /// Clé de l'élément dans le tableau.
    fn key(&self) -> String {
        match self {
            Hit::Note(id) => id.as_str().to_owned(),
            Hit::Text(k) | Hit::Image(k) | Hit::Group(k) | Hit::Edge(k) => k.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    /// Élément saisi (clé), avec le décalage pointeur → coin.
    Item(String, Vec2),
    Pan,
    /// Redimensionnement : clé, taille de départ.
    Resize(String, Vec2),
    /// Flèche en cours depuis cet élément.
    Connect(String),
}

/// Modification en cours dans une petite zone de saisie.
#[derive(Clone, Debug, PartialEq)]
enum Editing {
    Text(String),
    GroupLabel(String),
    EdgeLabel(String),
}

#[derive(Default)]
pub struct BoardView {
    camera: Camera2D,
    fitted: bool,
    drag: Option<Drag>,
    menu: Option<Option<Hit>>,
    /// Position (tableau) du dernier clic droit.
    menu_at: Vec2,
    add_query: String,
    last_rect: Option<Rect>,
    /// Élément sélectionné autre qu'une fiche.
    selected: Option<String>,
    editing: Option<Editing>,
    edit_buf: String,
    edit_focus: bool,
    /// Proportions (hauteur / largeur) des images chargées.
    aspects: HashMap<String, f32>,
}

fn v(p: Pos2) -> Vec2 {
    Vec2::new(p.x, p.y)
}

fn p(v: Vec2) -> Pos2 {
    Pos2::new(v.x, v.y)
}

fn hex(c: Option<&str>) -> Option<Color32> {
    let c = c?.strip_prefix('#')?;
    let n = u32::from_str_radix(c, 16).ok()?;
    Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// Fiches du tableau qui existent encore, dans l'ordre d'empilement.
fn visible_cards<'a>(board: &'a Board, project: &Project) -> Vec<&'a enigma_core::board::Card> {
    board
        .cards
        .iter()
        .filter(|c| project.note(&c.id).is_some())
        .collect()
}

/// Point du bord de `r` sur le segment centre → `toward`.
fn border_point(r: Rect, toward: Pos2) -> Pos2 {
    let c = r.center();
    let d = toward - c;
    if d.length() < 1e-3 {
        return c;
    }
    let sx = if d.x.abs() > 1e-3 {
        (r.width() / 2.0) / d.x.abs()
    } else {
        f32::INFINITY
    };
    let sy = if d.y.abs() > 1e-3 {
        (r.height() / 2.0) / d.y.abs()
    } else {
        f32::INFINITY
    };
    c + d * sx.min(sy).min(1.0)
}

fn dist_to_segment(pt: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((pt - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - pt).length()
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

    /// Pose une image (chemin relatif au projet) au centre de la vue.
    pub fn add_image(&mut self, board: &mut Board, src: &str) {
        let c = self.view_center();
        let key = board.add_image(c.x, c.y, src);
        self.selected = Some(key);
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

    /// Position à l'écran du centre d'un élément (dernière image).
    #[cfg(test)]
    pub fn item_center(&self, board: &Board, key: &str) -> Option<Pos2> {
        let rect = self.last_rect?;
        let r = self.world_rect(board, key)?;
        let center = Vec2::new(r.x + r.w / 2.0, r.y + r.h / 2.0);
        Some(p(self.camera.to_screen(center, v(rect.center()))))
    }

    /// Point de départ des flèches d'un élément, à l'écran (tests).
    #[cfg(test)]
    pub fn connector(&self, board: &Board, key: &str) -> Option<Pos2> {
        let rect = self.last_rect?;
        self.handles(board, key, v(rect.center()))
            .map(|(_, c)| c.center())
    }

    /// Position à l'écran du centre d'une carte (dernière image).
    #[cfg(test)]
    pub fn card_center(&self, board: &Board, id: &NoteId) -> Option<Pos2> {
        self.item_center(board, id.as_str())
    }

    /// Rectangle (tableau) d'un élément ; hauteur des images selon leurs
    /// proportions.
    fn world_rect(&self, board: &Board, key: &str) -> Option<enigma_core::board::ItemRect> {
        let mut r = board.rect_of(key)?;
        if let Some(img) = board.images.iter().find(|i| i.key == key) {
            r.h = img.w * self.aspects.get(&img.src).copied().unwrap_or(0.75);
        }
        Some(r)
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
            ui.separator();
            if ui
                .button("+ Texte")
                .on_hover_text("Carte de texte libre (ou double-clic sur le fond)")
                .clicked()
            {
                let c = self.view_center();
                let key = board.add_text(c.x, c.y, "");
                self.start_edit(Editing::Text(key.clone()), "");
                self.selected = Some(key);
                out.changed = true;
            }
            if ui.button("+ Image…").clicked() {
                out.add_image = true;
            }
            if ui
                .button("+ Groupe")
                .on_hover_text("Cadre titré : les éléments posés dedans bougent avec lui")
                .clicked()
            {
                let c = self.view_center();
                let key = board.add_group(c.x - 40.0, c.y - 40.0, 520.0, 320.0, "Groupe");
                self.start_edit(Editing::GroupLabel(key.clone()), "Groupe");
                self.selected = Some(key);
                out.changed = true;
            }
            ui.separator();
            if ui.button("Recentrer").clicked() {
                self.recenter();
            }
            ui.weak(format!("{} carte(s)", visible_cards(board, project).len()));
        });
    }

    fn start_edit(&mut self, what: Editing, text: &str) {
        self.editing = Some(what);
        self.edit_buf = text.to_owned();
        self.edit_focus = true;
    }

    /// Élément sous un point de l'écran : textes, images, cartes (du dessus
    /// vers le dessous), flèches, puis groupes.
    fn pick(&self, board: &Board, project: &Project, screen: Pos2, vc: Vec2) -> Option<Hit> {
        let cam = self.camera;
        let srect = |key: &str| {
            self.world_rect(board, key).map(|r| {
                Rect::from_min_max(
                    p(cam.to_screen(Vec2::new(r.x, r.y), vc)),
                    p(cam.to_screen(Vec2::new(r.x + r.w, r.y + r.h), vc)),
                )
            })
        };
        let inside = |key: &str| srect(key).is_some_and(|r| r.contains(screen));
        if let Some(t) = board.texts.iter().rev().find(|t| inside(&t.key)) {
            return Some(Hit::Text(t.key.clone()));
        }
        if let Some(i) = board.images.iter().rev().find(|i| inside(&i.key)) {
            return Some(Hit::Image(i.key.clone()));
        }
        if let Some(c) = visible_cards(board, project)
            .iter()
            .rev()
            .find(|c| inside(c.id.as_str()))
        {
            return Some(Hit::Note(c.id.clone()));
        }
        for e in &board.edges {
            if let (Some(a), Some(b)) = (srect(&e.from), srect(&e.to)) {
                let (pa, pb) = (border_point(a, b.center()), border_point(b, a.center()));
                if dist_to_segment(screen, pa, pb) < 6.0 {
                    return Some(Hit::Edge(e.key.clone()));
                }
            }
        }
        board
            .groups
            .iter()
            .rev()
            .find(|g| inside(&g.key))
            .map(|g| Hit::Group(g.key.clone()))
    }

    /// Poignées d'un élément à l'écran : (redimensionner, relier).
    fn handles(&self, board: &Board, key: &str, vc: Vec2) -> Option<(Rect, Rect)> {
        let r = self.world_rect(board, key)?;
        let cam = self.camera;
        let s = Rect::from_min_max(
            p(cam.to_screen(Vec2::new(r.x, r.y), vc)),
            p(cam.to_screen(Vec2::new(r.x + r.w, r.y + r.h), vc)),
        );
        let resize = Rect::from_center_size(s.right_bottom(), egui::Vec2::splat(HANDLE));
        let connect = Rect::from_center_size(
            Pos2::new(s.right() + HANDLE * 0.9, s.center().y),
            egui::Vec2::splat(HANDLE),
        );
        Some((resize, connect))
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
        let accent = crate::theme::accent(&visuals);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);

        if !self.fitted {
            let keys = board.keys();
            let pts: Vec<Vec2> = keys
                .iter()
                .filter(|k| {
                    NoteId::parse(k)
                        .is_none_or(|id| !board.contains(&id) || project.note(&id).is_some())
                })
                .filter_map(|k| self.world_rect(board, k))
                .flat_map(|r| [Vec2::new(r.x, r.y), Vec2::new(r.x + r.w, r.y + r.h)])
                .collect();
            if pts.is_empty() {
                self.camera = Camera2D::default();
                self.camera.center = Vec2::new(CARD_W, CARD_H) * 0.5;
            } else {
                self.camera.fit(pts, v(rect.size().to_pos2()), 40.0);
                self.camera.zoom = self.camera.zoom.min(1.0);
            }
            self.fitted = true;
        }

        // Poignées de l'élément survolé ou sélectionné.
        let hovered = resp
            .hover_pos()
            .and_then(|s| self.pick(board, project, s, vc));
        let focus_key = hovered
            .as_ref()
            .filter(|h| !matches!(h, Hit::Edge(_)))
            .map(Hit::key)
            .or_else(|| self.selected.clone());

        // --- Interactions ---
        if resp.drag_started()
            && let Some(start) = ui
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
        {
            let primary = ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary));
            let mut drag = Drag::Pan;
            if primary {
                // Poignées de l'élément courant d'abord.
                let around = self
                    .selected
                    .clone()
                    .into_iter()
                    .chain(focus_key.clone())
                    .chain(selected.map(|s| s.as_str().to_owned()));
                for key in around {
                    if let Some((resize, connect)) = self.handles(board, &key, vc) {
                        let resizable = board.texts.iter().any(|t| t.key == key)
                            || board.images.iter().any(|i| i.key == key)
                            || board.groups.iter().any(|g| g.key == key);
                        if resizable && resize.expand(3.0).contains(start) {
                            let r = self.world_rect(board, &key);
                            drag = Drag::Resize(key, r.map_or(Vec2::ZERO, |r| Vec2::new(r.w, r.h)));
                            break;
                        }
                        if connect.expand(3.0).contains(start) {
                            drag = Drag::Connect(key);
                            break;
                        }
                    }
                }
                if drag == Drag::Pan
                    && let Some(hit) = self.pick(board, project, start, vc)
                    && !matches!(hit, Hit::Edge(_))
                {
                    let key = hit.key();
                    if let Some(r) = self.world_rect(board, &key) {
                        let grab = self.camera.to_world(v(start), vc) - Vec2::new(r.x, r.y);
                        drag = Drag::Item(key, grab);
                    }
                }
            }
            self.drag = Some(drag);
        }
        if resp.dragged() {
            let ptr = resp.interact_pointer_pos();
            match (&self.drag, ptr) {
                (Some(Drag::Item(key, grab)), Some(ptr)) => {
                    let q = self.camera.to_world(v(ptr), vc) - *grab;
                    match NoteId::parse(key).filter(|id| board.contains(id)) {
                        Some(id) => board.move_to(&id, q.x, q.y),
                        None => board.move_item(key, q.x, q.y),
                    }
                }
                (Some(Drag::Resize(key, _)), Some(ptr)) => {
                    if let Some(r) = self.world_rect(board, key) {
                        let q = self.camera.to_world(v(ptr), vc);
                        board.resize_item(key, q.x - r.x, q.y - r.y);
                    }
                }
                (Some(Drag::Connect(_)), _) => {}
                _ => {
                    let d = resp.drag_delta();
                    self.camera.pan(Vec2::new(d.x, d.y));
                }
            }
            ui.ctx().request_repaint();
        }
        if resp.drag_stopped() {
            match self.drag.take() {
                Some(Drag::Item(key, _)) => {
                    out.changed = true;
                    match NoteId::parse(&key).filter(|id| board.contains(id)) {
                        Some(id) => out.select = Some(id),
                        None => self.selected = Some(key),
                    }
                }
                Some(Drag::Resize(..)) => out.changed = true,
                Some(Drag::Connect(from)) => {
                    let target = resp
                        .interact_pointer_pos()
                        .and_then(|s| self.pick(board, project, s, vc))
                        .filter(|h| !matches!(h, Hit::Edge(_)));
                    if let Some(to) = target
                        && let Some(key) = board.add_edge(&from, &to.key())
                    {
                        out.changed = true;
                        self.selected = Some(key.clone());
                        self.start_edit(Editing::EdgeLabel(key), "");
                    }
                }
                _ => {}
            }
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
            match &hovered {
                Some(Hit::Note(id)) => out.open_editor = Some(id.clone()),
                Some(Hit::Text(k)) => {
                    let text = board
                        .texts
                        .iter()
                        .find(|t| t.key == *k)
                        .map(|t| t.text.clone())
                        .unwrap_or_default();
                    self.start_edit(Editing::Text(k.clone()), &text);
                }
                Some(Hit::Group(k)) => {
                    let label = board
                        .groups
                        .iter()
                        .find(|g| g.key == *k)
                        .map(|g| g.label.clone())
                        .unwrap_or_default();
                    self.start_edit(Editing::GroupLabel(k.clone()), &label);
                }
                Some(Hit::Edge(k)) => {
                    let label = board
                        .edges
                        .iter()
                        .find(|e| e.key == *k)
                        .map(|e| e.label.clone())
                        .unwrap_or_default();
                    self.start_edit(Editing::EdgeLabel(k.clone()), &label);
                }
                Some(Hit::Image(_)) => {}
                None => {
                    if let Some(ptr) = resp.hover_pos() {
                        let q = self.camera.to_world(v(ptr), vc);
                        let key = board.add_text(q.x - 110.0, q.y - 20.0, "");
                        self.start_edit(Editing::Text(key.clone()), "");
                        self.selected = Some(key);
                        out.changed = true;
                    }
                }
            }
        } else if resp.clicked() {
            match &hovered {
                Some(Hit::Note(id)) => {
                    self.selected = None;
                    out.select = Some(id.clone());
                }
                Some(h) => self.selected = Some(h.key()),
                None => {
                    self.selected = None;
                    out.deselect = true;
                }
            }
        }
        if resp.secondary_clicked() {
            self.menu = Some(hovered.clone());
            if let Some(ptr) = resp.interact_pointer_pos() {
                self.menu_at = self.camera.to_world(v(ptr), vc);
            }
        }
        // Suppr : retirer l'élément sélectionné (hors fiches).
        if self.editing.is_none()
            && let Some(key) = self.selected.clone()
            && ui.input(|i| i.key_pressed(egui::Key::Delete))
            && board.remove_item(&key)
        {
            self.selected = None;
            out.changed = true;
        }

        // --- Dessin ---
        let cam = self.camera;
        let zoom = cam.zoom;
        let to_s = |q: Vec2| p(cam.to_screen(q, vc));
        let srect = |key: &str| {
            self.world_rect(board, key).map(|r| {
                Rect::from_min_max(
                    to_s(Vec2::new(r.x, r.y)),
                    to_s(Vec2::new(r.x + r.w, r.y + r.h)),
                )
            })
        };
        let cards: Vec<(NoteId, Vec2)> = visible_cards(board, project)
            .iter()
            .map(|c| (c.id.clone(), Vec2::new(c.x, c.y)))
            .collect();
        let empty = cards.is_empty()
            && board.texts.is_empty()
            && board.images.is_empty()
            && board.groups.is_empty();
        if empty {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Tableau vide : « Ajouter une fiche… », « + Texte », « + Image… », « + Groupe », \
                 ou double-clic sur le fond pour écrire.",
                FontId::proportional(14.0),
                visuals.weak_text_color(),
            );
        }
        let is_sel = |key: &str| self.selected.as_deref() == Some(key);

        // Groupes (derrière tout).
        for g in &board.groups {
            let Some(r) = srect(&g.key) else { continue };
            let color = hex(g.color.as_deref()).unwrap_or(visuals.weak_text_color());
            painter.rect_filled(r, 0.0, color.gamma_multiply(0.08));
            let stroke = if is_sel(&g.key) {
                Stroke::new(2.0, accent)
            } else {
                Stroke::new(1.0, color.gamma_multiply(0.7))
            };
            painter.rect_stroke(r, 0.0, stroke, StrokeKind::Inside);
            if self.editing != Some(Editing::GroupLabel(g.key.clone())) {
                painter.text(
                    r.left_top() + egui::vec2(8.0, 6.0) * zoom.max(0.5),
                    egui::Align2::LEFT_TOP,
                    &g.label,
                    FontId::proportional((15.0 * zoom).clamp(8.0, 28.0)),
                    color,
                );
            }
        }

        // Fils (liens entre fiches).
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

        // Flèches tirées à la main.
        for e in &board.edges {
            let (Some(a), Some(b)) = (srect(&e.from), srect(&e.to)) else {
                continue;
            };
            let color = if is_sel(&e.key) {
                accent
            } else {
                hex(e.color.as_deref()).unwrap_or(visuals.strong_text_color())
            };
            let (pa, pb) = (border_point(a, b.center()), border_point(b, a.center()));
            let w = (2.0 * zoom).clamp(1.2, 3.0);
            painter.line_segment([pa, pb], Stroke::new(w, color));
            let dir = (pb - pa).normalized();
            let size = (10.0 * zoom).clamp(6.0, 14.0);
            let n = egui::vec2(-dir.y, dir.x);
            painter.add(Shape::convex_polygon(
                vec![
                    pb,
                    pb - dir * size + n * size * 0.5,
                    pb - dir * size - n * size * 0.5,
                ],
                color,
                Stroke::NONE,
            ));
            if !e.label.is_empty() && self.editing != Some(Editing::EdgeLabel(e.key.clone())) {
                let mid = pa + (pb - pa) * 0.5;
                let g = painter.layout_no_wrap(
                    e.label.clone(),
                    FontId::proportional((12.0 * zoom).clamp(8.0, 18.0)),
                    color,
                );
                let r = Rect::from_center_size(mid, g.size()).expand(3.0);
                painter.rect_filled(r, 0.0, visuals.extreme_bg_color);
                painter.rect_stroke(r, 0.0, Stroke::new(1.0, color), StrokeKind::Inside);
                painter.galley(r.min + egui::vec2(3.0, 3.0), g, color);
            }
        }
        if let Some(Drag::Connect(from)) = &self.drag
            && let (Some(a), Some(ptr)) = (srect(from), resp.interact_pointer_pos())
        {
            painter.line_segment([border_point(a, ptr), ptr], Stroke::new(2.0, accent));
        }

        // Cartes de fiches.
        let radius = CornerRadius::ZERO;
        for (id, q) in &cards {
            let r = Rect::from_min_max(to_s(*q), to_s(*q + Vec2::new(CARD_W, CARD_H)));
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
                Stroke::new(2.5, accent)
            } else if hovered.as_ref() == Some(&Hit::Note(id.clone())) {
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

        // Images.
        let mut new_aspects = Vec::new();
        for img in &board.images {
            let Some(r) = srect(&img.key) else { continue };
            if !r.intersects(rect) {
                continue;
            }
            let path = project.root().join(&img.src);
            let uri = crate::reading::file_uri(&path);
            let image = egui::Image::new(uri);
            match image.load_for_size(ui.ctx(), r.size()) {
                Ok(poll) => {
                    if let Some(size) = poll.size().filter(|s| s.x > 0.0) {
                        let aspect = size.y / size.x;
                        if self.aspects.get(&img.src) != Some(&aspect) {
                            new_aspects.push((img.src.clone(), aspect));
                            ui.ctx().request_repaint();
                        }
                    }
                    image.paint_at(ui, r);
                }
                Err(_) => {
                    painter.rect_stroke(
                        r,
                        0.0,
                        Stroke::new(1.0, visuals.error_fg_color),
                        StrokeKind::Inside,
                    );
                    painter.text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("Image introuvable : {}", img.src),
                        FontId::proportional(11.0),
                        visuals.error_fg_color,
                    );
                }
            }
            if is_sel(&img.key) {
                painter.rect_stroke(r, 0.0, Stroke::new(2.0, accent), StrokeKind::Outside);
            }
        }

        // Cartes de texte.
        for t in &board.texts {
            let Some(r) = srect(&t.key) else { continue };
            if !r.intersects(rect) {
                continue;
            }
            let color = hex(t.color.as_deref());
            painter.rect_filled(
                r,
                0.0,
                color.map_or(visuals.window_fill, |c| c.gamma_multiply(0.25)),
            );
            let stroke = if is_sel(&t.key) {
                Stroke::new(2.0, accent)
            } else {
                Stroke::new(
                    1.0,
                    color.unwrap_or(visuals.widgets.noninteractive.bg_stroke.color),
                )
            };
            painter.rect_stroke(r, 0.0, stroke, StrokeKind::Inside);
            if self.editing == Some(Editing::Text(t.key.clone())) || zoom < 0.25 {
                continue;
            }
            let pad = 8.0 * zoom;
            let mut job = LayoutJob::simple(
                if t.text.is_empty() {
                    "(double-clic pour écrire)".to_owned()
                } else {
                    t.text.clone()
                },
                FontId::proportional(13.0 * zoom),
                if t.text.is_empty() {
                    visuals.weak_text_color()
                } else {
                    visuals.text_color()
                },
                (r.width() - 2.0 * pad).max(10.0),
            );
            job.wrap.max_rows = ((r.height() - 2.0 * pad) / (16.0 * zoom)).max(1.0) as usize;
            job.wrap.overflow_character = Some('…');
            let g = painter.layout_job(job);
            painter.galley(r.min + egui::vec2(pad, pad), g, visuals.text_color());
        }

        // Poignées : redimensionner (coin), relier (point à droite).
        if self.drag.is_none()
            && let Some(key) = &focus_key
            && let Some((resize, connect)) = self.handles(board, key, vc)
        {
            let resizable = board.texts.iter().any(|t| t.key == *key)
                || board.images.iter().any(|i| i.key == *key)
                || board.groups.iter().any(|g| g.key == *key);
            if resizable {
                painter.rect_filled(resize, 0.0, accent);
                if resp
                    .hover_pos()
                    .is_some_and(|h| resize.expand(3.0).contains(h))
                {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                }
            }
            painter.circle_filled(connect.center(), HANDLE * 0.4, accent);
            painter.circle_stroke(
                connect.center(),
                HANDLE * 0.4,
                Stroke::new(1.0, visuals.extreme_bg_color),
            );
            if resp
                .hover_pos()
                .is_some_and(|h| connect.expand(3.0).contains(h))
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                resp.clone().on_hover_text_at_pointer(
                    "Glisser vers un autre élément pour tirer une flèche",
                );
            }
        }

        // --- Zone de saisie (texte, nom de groupe, étiquette de flèche) ---
        let rects: HashMap<String, Rect> = board
            .keys()
            .into_iter()
            .filter_map(|k| srect(&k).map(|r| (k, r)))
            .collect();
        self.aspects.extend(new_aspects);
        self.edit_overlay(ui, board, &rects, out);

        // --- Menu contextuel ---
        let menu = self.menu.clone().flatten();
        let at = self.menu_at;
        let mut start_edit = None;
        resp.context_menu(|ui| match &menu {
            Some(Hit::Note(id)) => {
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
            }
            Some(Hit::Text(k)) => {
                if ui.button("Modifier le texte").clicked() {
                    let text = board
                        .texts
                        .iter()
                        .find(|t| t.key == *k)
                        .map(|t| t.text.clone())
                        .unwrap_or_default();
                    start_edit = Some((Editing::Text(k.clone()), text));
                    ui.close();
                }
                if let Some(c) = color_menu(ui) {
                    if let Some(t) = board.texts.iter_mut().find(|t| t.key == *k) {
                        t.color = c;
                    }
                    out.changed = true;
                }
                if ui.button("Supprimer").clicked() {
                    out.changed |= board.remove_item(k);
                    ui.close();
                }
            }
            Some(Hit::Image(k)) => {
                if ui.button("Ouvrir l'image").clicked() {
                    if let Some(img) = board.images.iter().find(|i| i.key == *k) {
                        let path = project.root().join(&img.src);
                        if let Err(e) = crate::system_open::open(&path) {
                            tracing::warn!("ouverture de l'image : {e}");
                        }
                    }
                    ui.close();
                }
                if ui.button("Supprimer du tableau").clicked() {
                    out.changed |= board.remove_item(k);
                    ui.close();
                }
            }
            Some(Hit::Group(k)) => {
                if ui.button("Renommer").clicked() {
                    let label = board
                        .groups
                        .iter()
                        .find(|g| g.key == *k)
                        .map(|g| g.label.clone())
                        .unwrap_or_default();
                    start_edit = Some((Editing::GroupLabel(k.clone()), label));
                    ui.close();
                }
                if let Some(c) = color_menu(ui) {
                    if let Some(g) = board.groups.iter_mut().find(|g| g.key == *k) {
                        g.color = c;
                    }
                    out.changed = true;
                }
                if ui
                    .button("Supprimer le groupe")
                    .on_hover_text("Le contenu reste sur le tableau")
                    .clicked()
                {
                    out.changed |= board.remove_item(k);
                    ui.close();
                }
            }
            Some(Hit::Edge(k)) => {
                if ui.button("Étiquette…").clicked() {
                    let label = board
                        .edges
                        .iter()
                        .find(|e| e.key == *k)
                        .map(|e| e.label.clone())
                        .unwrap_or_default();
                    start_edit = Some((Editing::EdgeLabel(k.clone()), label));
                    ui.close();
                }
                if let Some(c) = color_menu(ui) {
                    if let Some(e) = board.edges.iter_mut().find(|e| e.key == *k) {
                        e.color = c;
                    }
                    out.changed = true;
                }
                if ui.button("Inverser le sens").clicked() {
                    if let Some(e) = board.edges.iter_mut().find(|e| e.key == *k) {
                        std::mem::swap(&mut e.from, &mut e.to);
                    }
                    out.changed = true;
                    ui.close();
                }
                if ui.button("Supprimer la flèche").clicked() {
                    out.changed |= board.remove_item(k);
                    ui.close();
                }
            }
            None => {
                if ui.button("+ Texte ici").clicked() {
                    let key = board.add_text(at.x, at.y, "");
                    start_edit = Some((Editing::Text(key), String::new()));
                    out.changed = true;
                    ui.close();
                }
                if ui.button("+ Groupe ici").clicked() {
                    let key = board.add_group(at.x, at.y, 520.0, 320.0, "Groupe");
                    start_edit = Some((Editing::GroupLabel(key), "Groupe".to_owned()));
                    out.changed = true;
                    ui.close();
                }
                if ui.button("+ Image…").clicked() {
                    out.add_image = true;
                    ui.close();
                }
            }
        });
        if let Some((what, text)) = start_edit {
            self.start_edit(what, &text);
        }

        if self.drag.is_none()
            && self.editing.is_none()
            && let Some(hit) = &hovered
        {
            let tip = match hit {
                Hit::Note(id) => project.note(id).map(|n| n.meta.title.clone()).map(|t| {
                    (
                        t,
                        "Glisser : déplacer · double-clic : ouvrir · point à droite : tirer une flèche",
                    )
                }),
                Hit::Text(_) => Some((
                    "Texte".to_owned(),
                    "Double-clic : modifier · coin : redimensionner · Suppr : supprimer",
                )),
                Hit::Image(_) => Some((
                    "Image".to_owned(),
                    "Coin : redimensionner · clic droit : ouvrir ou supprimer",
                )),
                Hit::Group(_) => Some((
                    "Groupe".to_owned(),
                    "Glisser : déplacer avec son contenu · double-clic : renommer",
                )),
                Hit::Edge(_) => Some((
                    "Flèche".to_owned(),
                    "Double-clic : étiquette · clic droit : couleur, sens, supprimer",
                )),
            };
            if let Some((title, help)) = tip {
                resp.on_hover_ui_at_pointer(|ui| {
                    ui.label(RichText::new(title).strong());
                    ui.label(RichText::new(help).small().weak());
                });
            }
        }
    }

    /// Zone de saisie posée sur l'élément modifié ; Entrée (hors texte),
    /// Échap ou un clic ailleurs terminent.
    fn edit_overlay(
        &mut self,
        ui: &mut egui::Ui,
        board: &mut Board,
        rects: &HashMap<String, Rect>,
        out: &mut BoardOutcome,
    ) {
        let Some(editing) = self.editing.clone() else {
            return;
        };
        let srect = |k: &str| rects.get(k).copied();
        let multiline = matches!(editing, Editing::Text(_));
        let area = match &editing {
            Editing::Text(k) => srect(k),
            Editing::GroupLabel(k) => {
                srect(k).map(|r| Rect::from_min_size(r.min, egui::vec2(r.width().min(260.0), 26.0)))
            }
            Editing::EdgeLabel(k) => board
                .edges
                .iter()
                .find(|e| e.key == *k)
                .and_then(|e| Some((srect(&e.from)?, srect(&e.to)?)))
                .map(|(a, b)| {
                    let mid = a.center() + (b.center() - a.center()) * 0.5;
                    Rect::from_center_size(mid, egui::vec2(160.0, 24.0))
                }),
        };
        let Some(area) = area else {
            self.editing = None;
            return;
        };
        let mut done = false;
        egui::Area::new(egui::Id::unique("board_edit"))
            .order(egui::Order::Foreground)
            .fixed_pos(area.min)
            .show(ui.ctx(), |ui| {
                ui.set_width(area.width());
                let te = if multiline {
                    egui::TextEdit::multiline(&mut self.edit_buf)
                        .desired_width(area.width())
                        .desired_rows(((area.height() / 18.0) as usize).max(2))
                        .hint_text("Texte libre…")
                } else {
                    egui::TextEdit::singleline(&mut self.edit_buf)
                        .desired_width(area.width())
                        .hint_text(match editing {
                            Editing::EdgeLabel(_) => "Étiquette de la flèche",
                            _ => "Nom du groupe",
                        })
                };
                // Champ propre à l'élément modifié : la perte de focus d'une
                // saisie précédente ne termine pas celle-ci.
                let r = ui.add(te.id_salt(format!("{editing:?}")));
                if self.edit_focus {
                    r.request_focus();
                    self.edit_focus = false;
                }
                if r.lost_focus() {
                    done = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    done = true;
                }
            });
        // Le texte est appliqué à chaque frappe.
        let text = self.edit_buf.clone();
        match &editing {
            Editing::Text(k) => {
                if let Some(t) = board.texts.iter_mut().find(|t| t.key == *k)
                    && t.text != text
                {
                    t.text = text;
                    out.changed = true;
                }
            }
            Editing::GroupLabel(k) => {
                if let Some(g) = board.groups.iter_mut().find(|g| g.key == *k)
                    && g.label != text
                {
                    g.label = text;
                    out.changed = true;
                }
            }
            Editing::EdgeLabel(k) => {
                if let Some(e) = board.edges.iter_mut().find(|e| e.key == *k)
                    && e.label != text
                {
                    e.label = text;
                    out.changed = true;
                }
            }
        }
        if done {
            self.editing = None;
        }
    }
}

/// Sous-menu « Couleur » ; `Some(None)` : couleur par défaut.
fn color_menu(ui: &mut egui::Ui) -> Option<Option<String>> {
    let mut chosen = None;
    ui.menu_button("Couleur", |ui| {
        if ui.button("Par défaut").clicked() {
            chosen = Some(None);
            ui.close();
        }
        for (name, code) in SWATCHES {
            let c = hex(Some(code)).unwrap_or(Color32::GRAY);
            if ui
                .button(RichText::new(format!("■ {name}")).color(c))
                .clicked()
            {
                chosen = Some(Some(code.to_owned()));
                ui.close();
            }
        }
    });
    chosen
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

    #[test]
    fn geometry_helpers() {
        let r = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 50.0));
        assert_eq!(
            border_point(r, Pos2::new(300.0, 25.0)),
            Pos2::new(100.0, 25.0)
        );
        assert_eq!(
            border_point(r, Pos2::new(50.0, -100.0)),
            Pos2::new(50.0, 0.0)
        );
        assert!(
            dist_to_segment(Pos2::new(5.0, 3.0), Pos2::ZERO, Pos2::new(10.0, 0.0)) - 3.0 < 1e-4
        );
        assert_eq!(hex(Some("#ff8000")), Some(Color32::from_rgb(255, 128, 0)));
        assert_eq!(hex(Some("nope")), None);
    }
}
