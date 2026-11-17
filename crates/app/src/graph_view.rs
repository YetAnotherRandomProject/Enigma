//! Vue graphe 2D / 3D : nœuds multi-catégories, arêtes typées, disposition
//! en forces interactive, filtres, voisinage, aperçu au survol, épinglage.
//!
//! Les deux modes partagent la même simulation (en 2D, `z` reste nul) et le
//! même pipeline de rendu : chaque nœud est projeté à l'écran (position,
//! profondeur, échelle), puis arêtes et nœuds sont dessinés avec le painter
//! d'egui — en 3D du plus loin au plus proche, en billboards dont la taille
//! dépend de la distance.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use eframe::egui::{
    self, Color32, FontId, PointerButton, Pos2, Rect, RichText, Sense, Shape, Stroke,
};
use enigma_core::filter::NoteFilter;
use enigma_core::links::LinkEnd;
use enigma_core::pack::FieldKind;
use enigma_core::{NoteId, Pos, Project};
use enigma_graph::pick::{neighborhood, pick_front, pick_segment};
use enigma_graph::{
    Camera2D, Layout, LayoutParams, OrbitCamera, Projected, Vec2, Vec3, initial_depth,
    initial_position,
};

use crate::colors::c32;
use crate::filter_ui;

/// Nombre maximal d'étiquettes dessinées par image (2D).
const MAX_LABELS: usize = 250;
/// Nombre maximal d'étiquettes en 3D (les plus proches).
const MAX_LABELS_3D: usize = 80;
/// En dessous de ce zoom (2D), seules les étiquettes importantes sont dessinées.
const LABEL_ZOOM: f32 = 0.55;
/// Rayon apparent minimal (px) pour qu'un nœud ait une étiquette en 3D.
const LABEL_MIN_RADIUS_3D: f32 = 6.0;

/// Mode d'affichage du graphe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    Flat,
    Space,
}

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
    note: Option<String>,
    /// Écrit dans le texte de la fiche source (`[[...]]`).
    in_body: bool,
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
    /// Créations / modifications / suppressions de liens.
    pub link_ops: Vec<LinkOp>,
}

/// Modification d'un lien demandée depuis le graphe.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkOp {
    Add {
        from: NoteId,
        to: NoteId,
        kind: Option<String>,
        note: Option<String>,
    },
    Update {
        from: NoteId,
        to: NoteId,
        old_kind: Option<String>,
        kind: Option<String>,
        note: Option<String>,
    },
    Remove {
        from: NoteId,
        to: NoteId,
        kind: Option<String>,
    },
}

impl LinkOp {
    /// Fiche dont le contenu change.
    pub fn source(&self) -> &NoteId {
        match self {
            LinkOp::Add { from, .. }
            | LinkOp::Update { from, .. }
            | LinkOp::Remove { from, .. } => from,
        }
    }
}

/// Dialogue de création ou de modification d'un lien.
#[derive(Clone, Debug)]
struct LinkDialog {
    from: NoteId,
    to: NoteId,
    /// Type actuel du lien modifié ; `None` : création.
    existing: Option<Option<String>>,
    kind: Option<String>,
    note: String,
    in_body: bool,
    focus: bool,
}

/// Échelle de l'axe Z lié à un champ numérique.
#[derive(Clone, Debug, PartialEq)]
struct ZAxis {
    label: String,
    min: f64,
    max: f64,
    /// Valeurs entières à graduer (si peu nombreuses), sinon min et max.
    ticks: Vec<f64>,
    span: f32,
}

impl ZAxis {
    fn z_of(&self, value: f64) -> f32 {
        if self.max > self.min {
            (((value - self.min) / (self.max - self.min)) as f32 - 0.5) * self.span
        } else {
            0.0
        }
    }
}

type StructureKey = (
    u64,
    u64,
    NoteFilter,
    Vec<Option<String>>,
    bool,
    Mode,
    Option<String>,
);

