//! Vue graphe 2D : nœuds multi-catégories, arêtes typées, disposition en
//! forces interactive, filtres, voisinage, aperçu au survol, épinglage.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke};
use enigma_core::filter::NoteFilter;
use enigma_core::links::LinkEnd;
use enigma_core::{NoteId, Pos, Project};
use enigma_graph::pick::{neighborhood, pick_node, pick_segment};
use enigma_graph::{Camera2D, Layout, LayoutParams, Vec2, initial_position};

use crate::colors::c32;
use crate::filter_ui;

/// Nombre maximal d'étiquettes dessinées par image.
const MAX_LABELS: usize = 250;
/// En dessous de ce zoom, seules les étiquettes importantes sont dessinées.
const LABEL_ZOOM: f32 = 0.55;

/// Arête affichée (plusieurs types entre deux fiches = plusieurs arêtes).
#[derive(Clone, Debug, PartialEq)]
struct Edge {
    a: usize,
    b: usize,
    kind: Option<String>,
    directed: bool,
    color: Color32,
    /// Décalage (en multiples) pour séparer les arêtes parallèles.
    offset: f32,
}

/// Ce que la vue demande à l'espace de travail.
#[derive(Default)]
pub struct GraphOutcome {
    /// Sélectionner (ouvrir dans le panneau latéral) cette fiche.
    pub select: Option<NoteId>,
    /// Ouvrir cette fiche dans l'éditeur de texte.
    pub open_editor: Option<NoteId>,
    /// Désélectionner.
    pub deselect: bool,
    /// Positions à écrire dans les fiches (`None` : désépingler).
    pub pins: Vec<(NoteId, Option<Pos>)>,
}

type StructureKey = (u64, u64, NoteFilter, Vec<Option<String>>, bool);

pub struct GraphView {
    ids: Vec<NoteId>,
    index: HashMap<NoteId, usize>,
    edges: Vec<Edge>,
    layout: Layout,
    key: Option<StructureKey>,
    /// Filtre des nœuds (catégories, statuts).
    pub filter: NoteFilter,
    /// Types de liens masqués.
    pub hidden_link_types: Vec<Option<String>>,
    pub hide_isolated: bool,
    /// Profondeur du voisinage mis en évidence.
    pub depth: u32,
    pub show_filters: bool,
    camera: Camera2D,
    auto_fit: bool,
    dragging: Option<usize>,
    menu_node: Option<usize>,
    last_rect: Option<Rect>,
}

impl Default for GraphView {
    fn default() -> Self {
        Self {
            ids: Vec::new(),
            index: HashMap::new(),
            edges: Vec::new(),
            layout: Layout::new(Vec::new(), &[], LayoutParams::default()),
            key: None,
            filter: NoteFilter::default(),
            hidden_link_types: Vec::new(),
            hide_isolated: false,
            depth: 1,
            show_filters: false,
            camera: Camera2D::default(),
            auto_fit: true,
            dragging: None,
            menu_node: None,
            last_rect: None,
        }
    }
}

fn v(p: Pos2) -> Vec2 {
    Vec2::new(p.x, p.y)
}

fn p(v: Vec2) -> Pos2 {
    Pos2::new(v.x, v.y)
}

fn seed(id: &NoteId) -> u64 {
    let mut h = DefaultHasher::new();
    id.as_str().hash(&mut h);
    h.finish()
}

