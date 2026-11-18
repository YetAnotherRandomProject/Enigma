//! Un projet ouvert dans l'interface : liste des fiches, éditeur, dialogues,
//! sauvegarde automatique et surveillance du dossier.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, Sense};
use enigma_core::board::{BOARD_FILE, Board};
use enigma_core::filter::NoteFilter;
use enigma_core::links::{LinkEnd, ResolvedLink};
use enigma_core::project::FileChange;
use enigma_core::shortcut::Keymap;
use enigma_core::{NoteId, Project};

use crate::board_view::BoardView;
use crate::colors::c32;
use crate::commands::{self, Command};
use crate::editor::Editor;
use crate::filter_ui;
use crate::graph_view::{GraphOutcome, GraphView, LinkOp};
use crate::notices::Notices;
use crate::pack_editor::{self, PackEditor};
use crate::palette::{Palette, PaletteItem, PaletteOutcome};
use crate::shortcuts_ui::ShortcutEditor;
use crate::watcher::FsWatcher;

/// Délai après la dernière modification avant la sauvegarde automatique.
const AUTOSAVE_DELAY: Duration = Duration::from_millis(1000);

/// Capture d'écran reçue (remplie par le rappel d'egui).
type ScreenshotSlot = std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<egui::ColorImage>>>>;

/// Export PNG en cours : fichier visé et capture demandée.
struct PendingPng {
    path: PathBuf,
    slot: Option<ScreenshotSlot>,
}

/// Demande adressée à l'application (hors du projet ouvert).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppRequest {
    /// Choisir et ouvrir un autre projet (`.enigma`).
    OpenProject,
    /// Créer un nouveau projet.
    NewProject,
    /// Convertir un dossier de fiches `.md` en projet.
    ImportFolder,
    /// Associer les fichiers `.enigma` à l'application.
    AssociateFiles,
}

/// Vue de la zone centrale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Note,
    Graph,
    Board,
}

enum Dialog {
    NewNote {
        title: String,
        /// Identifiant du modèle du pack (`None` : fiche vide).
        template: Option<String>,
        error: Option<String>,
        focus: bool,
    },
    ConfirmDelete(NoteId),
}

/// Pack proposé dans le sélecteur.
struct PackChoice {
    id: String,
    name: String,
}

pub struct Workspace {
    pub project: Project,
    watcher: Option<FsWatcher>,
    packs: Vec<PackChoice>,
    filter: NoteFilter,
    list: Vec<NoteId>,
    list_key: Option<(NoteFilter, u64, u64)>,
    editor: Option<Editor>,
    dialog: Option<Dialog>,
    last_edit: Option<Instant>,
    focus_search: bool,
    pack_editor: Option<PackEditor>,
    pub view: View,
    graph: GraphView,
    /// Dossier des packs de l'utilisateur (pour tous les projets).
    user_packs_dir: Option<PathBuf>,
    keymap: Keymap,
    /// Fichier des raccourcis (`None` : non enregistrés).
    keymap_path: Option<PathBuf>,
    palette: Option<Palette>,
    shortcut_editor: Option<ShortcutEditor>,
    board: Board,
    /// Le fichier du tableau est illisible : on ne l'écrase pas.
    board_broken: bool,
    board_view: BoardView,
    /// Export PNG en attente de la capture d'écran.
    pending_png: Option<PendingPng>,
    requests: Vec<AppRequest>,
    /// Un fichier du projet a changé sur le disque depuis le dernier
    /// enregistrement du fichier `.enigma`.
    disk_changed: bool,
    /// Mode Lecture (fiche mise en forme) plutôt qu'Édition.
    pub reading: bool,
    /// Horloge de la dernière image (historique de l'éditeur).
    now_ms: u64,
}

impl Workspace {
    pub fn open(
        root: &Path,
        pack_roots: &[PathBuf],
        ctx: &egui::Context,
        notices: &mut Notices,
    ) -> Result<Workspace, enigma_core::project::ProjectError> {
        let project = Project::open(root, pack_roots)?;
        let watcher = match FsWatcher::new(root, ctx.clone()) {
            Ok(w) => Some(w),
            Err(e) => {
                notices.error(format!(
                    "Surveillance du dossier indisponible (les modifications externes ne seront pas vues) : {e}"
                ));
                None
            }
        };
        if let Some(issue) = project.pack_issue() {
            notices.error(format!("Pack de jeu : {issue}"));
        }
        if !project.issues().is_empty() {
            notices.error(format!(
                "{} fichier(s) illisible(s) : voir le bas de la liste.",
                project.issues().len()
            ));
        }
        let board_path = project
            .root()
            .join(enigma_core::project::META_DIR)
            .join(BOARD_FILE);
        let (board, board_broken) = match Board::load(&board_path) {
            Ok(b) => (b, false),
            Err(e) => {
                notices.error(format!("{e} (le tableau ne sera pas enregistré)"));
                (Board::default(), true)
            }
        };
        let mut ws = Workspace {
            project,
            watcher,
            packs: Vec::new(),
            filter: NoteFilter::default(),
            list: Vec::new(),
            list_key: None,
            editor: None,
            dialog: None,
            last_edit: None,
            focus_search: false,
            pack_editor: None,
            view: View::Note,
            graph: GraphView::default(),
            user_packs_dir: pack_roots.first().cloned(),
            keymap: Keymap::default(),
            keymap_path: None,
            palette: None,
            shortcut_editor: None,
            board,
            board_broken,
            board_view: BoardView::default(),
            pending_png: None,
            requests: Vec::new(),
            disk_changed: false,
            reading: false,
            now_ms: 0,
        };
        ws.refresh_packs();
        Ok(ws)
    }

    /// Raccourcis de l'utilisateur et fichier où les enregistrer.
    pub fn set_keymap(&mut self, keymap: Keymap, path: Option<PathBuf>) {
        self.keymap = keymap;
        self.keymap_path = path;
    }

    /// Le projet a-t-il changé sur le disque depuis le dernier appel ?
    pub fn take_disk_changed(&mut self) -> bool {
        std::mem::take(&mut self.disk_changed)
    }

    /// Demandes à traiter par l'application.
    pub fn take_requests(&mut self) -> Vec<AppRequest> {
        std::mem::take(&mut self.requests)
    }

    fn board_path(&self) -> PathBuf {
        self.project
            .root()
            .join(enigma_core::project::META_DIR)
            .join(BOARD_FILE)
    }

    fn save_board(&mut self, notices: &mut Notices) {
        if self.board_broken {
            notices.error(
                "Le fichier du tableau d'enquête est illisible : modifications non enregistrées.",
            );
            return;
        }
        match self.board.save(&self.board_path()) {
            Ok(()) => self.disk_changed = true,
            Err(e) => notices.error(e.to_string()),
        }
    }

    /// Importe des fiches `.md` (et leurs images) dans le projet.
    pub fn import_notes(&mut self, files: &[PathBuf], notices: &mut Notices) {
        match enigma_core::import::import_files(self.project.root(), files) {
            Ok(report) => {
                for path in &report.notes {
                    self.project.on_file_changed(path);
                }
                self.disk_changed = true;
                let mut msg = format!(
                    "{} fiche(s) importée(s), {} image(s) copiée(s).",
                    report.notes.len(),
                    report.images
                );
                if !report.missing.is_empty() {
                    msg.push_str(&format!(
                        " Images introuvables : {}.",
                        report.missing.join(", ")
                    ));
                }
                notices.info(msg);
                if let Some(id) = report
                    .notes
                    .first()
                    .and_then(|p| self.project.id_for_path(p))
                    .cloned()
                {
                    self.select(&id, notices);
                }
            }
            Err(e) => notices.error(format!("Import impossible : {e}")),
        }
    }

