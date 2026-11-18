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

/// Tags et nombre de fiches qui les portent.
type TagCounts = Vec<(String, usize)>;

/// Onglet du panneau de gauche.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LeftTab {
    Notes,
    Search,
    Tags,
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
    /// Choix d'un modèle du pack à insérer dans la fiche ouverte.
    InsertTemplate,
    /// Historique des versions d'une fiche (version choisie).
    Versions {
        id: NoteId,
        selected: usize,
    },
    /// Choix de la fiche dans laquelle fusionner.
    Merge {
        id: NoteId,
        query: String,
    },
    /// Titre de la fiche extraite de la sélection.
    Extract {
        title: String,
        error: Option<String>,
    },
}

/// Ligne de la liste des fiches (en-tête de catégorie ou fiche).
#[derive(Clone, Debug, PartialEq)]
enum ListRow {
    Header {
        key: String,
        label: String,
        color: Option<Color32>,
        count: usize,
        open: bool,
    },
    Note(NoteId),
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
    /// Incrémenté à chaque recalcul de la liste.
    list_version: u64,
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
    /// Horloge de la dernière image (historique de l'éditeur).
    now_ms: u64,
    /// Liste rangée par catégorie (une fiche peut apparaître plusieurs fois).
    pub group_by_category: bool,
    /// Groupes repliés (clé : `cat:<id>` ou `none`).
    collapsed: std::collections::HashSet<String>,
    /// Lignes affichées et leur clé de cache.
    rows: Vec<ListRow>,
    rows_key: Option<(u64, bool, Vec<String>)>,
    pub left_tab: LeftTab,
    search: crate::search_ui::SearchPanel,
    /// Mentions non liées de la fiche ouverte (fiche, génération, contenu).
    mentions: Option<((NoteId, u64, u64), Vec<enigma_core::mentions::Mention>)>,
    /// Tags du projet (version du contenu, tag → nombre de fiches).
    tags: Option<(u64, u64, TagCounts)>,
    /// Onglets ouverts (fiche active comprise), dans l'ordre.
    tab_ids: Vec<NoteId>,
    /// Éditeurs des onglets en arrière-plan (historique d'annulation gardé).
    parked: std::collections::HashMap<NoteId, Editor>,
    /// Second volet (fiches côte à côte).
    other: Option<Editor>,
    /// Le second volet est à droite (sinon à gauche).
    other_right: bool,
    /// Historique de navigation : fiches précédentes et suivantes.
    back: Vec<NoteId>,
    forward: Vec<NoteId>,
    /// Compteur des éditeurs ouverts (identifiants d'affichage distincts).
    next_salt: u64,
}

/// Ce qu'a demandé un volet de fiche.
#[derive(Default)]
struct PaneOutcome {
    edited: bool,
    insert_image: bool,
    insert_template: bool,
    search_tag: Option<String>,
    open: Option<(NoteId, Option<String>)>,
    extract: bool,
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
            list_version: 0,
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
            now_ms: 0,
            group_by_category: true,
            collapsed: std::collections::HashSet::new(),
            rows: Vec::new(),
            rows_key: None,
            left_tab: LeftTab::Notes,
            search: crate::search_ui::SearchPanel::default(),
            mentions: None,
            tags: None,
            tab_ids: Vec::new(),
            parked: std::collections::HashMap::new(),
            other: None,
            other_right: true,
            back: Vec::new(),
            forward: Vec::new(),
            next_salt: 0,
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