/// Glisser en cours.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// Un nœud, à cette profondeur (3D).
    Node(usize, f32),
    Pan,
    Orbit,
    /// Fil tiré depuis un nœud (création de lien).
    Wire(usize, Pos2),
}

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
    pub mode: Mode,
    /// Champ numérique porté par l'axe Z (3D), `None` : profondeur libre.
    pub z_field: Option<String>,
    z_axis: Option<ZAxis>,
    camera: Camera2D,
    orbit: OrbitCamera,
    auto_fit: bool,
    drag: Option<Drag>,
    menu_node: Option<usize>,
    last_rect: Option<Rect>,
    menu_edge: Option<usize>,
    /// Fil en attente d'une cible (depuis le menu contextuel).
    pending_wire: Option<usize>,
    link_dialog: Option<LinkDialog>,
    /// Dernier type choisi pour un nouveau lien.
    last_kind: Option<Option<String>>,
    /// Projection de chaque nœud (dernière image) ; `None` : hors champ.
    proj: Vec<Option<Projected>>,
    /// Ordre de dessin (du plus loin au plus proche en 3D).
    order: Vec<usize>,
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
            mode: Mode::Flat,
            z_field: None,
            z_axis: None,
            camera: Camera2D::default(),
            orbit: OrbitCamera::default(),
            auto_fit: true,
            drag: None,
            menu_node: None,
            last_rect: None,
            proj: Vec::new(),
            order: Vec::new(),
            menu_edge: None,
            pending_wire: None,
            link_dialog: None,
            last_kind: None,
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
    /// Bascule 2D / 3D (la sélection, gérée par l'espace de travail, est
    /// conservée) ; la vue se recadre.
    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.auto_fit = true;
            self.drag = None;
            self.pending_wire = None;
        }
    }

    /// Reconstruit nœuds et arêtes si le projet ou les filtres ont changé.
    /// La disposition n'est relancée que si la structure change.
    fn sync(&mut self, project: &Project) {
        let key: StructureKey = (
            project.generation(),
            project.content_version(),
            self.filter.clone(),
            self.hidden_link_types.clone(),
            self.hide_isolated,
            self.mode,
            self.z_field.clone(),
        );
        if self.key.as_ref() == Some(&key) {
            return;
        }
        if self.key.as_ref().is_some_and(|k| k.6 != key.6) {
            // Nouvel axe Z : l'étendue change, on recadre.
            self.auto_fit = true;
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
                        note: link.note,
                        in_body: link.in_body,
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
        let flat = self.mode == Mode::Flat;
        let n = ids.len();

        // Épinglages lus dans les fiches.
        let pins: Vec<Option<Vec3>> = ids
            .iter()
            .map(|id| {
                project
                    .note(id)
                    .and_then(|n| n.meta.pos)
                    .filter(|pos| pos.pinned)
                    .map(|pos| Vec3::new(pos.x, pos.y, if flat { 0.0 } else { pos.z }))
            })
            .collect();

        // Axe Z lié à un champ.
        self.z_axis = self
            .z_field
            .as_deref()
            .and_then(|f| z_axis(project, &ids, f));
        let z_targets: Vec<Option<f32>> = match (&self.z_axis, self.z_field.as_deref()) {
            (Some(axis), Some(field)) => ids
                .iter()
                .map(|id| {
                    project
                        .note(id)
                        .and_then(|n| n.meta.fields.get(field))
                        .and_then(|v| v.as_f64())
                        .map(|v| axis.z_of(v))
                })
                .collect(),
            _ => vec![None; n],
        };
        let start_z = |ids: &[NoteId], i: usize| initial_depth(seed(&ids[i]), ids.len());

        if structure_changed {
            let first = self.ids.is_empty();
            let mut positions = Vec::with_capacity(n);
            for (i, id) in ids.iter().enumerate() {
                let pos = if let Some(pin) = pins[i] {
                    pin
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
                    let fresh = || {
                        let mut q = initial_position(seed(id), n);
                        q.z = start_z(&ids, i);
                        q
                    };
                    neighbor.map_or_else(fresh, |q| q + jitter)
                };
                positions.push(pos);
            }
            let mut layout = if flat {
                Layout::new(positions, &pairs, LayoutParams::default())
            } else {
                Layout::new_3d(positions, &pairs, LayoutParams::default())
            };
            for (i, pin) in pins.iter().enumerate() {
                layout.pinned[i] = pin.is_some();
            }
            layout.z_target = z_targets;
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
            self.drag = None;
            self.pending_wire = None;
            self.menu_node = None;
            self.menu_edge = None;
        } else {
            if self.layout.z_target != z_targets {
                self.layout.z_target = z_targets;
                self.layout.reheat(0.3);
            }
            if self.layout.is_flat() != flat {
                let ids_ref = &ids;
                self.layout.set_flat(flat, |i| start_z(ids_ref, i));
            }
            for (i, pin) in pins.iter().enumerate() {
                if matches!(self.drag, Some(Drag::Node(d, _)) if d == i) {
                    continue;
                }
                match pin {
                    Some(pos) => {
                        self.layout.pinned[i] = true;
                        self.layout.pos[i] = *pos;
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

    /// Rayon apparent (px) d'un nœud projeté.
    fn screen_radius(&self, i: usize, pr: &Projected) -> f32 {
        let r = self.radius(i) * pr.scale;
        match self.mode {
            Mode::Flat => r.max(3.0),
            // Billboards : taille selon la distance, bornée de près.
            Mode::Space => r.clamp(2.0, self.radius(i) * 2.0),
        }
    }

    /// Position à l'écran d'une fiche (dernière image affichée).
    #[cfg(test)]
    pub fn screen_pos(&self, id: &NoteId) -> Option<Pos2> {
        let &i = self.index.get(id)?;
        self.proj.get(i).copied().flatten().map(|pr| p(pr.screen))
    }

    /// Position de la fiche dans la disposition.
    #[cfg(test)]
    pub fn world_pos(&self, id: &NoteId) -> Option<Vec3> {
        self.index.get(id).map(|&i| self.layout.pos[i])
    }

    #[cfg(test)]
    pub fn orbit_camera(&self) -> OrbitCamera {
        self.orbit
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
            let mut mode = self.mode;
            ui.selectable_value(&mut mode, Mode::Flat, "2D")
                .on_hover_text("Graphe à plat");
            ui.selectable_value(&mut mode, Mode::Space, "3D")
                .on_hover_text("Graphe en 3D (caméra orbitale)");
            self.set_mode(mode);
            ui.separator();
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
            if self.mode == Mode::Space {
                ui.separator();
                self.z_axis_ui(ui, project);
            }
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
        }
        // Mode, axe Z ou filtres ont pu changer : on resynchronise avant de dessiner.
        self.sync(project);

        self.canvas(ui, project, selected, &mut outcome);
        self.link_dialog_ui(ui.ctx(), project, &mut outcome);
        outcome
    }

    /// Choix du champ numérique porté par l'axe Z.
    fn z_axis_ui(&mut self, ui: &mut egui::Ui, project: &Project) {
        let fields: Vec<(String, String)> = project
            .pack()
            .fields
            .iter()
            .filter(|f| f.kind == FieldKind::Number)
            .map(|f| (f.id.clone(), f.label.clone()))
            .collect();
        let current = self
            .z_field
            .as_deref()
            .and_then(|id| fields.iter().find(|(f, _)| f == id))
            .map_or_else(|| "Libre".to_owned(), |(_, l)| l.clone());
        egui::ComboBox::new("graph_z_axis", "Axe Z")
            .selected_text(current)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.z_field, None, "Libre");
                for (id, label) in &fields {
                    ui.selectable_value(&mut self.z_field, Some(id.clone()), label);
                }
            })
            .response
            .on_hover_text("Profondeur libre, ou donnée par un champ numérique des fiches");
        if let Some(axis) = &self.z_axis {
            ui.weak(format!("de {} à {}", fmt_num(axis.min), fmt_num(axis.max)));
        } else if self.z_field.is_some() {
            ui.weak("aucune valeur");
        }
    }

    /// Projette tous les nœuds et calcule l'ordre de dessin.
    fn project_nodes(&mut self, rect: Rect) {
        let vc = v(rect.center());
        self.proj.clear();
        match self.mode {
            Mode::Flat => {
                self.proj.extend(self.layout.pos.iter().map(|q| {
                    Some(Projected {
                        screen: self.camera.to_screen(q.xy(), vc),
                        depth: 0.0,
                        scale: self.camera.zoom,
                    })
                }));
                self.order.clear();
                self.order.extend(0..self.ids.len());
            }
            Mode::Space => {
                let projector = self.orbit.projector(vc, rect.height());
                self.proj
                    .extend(self.layout.pos.iter().map(|q| projector.project(*q)));
                self.order.clear();
                self.order
                    .extend((0..self.ids.len()).filter(|&i| self.proj[i].is_some()));
                let depth = |i: usize| self.proj[i].map_or(0.0, |pr| pr.depth);
                self.order.sort_by(|&a, &b| depth(b).total_cmp(&depth(a)));
            }
        }
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
            self.proj.clear();
            self.order.clear();
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Aucune fiche à afficher avec ces filtres.",
                FontId::proportional(14.0),
                visuals.weak_text_color(),
            );
            return;
        }
        let space = self.mode == Mode::Space;

        // Simulation.
        if self.layout.is_running() {
            self.layout.step();
            ui.ctx().request_repaint();
        }
        if self.auto_fit {
            let size = v(rect.size().to_pos2());
            match self.mode {
                Mode::Flat => self
                    .camera
                    .fit(self.layout.pos.iter().map(|q| q.xy()), size, 40.0),
                Mode::Space => self.orbit.fit(self.layout.pos.iter().copied(), size, 40.0),
            }
            if !self.layout.is_running() {
                self.auto_fit = false;
            }
        }
        self.project_nodes(rect);

        // --- Interactions ---
        let pick_at = |this: &Self, s: Pos2| {
            pick_front(
                this.order.iter().filter_map(|&i| {
                    let pr = this.proj[i]?;
                    Some((i, pr.screen, this.screen_radius(i, &pr), pr.depth))
                }),
                v(s),
                4.0,
            )
        };
        let hovered_node = resp.hover_pos().and_then(|s| pick_at(self, s));

        if resp.drag_started()
            && let Some(start) = ui
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
        {
            let (primary, shift, command) = ui.input(|i| {
                (
                    i.pointer.button_down(PointerButton::Primary),
                    i.modifiers.shift,
                    i.modifiers.command,
                )
            });
            let node = if primary && !shift {
                pick_at(self, start)
            } else {
                None
            };
            self.pending_wire = None;
            self.drag = Some(match node {
                Some(i) if command => Drag::Wire(i, start),
                Some(i) => {
                    self.layout.pinned[i] = true;
                    self.layout.set_alpha_target(0.3);
                    self.layout.reheat(0.3);
                    Drag::Node(i, self.proj[i].map_or(0.0, |pr| pr.depth))
                }
                None if space && primary && !shift => Drag::Orbit,
                None => Drag::Pan,
            });
            self.auto_fit = false;
        }
        if resp.dragged() {
            let d = v(resp.drag_delta().to_pos2());
            match (self.drag, resp.interact_pointer_pos()) {
                (Some(Drag::Node(i, depth)), Some(ptr)) => {
                    self.layout.pos[i] = match self.mode {
                        Mode::Flat => Vec3::flat(self.camera.to_world(v(ptr), vc)),
                        Mode::Space => {
                            let mut q = self.orbit.unproject(v(ptr), depth, vc, rect.height());
                            if let Some(z) = self.layout.z_target[i] {
                                q.z = z;
                            }
                            q
                        }
                    };
                }
                (Some(Drag::Orbit), _) => self.orbit.orbit(d),
                (Some(Drag::Wire(i, _)), Some(ptr)) => self.drag = Some(Drag::Wire(i, ptr)),
                _ => match self.mode {
                    Mode::Flat => self.camera.pan(d),
                    Mode::Space => self.orbit.pan(d, rect.height()),
                },
            }
            self.project_nodes(rect);
            ui.ctx().request_repaint();
        }
        if resp.drag_stopped() {
            if let Some(Drag::Wire(from, end)) = self.drag
                && let Some(to) = pick_at(self, resp.interact_pointer_pos().unwrap_or(end))
                && to != from
            {
                self.open_create_dialog(from, to);
            }
            if let Some(Drag::Node(i, _)) = self.drag {
                let q = self.layout.pos[i];
                let z = if space {
                    q.z
                } else {
                    // En 2D, on garde la profondeur déjà enregistrée.
                    project
                        .note(&self.ids[i])
                        .and_then(|n| n.meta.pos)
                        .map_or(0.0, |pos| pos.z)
                };
                outcome.pins.push((self.ids[i].clone(), Some(pin_at(q, z))));
            }
            self.drag = None;
            self.layout.set_alpha_target(0.0);
        }

        if resp.hovered() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = pinch * (scroll * 0.002).exp();
            if (factor - 1.0).abs() > 1e-4
                && let Some(ptr) = resp.hover_pos()
            {
                match self.mode {
                    Mode::Flat => self.camera.zoom_at(v(ptr), vc, factor),
                    Mode::Space => self.orbit.dolly(1.0 / factor),
                }
                self.auto_fit = false;
                self.project_nodes(rect);
            }
        }

        // --- Voisinage ---
        let sel = selected.and_then(|id| self.index.get(id).copied());
        let pairs: Vec<(usize, usize)> = self.edges.iter().map(|e| (e.a, e.b)).collect();
        let depths = sel.map(|s| neighborhood(self.ids.len(), &pairs, s, self.depth));
        let lit = |i: usize| depths.as_ref().is_none_or(|d| d[i].is_some());

        // Brouillard (3D) : les nœuds lointains s'estompent.
        let (near, far) = self
            .order
            .iter()
            .filter_map(|&i| self.proj[i].map(|pr| pr.depth))
            .fold((f32::MAX, f32::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)));
        let fog = |i: usize| -> f32 {
            match self.proj[i] {
                _ if Some(i) == sel => 1.0,
                Some(pr) if space && far > near => 1.0 - 0.5 * (pr.depth - near) / (far - near),
                _ => 1.0,
            }
        };

        // --- Axe Z ---
        if space && let Some(axis) = &self.z_axis {
            self.draw_z_axis(&painter, rect, axis, &visuals);
        }

        // --- Arêtes ---
        let screen = |i: usize| self.proj[i].map(|pr| pr.screen);
        let view = rect.expand(40.0);
        let mut hovered_edge = None;
        if hovered_node.is_none()
            && self.drag.is_none()
            && let Some(ptr) = resp.hover_pos()
        {
            let segs = self.edges.iter().enumerate().filter_map(|(k, e)| {
                let (a, b) = edge_points(e, screen(e.a)?, screen(e.b)?);
                Some((k, a, b))
            });
            hovered_edge = pick_segment(segs, v(ptr), 5.0);
        }

        let mut cancel_wire = ui.input(|i| i.key_pressed(egui::Key::Escape));
        let mut create_link = None;
        let mut edit_link = None;
        if resp.double_clicked() {
            if let Some(i) = hovered_node {
                outcome.open_editor = Some(self.ids[i].clone());
            }
        } else if resp.clicked() {
            if let Some(from) = self.pending_wire {
                // Fil lancé depuis le menu : le clic choisit la cible.
                cancel_wire = true;
                if let Some(to) = hovered_node.filter(|&t| t != from) {
                    create_link = Some((from, to));
                }
            } else {
                match (hovered_node, hovered_edge) {
                    (Some(i), _) => outcome.select = Some(self.ids[i].clone()),
                    (None, Some(k)) => edit_link = Some(k),
                    (None, None) => outcome.deselect = true,
                }
            }
        }
        let secondary = resp.secondary_clicked();
        cancel_wire |= secondary;
        for (k, e) in self.edges.iter().enumerate() {
            let (Some(sa), Some(sb)) = (screen(e.a), screen(e.b)) else {
                continue;
            };
            let (a, b) = edge_points(e, sa, sb);
            if !view.contains(p(a)) && !view.contains(p(b)) && !segment_crosses(rect, a, b) {
                continue;
            }
            let on = lit(e.a) && lit(e.b);
            let fade = (fog(e.a) + fog(e.b)) * 0.5;
            let mut color = if on {
                e.color.gamma_multiply(fade)
            } else {
                e.color.gamma_multiply(0.12 * fade)
            };
            let mut width = 1.4;
            if hovered_edge == Some(k) {
                width = 3.0;
                color = e.color;
            }
            painter.line_segment([p(a), p(b)], Stroke::new(width, color));
            if e.directed
                && let Some(pr) = self.proj[e.b]
            {
                let r = self.screen_radius(e.b, &pr);
                draw_arrow(&painter, a, b, r, color, width);
            }
        }

        // --- Étiquettes autorisées en 3D : les plus proches, assez grandes ---
        let mut label_ok = vec![!space; self.ids.len()];
        if space {
            let mut count = 0;
            for &i in self.order.iter().rev() {
                if count >= MAX_LABELS_3D {
                    break;
                }
                let Some(pr) = self.proj[i] else { continue };
                if self.screen_radius(i, &pr) >= LABEL_MIN_RADIUS_3D && rect.contains(p(pr.screen))
                {
                    label_ok[i] = true;
                    count += 1;
                }
            }
        }

        // --- Nœuds ---
        let pack = project.pack();
        let mut labels = 0;
        for &i in &self.order {
            let Some(pr) = self.proj[i] else { continue };
            let s = pr.screen;
            let r = self.screen_radius(i, &pr);
            if !rect.expand(r + 2.0).contains(p(s)) {
                continue;
            }
            let Some(note) = project.note(&self.ids[i]) else {
                continue;
            };
            let dim = if lit(i) { 1.0 } else { 0.18 } * fog(i);
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
                Some(c) => Stroke::new((r * 0.22).clamp(1.0, 3.5), c.gamma_multiply(dim)),
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
            let zoom_ok = space || self.camera.zoom >= LABEL_ZOOM;
            if (zoom_ok && label_ok[i] && lit(i) && labels < MAX_LABELS) || important {
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

        // --- Fil en cours (création de lien) ---
        let wire_from = match self.drag {
            Some(Drag::Wire(i, _)) => Some(i),
            _ => self.pending_wire,
        };
        if let Some(from) = wire_from
            && let Some(src) = self.proj[from]
            && let Some(ptr) = ui.input(|i| i.pointer.latest_pos())
        {
            let target = hovered_node.filter(|&t| t != from);
            let color = visuals.selection.stroke.color;
            let end = target
                .and_then(|t| self.proj[t])
                .map_or(v(ptr), |pr| pr.screen);
            painter.line_segment([p(src.screen), p(end)], Stroke::new(2.0, color));
            let r = target
                .and_then(|t| self.proj[t].map(|pr| self.screen_radius(t, &pr)))
                .unwrap_or(0.0);
            draw_arrow(&painter, src.screen, end, r, color, 2.0);
            if let Some(t) = target
                && let Some(pr) = self.proj[t]
            {
                let r = self.screen_radius(t, &pr);
                painter.circle_stroke(p(pr.screen), r + 4.0, Stroke::new(2.0, color));
            }
            ui.ctx().request_repaint();
        }

        let hint = if wire_from.is_some() {
            "Relâcher ou cliquer sur la fiche cible · Échap : annuler"
        } else if space {
            "Glisser : tourner · Maj+glisser ou clic droit glissé : déplacer · molette : zoom · Ctrl+glisser d'une fiche à l'autre : lier"
        } else {
            "Glisser le fond : déplacer · molette : zoom · Ctrl+glisser d'une fiche à l'autre : lier · clic sur un lien : le modifier"
        };
        painter.text(
            rect.left_bottom() + egui::vec2(8.0, -6.0),
            egui::Align2::LEFT_BOTTOM,
            hint,
            FontId::proportional(11.0),
            visuals.weak_text_color(),
        );

        if cancel_wire {
            self.pending_wire = None;
        }
        if secondary {
            self.menu_node = hovered_node;
            self.menu_edge = if hovered_node.is_none() {
                hovered_edge
            } else {
                None
            };
        }
        if let Some((from, to)) = create_link {
            self.open_create_dialog(from, to);
        }
        if let Some(k) = edit_link {
            self.open_edit_dialog(k);
        }

        // --- Menu contextuel ---
        let menu_node = self.menu_node;
        let menu_edge = self.menu_edge;
        let mut close_menu = false;
        resp.context_menu(|ui| {
            let Some(i) = menu_node else {
                if let Some(k) = menu_edge.filter(|&k| k < self.edges.len()) {
                    if ui.button("Modifier le lien…").clicked() {
                        self.open_edit_dialog(k);
                        ui.close();
                    }
                    if ui.button("Supprimer le lien").clicked() {
                        let e = &self.edges[k];
                        outcome.link_ops.push(LinkOp::Remove {
                            from: self.ids[e.a].clone(),
                            to: self.ids[e.b].clone(),
                            kind: e.kind.clone(),
                        });
                        ui.close();
                    }
                } else {
                    ui.weak("Clic droit sur une fiche ou un lien");
                }
                return;
            };
            let id = self.ids[i].clone();
            if ui.button("Créer un lien depuis cette fiche…").clicked() {
                self.pending_wire = Some(i);
                close_menu = true;
            }
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
                outcome.pins.push((id, Some(pin_at(q, q.z))));
                close_menu = true;
            }
            if close_menu {
                ui.close();
            }
        });

        // --- Aperçus au survol ---
        if self.drag.is_none() {
            if let Some(i) = hovered_node {
                let id = self.ids[i].clone();
                let z_field = self.z_field.clone().filter(|_| space);
                resp.on_hover_ui_at_pointer(|ui| {
                    note_preview(ui, project, &id, z_field.as_deref())
                });
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
                let note = e.note.clone();
                resp.on_hover_ui_at_pointer(|ui| {
                    ui.label(RichText::new(text).color(e.color));
                    if let Some(n) = note {
                        ui.label(RichText::new(n).italics());
                    }
                    ui.label(
                        RichText::new("Clic : modifier · clic droit : supprimer")
                            .small()
                            .weak(),
                    );
                });
            }
        }
    }

    fn open_create_dialog(&mut self, from: usize, to: usize) {
        let kind = self.last_kind.clone().unwrap_or(None);
        self.link_dialog = Some(LinkDialog {
            from: self.ids[from].clone(),
            to: self.ids[to].clone(),
            existing: None,
            kind,
            note: String::new(),
            in_body: false,
            focus: true,
        });
    }

    fn open_edit_dialog(&mut self, k: usize) {
        let Some(e) = self.edges.get(k) else {
            return;
        };
        self.link_dialog = Some(LinkDialog {
            from: self.ids[e.a].clone(),
            to: self.ids[e.b].clone(),
            existing: Some(e.kind.clone()),
            kind: e.kind.clone(),
            note: e.note.clone().unwrap_or_default(),
            in_body: e.in_body,
            focus: true,
        });
    }

    /// Le dialogue de lien est-il ouvert ?
    #[cfg(test)]
    pub fn link_dialog_open(&self) -> bool {
        self.link_dialog.is_some()
    }

    fn link_dialog_ui(
        &mut self,
        ctx: &egui::Context,
        project: &Project,
        outcome: &mut GraphOutcome,
    ) {
        let Some(d) = &mut self.link_dialog else {
            return;
        };
        let title = |id: &NoteId| {
            project
                .note(id)
                .map_or_else(|| "?".to_owned(), |n| n.meta.title.clone())
        };
        let pack = project.pack();
        let mut close = false;
        let mut confirm = false;
        let mut remove = false;
        let resp = egui::Modal::new(egui::Id::unique("graph_link_dialog")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading(if d.existing.is_some() {
                "Modifier le lien"
            } else {
                "Nouveau lien"
            });
            ui.horizontal_wrapped(|ui| {
                ui.strong(title(&d.from));
                ui.label("»");
                ui.strong(title(&d.to));
                if d.existing.is_none() && ui.small_button("Inverser").clicked() {
                    std::mem::swap(&mut d.from, &mut d.to);
                }
            });
            ui.add_space(4.0);
            let label_of = |k: &Option<String>| match k {
                None => "Sans type".to_owned(),
                Some(k) => pack.link_type(k).map_or(k.clone(), |t| t.label.clone()),
            };
            egui::ComboBox::new("graph_link_type", "Type")
                .selected_text(label_of(&d.kind))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.kind, None, "Sans type");
                    for lt in &pack.link_types {
                        ui.selectable_value(
                            &mut d.kind,
                            Some(lt.id.clone()),
                            RichText::new(&lt.label).color(c32(lt.color)),
                        );
                    }
                });
            let note = ui.add_sized(
                [360.0, 22.0],
                egui::TextEdit::singleline(&mut d.note).hint_text("Note (facultative)"),
            );
            if std::mem::take(&mut d.focus) {
                note.request_focus();
            }
            if note.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                confirm = true;
            }
            if d.existing.is_some() && d.in_body {
                ui.label(
                    RichText::new(format!(
                        "Ce lien est écrit dans le texte de « {} » : changer son type modifie \
                         aussi le texte ; le supprimer garde le mot, sans les crochets.",
                        title(&d.from)
                    ))
                    .small()
                    .weak(),
                );
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let ok = if d.existing.is_some() {
                    "Enregistrer"
                } else {
                    "Créer le lien"
                };
                if ui.button(ok).clicked() {
                    confirm = true;
                }
                if d.existing.is_some()
                    && ui
                        .button(RichText::new("Supprimer").color(ui.visuals().error_fg_color))
                        .clicked()
                {
                    remove = true;
                }
                if ui.button("Annuler").clicked() {
                    close = true;
                }
            });
        });
        if resp.should_close() {
            close = true;
        }
        let note = Some(d.note.trim().to_owned()).filter(|n| !n.is_empty());
        if confirm {
            let op = match &d.existing {
                None => {
                    self.last_kind = Some(d.kind.clone());
                    LinkOp::Add {
                        from: d.from.clone(),
                        to: d.to.clone(),
                        kind: d.kind.clone(),
                        note,
                    }
                }
                Some(old) => LinkOp::Update {
                    from: d.from.clone(),
                    to: d.to.clone(),
                    old_kind: old.clone(),
                    kind: d.kind.clone(),
                    note,
                },
            };
            outcome.link_ops.push(op);
            close = true;
        } else if remove && let Some(old) = &d.existing {
            outcome.link_ops.push(LinkOp::Remove {
                from: d.from.clone(),
                to: d.to.clone(),
                kind: old.clone(),
            });
            close = true;
        }
        if close {
            self.link_dialog = None;
        }
    }

    /// Axe Z gradué (au centre de la disposition), derrière les nœuds.
    fn draw_z_axis(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        axis: &ZAxis,
        visuals: &egui::Visuals,
    ) {
        let projector = self.orbit.projector(v(rect.center()), rect.height());
        let at = |z: f32| projector.project(Vec3::new(0.0, 0.0, z));
        let color = visuals.weak_text_color().gamma_multiply(0.6);
        if let (Some(a), Some(b)) = (at(axis.z_of(axis.min)), at(axis.z_of(axis.max))) {
            painter.line_segment([p(a.screen), p(b.screen)], Stroke::new(1.0, color));
        }
        for &t in &axis.ticks {
            if let Some(pr) = at(axis.z_of(t)) {
                painter.circle_filled(p(pr.screen), 2.0, color);
                painter.text(
                    p(pr.screen) + egui::vec2(5.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{} {}", axis.label, fmt_num(t)),
                    FontId::proportional(10.0),
                    color,
                );
            }
        }
    }
}