impl GraphView {
    /// Reconstruit nœuds et arêtes si le projet ou les filtres ont changé.
    /// La disposition n'est relancée que si la structure change.
    fn sync(&mut self, project: &Project) {
        let key: StructureKey = (
            project.generation(),
            project.content_version(),
            self.filter.clone(),
            self.hidden_link_types.clone(),
            self.hide_isolated,
        );
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.key = Some(key);
        let mut node_filter = self.filter.clone();
        node_filter.text.clear();
        let mut ids = project.filter(&node_filter);
        let mut visible: HashMap<NoteId, usize> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();

        let collect_edges = |ids: &[NoteId], visible: &HashMap<NoteId, usize>| {
            let mut edges = Vec::new();
            for (a, id) in ids.iter().enumerate() {
                for link in project.outgoing_links(id) {
                    let LinkEnd::Note(target) = &link.other else {
                        continue;
                    };
                    let Some(&b) = visible.get(target) else {
                        continue;
                    };
                    if a == b || self.hidden_link_types.contains(&link.kind) {
                        continue;
                    }
                    let def = link
                        .kind
                        .as_deref()
                        .and_then(|k| project.pack().link_type(k));
                    edges.push(Edge {
                        a,
                        b,
                        directed: def.is_none_or(|d| d.directed),
                        color: def.map_or(Color32::from_gray(140), |d| c32(d.color)),
                        kind: link.kind,
                        offset: 0.0,
                    });
                }
            }
            edges
        };
        let mut edges = collect_edges(&ids, &visible);
        if self.hide_isolated {
            let mut linked = vec![false; ids.len()];
            for e in &edges {
                linked[e.a] = true;
                linked[e.b] = true;
            }
            ids = ids
                .into_iter()
                .enumerate()
                .filter(|(i, _)| linked[*i])
                .map(|(_, id)| id)
                .collect();
            visible = ids
                .iter()
                .enumerate()
                .map(|(i, id)| (id.clone(), i))
                .collect();
            edges = collect_edges(&ids, &visible);
        }
        assign_parallel_offsets(&mut edges);

        let pairs: Vec<(usize, usize)> = edges.iter().map(|e| (e.a, e.b)).collect();
        let old_pairs: Vec<(usize, usize)> = self.edges.iter().map(|e| (e.a, e.b)).collect();
        let structure_changed = ids != self.ids || pairs != old_pairs;

        // Épinglages lus dans les fiches.
        let pins: Vec<Option<Pos>> = ids
            .iter()
            .map(|id| {
                project
                    .note(id)
                    .and_then(|n| n.meta.pos)
                    .filter(|pos| pos.pinned)
            })
            .collect();

        if structure_changed {
            let first = self.ids.is_empty();
            let n = ids.len();
            let mut positions = Vec::with_capacity(n);
            for (i, id) in ids.iter().enumerate() {
                let pos = if let Some(pin) = pins[i] {
                    Vec2::new(pin.x, pin.y)
                } else if let Some(&old) = self.index.get(id) {
                    self.layout.pos[old]
                } else {
                    // Nouvelle fiche : près d'un voisin déjà placé, sinon au hasard (déterministe).
                    let neighbor = pairs.iter().find_map(|&(a, b)| {
                        let other = if a == i {
                            b
                        } else if b == i {
                            a
                        } else {
                            return None;
                        };
                        self.index.get(&ids[other]).map(|&o| self.layout.pos[o])
                    });
                    let jitter = initial_position(seed(id), 1) * 0.5;
                    neighbor.map_or_else(|| initial_position(seed(id), n), |q| q + jitter)
                };
                positions.push(pos);
            }
            let mut layout = Layout::new(positions, &pairs, LayoutParams::default());
            for (i, pin) in pins.iter().enumerate() {
                layout.pinned[i] = pin.is_some();
            }
            if first {
                // Premier affichage : quelques pas tout de suite (budget borné).
                let steps = (60_000 / n.max(1)).clamp(20, 300);
                layout.run(steps);
                self.auto_fit = true;
            } else {
                layout.stop();
                layout.reheat(0.3);
            }
            self.layout = layout;
            self.dragging = None;
        } else {
            for (i, pin) in pins.iter().enumerate() {
                if self.dragging == Some(i) {
                    continue;
                }
                match pin {
                    Some(pos) => {
                        self.layout.pinned[i] = true;
                        self.layout.pos[i] = Vec2::new(pos.x, pos.y);
                    }
                    None if self.layout.pinned[i] => {
                        self.layout.pinned[i] = false;
                        self.layout.reheat(0.1);
                    }
                    None => {}
                }
            }
        }
        self.index = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
        self.ids = ids;
        self.edges = edges;
    }

    fn radius(&self, i: usize) -> f32 {
        (7.0 + 2.5 * (self.layout.degree(i) as f32).sqrt()).min(22.0)
    }

