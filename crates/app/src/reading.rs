//! Affichage mis en forme d'une fiche, toujours actif : seule l'unité en
//! cours de modification (ligne, tableau, bloc de code — voir
//! `enigma_core::live`) est montrée en texte Markdown par l'éditeur.
//!
//! Le découpage en unités et en blocs n'est refait que si le texte change.
//! Un clic sur une unité demande son édition à la position cliquée. Tirer le
//! coin d'une image change sa largeur (écrite dans le texte au
//! relâchement) ; tirer le coin d'un tableau ajoute ou retire des lignes et
//! des colonnes.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::path::Path;

use eframe::egui::{
    self, Color32, CursorIcon, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, UiBuilder, Vec2,
    text::{LayoutJob, TextFormat},
};
use enigma_core::Project;
use enigma_core::import::{is_remote, resolve_image};
use enigma_core::live::{self, Unit, UnitKind};
use enigma_core::render::{Block, ImageRef, Inline, InlineLink, blocks};
use enigma_core::table;

use crate::editor::MenuChoice;
use crate::highlight::BODY_SIZE;

/// Largeur minimale d'une image redimensionnée.
const MIN_WIDTH: f32 = 32.0;
/// Côté de la poignée de redimensionnement.
const HANDLE: f32 = 12.0;
/// Largeur minimale d'une cellule de tableau.
const CELL_MIN: Vec2 = Vec2::new(56.0, BODY_SIZE + 6.0);

/// Ce que l'utilisateur a demandé dans la fiche mise en forme.
#[derive(Debug)]
pub enum ViewAction {
    /// Clic sur `[[Titre]]`.
    OpenTitle(String),
    /// Nouvelle largeur d'une image (fin du glisser) ; plage dans la fiche.
    Resize { image: ImageRef, width: u32 },
    /// Ouvrir une pièce jointe avec l'application du système.
    OpenFile(std::path::PathBuf),
    /// Modifier le texte à cet octet de la fiche.
    EditAt(usize),
    /// Cocher/décocher la tâche de cette ligne.
    ToggleTask(Range<usize>),
    /// Nouvelles dimensions d'un tableau (lignes de données, colonnes).
    TableResize {
        range: Range<usize>,
        rows: usize,
        cols: usize,
    },
    /// Choix du menu clic droit, à cet octet.
    Menu(MenuChoice, usize),
}

/// Cellule de tableau prête à afficher.
pub struct CellView {
    /// Contenu dans la fiche (octets).
    range: Range<usize>,
    source: String,
    rendered: String,
    blocks: Vec<Block>,
}

pub enum Content {
    Line {
        /// Retrait (colonnes).
        indent: usize,
        /// Début du texte sans le retrait (octet de la fiche).
        start: usize,
        source: String,
        rendered: String,
        blocks: Vec<Block>,
    },
    Code {
        code: String,
        /// Octet du premier caractère du code.
        start: usize,
    },
    Table {
        cols: usize,
        /// Cellules par ligne de données (en-tête compris).
        rows: Vec<Vec<CellView>>,
    },
}

pub struct CachedUnit {
    pub unit: Unit,
    pub content: Content,
}

impl CachedUnit {
    /// Images de l'unité (plages dans la fiche).
    pub fn has_images(&self) -> bool {
        match &self.content {
            Content::Line { blocks, .. } => blocks.iter().any(|b| matches!(b, Block::Image(_))),
            _ => false,
        }
    }
}

/// Glissers en cours et position du dernier clic droit.
#[derive(Default)]
pub struct Drags {
    /// Image : (début de sa syntaxe, largeur).
    image: Option<(usize, f32)>,
    /// Tableau : (début, déplacement cumulé).
    table: Option<(usize, Vec2)>,
    menu_byte: usize,
}