    /// Copie un fichier (image, son, vidéo, document) dans le projet et
    /// l'insère dans la fiche ouverte.
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
                let kind = enigma_core::import::file_kind(file);
                let md = enigma_core::import::attachment_markdown(&alt, &rel, kind);
                editor.insert_text(&md, true, &mut self.project, self.now_ms);
                self.disk_changed = true;
                self.mark_edited();
            }
            Err(e) => notices.error(format!("Image non insérée : {e}")),
        }
    }

    /// Pose une image sur le tableau d'enquête (copiée dans `assets/`).
    pub fn add_board_image(&mut self, file: &Path, notices: &mut Notices) {
        match enigma_core::import::add_asset(self.project.root(), file) {
            Ok(rel) => {
                self.board_view.add_image(&mut self.board, &rel);
                self.disk_changed = true;
                self.save_board(notices);
            }
            Err(e) => notices.error(format!("Image non ajoutée : {e}")),
        }
    }

    /// Image collée (Ctrl+V) : enregistrée en PNG dans le projet et insérée.
    fn handle_pasted_images(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        if self.view != View::Note || self.editor.is_none() {
            return;
        }
        let images: Vec<std::sync::Arc<egui::ColorImage>> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::PasteImage(img) => Some(img.clone()),
                    _ => None,
                })
                .collect()
        });
        for img in images {
            let result = crate::clipboard::image_png(&img).and_then(|png| {
                enigma_core::import::add_asset_bytes(self.project.root(), "image collée.png", &png)
                    .map_err(|e| e.to_string())
            });
            match (result, &mut self.editor) {
                (Ok(rel), Some(editor)) => {
                    let md = enigma_core::render::image_markdown("image collée", &rel);
                    editor.insert_text(&md, true, &mut self.project, self.now_ms);
                    self.disk_changed = true;
                    self.mark_edited();
                }
                (Err(e), _) => notices.error(format!("Image collée non enregistrée : {e}")),
                _ => {}
            }
        }
    }

    /// Fichiers glissés sur la fenêtre.
    fn handle_dropped_files(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_owned())
                .collect()
        });
        self.paste_files(&dropped, notices);
    }

    /// Fichiers collés ou déposés : fiches `.md` importées, autres fichiers
    /// (images, sons, vidéos, documents) joints à la fiche ouverte.
    pub fn paste_files(&mut self, dropped: &[PathBuf], notices: &mut Notices) {
        let dropped: Vec<PathBuf> = dropped.to_vec();
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
            if file.is_dir() {
                notices.error(format!(
                    "« {} » est un dossier : utilisez « Importer un dossier de fiches .md… ».",
                    file.display()
                ));
            } else if self.editor.is_none() {
                notices.info("Ouvrez d'abord une fiche pour y joindre un fichier.");
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
            Command::DeleteNote
            | Command::Undo
            | Command::Redo
            | Command::InsertImage
            | Command::InsertTemplate
            | Command::VersionHistory
            | Command::MergeNote
            | Command::ExtractSelection
            | Command::ExportPdf
            | Command::CloseTab
            | Command::SplitView
            | Command::FindInNote => self.editor.is_some(),
            Command::NavBack => !self.back.is_empty(),
            Command::NavForward => !self.forward.is_empty(),
            Command::NextTab => self.tab_ids.len() > 1,
            c if commands::format_of(c).is_some() => self.editor.is_some(),
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
            Command::QuickOpen => {
                self.palette = match self.palette {
                    Some(_) => None,
                    None => Some(Palette::notes_only()),
                };
            }
            Command::SearchAll => {
                self.left_tab = LeftTab::Search;
                self.search.focus = true;
            }
            Command::VersionHistory => {
                if let Some(id) = self.editor.as_ref().map(|e| e.id.clone()) {
                    self.flush(notices);
                    self.dialog = Some(Dialog::Versions { id, selected: 0 });
                }
            }
            Command::MergeNote => {
                if let Some(id) = self.editor.as_ref().map(|e| e.id.clone()) {
                    self.dialog = Some(Dialog::Merge {
                        id,
                        query: String::new(),
                    });
                }
            }
            Command::ExtractSelection => self.open_extract_dialog(notices),
            Command::ExportPdf => {
                let Some(ed) = &self.editor else {
                    return;
                };
                let title = self
                    .project
                    .note(&ed.id)
                    .map(|n| n.meta.title.clone())
                    .unwrap_or_default();
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Exporter la fiche en PDF")
                    .add_filter("PDF", &["pdf"])
                    .set_file_name(format!(
                        "{}.pdf",
                        enigma_core::storage::file_stem_for_title(&title)
                    ))
                    .save_file()
                {
                    let id = ed.id.clone();
                    self.export_pdf(&id, &path, notices);
                }
            }
            Command::InsertTemplate => {
                if self.editor.is_some() {
                    self.view = View::Note;
                    self.dialog = Some(Dialog::InsertTemplate);
                }
            }
            Command::NavBack => self.nav_back(notices),
            Command::NavForward => self.nav_forward(notices),
            Command::CloseTab => {
                if let Some(id) = self.editor.as_ref().map(|e| e.id.clone()) {
                    self.close_tab(&id, notices);
                }
            }
            Command::NextTab => {
                let cur = self.editor.as_ref().map(|e| e.id.clone());
                let n = self.tab_ids.len();
                if let Some(p) = cur.and_then(|c| self.tab_ids.iter().position(|t| *t == c))
                    && n > 1
                {
                    let next = self.tab_ids[(p + 1) % n].clone();
                    self.activate_tab(&next, notices);
                }
            }
            Command::SplitView => {
                self.view = View::Note;
                self.toggle_split(notices);
            }
            Command::FindInNote => {
                self.view = View::Note;
                if let Some(ed) = &mut self.editor {
                    ed.open_find();
                }
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
                self.left_tab = LeftTab::Notes;
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
                    .set_title("Insérer une image ou un fichier")
                    .add_filter("Images", enigma_core::import::IMAGE_EXTENSIONS)
                    .add_filter("Sons", enigma_core::import::AUDIO_EXTENSIONS)
                    .add_filter("Vidéos", enigma_core::import::VIDEO_EXTENSIONS)
                    .add_filter("Tous les fichiers", &["*"])
                    .pick_file()
                {
                    self.insert_image(&file, notices);
                }
            }
            Command::FormatBold
            | Command::FormatItalic
            | Command::FormatStrike
            | Command::FormatCode
            | Command::FormatLink
            | Command::Heading1
            | Command::Heading2
            | Command::Heading3
            | Command::BulletList
            | Command::NumberedList
            | Command::TaskList
            | Command::QuoteBlock
            | Command::CodeBlock
            | Command::InsertTable
            | Command::InsertRule
            | Command::InsertDate => {
                if let Some(f) = commands::format_of(cmd) {
                    if self.view != View::Note {
                        notices.info("Ouvrez une fiche pour la mettre en forme.");
                    } else if let Some(editor) = &mut self.editor {
                        editor.format(f, &mut self.project, self.now_ms);
                        self.mark_edited();
                    }
                }
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

    /// Ouvre une fiche dans l'onglet courant (ou passe à son onglet) ; la
    /// fiche quittée va dans l'historique (retour : Alt+Gauche).
    pub fn select(&mut self, id: &NoteId, notices: &mut Notices) {
        if self.editor.as_ref().is_some_and(|e| &e.id == id) {
            return;
        }
        self.flush(notices);
        if let Some(cur) = self.editor.as_ref().map(|e| e.id.clone()) {
            self.back.push(cur);
            self.forward.clear();
            if self.back.len() > 100 {
                self.back.remove(0);
            }
        }
        self.show_in_current_tab(id);
    }

    /// Une fiche dans un volet : en-tête, barre d'outils et corps.
    fn note_pane(
        ui: &mut egui::Ui,
        editor: &mut Editor,
        project: &mut Project,
        user_packs: Option<&Path>,
        notices: &mut Notices,
        now_ms: u64,
    ) -> PaneOutcome {
        let mut out = PaneOutcome::default();
        let edited = editor.header_ui(ui, project, now_ms, notices, user_packs);

        if project.entry(&editor.id).is_some_and(|e| e.has_conflict()) {
            let mut choice = None;
            egui::Frame::new()
                .fill(ui.visuals().error_fg_color.gamma_multiply(0.15))
                .inner_margin(8.0)
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
                match project.resolve_conflict(&editor.id, keep_mine) {
                    Ok(()) => editor.sync_from_project(project, now_ms),
                    Err(e) => notices.error(format!("Résolution du conflit impossible : {e}")),
                }
            }
        }

        let mut insert_table = None;
        ui.horizontal(|ui| {
            if ui
                .button("Insérer…")
                .on_hover_text(
                    "Image, son, vidéo ou document — ou glissez/collez (Ctrl+V) le fichier",
                )
                .clicked()
            {
                out.insert_image = true;
            }
            ui.menu_button("Tableau", |ui| {
                if let Some(size) = crate::editor::table_picker(ui) {
                    insert_table = Some(size);
                    ui.close();
                }
            })
            .response
            .on_hover_text("Insérer un tableau (redimensionnable ensuite par son coin)");
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Cliquez dans le texte pour le modifier ; Échap pour terminer ; / pour insérer.",
                    )
                    .weak(),
                )
                .truncate(),
            );
        });
        ui.separator();
        if let Some((r, c)) = insert_table {
            editor.format(crate::editor::Format::Table(r, c), project, now_ms);
        }
        let body = editor.body_ui(ui, project, now_ms, notices);
        out.insert_image |= body.insert_file;
        out.edited = edited || body.edited || insert_table.is_some();
        out.insert_template = body.insert_template;
        out.search_tag = body.search_tag;
        out.extract = body.extract;
        out.open = body.open.map(|id| (id, body.open_section));
        out
    }

    /// Applique ce qu'a demandé un volet.
    fn apply_pane(&mut self, out: PaneOutcome, ctx: &egui::Context, notices: &mut Notices) {
        if out.insert_image {
            self.run_command(Command::InsertImage, ctx, notices);
        }
        if out.edited {
            self.mark_edited();
        }
        if out.insert_template {
            self.dialog = Some(Dialog::InsertTemplate);
        }
        if out.extract {
            self.open_extract_dialog(notices);
        }
        if let Some(tag) = out.search_tag {
            self.search_tag(&tag);
        }
        if let Some((target, section)) = out.open {
            if ctx.input(|i| i.modifiers.command) {
                self.open_in_new_tab(&target, notices);
            } else {
                self.select(&target, notices);
            }
            self.reveal_section(&target, section.as_deref());
        }
    }

    /// Barre des onglets : fiches ouvertes, retour/suivant, côte à côte.
    fn tabs_ui(&mut self, ui: &mut egui::Ui, notices: &mut Notices) {
        let active = self.editor.as_ref().map(|e| e.id.clone());
        let mut activate = None;
        let mut close = None;
        let mut back = false;
        let mut forward = false;
        let mut split = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if ui
                .add_enabled(!self.back.is_empty(), egui::Button::new("<").small())
                .on_hover_text("Fiche précédente (Alt+Gauche)")
                .clicked()
            {
                back = true;
            }
            if ui
                .add_enabled(!self.forward.is_empty(), egui::Button::new(">").small())
                .on_hover_text("Fiche suivante (Alt+Droite)")
                .clicked()
            {
                forward = true;
            }
            ui.add_space(6.0);
            for id in &self.tab_ids {
                let Some(title) = self.project.note(id).map(|n| n.meta.title.clone()) else {
                    continue;
                };
                let on = active.as_ref() == Some(id);
                let r = ui
                    .selectable_label(on, title)
                    .on_hover_text("Clic du milieu : fermer");
                if r.clicked() {
                    activate = Some(id.clone());
                }
                if r.middle_clicked() {
                    close = Some(id.clone());
                }
                if ui
                    .add(egui::Button::new("×").small().frame(false))
                    .on_hover_text("Fermer l'onglet (Ctrl+W)")
                    .clicked()
                {
                    close = Some(id.clone());
                }
                ui.add_space(4.0);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let on = self.other.is_some();
                if ui
                    .selectable_label(on, "Côte à côte")
                    .on_hover_text("Afficher une deuxième fiche à côté")
                    .clicked()
                {
                    split = true;
                }
            });
        });
        ui.separator();
        if back {
            self.nav_back(notices);
        }
        if forward {
            self.nav_forward(notices);
        }
        if let Some(id) = activate {
            self.activate_tab(&id, notices);
        }
        if let Some(id) = close {
            self.close_tab(&id, notices);
        }
        if split {
            self.toggle_split(notices);
        }
    }

    /// Range la fiche active dans les onglets en arrière-plan.
    fn park_active(&mut self) {
        if let Some(ed) = self.editor.take() {
            self.parked.insert(ed.id.clone(), ed);
        }
    }

    /// Passe à l'onglet de cette fiche (l'ouvre s'il n'existe pas).
    fn activate_tab(&mut self, id: &NoteId, notices: &mut Notices) {
        if self.editor.as_ref().is_some_and(|e| &e.id == id) {
            return;
        }
        self.flush(notices);
        self.park_active();
        let ed = match self.parked.remove(id) {
            Some(e) => Some(e),
            None => self.new_editor(id),
        };
        if let Some(mut ed) = ed {
            ed.sync_from_project(&self.project, self.now_ms);
            if !self.tab_ids.contains(id) {
                self.tab_ids.push(id.clone());
            }
            self.editor = Some(ed);
        }
    }

    /// Montre une fiche dans l'onglet courant (ou passe à son onglet).
    fn show_in_current_tab(&mut self, id: &NoteId) {
        if self.tab_ids.contains(id) {
            self.park_active();
            let ed = match self.parked.remove(id) {
                Some(e) => Some(e),
                None => self.new_editor(id),
            };
            if let Some(mut ed) = ed {
                ed.sync_from_project(&self.project, self.now_ms);
                self.editor = Some(ed);
            }
            return;
        }
        let current = self.editor.as_ref().map(|e| e.id.clone());
        match current.and_then(|c| self.tab_ids.iter().position(|t| *t == c)) {
            Some(p) => self.tab_ids[p] = id.clone(),
            None => self.tab_ids.push(id.clone()),
        }
        self.editor = self.new_editor(id);
    }

    /// Ouvre une fiche dans un nouvel onglet (après l'onglet actif).
    pub fn open_in_new_tab(&mut self, id: &NoteId, notices: &mut Notices) {
        if !self.tab_ids.contains(id) {
            let at = self
                .editor
                .as_ref()
                .and_then(|e| self.tab_ids.iter().position(|t| *t == e.id))
                .map_or(self.tab_ids.len(), |p| p + 1);
            self.tab_ids.insert(at, id.clone());
        }
        self.activate_tab(id, notices);
        self.view = View::Note;
    }

    /// Ferme l'onglet d'une fiche ; l'onglet voisin devient actif.
    fn close_tab(&mut self, id: &NoteId, notices: &mut Notices) {
        let Some(p) = self.tab_ids.iter().position(|t| t == id) else {
            return;
        };
        self.flush(notices);
        self.tab_ids.remove(p);
        self.parked.remove(id);
        if self.editor.as_ref().is_some_and(|e| &e.id == id) {
            self.editor = None;
            if let Some(next) = self
                .tab_ids
                .get(p.min(self.tab_ids.len().saturating_sub(1)))
            {
                let next = next.clone();
                self.activate_tab(&next, notices);
            }
        }
    }

    /// Oublie une fiche supprimée (onglets, volet, historique).
    fn forget_note(&mut self, id: &NoteId) {
        self.tab_ids.retain(|t| t != id);
        self.parked.remove(id);
        if self.other.as_ref().is_some_and(|e| &e.id == id) {
            self.other = None;
        }
        self.back.retain(|t| t != id);
        self.forward.retain(|t| t != id);
    }

    fn nav_back(&mut self, notices: &mut Notices) {
        if let Some(prev) = self.back.pop() {
            self.flush(notices);
            if let Some(cur) = self.editor.as_ref().map(|e| e.id.clone()) {
                self.forward.push(cur);
            }
            self.show_in_current_tab(&prev);
            self.view = View::Note;
        }
    }

    fn nav_forward(&mut self, notices: &mut Notices) {
        if let Some(next) = self.forward.pop() {
            self.flush(notices);
            if let Some(cur) = self.editor.as_ref().map(|e| e.id.clone()) {
                self.back.push(cur);
            }
            self.show_in_current_tab(&next);
            self.view = View::Note;
        }
    }

    /// Ouvre ou ferme le second volet (avec la fiche précédente, ou la même).
    fn toggle_split(&mut self, notices: &mut Notices) {
        if self.other.take().is_some() {
            return;
        }
        let Some(cur) = self.editor.as_ref().map(|e| e.id.clone()) else {
            return;
        };
        let pick = self
            .back
            .iter()
            .rev()
            .find(|id| **id != cur)
            .cloned()
            .unwrap_or(cur);
        self.open_beside(&pick, notices);
    }

    /// Montre une fiche dans le volet de droite.
    pub fn open_beside(&mut self, id: &NoteId, notices: &mut Notices) {
        self.flush(notices);
        self.other = self.new_editor(id);
        self.other_right = true;
        self.view = View::Note;
    }

    /// Nouvel éditeur, avec un identifiant d'affichage propre (deux volets
    /// peuvent montrer la même fiche).
    fn new_editor(&mut self, id: &NoteId) -> Option<Editor> {
        self.next_salt += 1;
        let salt = self.next_salt;
        Editor::open(&self.project, id).map(|mut e| {
            e.set_salt(salt);
            e
        })
    }

    /// Écrit une fiche en PDF (images de la fiche, formules et diagrammes
    /// compris).
    pub fn export_pdf(&mut self, id: &NoteId, path: &Path, notices: &mut Notices) {
        let Some(note) = self.project.note(id) else {
            return;
        };
        let root = self.project.root().to_owned();
        let note_path = self
            .project
            .entry(id)
            .map(|e| e.path.clone())
            .unwrap_or_default();
        let load = |src: &str| {
            enigma_core::import::resolve_image(&root, &note_path, src)
                .and_then(|p| std::fs::read(p).ok())
        };
        match enigma_typeset::note_pdf(&note.meta.title, &note.body, &load) {
            Ok(bytes) => match enigma_core::storage::write_atomic(path, &bytes) {
                Ok(()) => notices.info(format!("PDF enregistré : {}", path.display())),
                Err(e) => notices.error(format!("PDF non enregistré : {e}")),
            },
            Err(e) => notices.error(format!("Export PDF impossible : {e}")),
        }
    }

    /// Demande le titre de la fiche à extraire de la sélection.
    fn open_extract_dialog(&mut self, notices: &mut Notices) {
        let Some(sel) = self.editor.as_ref().map(|e| e.selection_text()) else {
            return;
        };
        if sel.trim().is_empty() {
            notices.info("Sélectionnez d'abord le texte à extraire.");
            return;
        }
        let first = sel
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim_start_matches(['#', '-', '*', '>', ' '])
            .chars()
            .take(60)
            .collect::<String>();
        self.view = View::Note;
        self.dialog = Some(Dialog::Extract {
            title: first.trim().to_owned(),
            error: None,
        });
    }

    /// Montre les fiches qui portent ce tag (onglet Recherche).
    fn search_tag(&mut self, tag: &str) {
        self.left_tab = LeftTab::Search;
        self.search.query = format!("tag:{tag}");
    }

    /// Onglet « Tags » : tags du projet et nombre de fiches ; rend le tag
    /// cliqué.
    fn tags_ui(&mut self, ui: &mut egui::Ui) -> Option<String> {
        let key = (self.project.generation(), self.project.content_version());
        if self.tags.as_ref().map(|(g, c, _)| (*g, *c)) != Some(key) {
            let all = self.project.all_tags().into_iter().collect();
            self.tags = Some((key.0, key.1, all));
        }
        let tags = self
            .tags
            .as_ref()
            .map(|(_, _, t)| t.as_slice())
            .unwrap_or_default();
        if tags.is_empty() {
            ui.weak("Aucun tag. Écrivez #nom dans une fiche (#lieu/manoir pour ranger).");
            return None;
        }
        let mut clicked = None;
        let accent = crate::theme::accent(ui.visuals());
        egui::ScrollArea::vertical()
            .id_salt("tags_scroll")
            .auto_shrink(false)
            .show(ui, |ui| {
                for (tag, n) in tags {
                    let depth = tag.matches('/').count();
                    let short = tag.rsplit('/').next().unwrap_or(tag);
                    ui.horizontal(|ui| {
                        ui.add_space(14.0 * depth as f32);
                        let r = ui
                            .add(
                                egui::Label::new(RichText::new(format!("#{short}")).color(accent))
                                    .sense(Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(format!("Fiches avec #{tag}"));
                        if r.clicked() {
                            clicked = Some(tag.clone());
                        }
                        ui.weak(n.to_string());
                    });
                }
            });
        clicked
    }

    /// Place l'éditeur sur une section (`Titre`) ou un paragraphe (`^id`)
    /// de la fiche ouverte.
    fn reveal_section(&mut self, id: &NoteId, section: Option<&str>) {
        let Some(section) = section else {
            return;
        };
        let at = self
            .project
            .note(id)
            .and_then(|n| enigma_core::sections::locate(&n.body, Some(section)));
        if let (Some(r), Some(ed)) = (at, &mut self.editor) {
            let line_end = ed_line_end(ed.body_text(), r.start);
            ed.reveal(line_end);
        }
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
        self.handle_pasted_images(&ui.ctx().clone(), notices);
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
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.left_tab, LeftTab::Notes, "Fiches");
                    ui.selectable_value(&mut self.left_tab, LeftTab::Search, "Recherche")
                        .on_hover_text(
                            "Rechercher dans le texte de toutes les fiches (Ctrl+Maj+F)",
                        );
                    ui.selectable_value(&mut self.left_tab, LeftTab::Tags, "Tags")
                        .on_hover_text("Tous les #tags du projet");
                });
                ui.separator();
                match self.left_tab {
                    LeftTab::Notes => self.list_ui(ui, notices),
                    LeftTab::Tags => {
                        if let Some(tag) = self.tags_ui(ui) {
                            self.search_tag(&tag);
                        }
                    }
                    LeftTab::Search => {
                        if let Some((id, at)) = self.search.ui(ui, &self.project) {
                            self.view = View::Note;
                            self.select(&id, notices);
                            if let (Some(b), Some(ed)) = (at, &mut self.editor) {
                                ed.reveal(b);
                            }
                        }
                    }
                }
            });

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
                                Some(ed) => ed.fields_ui(
                                    ui,
                                    &mut self.project,
                                    now_ms,
                                    notices,
                                    self.user_packs_dir.as_deref(),
                                ),
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
                            if let Some(ed) = &mut self.editor {
                                ed.outline_ui(ui);
                            }
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
                    self.forget_note(&id);
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
            PaletteOutcome::Run(PaletteItem::Create(title)) => {
                match self.project.create_note(&title, Vec::new(), String::new()) {
                    Ok(id) => {
                        self.select(&id, notices);
                        self.view = View::Note;
                    }
                    Err(e) => notices.error(format!("Création impossible : {e}")),
                }
            }
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
            self.list_version += 1;
        }
        self.refresh_rows();
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
        ui.horizontal(|ui| {
            ui.weak(if filtering {
                format!("{count} résultat(s) sur {}", self.project.len())
            } else {
                format!("{count} fiche(s)")
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(
                    &mut self.group_by_category,
                    RichText::new("Par catégorie").small(),
                )
                .on_hover_text(
                    "Ranger la liste par catégorie (une fiche à plusieurs catégories \
                         apparaît dans chacune)",
                );
            });
        });
        self.refresh_rows();
        let count = self.rows.len();

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
        let mut to_toggle: Option<String> = None;
        let mut to_tab = None;
        let mut to_beside = None;
        let selected = self.editor.as_ref().map(|e| e.id.clone());
        egui::ScrollArea::vertical()
            .id_salt("notes_scroll")
            .auto_shrink(false)
            .max_height((ui.available_height() - bottom_reserved).max(row_h))
            .show_rows(ui, row_h, count, |ui, range| {
                for row in &self.rows[range] {
                    let id = match row {
                        ListRow::Header {
                            key,
                            label,
                            color,
                            count,
                            open,
                        } => {
                            if header_row(ui, label, *color, *count, *open, row_h).clicked() {
                                to_toggle = Some(key.clone());
                            }
                            continue;
                        }
                        ListRow::Note(id) => id,
                    };
                    let indent = if self.group_by_category { 12.0 } else { 0.0 };
                    let resp = note_row(
                        ui,
                        &self.project,
                        id,
                        selected.as_ref() == Some(id),
                        row_h,
                        indent,
                    );
                    if resp.clicked() && ui.input(|i| i.modifiers.command) || resp.middle_clicked()
                    {
                        to_tab = Some(id.clone());
                    } else if resp.clicked() {
                        to_select = Some(id.clone());
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Ouvrir dans un nouvel onglet").clicked() {
                            to_tab = Some(id.clone());
                            ui.close();
                        }
                        if ui.button("Ouvrir à côté").clicked() {
                            to_beside = Some(id.clone());
                            ui.close();
                        }
                        ui.separator();
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

        if let Some(key) = to_toggle
            && !self.collapsed.remove(&key)
        {
            self.collapsed.insert(key);
        }
        if let Some(id) = to_select {
            self.select(&id, notices);
        }
        if let Some(id) = to_tab {
            self.open_in_new_tab(&id, notices);
        }
        if let Some(id) = to_beside {
            self.open_beside(&id, notices);
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

    /// Recalcule les lignes de la liste si la liste, le rangement ou les
    /// groupes repliés ont changé.
    fn refresh_rows(&mut self) {
        let mut collapsed: Vec<String> = self.collapsed.iter().cloned().collect();
        collapsed.sort();
        let key = (
            self.list_version ^ (self.project.content_version() << 20),
            self.group_by_category,
            collapsed,
        );
        if self.rows_key.as_ref() == Some(&key) {
            return;
        }
        self.rows_key = Some(key);
        if !self.group_by_category {
            self.rows = self.list.iter().cloned().map(ListRow::Note).collect();
            return;
        }
        let pack = self.project.pack();
        let order: Vec<String> = pack.categories.iter().map(|c| c.id.clone()).collect();
        let groups = enigma_core::filter::group_by_category(
            &self.list,
            |id| {
                self.project
                    .note(id)
                    .map(|n| n.meta.categories.clone())
                    .unwrap_or_default()
            },
            &order,
        );
        let mut rows = Vec::new();
        for g in groups {
            let key = g
                .category
                .as_ref()
                .map_or_else(|| "none".to_owned(), |c| format!("cat:{c}"));
            let (label, color) = match &g.category {
                Some(c) => match pack.category(c) {
                    Some(def) => (def.label.clone(), Some(c32(def.color))),
                    None => (c.clone(), None),
                },
                None => ("Sans catégorie".to_owned(), None),
            };
            let open = !self.collapsed.contains(&key);
            rows.push(ListRow::Header {
                key,
                label,
                color,
                count: g.notes.len(),
                open,
            });
            if open {
                rows.extend(g.notes.into_iter().map(ListRow::Note));
            }
        }
        self.rows = rows;
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
            if out.add_image
                && let Some(file) = rfd::FileDialog::new()
                    .set_title("Poser une image sur le tableau")
                    .add_filter("Images", enigma_core::import::IMAGE_EXTENSIONS)
                    .pick_file()
            {
                self.add_board_image(&file, notices);
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
                self.park_active();
            }
            return;
        }
        if self.view == View::Graph {
            let selected = self.editor.as_ref().map(|e| e.id.clone());
            let outcome = self.graph.ui(ui, &self.project, selected.as_ref());
            self.apply_graph_outcome(outcome, notices, now_ms);
            return;
        }
        if self.editor.is_none() {
            if !self.tab_ids.is_empty() {
                self.tabs_ui(ui, notices);
            }
            ui.centered_and_justified(|ui| {
                ui.weak(if self.project.is_empty() {
                    "Aucune fiche. Créez-en une avec « + Nouvelle » (Ctrl+N)."
                } else {
                    "Sélectionnez une fiche dans la liste."
                });
            });
            return;
        }
        self.tabs_ui(ui, notices);
        let ctx = ui.ctx().clone();
        let user_packs = self.user_packs_dir.clone();
        let (Some(editor), other) = (&mut self.editor, &mut self.other) else {
            return;
        };
        let mut outs = Vec::new();
        let mut focus_other = false;
        match other {
            None => {
                outs.push(Self::note_pane(
                    ui,
                    editor,
                    &mut self.project,
                    user_packs.as_deref(),
                    notices,
                    now_ms,
                ));
            }
            Some(other) => {
                let right = self.other_right;
                let press = ctx.input(|i| {
                    i.pointer
                        .any_pressed()
                        .then(|| i.pointer.press_origin())
                        .flatten()
                });
                ui.columns(2, |cols| {
                    let (main_col, other_col) = if right {
                        let [a, b] = cols else { return };
                        (a, b)
                    } else {
                        let [a, b] = cols else { return };
                        (b, a)
                    };
                    let main_rect = main_col.max_rect();
                    let other_rect = other_col.max_rect();
                    main_col.push_id("volet_actif", |ui| {
                        ui.painter().hline(
                            main_rect.x_range(),
                            main_rect.top(),
                            egui::Stroke::new(2.0, crate::theme::accent(ui.visuals())),
                        );
                        outs.push(Self::note_pane(
                            ui,
                            editor,
                            &mut self.project,
                            user_packs.as_deref(),
                            notices,
                            now_ms,
                        ));
                    });
                    other_col.push_id("volet_autre", |ui| {
                        outs.push(Self::note_pane(
                            ui,
                            other,
                            &mut self.project,
                            user_packs.as_deref(),
                            notices,
                            now_ms,
                        ));
                    });
                    // Un clic dans l'autre volet le rend actif (champs,
                    // liens, raccourcis).
                    focus_other =
                        press.is_some_and(|p| other_rect.contains(p) && !main_rect.contains(p));
                });
                // Même fiche dans les deux volets : chacun reprend le texte
                // de l'autre.
                if other.id == editor.id {
                    other.sync_from_project(&self.project, now_ms);
                    editor.sync_from_project(&self.project, now_ms);
                }
            }
        }
        if focus_other {
            std::mem::swap(&mut self.editor, &mut self.other);
            self.other_right = !self.other_right;
            if let Some(id) = self.editor.as_ref().map(|e| e.id.clone())
                && !self.tab_ids.contains(&id)
            {
                // Le volet actif devient l'onglet courant.
                let old = self.other.as_ref().map(|e| e.id.clone());
                match old.and_then(|o| self.tab_ids.iter().position(|t| *t == o)) {
                    Some(p) => self.tab_ids.insert(p + 1, id),
                    None => self.tab_ids.push(id),
                }
            }
        }
        for out in outs {
            self.apply_pane(out, &ctx, notices);
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
        if let Some(item) = outcome.new_link_type {
            let mut id = String::new();
            match self
                .project
                .update_pack(self.user_packs_dir.as_deref(), |p| {
                    id = p.add_link_type(&item.label, item.color, item.directed);
                }) {
                Ok(()) => {
                    self.graph.set_dialog_kind(Some(id));
                    self.disk_changed = true;
                    notices.info(format!("Type de lien « {} » ajouté au pack.", item.label));
                }
                Err(e) => notices.error(format!("Type de lien non créé : {e}")),
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
            self.park_active();
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

        // Mentions non liées (recalculées seulement si le projet change).
        let key = (
            id.clone(),
            self.project.generation(),
            self.project.content_version(),
        );
        if self.mentions.as_ref().map(|(k, _)| k) != Some(&key) {
            let found = self.project.unlinked_mentions(&id);
            self.mentions = Some((key, found));
        }
        let mentions = self
            .mentions
            .as_ref()
            .map(|(_, m)| m.clone())
            .unwrap_or_default();
        let mut to_link: Vec<enigma_core::mentions::Mention> = Vec::new();
        egui::CollapsingHeader::new(format!("Mentions non liées ({})", mentions.len()))
            .id_salt(("mentions", id.as_str()))
            .default_open(!mentions.is_empty() && mentions.len() <= 8)
            .show(ui, |ui| {
                if mentions.is_empty() {
                    ui.weak("Aucune : le titre n'est cité nulle part sans lien.");
                    return;
                }
                if mentions.len() > 1 && ui.button("Tout lier").clicked() {
                    to_link = mentions.clone();
                }
                let accent = crate::theme::accent(ui.visuals());
                for m in &mentions {
                    ui.horizontal(|ui| {
                        if ui
                            .link(RichText::new(&m.from_title).strong())
                            .on_hover_text("Ouvrir cette fiche")
                            .clicked()
                        {
                            open = Some(m.from.clone());
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .small_button("Lier")
                                .on_hover_text("Remplacer ce texte par un lien [[…]]")
                                .clicked()
                            {
                                to_link.push(m.clone());
                            }
                        });
                    });
                    let mut job = egui::text::LayoutJob::default();
                    let small = egui::FontId::proportional(12.0);
                    let color = ui.visuals().text_color();
                    let fmt = egui::text::TextFormat::simple(small.clone(), color);
                    let mut hl = fmt.clone();
                    hl.background = accent.gamma_multiply(0.45);
                    let line = &m.line;
                    let s = line[..m.in_line.start]
                        .char_indices()
                        .rev()
                        .nth(40)
                        .map_or(0, |(b, _)| b);
                    let e = line[m.in_line.end..]
                        .char_indices()
                        .nth(60)
                        .map_or(line.len(), |(b, _)| m.in_line.end + b);
                    job.append(if s > 0 { "…" } else { "" }, 0.0, fmt.clone());
                    job.append(&line[s..m.in_line.start], 0.0, fmt.clone());
                    job.append(&line[m.in_line.clone()], 0.0, hl);
                    job.append(&line[m.in_line.end..e], 0.0, fmt.clone());
                    job.append(if e < line.len() { "…" } else { "" }, 0.0, fmt);
                    job.wrap.max_width = ui.available_width();
                    ui.label(job);
                    ui.add_space(4.0);
                }
            });
        if !to_link.is_empty() {
            match self.project.link_mentions(&to_link) {
                Ok(n) => {
                    notices.info(format!("{n} mention(s) transformée(s) en lien."));
                    self.mark_edited();
                    if let Some(ed) = &mut self.editor {
                        ed.sync_from_project(&self.project, self.now_ms);
                    }
                }
                Err(e) => notices.error(format!("Lien impossible : {e}")),
            }
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
        let mut insert_template: Option<enigma_core::pack::Template> = None;
        let mut restore: Option<(NoteId, String)> = None;
        let mut merge: Option<(NoteId, NoteId)> = None;
        let mut extract: Option<String> = None;
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
                            let vars = crate::project_file::template_vars();
                            match self.project.create_from_template(title, chosen, &vars) {
                                Ok(id) => {
                                    let cursor = chosen.and_then(|t| {
                                        enigma_core::pack::expand_template(
                                            &t.body,
                                            title.trim(),
                                            &vars,
                                        )
                                        .1
                                    });
                                    created = Some((id, cursor));
                                }
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
            Dialog::Versions { id, selected } => {
                let versions = enigma_core::versions::list(self.project.root(), id);
                let current = self
                    .project
                    .note(id)
                    .and_then(|n| n.serialize().ok())
                    .unwrap_or_default();
                let title = self
                    .project
                    .note(id)
                    .map(|n| n.meta.title.clone())
                    .unwrap_or_default();
                let resp = egui::Modal::new(egui::Id::unique("versions")).show(ctx, |ui| {
                    ui.set_width(760.0);
                    ui.heading(format!("Historique : {title}"));
                    if versions.is_empty() {
                        ui.weak(
                            "Aucune version gardée pour l'instant : une copie est prise à \
                             l'enregistrement, au plus toutes les 10 minutes.",
                        );
                    }
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(200.0);
                            egui::ScrollArea::vertical()
                                .id_salt("versions_list")
                                .max_height(420.0)
                                .show(ui, |ui| {
                                    for (k, v) in versions.iter().enumerate() {
                                        let when = crate::project_file::file_date(
                                            std::time::UNIX_EPOCH
                                                + std::time::Duration::from_secs(v.time),
                                        );
                                        let label = format!("{when} ({} o)", v.size);
                                        if ui.selectable_label(*selected == k, label).clicked() {
                                            *selected = k;
                                        }
                                    }
                                });
                        });
                        ui.add_space(12.0);
                        ui.vertical(|ui| {
                            let Some(v) = versions.get(*selected) else {
                                return;
                            };
                            let old = enigma_core::versions::read(v).unwrap_or_default();
                            ui.label(
                                RichText::new("Différences avec la fiche actuelle (rouge : seulement dans cette version, vert : ajouté depuis)")
                                    .small()
                                    .weak(),
                            );
                            egui::ScrollArea::vertical()
                                .id_salt("versions_diff")
                                .max_height(400.0)
                                .show(ui, |ui| {
                                    use enigma_core::versions::Diff;
                                    let red = ui.visuals().error_fg_color;
                                    let green = Color32::from_rgb(0x27, 0xae, 0x60);
                                    for d in enigma_core::versions::line_diff(&old, &current) {
                                        let (prefix, text, color) = match d {
                                            Diff::Same(t) => ("  ", t, ui.visuals().weak_text_color()),
                                            Diff::Removed(t) => ("- ", t, red),
                                            Diff::Added(t) => ("+ ", t, green),
                                        };
                                        ui.label(
                                            RichText::new(format!("{prefix}{text}"))
                                                .monospace()
                                                .color(color),
                                        );
                                    }
                                });
                        });
                    });
                    ui.horizontal(|ui| {
                        if let Some(v) = versions.get(*selected)
                            && ui
                                .button("Restaurer cette version")
                                .on_hover_text("Annulable avec Ctrl+Z")
                                .clicked()
                        {
                            restore = Some((
                                id.clone(),
                                enigma_core::versions::read(v).unwrap_or_default(),
                            ));
                        }
                        if ui.button("Fermer").clicked() {
                            close = true;
                        }
                    });
                });
                if resp.should_close() {
                    close = true;
                }
            }
            Dialog::Merge { id, query } => {
                let resp = egui::Modal::new(egui::Id::unique("merge_note")).show(ctx, |ui| {
                    ui.set_width(380.0);
                    let title = self
                        .project
                        .note(id)
                        .map(|n| n.meta.title.clone())
                        .unwrap_or_default();
                    ui.heading(format!("Fusionner « {title} » avec…"));
                    ui.label(
                        RichText::new(
                            "Le texte, les catégories, les champs et les liens vont dans la \
                             fiche choisie ; les liens qui visaient cette fiche visent la \
                             nouvelle ; cette fiche va à la corbeille.",
                        )
                        .small()
                        .weak(),
                    );
                    ui.add(
                        egui::TextEdit::singleline(query)
                            .hint_text("Fiche de destination")
                            .desired_width(360.0),
                    )
                    .request_focus();
                    for other in self.project.search(query).into_iter().take(10) {
                        if other == *id {
                            continue;
                        }
                        let Some(n) = self.project.note(&other) else {
                            continue;
                        };
                        if ui.button(&n.meta.title).clicked() {
                            merge = Some((id.clone(), other.clone()));
                        }
                    }
                    if ui.button("Annuler").clicked() {
                        close = true;
                    }
                });
                if resp.should_close() {
                    close = true;
                }
            }
            Dialog::Extract { title, error } => {
                let resp = egui::Modal::new(egui::Id::unique("extract_note")).show(ctx, |ui| {
                    ui.set_width(380.0);
                    ui.heading("Extraire la sélection");
                    ui.label(
                        RichText::new(
                            "Le texte sélectionné devient une nouvelle fiche ; il est \
                             remplacé ici par un lien vers elle.",
                        )
                        .small()
                        .weak(),
                    );
                    let edit = ui.add(
                        egui::TextEdit::singleline(title)
                            .hint_text("Titre de la nouvelle fiche")
                            .desired_width(360.0),
                    );
                    edit.request_focus();
                    if let Some(e) = error {
                        ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                    }
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        if ui.button("Extraire").clicked() || enter {
                            extract = Some(title.clone());
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
            Dialog::InsertTemplate => {
                let templates = self.project.pack().templates.clone();
                let resp = egui::Modal::new(egui::Id::unique("insert_template")).show(ctx, |ui| {
                    ui.set_width(340.0);
                    ui.heading("Insérer un modèle");
                    if templates.is_empty() {
                        ui.weak("Le pack n'a aucun modèle (Modifier le pack de jeu… › Modèles).");
                    }
                    for t in &templates {
                        if ui.button(&t.label).clicked() {
                            insert_template = Some(t.clone());
                        }
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Variables : {{titre}}, {{date}}, {{heure}}, {{curseur}} (position du curseur)",
                        )
                        .small()
                        .weak(),
                    );
                    if ui.button("Annuler").clicked() {
                        close = true;
                    }
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

        if let Some((id, text)) = restore {
            self.dialog = None;
            match enigma_core::Note::parse(&text, "") {
                Ok(old) => {
                    let mut meta = old.meta;
                    if let Some(cur) = self.project.note(&id) {
                        // Le titre (et le fichier) restent ceux d'aujourd'hui.
                        meta.title = cur.meta.title.clone();
                        meta.id = cur.meta.id.clone();
                    }
                    let res = self
                        .project
                        .set_meta(&id, meta)
                        .and_then(|()| self.project.set_body(&id, old.body));
                    match res {
                        Ok(()) => {
                            if let Some(ed) = self.editor.as_mut().filter(|e| e.id == id) {
                                ed.sync_from_project(&self.project, self.now_ms);
                            }
                            self.mark_edited();
                            notices.info("Version restaurée (Ctrl+Z pour revenir).");
                        }
                        Err(e) => notices.error(format!("Restauration impossible : {e}")),
                    }
                }
                Err(e) => notices.error(format!("Version illisible : {e}")),
            }
        } else if let Some((src, dst)) = merge {
            self.dialog = None;
            self.flush(notices);
            if self.editor.as_ref().is_some_and(|e| e.id == src) {
                self.editor = None;
            }
            self.forget_note(&src);
            match self.project.merge_into(&src, &dst) {
                Ok(updated) => {
                    notices.info(format!(
                        "Fiches fusionnées ; {} fiche(s) mise(s) à jour.",
                        updated.len()
                    ));
                    self.disk_changed = true;
                    self.select(&dst, notices);
                    for ed in self.parked.values_mut() {
                        ed.sync_from_project(&self.project, self.now_ms);
                    }
                }
                Err(e) => notices.error(format!("Fusion impossible : {e}")),
            }
        } else if let Some(title) = extract {
            let text = self
                .editor
                .as_ref()
                .map(|e| e.selection_text())
                .unwrap_or_default();
            match self.project.create_note(&title, Vec::new(), text) {
                Ok(new_id) => {
                    self.dialog = None;
                    let clean = self
                        .project
                        .note(&new_id)
                        .map(|n| n.meta.title.clone())
                        .unwrap_or(title);
                    if let Some(ed) = &mut self.editor {
                        ed.replace_selection(
                            &format!("[[{clean}]]"),
                            &mut self.project,
                            self.now_ms,
                        );
                    }
                    self.mark_edited();
                    notices.info(format!("Fiche « {clean} » créée à partir de la sélection."));
                }
                Err(e) => {
                    if let Some(Dialog::Extract { error, .. }) = &mut self.dialog {
                        *error = Some(e.to_string());
                    }
                }
            }
        } else if let Some(t) = insert_template {
            self.dialog = None;
            if let Some(ed) = &mut self.editor {
                let title = self
                    .project
                    .note(&ed.id)
                    .map(|n| n.meta.title.clone())
                    .unwrap_or_default();
                let vars = crate::project_file::template_vars();
                let (text, cursor) = enigma_core::pack::expand_template(&t.body, &title, &vars);
                ed.insert_template(&text, cursor, &mut self.project, self.now_ms);
                self.mark_edited();
            }
        } else if let Some((id, cursor)) = created {
            self.dialog = None;
            self.filter.text.clear();
            self.select(&id, notices);
            if let (Some(c), Some(ed)) = (cursor, &mut self.editor) {
                ed.reveal(c);
            }
        } else if let Some(id) = delete {
            self.dialog = None;
            if self.editor.as_ref().is_some_and(|e| e.id == id) {
                self.flush(notices);
                self.editor = None;
            }
            self.forget_note(&id);
            match self.project.delete(&id) {
                Ok(_) => notices.info("Fiche déplacée dans la corbeille du projet."),
                Err(e) => notices.error(format!("Suppression impossible : {e}")),
            }
        } else if close {
            self.dialog = None;
        }
    }
}

/// Fin de la ligne qui commence à `start`.
fn ed_line_end(body: &str, start: usize) -> usize {
    body.get(start..)
        .and_then(|t| t.find('\n'))
        .map_or(body.len(), |i| start + i)
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
/// En-tête de groupe de la liste (catégorie), cliquable pour replier.
fn header_row(
    ui: &mut egui::Ui,
    label: &str,
    color: Option<Color32>,
    count: usize,
    open: bool,
    h: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Groupe {label}"))
    });
    let visuals = ui.visuals();
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let cy = rect.center().y;
    let x = rect.left() + 6.0;
    // Triangle : vers le bas si ouvert, vers la droite si replié.
    let tri = if open {
        vec![
            egui::pos2(x, cy - 3.0),
            egui::pos2(x + 8.0, cy - 3.0),
            egui::pos2(x + 4.0, cy + 3.0),
        ]
    } else {
        vec![
            egui::pos2(x + 1.0, cy - 4.0),
            egui::pos2(x + 7.0, cy),
            egui::pos2(x + 1.0, cy + 4.0),
        ]
    };
    ui.painter().add(egui::Shape::convex_polygon(
        tri,
        visuals.weak_text_color(),
        egui::Stroke::NONE,
    ));
    let mut tx = x + 14.0;
    if let Some(c) = color {
        ui.painter().circle_filled(egui::pos2(tx + 4.0, cy), 4.0, c);
        tx += 12.0;
    }
    let text = format!("{label}  {count}");
    ui.painter().text(
        egui::pos2(tx, cy),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(13.0),
        visuals.strong_text_color(),
    );
    resp
}

fn note_row(
    ui: &mut egui::Ui,
    project: &Project,
    id: &NoteId,
    selected: bool,
    h: f32,
    indent: f32,
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
            .rect_filled(rect, 0.0, visuals.selection.bg_fill);
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 0.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let mut x = rect.left() + 8.0 + indent;
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
        // Premier dossier = packs de « l'utilisateur » : dossier jetable,
        // pour que les tests ne modifient jamais les packs du dépôt.
        let packs = vec![
            dir.join(".packs-utilisateur"),
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs"),
        ];
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
        crate::theme::install(&harness.ctx);
        let ctx = harness.ctx.clone();
        {
            let state = harness.state_mut();
            let mut ws = Workspace::open(dir, &packs, &ctx, &mut state.notices).unwrap();
            // Liste à plat : chaque fiche une seule fois (titres non ambigus).
            ws.group_by_category = false;
            state.ws = Some(ws);
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
        harness
            .state_mut()
            .ws
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .edit_at_end();
        harness.run_steps(4);
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
    fn live_editing_lines_tables_and_images() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        egui_extras::install_image_loaders(&harness.ctx);
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(
            dir.path().join("assets/plan.svg"),
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="200" height="100" fill="#4aa3ff"/></svg>"##,
        )
        .unwrap();
        let start = "# Plan\n\n- [ ] fouiller\n- [x] lire\n\n| ![a|90](assets/plan.svg) | ![b|90](assets/plan.svg) |\n|---|---|\n|  |  |\n\nFin";
        {
            let state = harness.state_mut();
            let ws = state.ws.as_mut().unwrap();
            let id = ws
                .project
                .create_note("Essai", vec![], start.to_owned())
                .unwrap();
            ws.select(&id, &mut state.notices);
        }
        for _ in 0..20 {
            harness.run_steps(2);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        fn editor<'a>(h: &'a mut Harness<'_, State>) -> &'a mut crate::editor::Editor {
            h.state_mut().ws.as_mut().unwrap().editor.as_mut().unwrap()
        }
        // Tout est mis en forme : aucun texte Markdown affiché.
        assert!(harness.query_by_role(Role::MultilineTextInput).is_none());
        snapshot(&mut harness, "30_lecture_directe");

        // Modifier la dernière ligne, Entrée : nouvelle ligne en édition.
        editor(&mut harness).edit_at_end();
        harness.run_steps(4);
        assert_eq!(
            harness
                .get_by_role(Role::MultilineTextInput)
                .value()
                .as_deref(),
            Some("Fin")
        );
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("\nsuite");
        harness.run_steps(4);
        assert!(body_text(&harness).ends_with("Fin\nsuite"));
        assert_eq!(
            harness
                .get_by_role(Role::MultilineTextInput)
                .value()
                .as_deref(),
            Some("suite")
        );
        // Retour arrière en début de ligne : fusion avec la précédente.
        harness.key_press(egui::Key::Home);
        harness.run_steps(2);
        harness.key_press(egui::Key::Backspace);
        harness.run_steps(4);
        assert!(
            body_text(&harness).ends_with("\nFinsuite"),
            "{}",
            body_text(&harness)
        );
        assert_eq!(
            harness
                .get_by_role(Role::MultilineTextInput)
                .value()
                .as_deref(),
            Some("Finsuite")
        );
        // Flèche haut depuis la première ligne : unité précédente.
        harness.key_press(egui::Key::ArrowUp);
        harness.run_steps(4);
        assert_eq!(
            harness
                .get_by_role(Role::MultilineTextInput)
                .value()
                .as_deref(),
            Some("")
        );

        // Tableau : frappe dans une cellule vide, colonnes réalignées.
        let body = body_text(&harness);
        let cell = body.find("|  |  |").unwrap() + 2;
        editor(&mut harness).edit_at(cell);
        harness.run_steps(4);
        harness.get_by_role(Role::MultilineTextInput).type_text("x");
        harness.run_steps(4);
        harness.key_press(egui::Key::Tab);
        harness.run_steps(4);
        harness.get_by_role(Role::MultilineTextInput).type_text("y");
        harness.run_steps(4);
        let body = body_text(&harness);
        let units = enigma_core::live::units(&body);
        let tab = units
            .iter()
            .find(|u| u.kind == enigma_core::live::UnitKind::Table)
            .unwrap();
        let text = &body[tab.range.clone()];
        let widths: Vec<usize> = text.lines().map(|l| l.chars().count()).collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{text}");
        let p = enigma_core::table::parse(text).unwrap();
        assert_eq!(p.table.rows[1], ["x", "y"], "{text}");
        snapshot(&mut harness, "31_tableau_edition");

        // Échap : fin de la modification, tout est de nouveau mis en forme.
        harness.key_press(egui::Key::Escape);
        harness.run_steps(4);
        assert!(!editor(&mut harness).is_editing());
        assert!(harness.query_by_role(Role::MultilineTextInput).is_none());
        snapshot(&mut harness, "32_tableau_lecture");
    }

    fn new_note(harness: &mut Harness<'_, State>, title: &str, body: &str) {
        let state = harness.state_mut();
        let ws = state.ws.as_mut().unwrap();
        let id = ws
            .project
            .create_note(title, vec![], body.to_owned())
            .unwrap();
        ws.select(&id, &mut state.notices);
        harness.run_steps(8);
    }

    fn editing_text(harness: &Harness<'_, State>) -> Option<String> {
        harness
            .query_by_role(Role::MultilineTextInput)
            .and_then(|n| n.value())
    }

    #[test]
    fn multi_line_selection_and_list_continuation() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        new_note(&mut harness, "Essai", "a\nb\nc");
        harness
            .state_mut()
            .ws
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .edit_at(0);
        harness.run_steps(4);
        assert_eq!(editing_text(&harness).as_deref(), Some("a"));
        // Maj+Bas : la ligne suivante passe aussi en édition.
        harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowDown);
        harness.run_steps(4);
        assert_eq!(editing_text(&harness).as_deref(), Some("a\nb"));
        // Ctrl+A : toute la fiche.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run_steps(4);
        assert_eq!(editing_text(&harness).as_deref(), Some("a\nb\nc"));
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("- un");
        harness.run_steps(4);
        assert_eq!(body_text(&harness), "- un");
        // Entrée : puce suivante ; Tab : un niveau de plus.
        harness.key_press(egui::Key::Enter);
        harness.run_steps(4);
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("deux");
        harness.run_steps(4);
        harness.key_press(egui::Key::Tab);
        harness.run_steps(4);
        assert_eq!(body_text(&harness), "- un\n  - deux");
        // Entrée sur un élément vide : il remonte d'un niveau.
        harness.key_press(egui::Key::Enter);
        harness.run_steps(4);
        harness.key_press(egui::Key::Enter);
        harness.run_steps(4);
        assert_eq!(body_text(&harness), "- un\n  - deux\n- ");
        assert_eq!(editing_text(&harness).as_deref(), Some("- "));
    }

    #[test]
    fn full_text_search_and_quick_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        new_note(
            &mut harness,
            "Essai",
            "Intro\nLe code 1945 ouvre le coffre.\nFin",
        );
        harness.key_press_modifiers(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::F,
        );
        harness.run_steps(4);
        let is_search = |n: &egui_kittest::kittest::AccessKitNode<'_>| {
            n.role() == Role::TextInput
                && n.placeholder() == Some("Rechercher dans tout le projet…")
        };
        harness.get_by(is_search).type_text("/\\d{4}/ coffre");
        harness.run_steps(4);
        snapshot(&mut harness, "40_recherche");
        // Clic sur la ligne trouvée : la fiche s'ouvre, ligne en édition.
        harness.get_by_label_contains("Le code 1945").click();
        harness.run_steps(6);
        assert_eq!(open_title(&harness), "Essai");
        assert_eq!(
            editing_text(&harness).as_deref(),
            Some("Le code 1945 ouvre le coffre.")
        );

        // Ouverture rapide : fiches seulement, création si absente.
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        harness.key_press_modifiers(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::O,
        );
        harness.run_steps(4);
        let quick = |n: &egui_kittest::kittest::AccessKitNode<'_>| {
            n.role() == Role::TextInput && n.placeholder() == Some("Titre de la fiche à ouvrir…")
        };
        harness.get_by(quick).focus();
        harness.run_steps(1);
        harness.get_by(quick).type_text("Nouvelle piste");
        harness.run_steps(2);
        harness.key_press(egui::Key::Enter);
        harness.run_steps(6);
        assert_eq!(open_title(&harness), "Nouvelle piste");
    }

    #[test]
    fn unlinked_mentions_can_be_linked() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        new_note(
            &mut harness,
            "Journal",
            "Trouvé la Lanterne rouge près du puits.",
        );
        new_note(&mut harness, "Lanterne rouge", "");
        harness.run_steps(4);
        assert!(harness.query_by_label("Mentions non liées (1)").is_some());
        snapshot(&mut harness, "41_mentions");
        harness.get_by_label("Lier").click();
        harness.run_steps(4);
        let ws = harness.state().ws.as_ref().unwrap();
        let journal = ws.project.resolve_title("Journal").unwrap();
        assert_eq!(
            ws.project.note(journal).unwrap().body,
            "Trouvé la [[Lanterne rouge]] près du puits."
        );
        assert!(harness.query_by_label("Mentions non liées (0)").is_some());
        assert!(harness.query_by_label("Rétroliens (1)").is_some());
    }

    #[test]
    fn section_links_and_embeds() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        new_note(
            &mut harness,
            "Source",
            "# Manoir\nintro\n## Code\nLe code est 1945. ^c1\n## Fin\nz",
        );
        new_note(
            &mut harness,
            "Cible",
            "Voir [[Source#^c1]].\n\n![[Source#Code]]\n\nFin.",
        );
        harness.run_steps(6);
        snapshot(&mut harness, "42_insertion");
        // Le cadre inséré porte le titre de la section ; clic : ouverture
        // de la fiche sur la section.
        harness.get_by_label("Source › Code").click();
        harness.run_steps(6);
        assert_eq!(open_title(&harness), "Source");
        assert_eq!(editing_text(&harness).as_deref(), Some("## Code"));
        // Rétrolien compté une fois malgré les deux références.
        let ws = harness.state().ws.as_ref().unwrap();
        let source = ws.project.resolve_title("Source").unwrap().clone();
        assert_eq!(ws.project.backlinks(&source).len(), 1);
    }

    #[test]
    fn callouts_tags_highlights_comments_footnotes() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        new_note(
            &mut harness,
            "Essai",
            "> [!indice]- Le coffre\n> Code à **4 chiffres**.\n\n> [!attention] Piège\n> Ne pas ouvrir.\n\nUn ==mot clé== %%note perso%% et #lieu/manoir, voir[^1].\n\n%%\nbrouillon\n%%\n\n[^1]: Trouvé le jour 3.",
        );
        harness.run_steps(4);
        snapshot(&mut harness, "43_encadres_replie");
        // L'encadré replié s'ouvre d'un clic sur son titre.
        harness.get_by_label("Le coffre").click();
        harness.run_steps(4);
        snapshot(&mut harness, "44_encadres");
        // Onglet Tags : le tag et son parent ; clic : recherche.
        harness.get_by_label("Tags").click();
        harness.run_steps(4);
        assert!(harness.query_by_label("#lieu").is_some());
        harness.get_by_label("#manoir").click();
        harness.run_steps(4);
        // L'onglet et le résultat de recherche.
        assert_eq!(
            harness.query_all_by_label("Essai").count(),
            2,
            "résultat de tag:lieu/manoir"
        );
    }

    #[test]
    fn slash_commands_move_lines_find_replace_and_fold() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        new_note(&mut harness, "Essai", "# Partie\nun\ndeux\n## Sous\ntrois");
        let edit_at = |h: &mut Harness<'_, State>, b: usize| {
            h.state_mut()
                .ws
                .as_mut()
                .unwrap()
                .editor
                .as_mut()
                .unwrap()
                .edit_at(b);
            h.run_steps(4);
        };
        // Alt+Haut : la ligne « deux » remonte.
        edit_at(&mut harness, 16);
        harness.key_press_modifiers(egui::Modifiers::ALT, egui::Key::ArrowUp);
        harness.run_steps(4);
        assert_eq!(body_text(&harness), "# Partie\ndeux\nun\n## Sous\ntrois");
        assert_eq!(editing_text(&harness).as_deref(), Some("deux"));

        // « / » en fin de fiche : commandes, filtrées ; Entrée insère.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::End);
        harness.run_steps(4);
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("\n/tabl");
        harness.run_steps(4);
        assert!(
            harness.query_by_label("Insérer").is_some(),
            "commandes proposées"
        );
        snapshot(&mut harness, "45_commandes_slash");
        harness.key_press(egui::Key::Enter);
        harness.run_steps(4);
        let body = body_text(&harness);
        assert!(body.contains("| Colonne 1 |"), "{body}");
        assert!(!body.contains("/tabl"), "{body}");

        // Rechercher / remplacer (Ctrl+H) : sans accents ni casse.
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::H);
        harness.run_steps(4);
        let field = |hint: &'static str| {
            move |n: &egui_kittest::kittest::AccessKitNode<'_>| {
                n.role() == Role::TextInput && n.placeholder() == Some(hint)
            }
        };
        harness
            .get_by(field("Rechercher dans la fiche"))
            .type_text("TROIS");
        harness.get_by(field("Remplacer par")).focus();
        harness.run_steps(1);
        harness.get_by(field("Remplacer par")).type_text("3");
        harness.run_steps(2);
        assert!(harness.query_by_label("1 / 1").is_some());
        harness.get_by_label("Tout").click();
        harness.run_steps(4);
        assert!(
            body_text(&harness).contains("## Sous\n3"),
            "{}",
            body_text(&harness)
        );
        harness.get_by_label("Fermer").click();
        harness.run_steps(2);

        // Plan : les titres ; repli d'une section.
        assert!(harness.query_by_label("Plan (2)").is_some());
        harness
            .state_mut()
            .ws
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .toggle_fold("Partie");
        harness.run_steps(4);
        assert!(harness.query_by_label("Partie (replié)").is_some());
        snapshot(&mut harness, "46_section_repliee");
    }

    #[test]
    fn date_list_link_fields_and_templates() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        {
            let state = harness.state_mut();
            let ws = state.ws.as_mut().unwrap();
            let user = ws.user_packs_dir.clone();
            ws.project
                .update_pack(user.as_deref(), |p| {
                    p.add_field("Vu le", enigma_core::pack::FieldKind::Date);
                    p.add_field("Objets", enigma_core::pack::FieldKind::List);
                    p.add_field("Lieu", enigma_core::pack::FieldKind::Link);
                    p.templates.push(enigma_core::pack::Template {
                        id: "journal".into(),
                        label: "Entrée de journal".into(),
                        body: "## {{titre}} ({{date}})\n{{curseur}}".into(),
                        ..Default::default()
                    });
                })
                .unwrap();
        }
        new_note(&mut harness, "Essai", "Début");
        let field = |hint: &'static str| {
            move |n: &egui_kittest::kittest::AccessKitNode<'_>| {
                n.role() == Role::TextInput && n.placeholder() == Some(hint)
            }
        };
        // Liste : deux valeurs ajoutées avec Entrée.
        for v in ["clé", "lampe"] {
            harness.get_by(field("+ ajouter (Entrée)")).focus();
            harness.run_steps(1);
            harness.get_by(field("+ ajouter (Entrée)")).type_text(v);
            harness.key_press(egui::Key::Enter);
            harness.run_steps(3);
        }
        // Date : « Auj. ».
        harness.get_by_label("Auj.").click();
        harness.run_steps(3);
        // Lien : choix d'une fiche dans la liste.
        // Le panneau des champs est dessiné avant la fiche : premier menu.
        harness
            .get_all_by_role(Role::ComboBox)
            .next()
            .unwrap()
            .click();
        harness.run_steps(4);
        harness
            .get_all_by_label("Majordome")
            .last()
            .unwrap()
            .click();
        harness.run_steps(4);
        snapshot(&mut harness, "47_champs");
        let ws = harness.state().ws.as_ref().unwrap();
        let id = ws.project.resolve_title("Essai").unwrap();
        let fields = &ws.project.note(id).unwrap().meta.fields;
        assert_eq!(fields.len(), 3, "{fields:?}");
        assert_eq!(
            fields["objets"],
            enigma_core::FieldValue::List(vec!["clé".into(), "lampe".into()])
        );
        assert!(matches!(&fields["vu_le"], enigma_core::FieldValue::Text(d) if d.len() == 10));
        let majordome = ws.project.resolve_title("Majordome").unwrap();
        assert_eq!(
            fields["lieu"],
            enigma_core::FieldValue::Text(majordome.as_str().to_owned())
        );

        // Modèle inséré à la fin, curseur sur {{curseur}}.
        harness
            .state_mut()
            .ws
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .edit_at_end();
        harness.run_steps(4);
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        {
            let state = harness.state_mut();
            let ws = state.ws.as_mut().unwrap();
            ws.run_command(
                Command::InsertTemplate,
                &egui::Context::default(),
                &mut state.notices,
            );
        }
        harness.run_steps(10);
        harness.get_by_label("Entrée de journal").click();
        harness.run_steps(6);
        let body = body_text(&harness);
        assert!(body.starts_with("Début\n\n## Essai ("), "{body}");
        assert_eq!(editing_text(&harness).as_deref(), Some(""));
    }

    #[test]
    fn tabs_history_and_split_view() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.get_by_label("Majordome").click();
        harness.run_steps(4);
        harness.get_by_label("Coffre du bureau").click();
        harness.run_steps(4);
        assert_eq!(open_title(&harness), "Coffre du bureau");
        // Retour / suivant.
        harness.key_press_modifiers(egui::Modifiers::ALT, egui::Key::ArrowLeft);
        harness.run_steps(4);
        assert_eq!(open_title(&harness), "Majordome");
        harness.key_press_modifiers(egui::Modifiers::ALT, egui::Key::ArrowRight);
        harness.run_steps(4);
        assert_eq!(open_title(&harness), "Coffre du bureau");
        // Nouvel onglet (comme Ctrl+clic dans la liste), puis fermeture
        // (Ctrl+W).
        {
            let state = harness.state_mut();
            let ws = state.ws.as_mut().unwrap();
            let salle = ws
                .project
                .resolve_title("Salle des Archives")
                .unwrap()
                .clone();
            ws.open_in_new_tab(&salle, &mut state.notices);
        }
        harness.run_steps(4);
        assert_eq!(open_title(&harness), "Salle des Archives");
        assert_eq!(harness.state().ws.as_ref().unwrap().tab_ids.len(), 2);
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::W);
        harness.run_steps(4);
        assert_eq!(open_title(&harness), "Coffre du bureau");
        assert_eq!(harness.state().ws.as_ref().unwrap().tab_ids.len(), 1);
        // Côte à côte : la fiche précédente dans le second volet.
        harness.get_by_label("Côte à côte").click();
        harness.run_steps(6);
        let ws = harness.state().ws.as_ref().unwrap();
        let other = ws.other.as_ref().unwrap().id.clone();
        assert_eq!(ws.project.note(&other).unwrap().meta.title, "Majordome");
        snapshot(&mut harness, "48_cote_a_cote");
    }

    #[test]
    fn versions_extract_and_merge() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        new_note(&mut harness, "Essai", "Intro\nligne à extraire\nfin");
        let run = |h: &mut Harness<'_, State>, cmd: Command| {
            let state = h.state_mut();
            let ws = state.ws.as_mut().unwrap();
            ws.run_command(cmd, &egui::Context::default(), &mut state.notices);
            h.run_steps(10);
        };
        // Historique : la version gardée à la création, puis restaurée.
        {
            let ws = harness.state_mut().ws.as_mut().unwrap();
            let id = ws.project.resolve_title("Essai").unwrap().clone();
            ws.project.set_body(&id, "Tout changé".into()).unwrap();
            let now = ws.now_ms;
            ws.editor
                .as_mut()
                .unwrap()
                .sync_from_project(&ws.project, now);
        }
        run(&mut harness, Command::VersionHistory);
        snapshot(&mut harness, "50_versions");
        harness.get_by_label("Restaurer cette version").click();
        harness.run_steps(4);
        assert_eq!(body_text(&harness), "Intro\nligne à extraire\nfin");

        // Extraire la ligne sélectionnée.
        harness
            .state_mut()
            .ws
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .select_bytes(6, 23);
        harness.run_steps(4);
        run(&mut harness, Command::ExtractSelection);
        harness.get_by_label("Extraire").click();
        harness.run_steps(4);
        assert_eq!(body_text(&harness), "Intro\n[[ligne à extraire]]\nfin");
        let ws = harness.state().ws.as_ref().unwrap();
        let new = ws.project.resolve_title("ligne à extraire").unwrap();
        assert_eq!(ws.project.note(new).unwrap().body, "ligne à extraire");

        // Fusionner « Essai » dans « Majordome ».
        run(&mut harness, Command::MergeNote);
        harness
            .get_all_by_label("Majordome")
            .last()
            .unwrap()
            .click();
        harness.run_steps(6);
        let ws = harness.state().ws.as_ref().unwrap();
        assert!(ws.project.resolve_title("Essai").is_none());
        assert_eq!(open_title(&harness), "Majordome");
        assert!(body_text(&harness).contains("## Essai\n\nIntro\n[[ligne à extraire]]"));
    }

    #[test]
    fn math_mermaid_and_pdf_export() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        egui_extras::install_image_loaders(&harness.ctx);
        new_note(
            &mut harness,
            "Calculs",
            "L'aire vaut $\\pi r^2$ pour le bassin.\n\n$$\n\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}\n$$\n\n```mermaid\nflowchart LR\n  A[Clé] --> B[Coffre]\n```",
        );
        for _ in 0..10 {
            harness.run_steps(2);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        snapshot(&mut harness, "51_maths_mermaid");
        assert!(harness.query_by_label_contains("illisible").is_none());
        // Aucune formule en échec (le LaTeX brut serait affiché en rouge).
        assert!(harness.query_by_label_contains("$\\pi").is_none());
        let out = dir.path().join("Calculs.pdf");
        {
            let state = harness.state_mut();
            let ws = state.ws.as_mut().unwrap();
            let id = ws.project.resolve_title("Calculs").unwrap().clone();
            ws.export_pdf(&id, &out, &mut state.notices);
        }
        assert!(std::fs::read(&out).unwrap().starts_with(b"%PDF"));
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
        harness
            .state_mut()
            .ws
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .edit_at_end();
        harness.run_steps(4);
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
    fn paste_image_and_audio_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.get_by_label("Salle des Archives").click();
        harness.run_steps(8);
        // Ctrl+V d'une image (événement produit par clipboard::translate).
        harness.event(egui::Event::PasteImage(std::sync::Arc::new(
            egui::ColorImage::filled([4, 3], egui::Color32::RED),
        )));
        harness.run_steps(3);
        let body = harness
            .state()
            .ws
            .as_ref()
            .unwrap()
            .editor_body()
            .unwrap()
            .to_owned();
        assert!(
            body.contains("![image collée](<assets/image collée.png>)"),
            "{body}"
        );
        assert!(dir.path().join("assets/image collée.png").is_file());

        // Fichier audio collé : joint comme pièce jointe.
        let src = tempfile::tempdir().unwrap();
        let mp3 = src.path().join("ambiance.mp3");
        std::fs::write(&mp3, b"ID3").unwrap();
        {
            let state = harness.state_mut();
            let ws = state.ws.as_mut().unwrap();
            ws.paste_files(std::slice::from_ref(&mp3), &mut state.notices);
        }
        let body = harness
            .state()
            .ws
            .as_ref()
            .unwrap()
            .editor_body()
            .unwrap()
            .to_owned();
        assert!(body.contains("[ambiance](assets/ambiance.mp3)"), "{body}");
        harness.run_steps(8);
        assert!(
            harness.query_by_label("Lire").is_some(),
            "bouton de lecture"
        );
        snapshot(&mut harness, "22_pieces_jointes");
    }

    #[test]
    fn markdown_tools_shortcuts_and_context_menu() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        {
            let state = harness.state_mut();
            let ws = state.ws.as_mut().unwrap();
            let id = ws
                .project
                .create_note("Essai", vec![], String::new())
                .unwrap();
            ws.select(&id, &mut state.notices);
        }
        harness.run_steps(8);
        harness.get_by_role(Role::MultilineTextInput).focus();
        harness.run_steps(4);
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("indice");
        harness.run_steps(4);
        let body = |h: &Harness<'_, State>| {
            h.state()
                .ws
                .as_ref()
                .unwrap()
                .editor_body()
                .unwrap()
                .to_owned()
        };
        // Tout sélectionner puis Ctrl+B : gras ; Ctrl+1 : titre.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run_steps(3);
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::B);
        harness.run_steps(3);
        assert_eq!(body(&harness), "**indice**");
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Num1);
        harness.run_steps(3);
        assert_eq!(body(&harness), "# **indice**");
        // Ctrl+Z annule le titre.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        harness.run_steps(3);
        assert_eq!(body(&harness), "**indice**");

        // Clic droit dans le texte : menu de mise en forme et d'insertion.
        let at = harness
            .get_by_role(Role::MultilineTextInput)
            .rect()
            .center();
        harness.hover_at(at);
        harness.run_steps(2);
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            harness.run_steps(1);
        }
        harness.run_steps(6);
        assert!(harness.query_by_label("Citation").is_some(), "menu affiché");
        snapshot(&mut harness, "23_menu_texte");
        harness.get_by_label("Citation").click();
        harness.run_steps(4);
        assert!(body(&harness).starts_with("> "), "{}", body(&harness));
    }

    #[test]
    fn list_grouped_by_category() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.state_mut().ws.as_mut().unwrap().group_by_category = true;
        harness.run_steps(4);
        // Salle des Archives : pièce + indice → deux fois ; Majordome : sans catégorie.
        assert_eq!(harness.query_all_by_label("Salle des Archives").count(), 2);
        assert_eq!(harness.query_all_by_label("Coffre du bureau").count(), 2);
        assert!(harness.query_by_label("Groupe Sans catégorie").is_some());
        snapshot(&mut harness, "24_liste_par_categorie");
        // Replier « Pièce » : la Salle n'y apparaît plus qu'une fois (Indice).
        harness.get_by_label("Groupe Pièce").click();
        harness.run_steps(4);
        assert_eq!(harness.query_all_by_label("Salle des Archives").count(), 1);
        harness.get_by_label("Groupe Pièce").click();
        harness.run_steps(4);
        assert_eq!(harness.query_all_by_label("Salle des Archives").count(), 2);
        // Clic sur une occurrence : sélection.
        harness
            .query_all_by_label("Salle des Archives")
            .next()
            .unwrap()
            .click();
        harness.run_steps(4);
        assert!(harness.state().ws.as_ref().unwrap().editor_body().is_some());
    }

    #[test]
    fn quick_add_category_status_and_field_from_note() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        harness.get_by_label("Majordome").click();
        harness.run_steps(8);
        // « + » des catégories (le premier de l'en-tête).
        harness.get_by_label("+ Catégorie").click();
        harness.run_steps(6);
        harness
            .get_by(|n| n.role() == Role::TextInput && n.placeholder() == Some("Libellé"))
            .type_text("Suspect");
        harness.run_steps(2);
        snapshot(&mut harness, "25_nouvelle_categorie");
        harness.get_by_label("Créer").click();
        harness.run_steps(6);
        {
            let ws = harness.state().ws.as_ref().unwrap();
            assert!(ws.project.pack().category("suspect").is_some());
            let id = ws.project.resolve_title("Majordome").unwrap();
            assert!(ws.project.note(id).unwrap().meta.has_category("suspect"));
        }
        // Copie locale du pack fourni, l'original reste intact.
        let local = dir.path().join(".enigma/packs/blue_prince/pack.json");
        assert!(std::fs::read_to_string(&local).unwrap().contains("Suspect"));
        let original =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/blue_prince/pack.json");
        assert!(
            !std::fs::read_to_string(original)
                .unwrap()
                .contains("Suspect")
        );

        // Nouveau statut (deuxième « + »).
        harness.get_by_label("+ Statut").click();
        harness.run_steps(6);
        harness
            .get_by(|n| n.role() == Role::TextInput && n.placeholder() == Some("Libellé"))
            .type_text("Fausse piste");
        harness.key_press(egui::Key::Enter);
        harness.run_steps(6);
        let ws = harness.state().ws.as_ref().unwrap();
        let id = ws.project.resolve_title("Majordome").unwrap();
        assert_eq!(
            ws.project.note(id).unwrap().meta.status.as_deref(),
            Some("fausse_piste")
        );
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
    fn board_texts_images_groups_and_arrows() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        egui_extras::install_image_loaders(&harness.ctx);
        harness.state_mut().ws.as_mut().unwrap().view = View::Board;
        harness.run_steps(4);
        harness
            .get_by_label("Ajouter les fiches de la liste (3)")
            .click();
        harness.run_steps(6);
        let field = |hint: &'static str| {
            move |n: &egui_kittest::kittest::AccessKitNode<'_>| {
                n.role() == Role::TextInput && n.placeholder() == Some(hint)
            }
        };
        // Carte de texte : saisie directe.
        harness.get_by_label("+ Texte").click();
        harness.run_steps(4);
        harness
            .get_by(|n| {
                n.role() == Role::MultilineTextInput && n.placeholder() == Some("Texte libre…")
            })
            .type_text("Indice : 1945");
        harness.run_steps(2);
        harness.key_press(egui::Key::Escape);
        harness.run_steps(3);
        let text_key = {
            let ws = harness.state().ws.as_ref().unwrap();
            assert_eq!(ws.board().texts[0].text, "Indice : 1945");
            ws.board().texts[0].key.clone()
        };
        // Flèche : du point à droite du texte jusqu'à une carte de fiche.
        let coffre = note_id(&harness, "Coffre du bureau");
        let (from, to) = {
            let ws = harness.state().ws.as_ref().unwrap();
            (
                ws.board_view().connector(ws.board(), &text_key).unwrap(),
                ws.board_view().card_center(ws.board(), &coffre).unwrap(),
            )
        };
        harness.hover_at(from);
        harness.run_steps(1);
        harness.drag_at(from);
        harness.run_steps(1);
        for k in 1..=6 {
            harness.hover_at(from + (to - from) * (k as f32 / 6.0));
            harness.run_steps(1);
        }
        harness.drop_at(to);
        harness.run_steps(4);
        harness
            .get_by(field("Étiquette de la flèche"))
            .type_text("ouvre");
        harness.key_press(egui::Key::Enter);
        harness.run_steps(3);
        // Groupe nommé, image.
        harness.get_by_label("+ Groupe").click();
        harness.run_steps(4);
        // Champ prérempli « Groupe » : tout sélectionner puis remplacer.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness
            .get_by(|n| n.role() == Role::TextInput && n.value().as_deref() == Some("Groupe"))
            .type_text("Bureau");
        harness.key_press(egui::Key::Enter);
        harness.run_steps(3);
        let svg = dir.path().join("plan.svg");
        std::fs::write(
            &svg,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="200" height="100" fill="#4aa3ff"/></svg>"##,
        )
        .unwrap();
        {
            let state = harness.state_mut();
            let ws = state.ws.as_mut().unwrap();
            ws.add_board_image(&svg, &mut state.notices);
        }
        for _ in 0..10 {
            harness.run_steps(2);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        snapshot(&mut harness, "49_tableau_enrichi");
        let saved = Board::load(
            &dir.path()
                .join(enigma_core::project::META_DIR)
                .join(enigma_core::board::BOARD_FILE),
        )
        .unwrap();
        assert_eq!(saved.edges.len(), 1, "{saved:?}");
        assert_eq!(saved.edges[0].from, text_key);
        assert_eq!(saved.edges[0].to, coffre.as_str());
        assert_eq!(saved.edges[0].label, "ouvre");
        assert_eq!(saved.groups[0].label, "Bureau");
        assert_eq!(saved.images.len(), 1);
        assert!(saved.images[0].src.starts_with("assets/"));
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