    /// Copie une image dans le projet et l'insère dans la fiche ouverte.
    pub fn insert_image(&mut self, file: &Path, notices: &mut Notices) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        match enigma_core::import::add_asset(self.project.root(), file) {
            Ok(rel) => {
                let alt = file
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let md = enigma_core::render::image_markdown(&alt, &rel);
                editor.insert_text(&md, true, &mut self.project, self.now_ms);
                self.disk_changed = true;
                self.mark_edited();
            }
            Err(e) => notices.error(format!("Image non insérée : {e}")),
        }
    }

    /// Fichiers glissés sur la fenêtre : images insérées dans la fiche
    /// ouverte, fiches `.md` importées.
    fn handle_dropped_files(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_owned())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        let (md, other): (Vec<PathBuf>, Vec<PathBuf>) = dropped
            .into_iter()
            .partition(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")));
        if !md.is_empty() {
            self.import_notes(&md, notices);
        }
        for file in other {
            if !enigma_core::import::is_image_path(&file) {
                notices.error(format!(
                    "« {} » n'est ni une image ni une fiche .md.",
                    file.display()
                ));
            } else if self.editor.is_none() {
                notices.info("Ouvrez d'abord une fiche pour y déposer une image.");
            } else {
                self.insert_image(&file, notices);
            }
        }
    }

    /// Une commande est-elle utilisable maintenant ?
    fn command_available(&self, cmd: Command) -> bool {
        match cmd {
            Command::AddToBoard => self
                .editor
                .as_ref()
                .is_some_and(|e| !self.board.contains(&e.id)),
            Command::RemoveFromBoard => self
                .editor
                .as_ref()
                .is_some_and(|e| self.board.contains(&e.id)),
            Command::DeleteNote | Command::Undo | Command::Redo | Command::InsertImage => {
                self.editor.is_some()
            }
            // Association de fichiers : Windows seulement pour l'instant.
            Command::AssociateFiles => cfg!(windows),
            Command::EditPack => !self.project.pack().id.is_empty(),
            _ => true,
        }
    }

    /// Exécute une commande (raccourci, palette ou menu).
    pub fn run_command(&mut self, cmd: Command, ctx: &egui::Context, notices: &mut Notices) {
        match cmd {
            Command::Palette => {
                self.palette = match self.palette {
                    Some(_) => None,
                    None => Some(Palette::default()),
                };
            }
            Command::NewNote => self.open_new_note_dialog(),
            Command::Save => self.flush(notices),
            Command::Undo | Command::Redo => {
                if let Some(ed) = &mut self.editor {
                    let changed = if cmd == Command::Undo {
                        ed.undo(&mut self.project)
                    } else {
                        ed.redo(&mut self.project)
                    };
                    if changed {
                        self.mark_edited();
                    }
                }
            }
            Command::Find => {
                self.view = View::Note;
                self.focus_search = true;
            }
            Command::ShowNote => self.view = View::Note,
            Command::ShowGraph => self.view = View::Graph,
            Command::ShowBoard => self.view = View::Board,
            Command::ToggleGraph => {
                self.view = match self.view {
                    View::Graph => View::Note,
                    View::Note | View::Board => View::Graph,
                };
            }
            Command::Graph2D | Command::Graph3D => {
                self.view = View::Graph;
                self.graph.set_mode(if cmd == Command::Graph2D {
                    crate::graph_view::Mode::Flat
                } else {
                    crate::graph_view::Mode::Space
                });
            }
            Command::GraphRecenter => {
                self.view = View::Graph;
                self.graph.recenter();
            }
            Command::ExportSvg => self.export_svg(ctx, notices),
            Command::ExportPng => self.export_png(notices),
            Command::AddToBoard => {
                if let Some(id) = self.editor.as_ref().map(|e| e.id.clone())
                    && self.board_view.add_card(&mut self.board, &id)
                {
                    self.save_board(notices);
                    self.view = View::Board;
                }
            }
            Command::RemoveFromBoard => {
                if let Some(id) = self.editor.as_ref().map(|e| e.id.clone())
                    && self.board.remove(&id)
                {
                    self.save_board(notices);
                }
            }
            Command::ToggleTheme
            | Command::ThemeDark
            | Command::ThemeLight
            | Command::ThemeSystem => commands::apply_theme(ctx, cmd),
            Command::Shortcuts => self.shortcut_editor = Some(ShortcutEditor::default()),
            Command::EditPack => {
                if !self.project.pack().id.is_empty() {
                    self.pack_editor = Some(PackEditor::edit_current(
                        &self.project,
                        self.user_packs_dir.as_deref(),
                    ));
                }
            }
            Command::OpenProject => self.requests.push(AppRequest::OpenProject),
            Command::NewProject => self.requests.push(AppRequest::NewProject),
            Command::ImportFolder => self.requests.push(AppRequest::ImportFolder),
            Command::AssociateFiles => self.requests.push(AppRequest::AssociateFiles),
            Command::ImportNotes => {
                if let Some(files) = rfd::FileDialog::new()
                    .set_title("Importer des fiches .md")
                    .add_filter("Fiches Markdown", &["md"])
                    .pick_files()
                {
                    self.import_notes(&files, notices);
                }
            }
            Command::InsertImage => {
                if self.editor.is_none() {
                    notices.info("Ouvrez d'abord une fiche pour y insérer une image.");
                } else if let Some(file) = rfd::FileDialog::new()
                    .set_title("Insérer une image")
                    .add_filter("Images", enigma_core::import::IMAGE_EXTENSIONS)
                    .pick_file()
                {
                    self.insert_image(&file, notices);
                }
            }
            Command::ToggleReading => {
                self.reading = !self.reading;
                self.view = View::Note;
            }
            Command::DeleteNote => {
                if let Some(ed) = &self.editor {
                    self.dialog = Some(Dialog::ConfirmDelete(ed.id.clone()));
                }
            }
        }
    }

    /// Écrit le graphe en SVG dans `path`.
    fn write_svg(&mut self, ctx: &egui::Context, path: &Path) -> Result<(), String> {
        let visuals = ctx.global_style().visuals.clone();
        let svg = self
            .graph
            .export_svg(&self.project, &visuals)
            .ok_or_else(|| {
                "affichez d'abord le graphe (il est vide ou pas encore dessiné)".to_owned()
            })?;
        enigma_core::storage::write_atomic(path, svg.as_bytes()).map_err(|e| e.to_string())
    }

    fn export_svg(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        if !self.graph.has_frame() {
            self.view = View::Graph;
            notices.info("Le graphe est maintenant affiché : relancez l'export.");
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .set_title("Exporter le graphe en SVG")
            .add_filter("Image SVG", &["svg"])
            .set_file_name("graphe.svg")
            .save_file()
        else {
            return;
        };
        match self.write_svg(ctx, &path) {
            Ok(()) => notices.info(format!("Graphe exporté : {}", path.display())),
            Err(e) => notices.error(format!("Export impossible : {e}")),
        }
    }

    fn export_png(&mut self, notices: &mut Notices) {
        self.view = View::Graph;
        if !self.graph.has_frame() {
            notices.info("Le graphe est maintenant affiché : relancez l'export.");
            return;
        }
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Exporter le graphe en PNG")
            .add_filter("Image PNG", &["png"])
            .set_file_name("graphe.png")
            .save_file()
        {
            // La capture est demandée après la prochaine image (palette fermée).
            self.pending_png = Some(PendingPng { path, slot: None });
        }
    }

    /// Reçoit la capture d'écran demandée pour l'export PNG.
    fn handle_screenshot(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        let Some(pending) = &mut self.pending_png else {
            return;
        };
        let Some(slot) = &pending.slot else {
            // Première image après la commande : la palette est fermée, on
            // demande la capture de ce qui est affiché.
            let slot = ScreenshotSlot::default();
            let (target, repaint) = (slot.clone(), ctx.clone());
            ctx.request_screenshot(move |image| {
                if let Ok(mut s) = target.lock() {
                    *s = Some(image);
                }
                repaint.request_repaint();
            });
            pending.slot = Some(slot);
            return;
        };
        let image = slot.lock().ok().and_then(|mut s| s.take());
        let Some(image) = image else {
            return;
        };
        let path = pending.path.clone();
        self.pending_png = None;
        let Some(rect) = self.graph.canvas_rect() else {
            return;
        };
        let ppp = ctx.pixels_per_point();
        let px = egui::Rect::from_min_max(
            (rect.min.to_vec2() * ppp).to_pos2(),
            (rect.max.to_vec2() * ppp).to_pos2(),
        );
        let result = crate::graph_export::encode_png_region(&image, px).and_then(|bytes| {
            enigma_core::storage::write_atomic(&path, &bytes).map_err(|e| e.to_string())
        });
        match result {
            Ok(()) => notices.info(format!("Graphe exporté : {}", path.display())),
            Err(e) => notices.error(format!("Export PNG impossible : {e}")),
        }
    }

    /// Enregistre toutes les modifications en attente.
    pub fn flush(&mut self, notices: &mut Notices) {
        if !self.project.dirty_ids().is_empty() {
            self.disk_changed = true;
        }
        for (id, err) in self.project.save_dirty() {
            let title = self
                .project
                .note(&id)
                .map(|n| n.meta.title.clone())
                .unwrap_or_default();
            notices.error(format!("Sauvegarde de « {title} » impossible : {err}"));
        }
        self.last_edit = None;
    }

    fn refresh_packs(&mut self) {
        self.packs = self
            .project
            .available_packs()
            .into_iter()
            .filter_map(|d| match d.result {
                Ok(p) => Some(PackChoice {
                    id: p.id,
                    name: p.name,
                }),
                Err(e) => {
                    tracing::warn!("pack ignoré : {e}");
                    None
                }
            })
            .collect();
    }

    fn select(&mut self, id: &NoteId, notices: &mut Notices) {
        if self.editor.as_ref().is_some_and(|e| &e.id == id) {
            return;
        }
        self.flush(notices);
        self.editor = Editor::open(&self.project, id);
    }

    fn mark_edited(&mut self) {
        self.last_edit = Some(Instant::now());
    }

    /// Sélecteur de pack de jeu (barre du haut).
    pub fn pack_selector_ui(&mut self, ui: &mut egui::Ui, notices: &mut Notices) {
        let current = self.project.config().pack.clone();
        let label = match (&current, self.project.pack_issue()) {
            (None, _) => "Aucun pack".to_owned(),
            (Some(id), Some(_)) => format!("{id} (introuvable)"),
            (Some(_), None) => self.project.pack().name.clone(),
        };
        let mut chosen: Option<Option<String>> = None;
        let mut refresh = false;
        let mut new_pack: Option<bool> = None;
        egui::ComboBox::new("pack", "")
            .selected_text(format!("Pack : {label}"))
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(current.is_none(), "Aucun pack")
                    .clicked()
                {
                    chosen = Some(None);
                }
                for p in &self.packs {
                    let selected = current.as_deref() == Some(p.id.as_str());
                    if ui.selectable_label(selected, &p.name).clicked() {
                        chosen = Some(Some(p.id.clone()));
                    }
                }
                ui.separator();
                if ui.button("Nouveau pack vide…").clicked() {
                    new_pack = Some(false);
                }
                if !self.project.pack().id.is_empty()
                    && ui.button("Nouveau pack à partir de l'actuel…").clicked()
                {
                    new_pack = Some(true);
                }
                if ui.button("Actualiser la liste").clicked() {
                    refresh = true;
                }
            });
        if !self.project.pack().id.is_empty()
            && ui
                .button("Modifier le pack…")
                .on_hover_text("Catégories, types de liens, statuts, champs et modèles")
                .clicked()
        {
            self.pack_editor = Some(PackEditor::edit_current(
                &self.project,
                self.user_packs_dir.as_deref(),
            ));
        }
        if let Some(from_current) = new_pack {
            self.pack_editor = Some(PackEditor::new_pack(
                &self.project,
                self.user_packs_dir.as_deref(),
                from_current,
            ));
        }
        if refresh {
            self.refresh_packs();
        }
        if let Some(pack) = chosen
            && pack != current
        {
            match self.project.set_pack(pack) {
                Ok(()) => {
                    if let Some(issue) = self.project.pack_issue() {
                        notices.error(format!("Pack de jeu : {issue}"));
                    }
                }
                Err(e) => notices.error(format!("Changement de pack impossible : {e}")),
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, notices: &mut Notices, now_ms: u64) {
        self.now_ms = now_ms;
        self.handle_dropped_files(&ui.ctx().clone(), notices);
        self.process_fs_events(notices, now_ms);
        self.handle_screenshot(&ui.ctx().clone(), notices);
        let modal_open = self.dialog.is_some()
            || self.pack_editor.is_some()
            || self.shortcut_editor.is_some()
            || self.graph.link_dialog_open();
        if !modal_open {
            // Les raccourcis sont consommés avant les widgets : l'historique
            // interne du TextEdit d'egui n'est donc jamais utilisé.
            let ctx = ui.ctx().clone();
            for cmd in commands::consume_shortcuts(&ctx, &self.keymap) {
                // Palette ouverte : seul son propre raccourci (fermer) agit.
                if self.palette.is_none() || cmd == Command::Palette {
                    self.run_command(cmd, &ctx, notices);
                }
            }
        }
        self.autosave(ui.ctx(), notices);

        egui::Panel::left("notes_list")
            .resizable(true)
            .default_size(260.0)
            .min_size(180.0)
            .show(ui, |ui| self.list_ui(ui, notices));

        // En vue graphe, le panneau reste affiché même sans sélection : sinon
        // sélectionner un nœud redimensionnerait le canevas sous le curseur.
        if self.editor.is_some() || self.view != View::Note {
            egui::Panel::right("fields")
                .resizable(true)
                .default_size(240.0)
                .min_size(160.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("side_scroll")
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            let edited = match &mut self.editor {
                                Some(ed) => ed.fields_ui(ui, &mut self.project, now_ms),
                                None => {
                                    ui.weak(match self.view {
                                        View::Board => {
                                            "Cliquez sur une carte pour afficher sa fiche ici ; \
                                             double-cliquez pour l'ouvrir dans l'éditeur."
                                        }
                                        _ => {
                                            "Cliquez sur un nœud pour afficher sa fiche ici ; \
                                             double-cliquez pour l'ouvrir dans l'éditeur."
                                        }
                                    });
                                    false
                                }
                            };
                            if edited {
                                self.mark_edited();
                            }
                            ui.add_space(12.0);
                            if let Some(target) = self.links_ui(ui, notices) {
                                self.select(&target, notices);
                            }
                        });
                });
        }

        egui::CentralPanel::default_margins().show(ui, |ui| self.central_ui(ui, notices, now_ms));

        self.dialog_ui(ui.ctx(), notices);
        self.palette_ui(ui.ctx(), notices);
        self.shortcut_editor_ui(ui.ctx(), notices);

        if let Some(editor) = &mut self.pack_editor {
            match editor.show(ui.ctx(), &mut self.project, notices) {
                pack_editor::Outcome::Open => {}
                pack_editor::Outcome::Closed => self.pack_editor = None,
                pack_editor::Outcome::Saved => {
                    self.pack_editor = None;
                    self.disk_changed = true;
                    self.refresh_packs();
                }
            }
        }
    }

    fn process_fs_events(&mut self, notices: &mut Notices, now_ms: u64) {
        let Some(watcher) = &self.watcher else {
            return;
        };
        let (paths, errors) = watcher.drain();
        if !paths.is_empty() {
            self.disk_changed = true;
        }
        for e in errors {
            tracing::warn!("surveillance des fichiers : {e}");
        }
        for path in paths {
            let change = self.project.on_file_changed(&path);
            let title = |p: &Project, id: &NoteId| {
                p.note(id).map(|n| n.meta.title.clone()).unwrap_or_default()
            };
            match change {
                FileChange::Ignored | FileChange::Added(_) => {}
                FileChange::Reloaded(id) => {
                    if let Some(ed) = self.editor.as_mut().filter(|e| e.id == id) {
                        ed.sync_from_project(&self.project, now_ms);
                        notices.info(format!(
                            "« {} » a été modifiée hors de l'application et rechargée.",
                            title(&self.project, &id)
                        ));
                    }
                }
                FileChange::Conflict(id) => notices.error(format!(
                    "« {} » a été modifiée hors de l'application pendant que vous l'éditiez.",
                    title(&self.project, &id)
                )),
                FileChange::Removed(id) => {
                    if self.editor.as_ref().is_some_and(|e| e.id == id) {
                        self.editor = None;
                        notices.info("La fiche ouverte a été supprimée hors de l'application.");
                    }
                }
                FileChange::RemovedWhileDirty(id) => notices.error(format!(
                    "Le fichier de « {} » a été supprimé ; il sera recréé à la prochaine sauvegarde.",
                    title(&self.project, &id)
                )),
                FileChange::Unreadable(p) => {
                    let msg = self.project.issues().get(&p).cloned().unwrap_or_default();
                    notices.error(format!("{} : {msg}", p.display()));
                }
            }
        }
    }

    fn autosave(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        let Some(t) = self.last_edit else {
            return;
        };
        let elapsed = t.elapsed();
        if elapsed >= AUTOSAVE_DELAY {
            self.flush(notices);
        } else {
            ctx.request_repaint_after(AUTOSAVE_DELAY - elapsed);
        }
    }

    fn palette_ui(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        let Some(mut palette) = self.palette.take() else {
            return;
        };
        let outcome = palette.show(ctx, &self.project, &self.keymap, |c| {
            self.command_available(c)
        });
        match outcome {
            PaletteOutcome::Open => self.palette = Some(palette),
            PaletteOutcome::Closed => {}
            PaletteOutcome::Run(PaletteItem::Command(cmd)) => self.run_command(cmd, ctx, notices),
            PaletteOutcome::Run(PaletteItem::Note(id)) => {
                self.select(&id, notices);
                if self.view != View::Board {
                    self.view = View::Note;
                }
            }
        }
    }

    fn shortcut_editor_ui(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        let Some(editor) = &mut self.shortcut_editor else {
            return;
        };
        let file = self.keymap_path.as_ref().map(|p| p.display().to_string());
        let out = editor.show(ctx, &mut self.keymap, file.as_deref());
        if out.changed
            && let Some(path) = &self.keymap_path
            && let Err(e) = self.keymap.save(path)
        {
            notices.error(format!("Raccourcis non enregistrés : {e}"));
        }
        if !out.open {
            self.shortcut_editor = None;
        }
    }

    /// Texte de la fiche ouverte (tests).
    #[cfg(test)]
    pub fn editor_body(&self) -> Option<&str> {
        self.editor.as_ref().map(|e| e.body())
    }

    #[cfg(test)]
    pub fn board(&self) -> &Board {
        &self.board
    }

    #[cfg(test)]
    pub fn board_view(&self) -> &BoardView {
        &self.board_view
    }

    #[cfg(test)]
    pub fn palette_open(&self) -> bool {
        self.palette.is_some()
    }

    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    fn open_new_note_dialog(&mut self) {
        self.dialog = Some(Dialog::NewNote {
            title: String::new(),
            template: None,
            error: None,
            focus: true,
        });
    }

    fn list_ui(&mut self, ui: &mut egui::Ui, notices: &mut Notices) {
        ui.horizontal(|ui| {
            ui.heading("Fiches");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("+ Nouvelle")
                    .on_hover_text("Nouvelle fiche (Ctrl+N)")
                    .clicked()
                {
                    self.open_new_note_dialog();
                }
            });
        });
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.filter.text)
                .hint_text("Rechercher par titre… (Ctrl+F)")
                .desired_width(f32::INFINITY),
        );
        if self.focus_search {
            search.request_focus();
            self.focus_search = false;
        }
        self.filters_ui(ui);

        // La liste n'est recalculée que si le filtre, la structure du projet
        // ou (pour les filtres qui en dépendent) le contenu changent.
        let content = if self.filter.depends_on_content() {
            self.project.content_version()
        } else {
            0
        };
        let key = (self.filter.clone(), self.project.generation(), content);
        if self.list_key.as_ref() != Some(&key) {
            self.list = self.project.filter(&self.filter);
            self.list_key = Some(key);
        }
        // Entrée dans la recherche : ouvrir le premier résultat.
        if search.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && let Some(first) = self.list.first().cloned()
        {
            self.select(&first, notices);
        }

        ui.add_space(4.0);
        let count = self.list.len();
        let filtering = !self.filter.text.trim().is_empty() || self.filter.active_count() > 0;
        ui.weak(if filtering {
            format!("{count} résultat(s) sur {}", self.project.len())
        } else {
            format!("{count} fiche(s)")
        });

        let issues: Vec<(PathBuf, String)> = self
            .project
            .issues()
            .iter()
            .map(|(p, m)| (p.clone(), m.clone()))
            .collect();
        let bottom_reserved = if issues.is_empty() { 0.0 } else { 28.0 };

        let row_h = ui.spacing().interact_size.y + 4.0;
        let mut to_select = None;
        let mut to_rename = None;
        let mut to_delete = None;
        let selected = self.editor.as_ref().map(|e| e.id.clone());
        egui::ScrollArea::vertical()
            .id_salt("notes_scroll")
            .auto_shrink(false)
            .max_height((ui.available_height() - bottom_reserved).max(row_h))
            .show_rows(ui, row_h, count, |ui, range| {
                for id in &self.list[range] {
                    let resp =
                        note_row(ui, &self.project, id, selected.as_ref() == Some(id), row_h);
                    if resp.clicked() {
                        to_select = Some(id.clone());
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Renommer").clicked() {
                            to_rename = Some(id.clone());
                            ui.close();
                        }
                        if ui.button("Supprimer…").clicked() {
                            to_delete = Some(id.clone());
                            ui.close();
                        }
                    });
                }
            });

        if !issues.is_empty() {
            egui::CollapsingHeader::new(
                RichText::new(format!("⚠ {} fichier(s) illisible(s)", issues.len()))
                    .color(ui.visuals().error_fg_color),
            )
            .id_salt("issues")
            .show(ui, |ui| {
                for (p, m) in &issues {
                    let rel = p.strip_prefix(self.project.root()).unwrap_or(p);
                    ui.label(RichText::new(rel.display().to_string()).strong());
                    ui.weak(m);
                }
            });
        }

        if let Some(id) = to_select {
            self.select(&id, notices);
        }
        if let Some(id) = to_rename {
            self.select(&id, notices);
            if let Some(ed) = &mut self.editor {
                ed.focus_title = true;
            }
        }
        if let Some(id) = to_delete {
            self.dialog = Some(Dialog::ConfirmDelete(id));
        }
    }

    /// Filtres combinables, repliables, sous la recherche.
    fn filters_ui(&mut self, ui: &mut egui::Ui) {
        let n = self.filter.active_count();
        let title = if n == 0 {
            "Filtres".to_owned()
        } else {
            format!("Filtres ({n})")
        };
        let pack = self.project.pack().clone();
        let f = &mut self.filter;
        egui::CollapsingHeader::new(title)
            .id_salt("filters")
            .show(ui, |ui| {
                ui.checkbox(&mut f.in_body, "Chercher aussi dans le texte");
                filter_ui::categories_ui(ui, f, &pack);
                filter_ui::statuses_ui(ui, f, &pack);
                filter_ui::link_types_ui(ui, "Liens présents", &mut f.link_types, &pack, false);
                ui.checkbox(&mut f.broken_only, "Avec liens cassés");

                if n > 0 && ui.button("Réinitialiser les filtres").clicked() {
                    f.clear_criteria();
                }
            });
    }

    fn central_ui(&mut self, ui: &mut egui::Ui, notices: &mut Notices, now_ms: u64) {
        let hint = |cmd: Command| {
            let keys = cmd.bindings_text(&self.keymap);
            if keys.is_empty() {
                String::new()
            } else {
                format!(" ({keys})")
            }
        };
        let (h_graph, h_board, h_palette) = (
            hint(Command::ToggleGraph),
            hint(Command::ShowBoard),
            hint(Command::Palette),
        );
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.view, View::Note, "Fiche")
                .on_hover_text("Éditeur de la fiche");
            ui.selectable_value(&mut self.view, View::Graph, "Graphe")
                .on_hover_text(format!("Graphe des fiches{h_graph}"));
            ui.selectable_value(&mut self.view, View::Board, "Tableau")
                .on_hover_text(format!(
                    "Tableau d'enquête : fiches posées à la main{h_board}"
                ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("Commandes")
                    .on_hover_text(format!("Palette de commandes{h_palette}"))
                    .clicked()
                {
                    self.palette = Some(Palette::default());
                }
            });
        });
        ui.separator();
        if self.view == View::Board {
            let selected = self.editor.as_ref().map(|e| e.id.clone());
            let out = self.board_view.ui(
                ui,
                &self.project,
                &mut self.board,
                selected.as_ref(),
                &self.list,
            );
            if out.changed {
                self.save_board(notices);
            }
            if let Some(id) = out.select {
                self.select(&id, notices);
            }
            if let Some(id) = out.open_editor {
                self.select(&id, notices);
                self.view = View::Note;
            }
            if out.deselect {
                self.flush(notices);
                self.editor = None;
            }
            return;
        }
        if self.view == View::Graph {
            let selected = self.editor.as_ref().map(|e| e.id.clone());
            let outcome = self.graph.ui(ui, &self.project, selected.as_ref());
            self.apply_graph_outcome(outcome, notices, now_ms);
            return;
        }
        let Some(editor) = &mut self.editor else {
            ui.centered_and_justified(|ui| {
                ui.weak(if self.project.is_empty() {
                    "Aucune fiche. Créez-en une avec « + Nouvelle » (Ctrl+N)."
                } else {
                    "Sélectionnez une fiche dans la liste."
                });
            });
            return;
        };

        let edited = editor.header_ui(ui, &mut self.project, now_ms, notices);
        let mut insert_image = false;

        if self
            .project
            .entry(&editor.id)
            .is_some_and(|e| e.has_conflict())
        {
            let mut choice = None;
            egui::Frame::new()
                .fill(ui.visuals().error_fg_color.gamma_multiply(0.15))
                .inner_margin(8.0)
                .corner_radius(6.0)
                .show(ui, |ui| {
                    ui.label(
                        "Ce fichier a été modifié hors de l'application alors que vous aviez \
                         des modifications non enregistrées. La sauvegarde est suspendue.",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Garder ma version").clicked() {
                            choice = Some(true);
                        }
                        if ui.button("Prendre la version du disque").clicked() {
                            choice = Some(false);
                        }
                    });
                });
            if let Some(keep_mine) = choice {
                match self.project.resolve_conflict(&editor.id, keep_mine) {
                    Ok(()) => editor.sync_from_project(&self.project, now_ms),
                    Err(e) => notices.error(format!("Résolution du conflit impossible : {e}")),
                }
            }
        }

        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.reading, false, "Édition")
                .on_hover_text("Texte Markdown");
            ui.selectable_value(&mut self.reading, true, "Lecture")
                .on_hover_text("Fiche mise en forme, avec ses images (glisser le coin d'une image pour la redimensionner)");
            ui.separator();
            if ui
                .button("Insérer une image…")
                .on_hover_text("Ou glissez une image sur la fenêtre")
                .clicked()
            {
                insert_image = true;
            }
        });
        ui.separator();
        let body = if self.reading {
            editor.reading_ui(ui, &mut self.project, now_ms, notices)
        } else {
            editor.body_ui(ui, &mut self.project, now_ms, notices)
        };
        if insert_image {
            self.run_command(Command::InsertImage, &ui.ctx().clone(), notices);
        }
        if edited || body.edited {
            self.mark_edited();
        }
        if let Some(target) = body.open {
            self.select(&target, notices);
        }
    }

    #[cfg(test)]
    fn graph_stop_for_bench(&mut self) {
        self.graph.stop_for_bench();
    }

    fn apply_graph_outcome(&mut self, outcome: GraphOutcome, notices: &mut Notices, now_ms: u64) {
        for (id, pos) in outcome.pins {
            let Some(note) = self.project.note(&id) else {
                continue;
            };
            let mut meta = note.meta.clone();
            meta.pos = pos;
            match self.project.set_meta(&id, meta) {
                Ok(()) => {
                    if let Some(ed) = self.editor.as_mut().filter(|e| e.id == id) {
                        ed.sync_from_project(&self.project, now_ms);
                    }
                    self.mark_edited();
                }
                Err(e) => notices.error(format!("Épinglage impossible : {e}")),
            }
        }
        for op in outcome.link_ops {
            // La fiche source est sélectionnée d'abord : le panneau latéral
            // montre le résultat et Ctrl+Z annule la modification.
            let source = op.source().clone();
            if self.editor.as_ref().is_none_or(|e| e.id != source) {
                self.select(&source, notices);
            }
            let result = match op {
                LinkOp::Add {
                    from,
                    to,
                    kind,
                    note,
                } => self.project.add_link(&from, &to, kind, note),
                LinkOp::Update {
                    from,
                    to,
                    old_kind,
                    kind,
                    note,
                } => self
                    .project
                    .update_link(&from, &to, old_kind.as_deref(), kind, note),
                LinkOp::Remove { from, to, kind } => {
                    self.project.remove_link(&from, &to, kind.as_deref())
                }
            };
            match result {
                Ok(()) => {
                    if let Some(ed) = self.editor.as_mut().filter(|e| e.id == source) {
                        ed.sync_from_project(&self.project, now_ms);
                    }
                    self.mark_edited();
                }
                Err(e) => notices.error(format!("Lien non modifié : {e}")),
            }
        }
        if let Some(id) = outcome.select {
            self.select(&id, notices);
        }
        if let Some(id) = outcome.open_editor {
            self.select(&id, notices);
            self.view = View::Note;
        }
        if outcome.deselect {
            self.flush(notices);
            self.editor = None;
        }
    }

    /// Panneau « Liens » : sortants, rétroliens et liens cassés de la fiche
    /// ouverte. Renvoie la fiche à ouvrir si l'on a cliqué sur un lien.
    fn links_ui(&mut self, ui: &mut egui::Ui, notices: &mut Notices) -> Option<NoteId> {
        let id = self.editor.as_ref()?.id.clone();
        let (broken, outgoing): (Vec<ResolvedLink>, Vec<ResolvedLink>) = self
            .project
            .outgoing_links(&id)
            .into_iter()
            .partition(|l| matches!(l.other, LinkEnd::Broken(_)));
        let backlinks = self.project.backlinks(&id);
        let mut open = None;
        let mut create: Option<String> = None;

        ui.heading("Liens");
        let sections = [
            ("Sortants", &outgoing, "»"),
            ("Rétroliens", &backlinks, "«"),
        ];
        for (title, links, arrow) in sections {
            egui::CollapsingHeader::new(format!("{title} ({})", links.len()))
                .id_salt(title)
                .default_open(true)
                .show(ui, |ui| {
                    if links.is_empty() {
                        ui.weak("Aucun");
                    }
                    for link in links {
                        if let Some(target) = link_row(ui, &self.project, link, arrow) {
                            open = Some(target);
                        }
                    }
                });
        }
        if !broken.is_empty() {
            egui::CollapsingHeader::new(
                RichText::new(format!("Cassés ({})", broken.len()))
                    .color(ui.visuals().error_fg_color),
            )
            .id_salt("broken")
            .default_open(true)
            .show(ui, |ui| {
                for link in &broken {
                    let LinkEnd::Broken(target) = &link.other else {
                        continue;
                    };
                    ui.horizontal_wrapped(|ui| {
                        type_chip(ui, &self.project, link.kind.as_deref());
                        ui.colored_label(ui.visuals().error_fg_color, target);
                        let creatable = link.in_body && Project::validate_title(target).is_ok();
                        if creatable {
                            if ui
                                .small_button("Créer")
                                .on_hover_text("Créer cette fiche et l'ouvrir")
                                .clicked()
                            {
                                create = Some(target.clone());
                            }
                        } else if link.in_meta {
                            ui.weak("(identifiant inconnu dans l'en-tête)");
                        }
                    });
                }
            });
        }

        if let Some(title) = create {
            match self.project.create_note(&title, Vec::new(), String::new()) {
                Ok(new_id) => {
                    notices.info(format!("Fiche « {title} » créée."));
                    open = Some(new_id);
                }
                Err(e) => notices.error(format!("Création impossible : {e}")),
            }
        }
        open
    }

    fn dialog_ui(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let mut close = false;
        let mut created = None;
        let mut delete = None;
        match dialog {
            Dialog::NewNote {
                title,
                template,
                error,
                focus,
            } => {
                let templates = self.project.pack().templates.clone();
                let resp = egui::Modal::new(egui::Id::unique("new_note")).show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.heading("Nouvelle fiche");
                    let edit = ui.add(
                        egui::TextEdit::singleline(title)
                            .hint_text("Titre")
                            .desired_width(f32::INFINITY),
                    );
                    if *focus {
                        edit.request_focus();
                        *focus = false;
                    }
                    if !templates.is_empty() {
                        let current = template
                            .as_deref()
                            .and_then(|id| templates.iter().find(|t| t.id == id))
                            .map_or("Fiche vide", |t| t.label.as_str())
                            .to_owned();
                        egui::ComboBox::new("template", "Modèle")
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(template, None, "Fiche vide");
                                for t in &templates {
                                    ui.selectable_value(template, Some(t.id.clone()), &t.label);
                                }
                            });
                    }
                    if let Some(err) = error {
                        ui.colored_label(ui.visuals().error_fg_color, err.as_str());
                    }
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        let create = ui.button("Créer").clicked() || enter;
                        if ui.button("Annuler").clicked() {
                            close = true;
                        }
                        if create {
                            let chosen = template
                                .as_deref()
                                .and_then(|id| templates.iter().find(|t| t.id == id));
                            match self.project.create_from_template(title, chosen) {
                                Ok(id) => created = Some(id),
                                Err(e) => {
                                    *error = Some(e.to_string());
                                    *focus = true;
                                }
                            }
                        }
                    });
                });
                if resp.should_close() {
                    close = true;
                }
            }
            Dialog::ConfirmDelete(id) => {
                let title = self
                    .project
                    .note(id)
                    .map(|n| n.meta.title.clone())
                    .unwrap_or_default();
                let resp = egui::Modal::new(egui::Id::unique("confirm_delete")).show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.heading("Supprimer la fiche ?");
                    ui.label(format!(
                        "« {title} » sera déplacée dans la corbeille du projet (.enigma/trash)."
                    ));
                    ui.horizontal(|ui| {
                        if ui
                            .button(
                                RichText::new("Supprimer")
                                    .color(Color32::from_rgb(0xff, 0x6b, 0x6b)),
                            )
                            .clicked()
                        {
                            delete = Some(id.clone());
                        }
                        if ui.button("Annuler").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() {
                    close = true;
                }
            }
        }

        if let Some(id) = created {
            self.dialog = None;
            self.filter.text.clear();
            self.select(&id, notices);
        } else if let Some(id) = delete {
            self.dialog = None;
            if self.editor.as_ref().is_some_and(|e| e.id == id) {
                self.flush(notices);
                self.editor = None;
            }
            match self.project.delete(&id) {
                Ok(_) => notices.info("Fiche déplacée dans la corbeille du projet."),
                Err(e) => notices.error(format!("Suppression impossible : {e}")),
            }
        } else if close {
            self.dialog = None;
        }
    }
}