#[derive(Default)]
pub struct Preview {
    key: Option<u64>,
    pub units: Vec<CachedUnit>,
    pub drags: Drags,
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

/// Blocs d'un morceau de texte, plages d'images décalées de `base`.
fn shifted_blocks(text: &str, base: usize) -> (Vec<Block>, String) {
    let mut out = blocks(text);
    let mut rendered = String::new();
    for b in &mut out {
        match b {
            Block::Image(img) => {
                img.range = img.range.start + base..img.range.end + base;
            }
            Block::Heading { spans, .. }
            | Block::Paragraph(spans)
            | Block::Quote(spans)
            | Block::ListItem { spans, .. } => {
                for s in spans {
                    rendered.push_str(&s.text);
                }
            }
            _ => {}
        }
    }
    (out, rendered)
}

fn build(body: &str) -> Vec<CachedUnit> {
    live::units(body)
        .into_iter()
        .map(|unit| {
            let text = &body[unit.range.clone()];
            let base = unit.range.start;
            let content = match unit.kind {
                UnitKind::Line => {
                    let (indent, lead) = live::indent(text);
                    let source = &text[lead..];
                    let (blocks, rendered) = shifted_blocks(source, base + lead);
                    Content::Line {
                        indent,
                        start: base + lead,
                        source: source.to_owned(),
                        rendered,
                        blocks,
                    }
                }
                UnitKind::Code => {
                    let lines = live::line_ranges(text);
                    let first = lines.first().map_or(0, |l| l.end + 1).min(text.len());
                    let last = lines
                        .last()
                        .filter(|l| lines.len() > 1 && text[(*l).clone()].trim().len() >= 3)
                        .map_or(text.len(), |l| l.start.saturating_sub(1).max(first));
                    Content::Code {
                        code: text[first..last.max(first)].to_owned(),
                        start: base + first,
                    }
                }
                UnitKind::Table => match table::parse(text) {
                    Some(p) => {
                        let cols = p.table.cols();
                        let rows = p
                            .cells
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| *i != 1)
                            .map(|(_, row)| {
                                row.iter()
                                    .map(|c| {
                                        let source = &text[c.content.clone()];
                                        let (blocks, rendered) =
                                            shifted_blocks(source, base + c.content.start);
                                        CellView {
                                            range: base + c.content.start..base + c.content.end,
                                            source: source.to_owned(),
                                            rendered,
                                            blocks,
                                        }
                                    })
                                    .collect()
                            })
                            .collect();
                        Content::Table { cols, rows }
                    }
                    None => Content::Code {
                        code: text.to_owned(),
                        start: base,
                    },
                },
            };
            CachedUnit { unit, content }
        })
        .collect()
}

impl Preview {
    /// Recalcule les unités si le texte a changé.
    pub fn sync(&mut self, body: &str) {
        let key = hash_text(body);
        if self.key != Some(key) {
            self.units = build(body);
            self.key = Some(key);
        }
    }

    pub fn unit_at(&self, b: usize) -> usize {
        self.units
            .iter()
            .position(|u| b <= u.unit.range.end)
            .unwrap_or(self.units.len().saturating_sub(1))
    }
}

/// Correspondance texte affiché → texte source, pour placer le curseur.
struct Src<'a> {
    source: &'a str,
    rendered: &'a str,
    /// Octet (dans la fiche) du début de `source`.
    base: usize,
    /// Caractères affichés avant ce morceau.
    offset: usize,
}

impl Src<'_> {
    fn byte(&self, n: usize) -> usize {
        self.base + live::source_offset(self.source, self.rendered, self.offset + n)
    }
}

pub type Menu<'a> = dyn FnMut(&mut egui::Ui) -> Option<MenuChoice> + 'a;

fn heading_size(level: u8) -> f32 {
    match level {
        1 => 26.0,
        2 => 22.0,
        3 => 19.0,
        4 => 17.0,
        _ => BODY_SIZE,
    }
}

fn spans_chars(spans: &[Inline]) -> usize {
    spans.iter().map(|s| s.text.chars().count()).sum()
}

impl Drags {
    /// Une unité mise en forme.
    pub fn unit_ui(
        &mut self,
        ui: &mut egui::Ui,
        cu: &CachedUnit,
        project: &Project,
        note_path: &Path,
        menu: &mut Menu<'_>,
    ) -> Option<ViewAction> {
        match &cu.content {
            Content::Line {
                indent,
                start,
                source,
                rendered,
                blocks,
            } => {
                if source.trim().is_empty() || blocks.is_empty() {
                    return self.raw_line_ui(ui, cu, source, *start, menu);
                }
                let src = Src {
                    source,
                    rendered,
                    base: *start,
                    offset: 0,
                };
                self.line_ui(ui, cu, *indent, blocks, src, project, note_path, menu)
            }
            Content::Code { code, start } => self.code_ui(ui, code, *start),
            Content::Table { cols, rows } => {
                self.table_ui(ui, &cu.unit.range, *cols, rows, project, note_path, menu)
            }
        }
    }

