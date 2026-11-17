//! Un projet ouvert dans l'interface : liste des fiches, éditeur, dialogues,
//! sauvegarde automatique et surveillance du dossier.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, KeyboardShortcut, Modifiers, RichText, Sense};
use enigma_core::filter::NoteFilter;
use enigma_core::links::{LinkEnd, ResolvedLink};
use enigma_core::project::FileChange;
use enigma_core::{NoteId, Project};

use crate::colors::c32;
use crate::editor::Editor;
use crate::filter_ui;
use crate::graph_view::{GraphOutcome, GraphView};
use crate::notices::Notices;
use crate::pack_editor::{self, PackEditor};
use crate::watcher::FsWatcher;

/// Délai après la dernière modification avant la sauvegarde automatique.
const AUTOSAVE_DELAY: Duration = Duration::from_millis(1000);

const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Z);
const REDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::Y);
const REDO_SHIFT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), egui::Key::Z);
const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::S);
const NEW_NOTE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::N);
const FIND: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::F);
const TOGGLE_GRAPH: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, egui::Key::G);

/// Vue de la zone centrale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Note,
    Graph,
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
        };
        ws.refresh_packs();
        Ok(ws)
    }

    pub fn root(&self) -> &Path {
        self.project.root()
    }

    /// Enregistre toutes les modifications en attente.
    pub fn flush(&mut self, notices: &mut Notices) {
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
        self.process_fs_events(notices, now_ms);
        if self.dialog.is_none() && self.pack_editor.is_none() {
            self.handle_shortcuts(ui.ctx(), notices);
        }
        self.autosave(ui.ctx(), notices);

        egui::Panel::left("notes_list")
            .resizable(true)
            .default_size(260.0)
            .min_size(180.0)
            .show(ui, |ui| self.list_ui(ui, notices));

        // En vue graphe, le panneau reste affiché même sans sélection : sinon
        // sélectionner un nœud redimensionnerait le canevas sous le curseur.
        if self.editor.is_some() || self.view == View::Graph {
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
                                    ui.weak(
                                        "Cliquez sur un nœud pour afficher sa fiche ici ;                                          double-cliquez pour l'ouvrir dans l'éditeur.",
                                    );
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

        if let Some(editor) = &mut self.pack_editor {
            match editor.show(ui.ctx(), &mut self.project, notices) {
                pack_editor::Outcome::Open => {}
                pack_editor::Outcome::Closed => self.pack_editor = None,
                pack_editor::Outcome::Saved => {
                    self.pack_editor = None;
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

    fn handle_shortcuts(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        // Les raccourcis sont consommés avant les widgets : l'historique
        // interne du TextEdit d'egui n'est donc jamais utilisé.
        let (redo, undo, save, new_note, find, toggle_graph) = ctx.input_mut(|i| {
            let redo = i.consume_shortcut(&REDO_SHIFT) || i.consume_shortcut(&REDO);
            let undo = i.consume_shortcut(&UNDO);
            (
                redo,
                undo,
                i.consume_shortcut(&SAVE),
                i.consume_shortcut(&NEW_NOTE),
                i.consume_shortcut(&FIND),
                i.consume_shortcut(&TOGGLE_GRAPH),
            )
        });
        if toggle_graph {
            self.view = match self.view {
                View::Note => View::Graph,
                View::Graph => View::Note,
            };
        }
        if let Some(ed) = &mut self.editor {
            let changed =
                (undo && ed.undo(&mut self.project)) || (redo && ed.redo(&mut self.project));
            if changed {
                self.mark_edited();
            }
        }
        if save {
            self.flush(notices);
        }
        if new_note {
            self.open_new_note_dialog();
        }
        if find {
            self.focus_search = true;
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
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.view, View::Note, "Fiche")
                .on_hover_text("Éditeur de la fiche (Ctrl+G pour basculer)");
            ui.selectable_value(&mut self.view, View::Graph, "Graphe")
                .on_hover_text("Graphe des fiches (Ctrl+G pour basculer)");
        });
        ui.separator();
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

        ui.separator();
        let body = editor.body_ui(ui, &mut self.project, now_ms, notices);
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
        harness.run();
        harness
    }

    #[test]
    fn edit_note_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let mut harness = setup(dir.path());
        let majordome_before = std::fs::read(dir.path().join("Majordome.md")).unwrap();

        // Ouvrir une fiche depuis la liste.
        harness.get_by_label("Salle des Archives").click();
        harness.run();
        let path = dir.path().join("Salle des Archives.md");
        {
            let ws = harness.state().ws.as_ref().unwrap();
            let id = ws.project.resolve_title("Salle des Archives").unwrap();
            assert_eq!(ws.editor.as_ref().map(|e| &e.id), Some(id));
        }
        snapshot(&mut harness, "1_editeur");

        // Ajouter une catégorie par sa pastille, puis annuler.
        harness.get_by_label("👤 Personnage").click();
        harness.run();
        flush(&mut harness);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("personnage"), "{text}");
        snapshot(&mut harness, "2_categorie");

        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        harness.run();
        flush(&mut harness);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("personnage"), "{text}");

        // Refaire.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Y);
        harness.run();
        flush(&mut harness);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("personnage")
        );

        // Taper dans le corps : l'en-tête reste intact.
        harness.get_by_role(Role::MultilineTextInput).focus();
        harness.run();
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("\nAjout [[Majordome]]");
        harness.run();
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
        harness.run();

        // Renommer via le champ titre : fichier renommé, liens réécrits.
        let is_title = |n: &egui_kittest::kittest::AccessKitNode<'_>| {
            n.role() == Role::TextInput && n.value().as_deref() == Some("Salle des Archives")
        };
        harness.get_by(is_title).focus();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.get_by(is_title).type_text("Archives secrètes");
        harness.key_press(egui::Key::Enter);
        harness.run();
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
            harness.run();
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
        harness.run();
        harness.get_by_role(Role::MultilineTextInput).focus();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::End);
        harness.run();

        // [[cof → Tab : titre inséré, puis suggestions de type.
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("\nPiste : [[cof");
        harness.run();
        snapshot(&mut harness, "5_suggestions_titre");
        harness.key_press(egui::Key::Tab);
        harness.run();
        assert!(
            body_text(&harness).ends_with("[[Coffre du bureau|]]"),
            "{}",
            body_text(&harness)
        );
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text("ou");
        harness.run();
        snapshot(&mut harness, "6_suggestions_type");
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert!(
            body_text(&harness).ends_with("Piste : [[Coffre du bureau|ouvre]]"),
            "{}",
            body_text(&harness)
        );

        // Création à la volée.
        harness
            .get_by_role(Role::MultilineTextInput)
            .type_text(" et [[Nouvelle piste");
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
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
        harness.run();
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(body_text(&harness).ends_with(" [[maj"));
        harness.key_press(egui::Key::Enter);
        harness.run();
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
        harness.run();
        assert_eq!(open_title(&harness), "Coffre du bureau");
        harness.get_by_label("Rétroliens (2)");
        let back: Vec<_> = harness.get_all_by(is_link("Salle des Archives")).collect();
        assert_eq!(back.len(), 2);
        back[0].click();
        harness.run();
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
        harness.run();
        harness.get_by_label("Pièce").click();
        harness.run();
        assert_eq!(listed(&harness), ["Salle des Archives"]);
        snapshot(&mut harness, "8_filtres");
        harness.get_by_label("Pièce").click();
        harness.run();
        assert_eq!(listed(&harness), ["Coffre du bureau", "Majordome"]);
        harness.get_by_label("− Pièce").click();
        harness.run();
        assert_eq!(listed(&harness).len(), 3);

        // Statut, type de lien, liens cassés.
        harness.get_by_label("Confirmé").click();
        harness.run();
        assert_eq!(listed(&harness), ["Coffre du bureau"]);
        harness.get_by_label("Réinitialiser les filtres").click();
        harness.run();
        harness.get_by_label("indique").click();
        harness.run();
        assert_eq!(listed(&harness), ["Coffre du bureau", "Salle des Archives"]);
        harness.get_by_label("Réinitialiser les filtres").click();
        harness.get_by_label("Avec liens cassés").click();
        harness.run();
        assert_eq!(listed(&harness), ["Salle des Archives"]);
        harness.get_by_label("Avec liens cassés").click();
        harness.run();

        // Nouvelle fiche à partir d'un modèle.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::N);
        harness.run();
        harness
            .get_by(|n| n.role() == Role::TextInput && n.placeholder() == Some("Titre"))
            .type_text("Serre");
        harness.get_by_label("Modèle").click();
        harness.run();
        harness.get_by_label("Nouvelle pièce").click();
        harness.run();
        harness.get_by_label("Créer").click();
        harness.run();
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
        harness.run();
        harness.get_by_label("+ Ajouter une catégorie").click();
        harness.run();
        let field = |placeholder: &'static str| {
            move |n: &egui_kittest::kittest::AccessKitNode<'_>| {
                n.role() == Role::TextInput
                    && n.placeholder() == Some(placeholder)
                    && n.value().as_deref() == Some("")
            }
        };
        harness.get_by(field("id")).focus();
        harness.run();
        harness.get_by(field("id")).type_text("lieu");
        harness.run();
        harness.get_by(field("libellé")).focus();
        harness.run();
        harness.get_by(field("libellé")).type_text("Lieu");
        harness.run();
        snapshot(&mut harness, "9_editeur_pack");
        harness.get_by_label("Enregistrer").click();
        harness.run();
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
    }
}