    /// Position à l'écran d'une fiche (dernière image affichée).
    #[cfg(test)]
    pub fn screen_pos(&self, id: &NoteId) -> Option<Pos2> {
        let rect = self.last_rect?;
        let &i = self.index.get(id)?;
        Some(p(self
            .camera
            .to_screen(self.layout.pos[i], v(rect.center()))))
    }

    /// La simulation est-elle encore en mouvement ?
    #[cfg(test)]
    pub fn is_animating(&self) -> bool {
        self.layout.is_running()
    }

    #[cfg(test)]
    pub fn stop_for_bench(&mut self) {
        self.layout.stop();
        self.auto_fit = false;
    }

    #[cfg(test)]
    pub fn node_count(&self) -> usize {
        self.ids.len()
    }

    #[cfg(test)]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Barre d'outils, filtres et canevas.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        selected: Option<&NoteId>,
    ) -> GraphOutcome {
        self.sync(project);
        let mut outcome = GraphOutcome::default();

        ui.horizontal_wrapped(|ui| {
            let n = self.filter.active_count()
                + usize::from(!self.hidden_link_types.is_empty())
                + usize::from(self.hide_isolated);
            let label = if n == 0 {
                "Filtres du graphe".to_owned()
            } else {
                format!("Filtres du graphe ({n})")
            };
            ui.toggle_value(&mut self.show_filters, label);
            ui.separator();
            ui.label("Voisinage");
            ui.add(egui::Slider::new(&mut self.depth, 1..=5).suffix(" niv."))
                .on_hover_text("Profondeur mise en évidence autour de la fiche sélectionnée");
            ui.separator();
            if ui.button("Recentrer").clicked() {
                self.auto_fit = true;
            }
            if ui.button("Relancer la disposition").clicked() {
                self.layout.reheat(1.0);
            }
            ui.weak(format!(
                "{} fiches · {} liens",
                self.ids.len(),
                self.edges.len()
            ));
        });

        if self.show_filters {
            let pack = project.pack().clone();
            egui::Frame::group(ui.style()).show(ui, |ui| {
                filter_ui::categories_ui(ui, &mut self.filter, &pack);
                filter_ui::statuses_ui(ui, &mut self.filter, &pack);
                filter_ui::link_types_ui(
                    ui,
                    "Liens affichés",
                    &mut self.hidden_link_types,
                    &pack,
                    true,
                );
                ui.checkbox(&mut self.hide_isolated, "Masquer les fiches sans lien");
                let active = self.filter.active_count() > 0
                    || !self.hidden_link_types.is_empty()
                    || self.hide_isolated;
                if active && ui.button("Réinitialiser").clicked() {
                    self.filter.clear_criteria();
                    self.hidden_link_types.clear();
                    self.hide_isolated = false;
                }
            });
            // Les filtres ont pu changer : on resynchronise avant de dessiner.
            self.sync(project);
        }

        self.canvas(ui, project, selected, &mut outcome);
        outcome
    }

    fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        selected: Option<&NoteId>,
        outcome: &mut GraphOutcome,
    ) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.last_rect = Some(rect);
        let vc = v(rect.center());
        let visuals = ui.visuals().clone();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);

        if self.ids.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Aucune fiche à afficher avec ces filtres.",
                FontId::proportional(14.0),
                visuals.weak_text_color(),
            );
            return;
        }

        // Simulation.
        if self.layout.is_running() {
            self.layout.step();
            ui.ctx().request_repaint();
        }
        if self.auto_fit {
            self.camera.fit(
                self.layout.pos.iter().copied(),
                v(rect.size().to_pos2()),
                40.0,
            );
            if !self.layout.is_running() {
                self.auto_fit = false;
            }
        }

        // --- Interactions ---
        let pointer_world = resp.hover_pos().map(|s| self.camera.to_world(v(s), vc));
        let tol = 4.0 / self.camera.zoom;
        let hovered_node = pointer_world
            .and_then(|w| pick_node(&self.layout.pos, w, tol, |i| self.radius(i), |_| true));

        if resp.drag_started()
            && let Some(start) = resp.interact_pointer_pos()
        {
            let w = self.camera.to_world(v(start), vc);
            self.dragging = pick_node(&self.layout.pos, w, tol, |i| self.radius(i), |_| true);
            if let Some(i) = self.dragging {
                self.layout.pinned[i] = true;
                self.layout.set_alpha_target(0.3);
                self.layout.reheat(0.3);
            }
            self.auto_fit = false;
        }
        if resp.dragged() {
            match (self.dragging, resp.interact_pointer_pos()) {
                (Some(i), Some(ptr)) => {
                    self.layout.pos[i] = self.camera.to_world(v(ptr), vc);
                }
                _ => {
                    let d = resp.drag_delta();
                    self.camera.pan(Vec2::new(d.x, d.y));
                }
            }
            ui.ctx().request_repaint();
        }
        if resp.drag_stopped() {
            if let Some(i) = self.dragging.take() {
                let q = self.layout.pos[i];
                outcome.pins.push((
                    self.ids[i].clone(),
                    Some(Pos {
                        x: q.x,
                        y: q.y,
                        z: 0.0,
                        pinned: true,
                    }),
                ));
            }
            self.layout.set_alpha_target(0.0);
        }

        if resp.hovered() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = pinch * (scroll * 0.002).exp();
            if (factor - 1.0).abs() > 1e-4
                && let Some(ptr) = resp.hover_pos()
            {
                self.camera.zoom_at(v(ptr), vc, factor);
                self.auto_fit = false;
            }
        }

        if resp.double_clicked() {
            if let Some(i) = hovered_node {
                outcome.open_editor = Some(self.ids[i].clone());
            }
        } else if resp.clicked() {
            match hovered_node {
                Some(i) => outcome.select = Some(self.ids[i].clone()),
                None => outcome.deselect = true,
            }
        }
        if resp.secondary_clicked() {
            self.menu_node = hovered_node;
        }

        // --- Voisinage ---
        let sel = selected.and_then(|id| self.index.get(id).copied());
        let pairs: Vec<(usize, usize)> = self.edges.iter().map(|e| (e.a, e.b)).collect();
        let depths = sel.map(|s| neighborhood(self.ids.len(), &pairs, s, self.depth));
        let lit = |i: usize| depths.as_ref().is_none_or(|d| d[i].is_some());

        // --- Arêtes ---
        let to_s = |i: usize| self.camera.to_screen(self.layout.pos[i], vc);
        let view = rect.expand(40.0);
        let mut hovered_edge = None;
        if hovered_node.is_none()
            && let Some(ptr) = resp.hover_pos()
        {
            let segs = self.edges.iter().enumerate().map(|(k, e)| {
                (
                    k,
                    edge_points(e, to_s(e.a), to_s(e.b)).0,
                    edge_points(e, to_s(e.a), to_s(e.b)).1,
                )
            });
            hovered_edge = pick_segment(segs, v(ptr), 5.0);
        }
        for (k, e) in self.edges.iter().enumerate() {
            let (a, b) = edge_points(e, to_s(e.a), to_s(e.b));
            if !view.contains(p(a)) && !view.contains(p(b)) && !segment_crosses(rect, a, b) {
                continue;
            }
            let on = lit(e.a) && lit(e.b);
            let mut color = if on {
                e.color
            } else {
                e.color.gamma_multiply(0.12)
            };
            let mut width = 1.4;
            if hovered_edge == Some(k) {
                width = 3.0;
                color = e.color;
            }
            painter.line_segment([p(a), p(b)], Stroke::new(width, color));
            if e.directed {
                let r = self.radius(e.b) * self.camera.zoom.max(0.0);
                draw_arrow(&painter, a, b, r.max(3.0), color, width);
            }
        }

        // --- Nœuds ---
        let pack = project.pack();
        let mut labels = 0;
        for i in 0..self.ids.len() {
            let s = to_s(i);
            let r = (self.radius(i) * self.camera.zoom).max(3.0);
            if !rect.expand(r + 2.0).contains(p(s)) {
                continue;
            }
            let Some(note) = project.note(&self.ids[i]) else {
                continue;
            };
            let dim = if lit(i) { 1.0 } else { 0.18 };
            let colors: Vec<Color32> = note
                .meta
                .categories
                .iter()
                .map(|c| {
                    pack.category(c)
                        .map_or(Color32::GRAY, |c| c32(c.color))
                        .gamma_multiply(dim)
                })
                .collect();
            draw_pie(
                &painter,
                p(s),
                r,
                &colors,
                Color32::from_gray(110).gamma_multiply(dim),
            );
            let status_color = note
                .meta
                .status
                .as_deref()
                .and_then(|st| pack.status(st))
                .map(|st| c32(st.color));
            let border = match status_color {
                Some(c) => Stroke::new((r * 0.22).clamp(1.5, 3.5), c.gamma_multiply(dim)),
                None => Stroke::new(1.0, visuals.weak_text_color().gamma_multiply(dim)),
            };
            painter.circle_stroke(p(s), r, border);
            if Some(i) == sel {
                painter.circle_stroke(
                    p(s),
                    r + 4.0,
                    Stroke::new(2.0, visuals.selection.stroke.color),
                );
            }
            if hovered_node == Some(i) {
                painter.circle_stroke(p(s), r + 2.0, Stroke::new(1.5, visuals.strong_text_color()));
            }
            if self.layout.pinned[i] {
                let pin = Pos2::new(s.x + r * 0.75, s.y - r * 0.75);
                painter.circle_filled(pin, 2.5, visuals.strong_text_color().gamma_multiply(dim));
            }
            let important = Some(i) == sel
                || hovered_node == Some(i)
                || depths.as_ref().is_some_and(|d| d[i] == Some(1));
            if (self.camera.zoom >= LABEL_ZOOM && dim == 1.0 && labels < MAX_LABELS) || important {
                labels += 1;
                painter.text(
                    Pos2::new(s.x, s.y + r + 3.0),
                    egui::Align2::CENTER_TOP,
                    &note.meta.title,
                    FontId::proportional(12.0),
                    visuals.text_color().gamma_multiply(dim.max(0.5)),
                );
            }
        }

        // --- Menu contextuel ---
        let menu_node = self.menu_node;
        let mut close_menu = false;
        resp.context_menu(|ui| {
            let Some(i) = menu_node else {
                ui.weak("Clic droit sur une fiche");
                return;
            };
            let id = self.ids[i].clone();
            if ui.button("Ouvrir dans l'éditeur").clicked() {
                outcome.open_editor = Some(id.clone());
                close_menu = true;
            }
            if self.layout.pinned[i] {
                if ui.button("Désépingler").clicked() {
                    outcome.pins.push((id, None));
                    close_menu = true;
                }
            } else if ui.button("Épingler ici").clicked() {
                let q = self.layout.pos[i];
                outcome.pins.push((
                    id,
                    Some(Pos {
                        x: q.x,
                        y: q.y,
                        z: 0.0,
                        pinned: true,
                    }),
                ));
                close_menu = true;
            }
            if close_menu {
                ui.close();
            }
        });

        // --- Aperçus au survol ---
        if self.dragging.is_none() {
            if let Some(i) = hovered_node {
                let id = self.ids[i].clone();
                resp.on_hover_ui_at_pointer(|ui| note_preview(ui, project, &id));
            } else if let Some(k) = hovered_edge {
                let e = &self.edges[k];
                let title = |i: usize| {
                    project
                        .note(&self.ids[i])
                        .map(|n| n.meta.title.clone())
                        .unwrap_or_default()
                };
                let kind = e
                    .kind
                    .as_deref()
                    .map(|k| {
                        project
                            .pack()
                            .link_type(k)
                            .map_or(k.to_owned(), |d| d.label.clone())
                    })
                    .unwrap_or_else(|| "lien".to_owned());
                let arrow = if e.directed { "»" } else { "—" };
                let text = format!("{} {arrow} {kind} {arrow} {}", title(e.a), title(e.b));
                resp.on_hover_ui_at_pointer(|ui| {
                    ui.label(RichText::new(text).color(e.color));
                });
            }
        }
    }
}