/// Position épinglée à enregistrer dans la fiche.
fn pin_at(q: Vec3, z: f32) -> Pos {
    Pos {
        x: q.x,
        y: q.y,
        z,
        pinned: true,
    }
}

fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{x:.2}")
    }
}

/// Échelle de l'axe Z pour un champ : bornes des valeurs des fiches
/// affichées, graduations et étendue (liée au nombre de fiches).
fn z_axis(project: &Project, ids: &[NoteId], field: &str) -> Option<ZAxis> {
    let values: Vec<f64> = ids
        .iter()
        .filter_map(|id| project.note(id)?.meta.fields.get(field)?.as_f64())
        .filter(|x| x.is_finite())
        .collect();
    let min = values.iter().copied().reduce(f64::min)?;
    let max = values.iter().copied().reduce(f64::max)?;
    let integers = values.iter().all(|x| x.fract() == 0.0);
    let ticks = if integers && max - min <= 30.0 {
        (min as i64..=max as i64).map(|x| x as f64).collect()
    } else if max > min {
        vec![min, max]
    } else {
        vec![min]
    };
    let label = project
        .pack()
        .fields
        .iter()
        .find(|f| f.id == field)
        .map_or_else(|| field.to_owned(), |f| f.label.clone());
    Some(ZAxis {
        label,
        min,
        max,
        ticks,
        span: (60.0 * (ids.len() as f32).sqrt()).clamp(300.0, 1500.0),
    })
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
            // Moins de segments pour les petits disques (3D lointaine).
            let segments = ((r * 1.5) as usize).clamp(8, 48) / n;
            let segments = segments.max(3);
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
fn note_preview(ui: &mut egui::Ui, project: &Project, id: &NoteId, z_field: Option<&str>) {
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
    if let Some(field) = z_field {
        let label = pack
            .fields
            .iter()
            .find(|f| f.id == field)
            .map_or(field, |f| f.label.as_str());
        let value = note
            .meta
            .fields
            .get(field)
            .and_then(|v| v.as_f64())
            .map_or_else(|| "—".to_owned(), fmt_num);
        ui.label(RichText::new(format!("{label} : {value}")).small());
    }
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
        RichText::new(
            "Clic : sélectionner · double-clic : ouvrir · glisser : épingler · Ctrl+glisser : lier",
        )
        .small()
        .weak(),
    );
}