    /// Ligne vide, ou sans rendu (définition de lien…) : texte atténué.
    fn raw_line_ui(
        &mut self,
        ui: &mut egui::Ui,
        cu: &CachedUnit,
        source: &str,
        start: usize,
        menu: &mut Menu<'_>,
    ) -> Option<ViewAction> {
        let height = BODY_SIZE * 1.25;
        let resp = if source.trim().is_empty() {
            ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click())
                .1
        } else {
            ui.add(
                egui::Label::new(RichText::new(source).size(BODY_SIZE).weak())
                    .sense(Sense::click()),
            )
        };
        let resp = resp.on_hover_cursor(CursorIcon::Text);
        let mut action = None;
        if resp.secondary_clicked() {
            self.menu_byte = cu.unit.range.start;
        }
        let at = self.menu_byte;
        resp.context_menu(|ui| {
            if let Some(c) = menu(ui) {
                action = Some(ViewAction::Menu(c, at));
            }
        });
        if resp.clicked() {
            return Some(ViewAction::EditAt(start + source.len()));
        }
        action
    }

    #[allow(clippy::too_many_arguments)]
    fn line_ui(
        &mut self,
        ui: &mut egui::Ui,
        cu: &CachedUnit,
        indent: usize,
        blocks: &[Block],
        mut src: Src<'_>,
        project: &Project,
        note_path: &Path,
        menu: &mut Menu<'_>,
    ) -> Option<ViewAction> {
        let mut action = None;
        let only_images = blocks.iter().all(|b| match b {
            Block::Image(_) => true,
            Block::Paragraph(s) => s.iter().all(|x| x.text.trim().is_empty()),
            _ => false,
        });
        if only_images {
            // Plusieurs images sur une ligne : côte à côte.
            ui.horizontal_wrapped(|ui| {
                ui.add_space(9.0 * indent as f32);
                for b in blocks {
                    if let Block::Image(img) = b
                        && let Some(a) = self.image_ui(ui, img, project, note_path)
                    {
                        action = Some(a);
                    }
                }
            });
            return action;
        }
        for block in blocks {
            let a = match block {
                Block::Heading { level, spans } => {
                    ui.add_space(if *level <= 2 { 6.0 } else { 2.0 });
                    let strong = ui.visuals().strong_text_color();
                    let a = self.rich_text(
                        ui,
                        spans,
                        heading_size(*level),
                        Some(strong),
                        project,
                        &src,
                        menu,
                    );
                    if *level <= 2 {
                        let r = ui.max_rect();
                        let y = ui.cursor().top();
                        ui.painter().hline(
                            r.x_range(),
                            y - 1.0,
                            Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                        );
                        ui.add_space(2.0);
                    }
                    src.offset += spans_chars(spans);
                    a
                }
                Block::Paragraph(spans) => {
                    let a = ui
                        .horizontal_top(|ui| {
                            ui.add_space(9.0 * indent as f32);
                            self.rich_text(ui, spans, BODY_SIZE, None, project, &src, menu)
                        })
                        .inner;
                    src.offset += spans_chars(spans);
                    a
                }
                Block::ListItem {
                    marker,
                    task,
                    spans,
                    ..
                } => {
                    let a = ui
                        .horizontal_top(|ui| {
                            ui.add_space(6.0 + 9.0 * indent as f32);
                            let mut out = None;
                            match task {
                                Some(done) => {
                                    if task_box(ui, *done) {
                                        out = Some(ViewAction::ToggleTask(cu.unit.range.clone()));
                                    }
                                }
                                None => {
                                    ui.label(
                                        RichText::new(marker)
                                            .size(BODY_SIZE)
                                            .color(crate::highlight::LIST_MARKER),
                                    );
                                }
                            }
                            let color = task.filter(|d| *d).map(|_| ui.visuals().weak_text_color());
                            let a =
                                self.rich_text(ui, spans, BODY_SIZE, color, project, &src, menu);
                            out.or(a)
                        })
                        .inner;
                    src.offset += spans_chars(spans);
                    a
                }
                Block::Quote(spans) => {
                    let mut out = None;
                    let weak = ui.visuals().weak_text_color();
                    let frame = egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 4,
                            top: 1,
                            bottom: 1,
                        })
                        .show(ui, |ui| {
                            out = self.rich_text(
                                ui,
                                spans,
                                BODY_SIZE,
                                Some(weak),
                                project,
                                &src,
                                menu,
                            );
                        });
                    let r = frame.response.rect;
                    ui.painter().vline(
                        r.left() + 1.5,
                        r.y_range(),
                        Stroke::new(3.0, crate::theme::accent(ui.visuals())),
                    );
                    src.offset += spans_chars(spans);
                    out
                }
                Block::Code(code) => self.code_ui(ui, code, src.base),
                Block::Rule => {
                    let resp = ui
                        .allocate_exact_size(
                            Vec2::new(ui.available_width(), BODY_SIZE),
                            Sense::click(),
                        )
                        .1
                        .on_hover_cursor(CursorIcon::Text);
                    ui.painter().hline(
                        resp.rect.x_range(),
                        resp.rect.center().y,
                        Stroke::new(2.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                    );
                    resp.clicked()
                        .then(|| ViewAction::EditAt(src.base + src.source.len()))
                }
                Block::Image(img) => self.image_ui(ui, img, project, note_path),
                Block::Attachment { label, src: file } => {
                    attachment_ui(ui, label, file, project, note_path, src.base)
                }
            };
            if a.is_some() {
                action = a;
            }
        }
        action
    }

    fn code_ui(&mut self, ui: &mut egui::Ui, code: &str, start: usize) -> Option<ViewAction> {
        let visuals = ui.visuals().clone();
        let inner = ui.scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
            egui::Frame::new()
                .fill(visuals.code_bg_color)
                .stroke(visuals.widgets.noninteractive.bg_stroke)
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut job = LayoutJob::simple(
                        code.to_owned(),
                        FontId::monospace(13.5),
                        visuals.text_color(),
                        ui.available_width(),
                    );
                    job.wrap.max_width = ui.available_width();
                    let galley = ui.fonts_mut(|f| f.layout_job(job));
                    let (rect, _) = ui.allocate_exact_size(
                        galley.size().max(Vec2::new(0.0, 14.0)),
                        Sense::hover(),
                    );
                    ui.painter()
                        .galley(rect.min, galley.clone(), visuals.text_color());
                    (rect, galley)
                })
                .inner
        });
        let resp = inner.response.on_hover_cursor(CursorIcon::Text);
        if resp.clicked() {
            let (rect, galley) = inner.inner;
            let at = resp
                .interact_pointer_pos()
                .map_or(code.chars().count(), |p| {
                    galley.cursor_from_pos(p - rect.min).index.0
                });
            let byte = code.char_indices().nth(at).map_or(code.len(), |(b, _)| b);
            return Some(ViewAction::EditAt(start + byte));
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn table_ui(
        &mut self,
        ui: &mut egui::Ui,
        range: &Range<usize>,
        cols: usize,
        rows: &[Vec<CellView>],
        project: &Project,
        note_path: &Path,
        menu: &mut Menu<'_>,
    ) -> Option<ViewAction> {
        let mut action = None;
        let bg = ui.painter().add(Shape::Noop);
        let mut rects: Vec<Vec<Rect>> = Vec::with_capacity(rows.len());
        // Hauteur de chaque ligne à l'image précédente : toutes les cases
        // d'une ligne ont la même hauteur, contenu calé en haut.
        let heights_id = ui.scope_id().with(("table_rows", range.start));
        let heights: Vec<f32> = ui.data(|d| d.get_temp(heights_id)).unwrap_or_default();
        ui.add_space(2.0);
        egui::Grid::new(("table", range.start))
            .spacing([0.0, 0.0])
            .min_col_width(0.0)
            .min_row_height(0.0)
            .show(ui, |ui| {
                for (r, row) in rows.iter().enumerate() {
                    let mut line = Vec::with_capacity(cols);
                    for c in 0..cols {
                        let cell = row.get(c);
                        let min_h = heights.get(r).copied().unwrap_or(0.0);
                        let builder = UiBuilder::new()
                            .sense(Sense::click())
                            .layout(egui::Layout::top_down(egui::Align::Min));
                        let inner = ui.scope_builder(builder, |ui| {
                            egui::Frame::new()
                                .inner_margin(egui::Margin::symmetric(8, 5))
                                .show(ui, |ui| {
                                    ui.set_min_size(CELL_MIN.max(Vec2::new(0.0, min_h - 10.0)));
                                    let cell = cell?;
                                    let strong = (r == 0).then(|| ui.visuals().strong_text_color());
                                    self.cell_ui(ui, cell, strong, project, note_path, menu)
                                })
                        });
                        let resp = inner.response.on_hover_cursor(CursorIcon::Text);
                        if let Some(a) = inner.inner.inner {
                            action = Some(a);
                        } else if resp.clicked() {
                            let at = cell.map_or(range.end, |c| c.range.end);
                            action = Some(ViewAction::EditAt(at));
                        }
                        line.push(inner.inner.response.rect);
                    }
                    ui.end_row();
                    rects.push(line);
                }
            });

        // Quadrillage : en-tête teinté, toutes les cases visibles.
        let (Some(first), Some(last)) = (rects.first(), rects.last()) else {
            return action;
        };
        let mut xs: Vec<f32> = (0..cols)
            .map(|c| {
                rects
                    .iter()
                    .filter_map(|r| r.get(c))
                    .map(|r| r.left())
                    .fold(f32::INFINITY, f32::min)
            })
            .collect();
        xs.push(
            rects
                .iter()
                .filter_map(|r| r.last())
                .map(|r| r.right())
                .fold(f32::NEG_INFINITY, f32::max),
        );
        let mut ys: Vec<f32> = rects
            .iter()
            .map(|r| r.iter().map(|x| x.top()).fold(f32::INFINITY, f32::min))
            .collect();
        ys.push(
            last.iter()
                .map(|x| x.bottom())
                .fold(f32::NEG_INFINITY, f32::max),
        );
        let _ = first;
        let measured: Vec<f32> = rects
            .iter()
            .map(|r| r.iter().map(|x| x.height()).fold(0.0, f32::max))
            .collect();
        if measured != heights {
            ui.data_mut(|d| d.insert_temp(heights_id, measured));
            ui.ctx().request_repaint();
        }
        let (x0, x1, y0, y1) = (xs[0], xs[cols], ys[0], ys[ys.len() - 1]);
        let outer = Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1));
        let visuals = ui.visuals().clone();
        let stroke = Stroke::new(
            1.0,
            visuals.widgets.inactive.bg_stroke.color.gamma_multiply(1.4),
        );
        let mut shapes = vec![Shape::rect_filled(
            Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, ys[1])),
            0.0,
            visuals.widgets.inactive.weak_bg_fill,
        )];
        for &x in &xs {
            shapes.push(Shape::line_segment(
                [Pos2::new(x, y0), Pos2::new(x, y1)],
                stroke,
            ));
        }
        for &y in &ys {
            shapes.push(Shape::line_segment(
                [Pos2::new(x0, y), Pos2::new(x1, y)],
                stroke,
            ));
        }
        ui.painter().set(bg, Shape::Vec(shapes));

        // Poignée : tirer pour ajouter/retirer des colonnes et des lignes.
        let handle_rect = Rect::from_center_size(outer.right_bottom(), Vec2::splat(HANDLE));
        let id = ui.scope_id().with(("table_handle", range.start));
        let handle = ui
            .interact(handle_rect, id, Sense::drag())
            .on_hover_cursor(CursorIcon::ResizeNwSe)
            .on_hover_text("Glisser pour ajouter ou retirer des colonnes et des lignes");
        let hovered = ui.rect_contains_pointer(outer.expand(HANDLE));
        let accent = crate::theme::accent(&visuals);
        let body_rows = rows.len().saturating_sub(1);
        if handle.drag_started() {
            self.table = Some((range.start, Vec2::ZERO));
        }
        let drag = self
            .table
            .filter(|(s, _)| *s == range.start)
            .map(|(_, d)| d);
        if hovered || drag.is_some() {
            ui.painter().rect_filled(handle_rect, 0.0, accent);
            ui.painter().rect_stroke(
                handle_rect,
                0.0,
                Stroke::new(1.0, visuals.extreme_bg_color),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(mut delta) = drag {
            delta += handle.drag_delta();
            self.table = Some((range.start, delta));
            let cw = outer.width() / cols as f32;
            let rh = outer.height() / rows.len().max(1) as f32;
            let new_cols = ((cols as f32 + delta.x / cw).round() as isize).max(1) as usize;
            let new_rows = ((body_rows as f32 + delta.y / rh).round() as isize).max(1) as usize;
            let ghost = Rect::from_min_size(
                outer.min,
                Vec2::new(cw * new_cols as f32, rh * (new_rows + 1) as f32),
            );
            ui.painter().rect_stroke(
                ghost,
                0.0,
                Stroke::new(1.5, accent),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                ghost.right_bottom() + Vec2::new(8.0, 4.0),
                egui::Align2::LEFT_TOP,
                format!("{new_cols} × {new_rows}"),
                FontId::proportional(12.0),
                accent,
            );
            ui.ctx().request_repaint();
            if handle.drag_stopped() {
                self.table = None;
                if (new_cols, new_rows) != (cols, body_rows) {
                    return Some(ViewAction::TableResize {
                        range: range.clone(),
                        rows: new_rows,
                        cols: new_cols,
                    });
                }
            }
        }
        ui.add_space(4.0);
        action
    }

    fn cell_ui(
        &mut self,
        ui: &mut egui::Ui,
        cell: &CellView,
        color: Option<Color32>,
        project: &Project,
        note_path: &Path,
        menu: &mut Menu<'_>,
    ) -> Option<ViewAction> {
        let mut action = None;
        let mut src = Src {
            source: &cell.source,
            rendered: &cell.rendered,
            base: cell.range.start,
            offset: 0,
        };
        ui.horizontal_wrapped(|ui| {
            for b in &cell.blocks {
                let a = match b {
                    Block::Image(img) => self.image_ui(ui, img, project, note_path),
                    Block::Heading { spans, .. }
                    | Block::Paragraph(spans)
                    | Block::Quote(spans)
                    | Block::ListItem { spans, .. } => {
                        let a = self.rich_text(ui, spans, BODY_SIZE, color, project, &src, menu);
                        src.offset += spans_chars(spans);
                        a
                    }
                    Block::Attachment { label, src: file } => {
                        attachment_ui(ui, label, file, project, note_path, cell.range.start)
                    }
                    Block::Code(code) => {
                        ui.label(RichText::new(code).monospace());
                        None
                    }
                    Block::Rule => None,
                };
                if a.is_some() {
                    action = a;
                }
            }
        });
        action
    }

    /// Image avec poignée de redimensionnement au coin bas-droit.
    pub fn image_ui(
        &mut self,
        ui: &mut egui::Ui,
        img: &ImageRef,
        project: &Project,
        note_path: &Path,
    ) -> Option<ViewAction> {
        let Some(path) = resolve_image(project.root(), note_path, &img.src) else {
            let msg = if is_remote(&img.src) {
                format!("Image en ligne non affichée : {}", img.src)
            } else {
                format!("Image introuvable : {}", img.src)
            };
            let r = ui.add(
                egui::Label::new(RichText::new(msg).color(ui.visuals().warn_fg_color))
                    .sense(Sense::click()),
            );
            return r.clicked().then_some(ViewAction::EditAt(img.range.end));
        };
        let uri = file_uri(&path);
        let available = ui.available_width().max(MIN_WIDTH);
        let max_w = ui.clip_rect().width().max(MIN_WIDTH);
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
        let wanted = match self.image {
            Some((start, w)) if start == img.range.start => w,
            _ => img.width.map_or(natural.x, |w| w as f32),
        };
        let width = wanted.clamp(MIN_WIDTH, max_w);
        let size = Vec2::new(width, width * natural.y / natural.x);
        let resp = ui.add(
            egui::Image::new(uri)
                .fit_to_exact_size(size)
                .alt_text(img.alt.clone())
                .sense(Sense::click()),
        );
        if !img.alt.is_empty() {
            resp.clone().on_hover_text(&img.alt);
        }

        // Poignée de redimensionnement (coin bas-droit, posée sur le coin).
        let handle_rect = Rect::from_min_size(
            resp.rect.right_bottom() - Vec2::splat(HANDLE),
            Vec2::splat(HANDLE),
        );
        let id = ui.scope_id().with(("resize", img.range.start));
        let handle = ui
            .interact(handle_rect, id, Sense::drag())
            .on_hover_cursor(CursorIcon::ResizeNwSe)
            .on_hover_text("Glisser pour redimensionner");
        let dragging = self.image.is_some_and(|(s, _)| s == img.range.start);
        let hot = handle.hovered() || dragging || resp.hovered();
        if hot {
            let accent = crate::theme::accent(ui.visuals());
            let painter = ui.painter();
            painter.rect_stroke(
                resp.rect,
                0.0,
                Stroke::new(1.0, accent),
                egui::StrokeKind::Inside,
            );
            painter.rect_filled(handle_rect, 0.0, accent);
            painter.rect_stroke(
                handle_rect,
                0.0,
                Stroke::new(1.0, ui.visuals().extreme_bg_color),
                egui::StrokeKind::Inside,
            );
            let label = format!("{} × {} px", size.x.round(), size.y.round());
            let pos = resp.rect.left_bottom() + Vec2::new(4.0, -4.0);
            let galley = ui.painter().layout_no_wrap(
                label,
                FontId::proportional(11.0),
                ui.visuals().strong_text_color(),
            );
            let bg = Rect::from_min_size(pos - Vec2::new(0.0, galley.size().y), galley.size())
                .expand(2.0);
            ui.painter()
                .rect_filled(bg, 0.0, ui.visuals().extreme_bg_color.gamma_multiply(0.85));
            ui.painter()
                .galley(bg.min + Vec2::splat(2.0), galley, Color32::WHITE);
        }
        if handle.drag_started() {
            self.image = Some((img.range.start, width));
        }
        if handle.dragged() {
            let new_w = (self.image.map_or(width, |(_, w)| w) + handle.drag_delta().x)
                .clamp(MIN_WIDTH, max_w);
            self.image = Some((img.range.start, new_w));
            ui.ctx().request_repaint();
        }
        if handle.drag_stopped()
            && let Some((start, w)) = self.image.take()
            && start == img.range.start
        {
            return Some(ViewAction::Resize {
                image: img.clone(),
                width: w.round() as u32,
            });
        }
        if resp.clicked() {
            return Some(ViewAction::EditAt(img.range.end));
        }
        None
    }

    /// Paragraphe mis en forme ; un clic sur un lien l'ouvre, ailleurs il
    /// demande l'édition à cet endroit.
    #[allow(clippy::too_many_arguments)]
    fn rich_text(
        &mut self,
        ui: &mut egui::Ui,
        spans: &[Inline],
        size: f32,
        color: Option<Color32>,
        project: &Project,
        src: &Src<'_>,
        menu: &mut Menu<'_>,
    ) -> Option<ViewAction> {
        let visuals = ui.visuals().clone();
        let base = color.unwrap_or(visuals.text_color());
        let mut job = LayoutJob::default();
        // Plages (en caractères) des morceaux cliquables.
        let mut links: Vec<(Range<usize>, &InlineLink)> = Vec::new();
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
                    InlineLink::Wiki { .. } => crate::colors::WIKI_LINK,
                    InlineLink::Url(_) => visuals.hyperlink_color,
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
        let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::click());
        ui.painter().galley(rect.min, galley.clone(), base);
        let index_at = |pos: Pos2| galley.cursor_from_pos(pos - rect.min).index.0;
        let link_at = |i: usize| links.iter().find(|(r, _)| r.contains(&i)).map(|(_, l)| *l);
        match resp.hover_pos().and_then(|p| link_at(index_at(p))) {
            Some(link) => {
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
            None if resp.hovered() => ui.ctx().set_cursor_icon(CursorIcon::Text),
            None => {}
        }
        let mut action = None;
        if resp.secondary_clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.menu_byte = src.byte(index_at(p));
        }
        let at = self.menu_byte;
        resp.context_menu(|ui| {
            if let Some(c) = menu(ui) {
                action = Some(ViewAction::Menu(c, at));
            }
        });
        if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let i = index_at(pos);
            return Some(match link_at(i) {
                Some(InlineLink::Wiki { target, .. }) => ViewAction::OpenTitle(target.clone()),
                Some(InlineLink::Url(u)) => {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(u));
                    return None;
                }
                None => ViewAction::EditAt(src.byte(i)),
            });
        }
        action
    }
}