/// Étiquette colorée du type de lien.
fn type_chip(ui: &mut egui::Ui, project: &Project, kind: Option<&str>) {
    let (label, color) = match kind {
        None => ("lien".to_owned(), ui.visuals().weak_text_color()),
        Some(k) => match project.pack().link_type(k) {
            Some(t) => (t.label.clone(), c32(t.color)),
            None => (format!("{k} ?"), ui.visuals().weak_text_color()),
        },
    };
    ui.label(RichText::new(label).color(color).small());
}

/// Une ligne du panneau « Liens ». Renvoie la fiche cliquée.
fn link_row(
    ui: &mut egui::Ui,
    project: &Project,
    link: &ResolvedLink,
    arrow: &str,
) -> Option<NoteId> {
    let LinkEnd::Note(target) = &link.other else {
        return None;
    };
    let title = project
        .note(target)
        .map(|n| n.meta.title.clone())
        .unwrap_or_default();
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        type_chip(ui, project, link.kind.as_deref());
        ui.weak(arrow);
        let mut resp = ui.link(title);
        let mut hover = Vec::new();
        if link.in_body {
            hover.push("dans le texte".to_owned());
        }
        if link.in_meta {
            hover.push("dans l'en-tête".to_owned());
        }
        if let Some(note) = &link.note {
            hover.push(format!("note : {note}"));
        }
        if !hover.is_empty() {
            resp = resp.on_hover_text(hover.join("\n"));
        }
        if resp.clicked() {
            clicked = Some(target.clone());
        }
    });
    clicked
}