/// Décalage des arêtes parallèles entre une même paire de fiches.
fn assign_parallel_offsets(edges: &mut [Edge]) {
    let mut groups: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (k, e) in edges.iter().enumerate() {
        groups
            .entry((e.a.min(e.b), e.a.max(e.b)))
            .or_default()
            .push(k);
    }
    for ks in groups.values() {
        let n = ks.len() as f32;
        for (j, &k) in ks.iter().enumerate() {
            edges[k].offset = j as f32 - (n - 1.0) / 2.0;
        }
    }
}

/// Extrémités d'une arête à l'écran, décalées si parallèle.
fn edge_points(e: &Edge, a: Vec2, b: Vec2) -> (Vec2, Vec2) {
    if e.offset == 0.0 {
        return (a, b);
    }
    // Sens canonique pour que le décalage ne dépende pas de la direction.
    let (lo, hi) = if e.a < e.b { (a, b) } else { (b, a) };
    let shift = (hi - lo).normalized().perp() * (e.offset * 5.0);
    (a + shift, b + shift)
}

/// Le segment traverse-t-il le rectangle ? (test grossier par sa boîte)
fn segment_crosses(rect: Rect, a: Vec2, b: Vec2) -> bool {
    let bb = Rect::from_two_pos(p(a), p(b));
    bb.intersects(rect)
}