/// Case à cocher d'une tâche ; `true` si elle a été cliquée.
fn task_box(ui: &mut egui::Ui, done: bool) -> bool {
    let side = BODY_SIZE - 2.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(side, BODY_SIZE + 2.0), Sense::click());
    let r = Rect::from_center_size(rect.center(), Vec2::splat(side - 2.0));
    let v = ui.visuals();
    let accent = crate::theme::accent(v);
    let stroke = if resp.hovered() {
        Stroke::new(1.5, accent)
    } else {
        Stroke::new(1.0, v.widgets.inactive.fg_stroke.color)
    };
    if done {
        ui.painter().rect_filled(r, 0.0, accent);
        let a = r.left_center() + Vec2::new(2.5, 0.0);
        let b = r.center_bottom() + Vec2::new(-1.0, -3.0);
        let c = r.right_top() + Vec2::new(-2.5, 3.0);
        ui.painter()
            .line(vec![a, b, c], Stroke::new(1.8, v.extreme_bg_color));
    } else {
        ui.painter()
            .rect_stroke(r, 0.0, stroke, egui::StrokeKind::Inside);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(if done { "Décocher" } else { "Cocher" })
        .clicked()
}

/// Pièce jointe (son, vidéo, document) : carte avec bouton « Ouvrir ».
fn attachment_ui(
    ui: &mut egui::Ui,
    label: &str,
    src: &str,
    project: &Project,
    note_path: &Path,
    edit_at: usize,
) -> Option<ViewAction> {
    let path = resolve_image(project.root(), note_path, src);
    let kind = enigma_core::import::file_kind(Path::new(src));
    let mut action = None;
    let inner = ui.scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(kind.label()).strong());
                ui.label(if label.is_empty() { src } else { label });
                match &path {
                    Some(p) => {
                        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                        ui.weak(human_size(size));
                        let verb = match kind {
                            enigma_core::import::FileKind::Audio
                            | enigma_core::import::FileKind::Video => "Lire",
                            _ => "Ouvrir",
                        };
                        if ui
                            .button(verb)
                            .on_hover_text("Ouvrir avec l'application du système")
                            .clicked()
                        {
                            action = Some(ViewAction::OpenFile(p.clone()));
                        }
                    }
                    None => {
                        ui.colored_label(ui.visuals().warn_fg_color, "fichier introuvable");
                    }
                }
            });
        });
    });
    if action.is_none() && inner.response.clicked() {
        action = Some(ViewAction::EditAt(edit_at + 1));
    }
    action
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} Mo", b as f64 / f64::from(1u32 << 20)),
        b if b >= 1 << 10 => format!("{} Ko", b >> 10),
        b => format!("{b} o"),
    }
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

    #[test]
    fn units_are_cached_with_shifted_image_ranges() {
        let body = "Intro\n| ![a|80](x.png) | b |\n|---|---|\n|  |  |";
        let mut p = Preview::default();
        p.sync(body);
        assert_eq!(p.units.len(), 2);
        let Content::Table { cols, rows } = &p.units[1].content else {
            panic!("tableau attendu");
        };
        assert_eq!((*cols, rows.len()), (2, 2));
        let Some(Block::Image(img)) = rows[0][0].blocks.first() else {
            panic!("image attendue");
        };
        assert_eq!(&body[img.range.clone()], "![a|80](x.png)");
        assert!(rows[1][0].source.is_empty());
        let code = "```\nfn a() {}\n```";
        p.sync(code);
        let Content::Code { code: c, start } = &p.units[0].content else {
            panic!("code attendu");
        };
        assert_eq!((c.as_str(), *start), ("fn a() {}", 4));
    }
}