/// Une ligne de la liste : pastilles de couleur des catégories et titre.
fn note_row(
    ui: &mut egui::Ui,
    project: &Project,
    id: &NoteId,
    selected: bool,
    h: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::click());
    let Some(entry) = project.entry(id) else {
        return resp;
    };
    // Libellé pour l'accessibilité (et les tests d'interface).
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::ListItem, true, selected, &entry.note.meta.title)
    });
    let visuals = ui.visuals();
    if selected {
        ui.painter()
            .rect_filled(rect, 4.0, visuals.selection.bg_fill);
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let mut x = rect.left() + 8.0;
    let cy = rect.center().y;
    let pack = project.pack();
    for cat in entry.note.meta.categories.iter().take(5) {
        let color = pack.category(cat).map_or(Color32::GRAY, |c| c32(c.color));
        ui.painter()
            .circle_filled(egui::pos2(x + 4.0, cy), 4.0, color);
        x += 10.0;
    }
    if !entry.note.meta.categories.is_empty() {
        x += 4.0;
    }
    let mut title = entry.note.meta.title.clone();
    if entry.is_dirty() {
        title.push_str(" •");
    }
    let color = if selected {
        visuals.selection.stroke.color
    } else {
        visuals.text_color()
    };
    let galley = ui.painter().layout(
        title,
        egui::FontId::proportional(14.0),
        color,
        (rect.right() - x - 4.0).max(10.0),
    );
    let y = cy - galley.size().y / 2.0;
    ui.painter()
        .with_clip_rect(rect)
        .galley(egui::pos2(x, y), galley, color);
    resp
}