fn draw_arrow(
    painter: &egui::Painter,
    from: Vec2,
    to: Vec2,
    target_radius: f32,
    color: Color32,
    width: f32,
) {
    let dir = (to - from).normalized();
    if dir == Vec2::ZERO {
        return;
    }
    let size = 6.0 + width * 1.5;
    let tip = to - dir * target_radius;
    let base = tip - dir * size;
    let side = dir.perp() * (size * 0.5);
    painter.add(Shape::convex_polygon(
        vec![p(tip), p(base + side), p(base - side)],
        color,
        Stroke::NONE,
    ));
}

/// Disque découpé en parts égales, une par couleur.
fn draw_pie(painter: &egui::Painter, center: Pos2, r: f32, colors: &[Color32], empty: Color32) {
    match colors {
        [] => {
            painter.circle_filled(center, r, empty);
        }
        [c] => {
            painter.circle_filled(center, r, *c);
        }
        _ => {
            let n = colors.len();
            let per = std::f32::consts::TAU / n as f32;
            let segments = (48 / n).max(6);
            for (k, color) in colors.iter().enumerate() {
                let a0 = -std::f32::consts::FRAC_PI_2 + per * k as f32;
                let mut pts = Vec::with_capacity(segments + 2);
                pts.push(center);
                for s in 0..=segments {
                    let a = a0 + per * s as f32 / segments as f32;
                    pts.push(Pos2::new(center.x + r * a.cos(), center.y + r * a.sin()));
                }
                painter.add(Shape::convex_polygon(pts, *color, Stroke::NONE));
            }
        }
    }
}