#[cfg(test)]
mod ui_tests {
    //! Tests d'interface sans fenêtre (egui_kittest). Si la variable
    //! `ENIGMA_SNAPSHOT_DIR` est définie, des captures PNG y sont écrites.

    use std::path::Path;

    use egui::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[derive(Default)]
    struct State {
        ws: Option<Workspace>,
        notices: Notices,
    }

    fn copy_demo(dst: &Path) {
        let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../exemples/blue_prince_demo");
        for entry in walk(&demo) {
            let rel = entry.strip_prefix(&demo).unwrap();
            let target = dst.join(rel);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::copy(&entry, &target).unwrap();
        }
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    fn snapshot(harness: &mut Harness<'_, State>, name: &str) {
        let Ok(dir) = std::env::var("ENIGMA_SNAPSHOT_DIR") else {
            return;
        };
        match harness.render() {
            Ok(img) => img
                .save(Path::new(&dir).join(format!("{name}.png")))
                .unwrap(),
            Err(e) => eprintln!("rendu impossible : {e}"),
        }
    }

    fn flush(harness: &mut Harness<'_, State>) {
        let state = harness.state_mut();
        if let Some(ws) = &mut state.ws {
            ws.flush(&mut state.notices);
        }
    }

    fn setup(dir: &Path) -> Harness<'static, State> {
        copy_demo(dir);
        let packs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")];
        let mut harness = Harness::builder()
            .with_size([1200.0, 760.0])
            // 60 images/s : délais réalistes (double-clic < 0,3 s).
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, state: &mut State| {
                    if let Some(ws) = &mut state.ws {
                        ws.ui(ui, &mut state.notices, 0);
                    }
                },
                State::default(),
            );
        let ctx = harness.ctx.clone();
        {
            let state = harness.state_mut();
            state.ws = Some(Workspace::open(dir, &packs, &ctx, &mut state.notices).unwrap());
        }
        harness.run_steps(8);
        harness
    }

    #[test]
    fn edit_note_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        let majordome_before = std::fs::read(dir.path().join("Majordome.md")).unwrap();

        // Ouvrir une fiche depuis la liste.
        harness.get_by_label("Salle des Archives").click();
        harness.run_steps(8);
        let path = dir.path().join("Salle des Archives.md");
        {
            let ws = harness.state().ws.as_ref().unwrap();
            let id = ws.project.resolve_title("Salle des Archives").unwrap();
            assert_eq!(ws.editor.as_ref().map(|e| &e.id), Some(id));
        }
        snapshot(&mut harness, "1_editeur");

        // Ajouter une catégorie par sa pastille, puis annuler.
        harness.get_by_label("👤 Personnage").click();
        harness.run_steps(8);
        flush(&mut harness);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("personnage"), "{text}");
        snapshot(&mut harness, "2_categorie");

        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        harness.run_steps(8);
        flush(&mut harness);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("personnage"), "{text}");

        // Refaire.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Y);
        harness.run_steps(8);
        flush(&mut harness);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("personnage")
        );

        // Taper dans le corps : l'en-tête reste intact.
        harness.get_by_role(Role::MultilineTextInput).focus();
        harness.run_steps(8);
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("\nAjout [[Majordome]]");
        harness.run_steps(8);
        flush(&mut harness);
        let saved = enigma_core::Note::parse(&std::fs::read_to_string(&path).unwrap(), "").unwrap();
        assert!(saved.body.contains("Ajout [[Majordome]]"), "{}", saved.body);
        assert_eq!(saved.meta.id.as_str(), "01JDEMO0000000000000000001");
        assert_eq!(saved.meta.fields.len(), 1);
        snapshot(&mut harness, "3_frappe");

        // Une fiche sans en-tête jamais modifiée n'est pas réécrite.
        assert_eq!(
            std::fs::read(dir.path().join("Majordome.md")).unwrap(),
            majordome_before
        );
    }

    #[test]
    fn rename_from_title_and_external_reload() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.get_by_label("Salle des Archives").click();
        harness.run_steps(8);

        // Renommer via le champ titre : fichier renommé, liens réécrits.
        let is_title = |n: &egui_kittest::kittest::AccessKitNode<'_>| {
            n.role() == Role::TextInput && n.value().as_deref() == Some("Salle des Archives")
        };
        harness.get_by(is_title).focus();
        harness.run_steps(8);
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.get_by(is_title).type_text("Archives secrètes");
        harness.key_press(egui::Key::Enter);
        harness.run_steps(8);
        let new_path = dir.path().join("Archives secrètes.md");
        assert!(new_path.exists());
        assert!(!dir.path().join("Salle des Archives.md").exists());
        let majordome = std::fs::read_to_string(dir.path().join("Majordome.md")).unwrap();
        assert!(majordome.contains("[[Archives secrètes]]"), "{majordome}");
        assert!(
            majordome.starts_with(
                "---
id: "
            ),
            "en-tête ajouté : {majordome}"
        );

        // Modification externe : la vraie surveillance recharge l'éditeur.
        let text = std::fs::read_to_string(&new_path).unwrap();
        std::fs::write(&new_path, text.replace("classeurs", "dossiers")).unwrap();
        let mut reloaded = false;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            harness.run_steps(8);
            let ws = harness.state().ws.as_ref().unwrap();
            if ws
                .editor
                .as_ref()
                .is_some_and(|e| e.body().contains("dossiers"))
            {
                reloaded = true;
                break;
            }
        }
        assert!(reloaded, "la modification externe n'a pas été rechargée");
        snapshot(&mut harness, "4_renomme_recharge");
    }

    fn body_text(harness: &Harness<'_, State>) -> String {
        harness
            .state()
            .ws
            .as_ref()
            .and_then(|ws| ws.editor.as_ref())
            .map(|e| e.body().to_owned())
            .unwrap_or_default()
    }

    fn open_title(harness: &Harness<'_, State>) -> String {
        let ws = harness.state().ws.as_ref().unwrap();
        let id = &ws.editor.as_ref().unwrap().id;
        ws.project.note(id).unwrap().meta.title.clone()
    }

    #[test]
    fn link_completion_and_links_panel() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.get_by_label("Salle des Archives").click();
        harness.run_steps(8);
        harness.get_by_role(Role::MultilineTextInput).focus();
        harness.run_steps(8);
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::End);
        harness.run_steps(8);

        // [[cof → Tab : titre inséré, puis suggestions de type.
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("\nPiste : [[cof");
        harness.run_steps(8);
        snapshot(&mut harness, "5_suggestions_titre");
        harness.key_press(egui::Key::Tab);
        harness.run_steps(8);
        assert!(
            body_text(&harness).ends_with("[[Coffre du bureau|]]"),
            "{}",
            body_text(&harness)
        );
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("ou");
        harness.run_steps(8);
        snapshot(&mut harness, "6_suggestions_type");
        harness.key_press(egui::Key::Enter);
        harness.run_steps(8);
        assert!(
            body_text(&harness).ends_with("Piste : [[Coffre du bureau|ouvre]]"),
            "{}",
            body_text(&harness)
        );

        // Création à la volée.
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text(" et [[Nouvelle piste");
        harness.run_steps(8);
        harness.key_press(egui::Key::Enter);
        harness.run_steps(8);
        assert!(body_text(&harness).ends_with("et [[Nouvelle piste]]"));
        assert!(
            harness
                .state()
                .ws
                .as_ref()
                .unwrap()
                .project
                .resolve_title("Nouvelle piste")
                .is_some()
        );

        // Échap ferme les suggestions sans rien insérer.
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text(" [[maj");
        harness.run_steps(8);
        harness.key_press(egui::Key::Escape);
        harness.run_steps(8);
        assert!(body_text(&harness).ends_with(" [[maj"));
        harness.key_press(egui::Key::Enter);
        harness.run_steps(8);
        assert!(
            body_text(&harness).ends_with(" [[maj\n"),
            "Entrée normale après Échap : {:?}",
            body_text(&harness)
        );

        // Panneau Liens : clic sur un lien sortant, puis rétrolien.
        snapshot(&mut harness, "7_panneau_liens");
        let is_link = |label: &'static str| {
            move |n: &egui_kittest::kittest::AccessKitNode<'_>| {
                n.role() == Role::Link && n.label().as_deref() == Some(label)
            }
        };
        // Deux relations vers le coffre (« ouvre » et « indique ») : deux lignes.
        let coffre_links: Vec<_> = harness.get_all_by(is_link("Coffre du bureau")).collect();
        assert_eq!(coffre_links.len(), 2);
        coffre_links[0].click();
        harness.run_steps(8);
        assert_eq!(open_title(&harness), "Coffre du bureau");
        harness.get_by_label("Rétroliens (2)");
        let back: Vec<_> = harness.get_all_by(is_link("Salle des Archives")).collect();
        assert_eq!(back.len(), 2);
        back[0].click();
        harness.run_steps(8);
        assert_eq!(open_title(&harness), "Salle des Archives");
        // Le lien cassé de la démo est listé.
        harness.get_by_label("Cassés (1)");
    }

    fn listed(harness: &Harness<'_, State>) -> Vec<String> {
        let ws = harness.state().ws.as_ref().unwrap();
        ws.list
            .iter()
            .map(|id| ws.project.note(id).unwrap().meta.title.clone())
            .collect()
    }

    #[test]
    fn filters_templates_and_pack_editor() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());

        // Filtres : inclure, exclure, neutre.
        harness.get_by_label("Filtres").click();
        harness.run_steps(8);
        harness.get_by_label("Pièce").click();
        harness.run_steps(8);
        assert_eq!(listed(&harness), ["Salle des Archives"]);
        snapshot(&mut harness, "8_filtres");
        harness.get_by_label("Pièce").click();
        harness.run_steps(8);
        assert_eq!(listed(&harness), ["Coffre du bureau", "Majordome"]);
        harness.get_by_label("− Pièce").click();
        harness.run_steps(8);
        assert_eq!(listed(&harness).len(), 3);

        // Statut, type de lien, liens cassés.
        harness.get_by_label("Confirmé").click();
        harness.run_steps(8);
        assert_eq!(listed(&harness), ["Coffre du bureau"]);
        harness.get_by_label("Réinitialiser les filtres").click();
        harness.run_steps(8);
        harness.get_by_label("indique").click();
        harness.run_steps(8);
        assert_eq!(listed(&harness), ["Coffre du bureau", "Salle des Archives"]);
        harness.get_by_label("Réinitialiser les filtres").click();
        harness.get_by_label("Avec liens cassés").click();
        harness.run_steps(8);
        assert_eq!(listed(&harness), ["Salle des Archives"]);
        harness.get_by_label("Avec liens cassés").click();
        harness.run_steps(8);

        // Nouvelle fiche à partir d'un modèle.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::N);
        harness.run_steps(8);
        harness
            .get_by(|n| n.role() == Role::TextInput && n.placeholder() == Some("Titre"))
            .type_text("Serre");
        harness.get_by_label("Modèle").click();
        harness.run_steps(8);
        harness.get_by_label("Nouvelle pièce").click();
        harness.run_steps(8);
        harness.get_by_label("Créer").click();
        harness.run_steps(8);
        {
            let ws = harness.state().ws.as_ref().unwrap();
            let id = ws.project.resolve_title("Serre").unwrap();
            let note = ws.project.note(id).unwrap();
            assert_eq!(note.meta.categories, ["piece"]);
            assert!(note.body.starts_with("## Description"), "{}", note.body);
        }

        // Éditeur de pack : ajout d'une catégorie, enregistrée en copie locale.
        {
            let state = harness.state_mut();
            let ws = state.ws.as_mut().unwrap();
            ws.pack_editor = Some(PackEditor::edit_current(&ws.project, None));
        }
        harness.run_steps(8);
        harness.get_by_label("+ Ajouter une catégorie").click();
        harness.run_steps(8);
        let field = |placeholder: &'static str| {
            move |n: &egui_kittest::kittest::AccessKitNode<'_>| {
                n.role() == Role::TextInput
                    && n.placeholder() == Some(placeholder)
                    && n.value().as_deref() == Some("")
            }
        };
        harness.get_by(field("id")).focus();
        harness.run_steps(8);
        harness.get_by(field("id")).type_text("lieu");
        harness.run_steps(8);
        harness.get_by(field("libellé")).focus();
        harness.run_steps(8);
        harness.get_by(field("libellé")).type_text("Lieu");
        harness.run_steps(8);
        snapshot(&mut harness, "9_editeur_pack");
        harness.get_by_label("Enregistrer").click();
        harness.run_steps(8);
        let ws = harness.state().ws.as_ref().unwrap();
        assert!(ws.pack_editor.is_none());
        assert_eq!(
            ws.project.pack().category("lieu").map(|c| c.label.as_str()),
            Some("Lieu")
        );
        let local = dir.path().join(".enigma/packs/blue_prince/pack.json");
        assert!(local.exists());
        // L'original fourni avec l'application n'est pas modifié.
        let original =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/blue_prince/pack.json");
        assert!(
            !std::fs::read_to_string(original)
                .unwrap()
                .contains("\"lieu\"")
        );
    }

    fn note_id(harness: &Harness<'_, State>, title: &str) -> NoteId {
        harness
            .state()
            .ws
            .as_ref()
            .unwrap()
            .project
            .resolve_title(title)
            .unwrap()
            .clone()
    }

    fn node_pos(harness: &Harness<'_, State>, title: &str) -> egui::Pos2 {
        let id = note_id(harness, title);
        harness
            .state()
            .ws
            .as_ref()
            .unwrap()
            .graph
            .screen_pos(&id)
            .unwrap()
    }

    fn click_at(harness: &mut Harness<'_, State>, pos: egui::Pos2) {
        harness.hover_at(pos);
        harness.run_steps(1);
        harness.drag_at(pos);
        harness.run_steps(1);
        harness.drop_at(pos);
        harness.run_steps(2);
    }

    #[test]
    fn graph_view_select_open_and_pin() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::G);
        harness.run_steps(600);
        {
            let ws = harness.state().ws.as_ref().unwrap();
            assert_eq!(ws.view, View::Graph);
            assert!(
                !ws.graph.is_animating(),
                "la disposition doit se stabiliser"
            );
            assert_eq!(ws.graph.node_count(), 3);
            // Salle→Coffre (ouvre, indique), Salle→Majordome, Majordome→Salle.
            assert_eq!(ws.graph.edge_count(), 4);
        }
        snapshot(&mut harness, "10_graphe");

        // Clic : sélection (le panneau latéral affiche la fiche).
        let coffre = note_id(&harness, "Coffre du bureau");
        let pos = node_pos(&harness, "Coffre du bureau");
        click_at(&mut harness, pos);
        {
            let ws = harness.state().ws.as_ref().unwrap();
            assert_eq!(ws.editor.as_ref().map(|e| &e.id), Some(&coffre));
            assert_eq!(ws.view, View::Graph);
        }
        harness.run_steps(30);
        snapshot(&mut harness, "11_selection");

        // Glisser « Majordome » : la fiche est épinglée et la position enregistrée.
        let from = node_pos(&harness, "Majordome");
        let to = from + egui::vec2(90.0, 60.0);
        harness.hover_at(from);
        harness.run_steps(1);
        harness.drag_at(from);
        harness.run_steps(1);
        for k in 1..=6 {
            harness.hover_at(from + (to - from) * (k as f32 / 6.0));
            harness.run_steps(1);
        }
        harness.drop_at(to);
        harness.run_steps(5);
        flush(&mut harness);
        let majordome = std::fs::read_to_string(dir.path().join("Majordome.md")).unwrap();
        assert!(majordome.contains("pinned: true"), "{majordome}");
        let after = node_pos(&harness, "Majordome");
        assert!((after - to).length() < 2.0, "le nœud reste où on l'a lâché");

        // Le clic sur le fond désélectionne.
        let rect_corner = node_pos(&harness, "Coffre du bureau") + egui::vec2(0.0, -400.0);
        let empty = rect_corner.clamp(egui::pos2(300.0, 120.0), egui::pos2(900.0, 700.0));
        let ws = harness.state().ws.as_ref().unwrap();
        let far_from_nodes = ["Coffre du bureau", "Majordome", "Salle des Archives"]
            .iter()
            .all(|t| (node_pos(&harness, t) - empty).length() > 40.0);
        let _ = ws;
        if far_from_nodes {
            click_at(&mut harness, empty);
            assert!(harness.state().ws.as_ref().unwrap().editor.is_none());
        }

        // Double-clic : ouvre l'éditeur. Pause d'abord, sinon egui compterait
        // un triple clic avec le clic précédent.
        harness.run_steps(30);
        let salle = note_id(&harness, "Salle des Archives");
        let pos = node_pos(&harness, "Salle des Archives");
        harness.hover_at(pos);
        harness.run_steps(1);
        for _ in 0..2 {
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
                harness.run_steps(1);
            }
        }
        let ws = harness.state().ws.as_ref().unwrap();
        assert_eq!(ws.view, View::Note);
        assert_eq!(ws.editor.as_ref().map(|e| &e.id), Some(&salle));
    }

    fn settle(harness: &mut Harness<'_, State>) {
        for _ in 0..60 {
            harness.run_steps(20);
            if !harness.state().ws.as_ref().unwrap().graph.is_animating() {
                return;
            }
        }
    }

    fn choose_link_type(harness: &mut Harness<'_, State>, label: &str) {
        harness.get_by_label("Type").click();
        harness.run_steps(2);
        harness.get_by_label(label).click();
        harness.run_steps(2);
    }

    #[test]
    fn graph_links_with_the_mouse() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::G);
        settle(&mut harness);
        let coffre = note_id(&harness, "Coffre du bureau");
        let majordome = note_id(&harness, "Majordome");

        // Ctrl+glisser de Majordome vers le Coffre : dialogue de création.
        let from = node_pos(&harness, "Majordome");
        let to = node_pos(&harness, "Coffre du bureau");
        harness.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
        harness.hover_at(from);
        harness.run_steps(1);
        harness.drag_at(from);
        harness.run_steps(1);
        for k in 1..=6 {
            harness.hover_at(from + (to - from) * (k as f32 / 6.0));
            harness.run_steps(1);
        }
        snapshot(&mut harness, "14_fil");
        harness.drop_at(to);
        harness.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        harness.run_steps(3);
        {
            let ws = harness.state().ws.as_ref().unwrap();
            assert!(ws.graph.link_dialog_open());
            assert!(
                !ws.project
                    .note(&majordome)
                    .unwrap()
                    .meta
                    .pos
                    .is_some_and(|p| p.pinned),
                "Ctrl+glisser n'épingle pas"
            );
        }
        choose_link_type(&mut harness, "contredit");
        harness
            .get_by(|n| n.placeholder() == Some("Note (facultative)"))
            .focus();
        harness.run_steps(1);
        harness
            .get_by(|n| n.placeholder() == Some("Note (facultative)"))
            .type_text("ment sur ses horaires");
        harness.run_steps(1);
        snapshot(&mut harness, "15_dialogue_lien");
        harness.get_by_label("Créer le lien").click();
        harness.run_steps(3);
        flush(&mut harness);
        let text = std::fs::read_to_string(dir.path().join("Majordome.md")).unwrap();
        assert!(text.contains(coffre.as_str()), "{text}");
        assert!(text.contains("type: contredit"), "{text}");
        assert!(text.contains("ment sur ses horaires"), "{text}");
        {
            let ws = harness.state().ws.as_ref().unwrap();
            assert!(!ws.graph.link_dialog_open());
            assert_eq!(ws.editor.as_ref().map(|e| &e.id), Some(&majordome));
            assert_eq!(ws.graph.edge_count(), 5);
        }

        // Clic sur le lien : modification du type.
        settle(&mut harness);
        let mid = |h: &Harness<'_, State>| {
            let a = node_pos(h, "Majordome");
            let b = node_pos(h, "Coffre du bureau");
            a + (b - a) * 0.5
        };
        let m = mid(&harness);
        click_at(&mut harness, m);
        assert!(
            harness
                .state()
                .ws
                .as_ref()
                .unwrap()
                .graph
                .link_dialog_open()
        );
        choose_link_type(&mut harness, "se trouve dans");
        harness.get_by_label("Enregistrer").click();
        harness.run_steps(3);
        flush(&mut harness);
        let text = std::fs::read_to_string(dir.path().join("Majordome.md")).unwrap();
        assert!(text.contains("type: situe_dans"), "{text}");
        assert!(
            text.contains("ment sur ses horaires"),
            "note conservée : {text}"
        );

        // Clic droit sur le lien : suppression ; Ctrl+Z la rétablit.
        settle(&mut harness);
        let m = mid(&harness);
        harness.hover_at(m);
        harness.run_steps(1);
        harness.event(egui::Event::PointerButton {
            pos: m,
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        harness.event(egui::Event::PointerButton {
            pos: m,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(3);
        harness.get_by_label("Supprimer le lien").click();
        harness.run_steps(3);
        flush(&mut harness);
        let text = std::fs::read_to_string(dir.path().join("Majordome.md")).unwrap();
        assert!(!text.contains(coffre.as_str()), "{text}");
        assert!(
            text.contains("[[salle des archives]]"),
            "texte intact : {text}"
        );
        assert_eq!(harness.state().ws.as_ref().unwrap().graph.edge_count(), 4);
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        harness.run_steps(3);
        assert_eq!(harness.state().ws.as_ref().unwrap().graph.edge_count(), 5);

        // Lien écrit dans le texte : le supprimer garde le mot.
        let salle = note_id(&harness, "Salle des Archives");
        harness
            .state_mut()
            .ws
            .as_mut()
            .unwrap()
            .apply_graph_outcome(
                GraphOutcome {
                    link_ops: vec![LinkOp::Remove {
                        from: salle.clone(),
                        to: majordome.clone(),
                        kind: None,
                    }],
                    ..GraphOutcome::default()
                },
                &mut Notices::default(),
                0,
            );
        let ws = harness.state().ws.as_ref().unwrap();
        let body = &ws.project.note(&salle).unwrap().body;
        assert!(body.contains("Voir aussi Majordome et"), "{body}");
    }

    #[test]
    fn graph_view_3d_toggle_orbit_z_axis_and_pin() {
        use crate::graph_view::Mode;
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::G);
        harness.run_steps(5);
        harness.get_by_label("3D").click();
        harness.run_steps(600);
        {
            let ws = harness.state().ws.as_ref().unwrap();
            assert_eq!(ws.graph.mode, Mode::Space);
            assert!(!ws.graph.is_animating(), "la disposition 3D se stabilise");
            let zs: Vec<f32> = ["Coffre du bureau", "Majordome", "Salle des Archives"]
                .iter()
                .map(|t| ws.graph.world_pos(&note_id(&harness, t)).unwrap().z)
                .collect();
            assert!(
                zs.iter().any(|z| z.abs() > 1.0),
                "profondeur utilisée : {zs:?}"
            );
        }
        snapshot(&mut harness, "12_graphe_3d");

        // Clic en 3D : sélection ; la bascule en 2D la conserve.
        let coffre = note_id(&harness, "Coffre du bureau");
        let pos = node_pos(&harness, "Coffre du bureau");
        click_at(&mut harness, pos);
        assert_eq!(
            harness
                .state()
                .ws
                .as_ref()
                .unwrap()
                .editor
                .as_ref()
                .map(|e| &e.id),
            Some(&coffre)
        );
        harness.get_by_label("2D").click();
        harness.run_steps(5);
        {
            let ws = harness.state().ws.as_ref().unwrap();
            assert_eq!(ws.graph.mode, Mode::Flat);
            assert_eq!(ws.editor.as_ref().map(|e| &e.id), Some(&coffre));
            assert_eq!(ws.graph.world_pos(&coffre).unwrap().z, 0.0, "à plat");
        }
        harness.get_by_label("3D").click();
        harness.run_steps(400);

        // Glisser le fond : la caméra tourne autour du graphe.
        let yaw = harness
            .state()
            .ws
            .as_ref()
            .unwrap()
            .graph
            .orbit_camera()
            .yaw;
        let nodes: Vec<egui::Pos2> = ["Coffre du bureau", "Majordome", "Salle des Archives"]
            .iter()
            .map(|t| node_pos(&harness, t))
            .collect();
        let empty = [
            egui::pos2(320.0, 650.0),
            egui::pos2(880.0, 650.0),
            egui::pos2(320.0, 200.0),
            egui::pos2(880.0, 200.0),
        ]
        .into_iter()
        .find(|q| nodes.iter().all(|n| (*n - *q).length() > 60.0))
        .unwrap();
        harness.hover_at(empty);
        harness.run_steps(1);
        harness.drag_at(empty);
        harness.run_steps(1);
        for k in 1..=5 {
            harness.hover_at(empty + egui::vec2(12.0 * k as f32, 0.0));
            harness.run_steps(1);
        }
        harness.drop_at(empty + egui::vec2(60.0, 0.0));
        harness.run_steps(2);
        let cam = harness.state().ws.as_ref().unwrap().graph.orbit_camera();
        assert!(
            (cam.yaw - yaw).abs() > 0.1,
            "rotation : {} -> {}",
            yaw,
            cam.yaw
        );

        // Axe Z sur « jour de découverte » : Salle (3) et Coffre (5) aux deux
        // extrémités, Majordome (sans valeur) libre.
        harness.state_mut().ws.as_mut().unwrap().graph.z_field = Some("jour_decouverte".to_owned());
        harness.run_steps(400);
        {
            let ws = harness.state().ws.as_ref().unwrap();
            let z = |t: &str| ws.graph.world_pos(&note_id(&harness, t)).unwrap().z;
            let (salle, coffre_z) = (z("Salle des Archives"), z("Coffre du bureau"));
            assert!(salle < 0.0 && coffre_z > 0.0, "{salle} / {coffre_z}");
            assert!((salle + coffre_z).abs() < 1e-3, "axe centré");
        }
        snapshot(&mut harness, "13_axe_z");

        // Glisser un nœud en 3D : épinglé, profondeur enregistrée.
        let from = node_pos(&harness, "Majordome");
        let to = from + egui::vec2(-70.0, 40.0);
        harness.hover_at(from);
        harness.run_steps(1);
        harness.drag_at(from);
        harness.run_steps(1);
        for k in 1..=6 {
            harness.hover_at(from + (to - from) * (k as f32 / 6.0));
            harness.run_steps(1);
        }
        harness.drop_at(to);
        harness.run_steps(5);
        flush(&mut harness);
        let majordome = std::fs::read_to_string(dir.path().join("Majordome.md")).unwrap();
        assert!(majordome.contains("pinned: true"), "{majordome}");
        let after = node_pos(&harness, "Majordome");
        assert!(
            (after - to).length() < 3.0,
            "le nœud reste sous le pointeur"
        );
        let id = note_id(&harness, "Majordome");
        let ws = harness.state().ws.as_ref().unwrap();
        let saved = ws.project.note(&id).unwrap().meta.pos.unwrap();
        let world = ws.graph.world_pos(&id).unwrap();
        assert!((saved.z - world.z).abs() < 1e-3 && saved.z != 0.0);
    }

    fn palette_input(harness: &mut Harness<'_, State>, text: &str) {
        const HINT: &str = "Commande ou titre de fiche…";
        harness
            .get_by(|n| n.role() == Role::TextInput && n.placeholder() == Some(HINT))
            .focus();
        harness.run_steps(1);
        harness
            .get_by(|n| n.role() == Role::TextInput && n.placeholder() == Some(HINT))
            .type_text(text);
        harness.run_steps(2);
    }

    #[test]
    fn palette_runs_commands_and_opens_notes() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::P);
        harness.run_steps(2);
        assert!(harness.state().ws.as_ref().unwrap().palette_open());
        palette_input(&mut harness, "majord");
        snapshot(&mut harness, "16_palette");
        harness.key_press(egui::Key::Enter);
        harness.run_steps(2);
        {
            let ws = harness.state().ws.as_ref().unwrap();
            assert!(!ws.palette_open());
            assert_eq!(
                ws.editor.as_ref().map(|e| e.id.clone()),
                ws.project.resolve_title("Majordome").cloned()
            );
        }
        // Commande : graphe en 3D (choisie aux flèches).
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::P);
        harness.run_steps(2);
        palette_input(&mut harness, "graphe 3d");
        harness.key_press(egui::Key::Enter);
        harness.run_steps(2);
        let ws = harness.state().ws.as_ref().unwrap();
        assert_eq!(ws.view, View::Graph);
        assert_eq!(ws.graph.mode, crate::graph_view::Mode::Space);
        // Échap ferme sans rien faire.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::P);
        harness.run_steps(2);
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        assert!(!harness.state().ws.as_ref().unwrap().palette_open());
    }

    #[test]
    fn shortcuts_are_configurable_and_saved() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = tempfile::tempdir().unwrap();
        let keymap_file = cfg.path().join("raccourcis.json");
        let mut harness = setup(dir.path());
        {
            let state = harness.state_mut();
            let ws = state.ws.as_mut().unwrap();
            ws.set_keymap(Keymap::default(), Some(keymap_file.clone()));
            let ctx = egui::Context::default();
            ws.run_command(Command::Shortcuts, &ctx, &mut state.notices);
        }
        harness.run_steps(10);
        snapshot(&mut harness, "17_raccourcis");
        let row = Command::ALL
            .iter()
            .position(|c| *c == Command::ToggleGraph)
            .unwrap();
        harness
            .get_all_by_label("Modifier")
            .nth(row)
            .unwrap()
            .click();
        harness.run_steps(2);
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
        harness.run_steps(2);
        assert_eq!(
            Command::ToggleGraph.bindings_text(harness.state().ws.as_ref().unwrap().keymap()),
            "Ctrl+K"
        );
        let saved = Keymap::load(&keymap_file).unwrap();
        assert_eq!(
            Command::ToggleGraph.bindings(&saved),
            ["Ctrl+K".parse().unwrap()]
        );
        harness.get_by_label("Fermer").click();
        harness.run_steps(2);
        // L'ancien raccourci ne fait plus rien, le nouveau bascule.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::G);
        harness.run_steps(2);
        assert_eq!(harness.state().ws.as_ref().unwrap().view, View::Note);
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::K);
        harness.run_steps(2);
        assert_eq!(harness.state().ws.as_ref().unwrap().view, View::Graph);
    }

    #[test]
    fn theme_toggle() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        let dark = harness.ctx.global_style().visuals.dark_mode;
        harness.key_press_modifiers(
            egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
            egui::Key::L,
        );
        harness.run_steps(3);
        assert_ne!(harness.ctx.global_style().visuals.dark_mode, dark);
        harness.state_mut().ws.as_mut().unwrap().view = View::Graph;
        harness.run_steps(400);
        snapshot(&mut harness, "18_theme_clair_graphe");
    }

    #[test]
    fn board_add_move_remove_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::T);
        harness.run_steps(2);
        assert_eq!(harness.state().ws.as_ref().unwrap().view, View::Board);
        harness
            .get_by_label("Ajouter les fiches de la liste (3)")
            .click();
        harness.run_steps(3);
        let board_file = dir
            .path()
            .join(".enigma")
            .join(enigma_core::board::BOARD_FILE);
        assert_eq!(Board::load(&board_file).unwrap().cards.len(), 3);
        snapshot(&mut harness, "19_tableau");

        // Glisser une carte : position enregistrée, carte sélectionnée.
        let coffre = note_id(&harness, "Coffre du bureau");
        let card = |h: &Harness<'_, State>| {
            let ws = h.state().ws.as_ref().unwrap();
            ws.board_view().card_center(ws.board(), &coffre).unwrap()
        };
        let before = Board::load(&board_file)
            .unwrap()
            .card(&coffre)
            .unwrap()
            .clone();
        let from = card(&harness);
        let to = from + egui::vec2(80.0, 140.0);
        harness.hover_at(from);
        harness.run_steps(1);
        harness.drag_at(from);
        harness.run_steps(1);
        for k in 1..=6 {
            harness.hover_at(from + (to - from) * (k as f32 / 6.0));
            harness.run_steps(1);
        }
        harness.drop_at(to);
        harness.run_steps(3);
        let after = Board::load(&board_file)
            .unwrap()
            .card(&coffre)
            .unwrap()
            .clone();
        assert!(after.y > before.y + 10.0, "{before:?} -> {after:?}");
        assert!((card(&harness) - to).length() < 2.0);
        assert_eq!(
            harness
                .state()
                .ws
                .as_ref()
                .unwrap()
                .editor
                .as_ref()
                .map(|e| &e.id),
            Some(&coffre)
        );

        // Clic droit : retirer du tableau.
        let at = card(&harness);
        harness.hover_at(at);
        harness.run_steps(1);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.run_steps(3);
        harness.get_by_label("Retirer du tableau").click();
        harness.run_steps(3);
        let saved = Board::load(&board_file).unwrap();
        assert_eq!(saved.cards.len(), 2);
        assert!(!saved.contains(&coffre));

        // Commande « Ajouter la fiche au tableau » (fiche sélectionnée).
        {
            let state = harness.state_mut();
            let ws = state.ws.as_mut().unwrap();
            let ctx = egui::Context::default();
            assert!(ws.command_available(Command::AddToBoard));
            ws.run_command(Command::AddToBoard, &ctx, &mut state.notices);
        }
        assert!(Board::load(&board_file).unwrap().contains(&coffre));
    }

    #[test]
    fn graph_svg_export() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.state_mut().ws.as_mut().unwrap().view = View::Graph;
        harness.run_steps(300);
        let out = dir.path().join("export").join("graphe.svg");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        let ctx = harness.ctx.clone();
        harness
            .state_mut()
            .ws
            .as_mut()
            .unwrap()
            .write_svg(&ctx, &out)
            .unwrap();
        let svg = std::fs::read_to_string(&out).unwrap();
        assert!(svg.starts_with("<?xml"));
        for title in ["Coffre du bureau", "Majordome", "Salle des Archives"] {
            assert!(svg.contains(title), "{title}");
        }
        assert_eq!(svg.matches("<line").count(), 4, "4 liens");
        assert!(svg.contains("<path"), "secteurs des catégories");
    }

    /// Mesure (ignorée par défaut) : coût d'une image de la vue graphe avec
    /// 2000 fiches et ~5000 liens. Lancer avec
    /// `cargo test --release -p enigma-app graph_frame_time -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn graph_frame_time() {
        let dir = tempfile::tempdir().unwrap();
        let n = 2000;
        let types = ["indique", "ouvre", "contredit", "situe_dans"];
        let cats = ["piece", "indice", "objet", "code", "personnage"];
        for i in 0..n {
            let links: String = (1..=5)
                .map(|k| format!("[[Fiche {}|{}]] ", (i * 7 + k * 13) % n, types[(i + k) % 4]))
                .take(if i % 2 == 0 { 3 } else { 2 })
                .collect();
            let c1 = cats[i % 5];
            let c2 = cats[(i / 5) % 5];
            std::fs::write(
                dir.path().join(format!("Fiche {i}.md")),
                format!("---\nid: N{i}\ntitle: Fiche {i}\ncategories: [{c1}, {c2}]\nstatus: theorie\n---\n{links}\n"),
            )
            .unwrap();
        }
        std::fs::create_dir_all(dir.path().join(".enigma")).unwrap();
        std::fs::write(
            dir.path().join(".enigma/project.json"),
            "{\"version\": 1, \"pack\": \"blue_prince\"}",
        )
        .unwrap();
        let mut harness = setup(dir.path());
        harness.state_mut().ws.as_mut().unwrap().view = View::Graph;
        let t = std::time::Instant::now();
        harness.run_steps(1);
        eprintln!(
            "première image (construction + pré-disposition) : {:?}",
            t.elapsed()
        );
        let ws = harness.state().ws.as_ref().unwrap();
        eprintln!(
            "{} nœuds, {} arêtes",
            ws.graph.node_count(),
            ws.graph.edge_count()
        );
        // Images pendant la simulation (un pas de disposition par image).
        let t = std::time::Instant::now();
        harness.run_steps(60);
        eprintln!("image pendant la simulation : {:?}", t.elapsed() / 60);
        // Images au repos (zoom réduit : peu d'étiquettes).
        harness
            .state_mut()
            .ws
            .as_mut()
            .unwrap()
            .graph_stop_for_bench();
        let t = std::time::Instant::now();
        harness.run_steps(60);
        eprintln!("image au repos : {:?}", t.elapsed() / 60);

        // Même chose en 3D.
        harness
            .state_mut()
            .ws
            .as_mut()
            .unwrap()
            .graph
            .set_mode(crate::graph_view::Mode::Space);
        harness.run_steps(1);
        let t = std::time::Instant::now();
        harness.run_steps(60);
        eprintln!("3D, image pendant la simulation : {:?}", t.elapsed() / 60);
        harness
            .state_mut()
            .ws
            .as_mut()
            .unwrap()
            .graph_stop_for_bench();
        let t = std::time::Instant::now();
        harness.run_steps(60);
        eprintln!("3D, image au repos : {:?}", t.elapsed() / 60);
        // Tessellation (formes egui -> triangles pour le GPU) d'une image 3D,
        // faite par eframe après chaque image.
        let shapes = harness.output().shapes.clone();
        let t = std::time::Instant::now();
        let mesh = harness.ctx.tessellate(shapes, 1.0);
        let vertices: usize = mesh
            .iter()
            .map(|m| match &m.primitive {
                egui::epaint::Primitive::Mesh(m) => m.vertices.len(),
                egui::epaint::Primitive::Callback(_) => 0,
            })
            .sum();
        eprintln!("3D, tessellation : {:?} ({vertices} sommets)", t.elapsed());
    }
}