/// Aperçu d'une fiche au survol.
fn note_preview(ui: &mut egui::Ui, project: &Project, id: &NoteId) {
    let Some(note) = project.note(id) else {
        return;
    };
    ui.set_max_width(320.0);
    ui.label(RichText::new(&note.meta.title).strong().size(15.0));
    let pack = project.pack();
    ui.horizontal_wrapped(|ui| {
        for c in &note.meta.categories {
            match pack.category(c) {
                Some(def) => ui.label(RichText::new(&def.label).color(c32(def.color)).small()),
                None => ui.label(RichText::new(c).small().weak()),
            };
        }
        if let Some(st) = note.meta.status.as_deref() {
            let (label, color) = pack
                .status(st)
                .map_or((st.to_owned(), ui.visuals().weak_text_color()), |s| {
                    (s.label.clone(), c32(s.color))
                });
            ui.label(RichText::new(format!("• {label}")).color(color).small());
        }
    });
    let excerpt: String = note
        .body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("\n");
    if !excerpt.is_empty() {
        ui.separator();
        let short: String = excerpt.chars().take(400).collect();
        ui.label(RichText::new(short).small());
    }
    ui.separator();
    ui.label(
        RichText::new("Clic : sélectionner · double-clic : ouvrir · glisser : épingler")
            .small()
            .weak(),
    );
}
