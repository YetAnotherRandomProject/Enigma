//! Éditeur d'une fiche : titre, pastilles de catégories, statut, corps
//! Markdown mis en forme, champs personnalisés. L'en-tête YAML n'est jamais
//! montré en texte brut.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{self, Color32, FontId, RichText, Stroke};
use enigma_core::history::{EditKind, History};
use enigma_core::live::{self, UnitKind};
use enigma_core::markdown;
use enigma_core::md_edit;
use enigma_core::pack::FieldKind;
use enigma_core::table;
use enigma_core::{FieldValue, NoteId, NoteMeta, Project};

use crate::colors::{c32, text_on};
use crate::completion::{self, ItemKind, Nav, Popup};
use crate::highlight::Highlighter;
use crate::notices::Notices;
use crate::quick_add::{self, NewItem, QuickAdd};
use crate::reading::{self, Preview, ViewAction};

/// État complet annulable d'une fiche (le titre est exclu : il ne change
/// que par renommage).
#[derive(Clone, PartialEq)]
struct Snapshot {
    meta: NoteMeta,
    body: String,
}

pub struct Editor {
    pub id: NoteId,
    /// Tampon édité par le `TextEdit`.
    body: String,
    /// Dernier état connu (sert d'« avant » pour l'historique).
    snap: Snapshot,
    history: History<Snapshot>,
    title_buf: String,
    field_bufs: BTreeMap<String, String>,
    highlighter: Highlighter,
    /// Demande de focus sur le titre (renommage depuis la liste).
    pub focus_title: bool,
    /// Suggestions de liens affichées.
    popup: Option<Popup>,
    /// Rectangle de la fenêtre de suggestions à la frame précédente.
    popup_rect: Option<egui::Rect>,
    /// Début (octet) d'une saisie de lien dont les suggestions ont été fermées.
    dismissed: Option<usize>,
    /// Position du curseur (en caractères) à appliquer à la prochaine frame.
    pending_cursor: Option<usize>,
    /// Dernière position connue du curseur (octet), pour insérer une image.
    last_cursor: Option<usize>,
    /// Dernière sélection connue (octets), pour la mise en forme.
    last_sel: Option<std::ops::Range<usize>>,
    /// Début (caractères) de la sélection à rétablir avec `pending_cursor`.
    pending_anchor: Option<usize>,
    /// Fiche mise en forme (unités en cache, glisser en cours).
    preview: Preview,
    /// Unité en cours de modification (plage dans `body`).
    active: Option<Range<usize>>,
    /// Texte affiché par le `TextEdit` à l'image précédente (flèches haut
    /// et bas : changer d'unité depuis la première ou la dernière ligne).
    last_galley: Option<Arc<egui::Galley>>,
    /// Position de ce texte à l'écran.
    last_galley_pos: egui::Pos2,
    /// Sélection à la souris commencée à cet octet (glisser sur plusieurs
    /// lignes : elles passent toutes en édition).
    drag_anchor: Option<usize>,
    /// Position verticale de chaque unité à l'image précédente.
    unit_rects: Vec<(Range<usize>, egui::Rangef)>,
    /// Faire défiler jusqu'au texte en édition (résultat de recherche…).
    reveal: bool,
    /// Barre « Rechercher / remplacer dans la fiche ».
    find: Option<FindBar>,
    /// Commandes « / » proposées.
    slash: Option<crate::slash::SlashPopup>,
    slash_rect: Option<egui::Rect>,
    /// `/` dont les propositions ont été fermées (Échap).
    slash_dismissed: Option<usize>,
    /// Commande « / » Modèle… choisie : ouvrir le choix des modèles.
    want_template: bool,
    /// Distingue deux éditeurs de la même fiche (volets côte à côte).
    salt: u64,
    /// Formulaires « + » pour enrichir le pack depuis la fiche.
    quick_category: QuickAdd,
    quick_status: QuickAdd,
    quick_field: QuickAdd,
}

/// Recherche dans la fiche.
#[derive(Default)]
struct FindBar {
    query: String,
    replace: String,
    /// Respecter la casse (sinon : ni casse ni accents).
    case: bool,
    current: usize,
    focus: bool,
}

/// Occurrences de `q` dans `text` (octets).
fn find_all(text: &str, q: &str, case: bool) -> Vec<Range<usize>> {
    if q.is_empty() {
        return Vec::new();
    }
    if case {
        return text.match_indices(q).map(|(i, m)| i..i + m.len()).collect();
    }
    let starts: Vec<usize> = text
        .char_indices()
        .map(|(b, _)| b)
        .chain([text.len()])
        .collect();
    let hay = enigma_core::search::fold(text);
    let needle = enigma_core::search::fold(q);
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == *needle {
            out.push(starts[i]..starts[i + needle.len()]);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Résultat de l'affichage du corps.
#[derive(Default)]
pub struct BodyOutcome {
    pub edited: bool,
    /// Fiche à ouvrir (Ctrl+clic sur un lien).
    pub open: Option<NoteId>,
    /// Section ou paragraphe (`^id`) où se placer dans la fiche ouverte.
    pub open_section: Option<String>,
    /// Chercher les fiches qui portent ce tag.
    pub search_tag: Option<String>,
    /// Ouvrir le choix d'un modèle à insérer.
    pub insert_template: bool,
    /// Extraire la sélection dans une nouvelle fiche.
    pub extract: bool,
    /// Ouvrir le sélecteur « Insérer une image ou un fichier ».
    pub insert_file: bool,
}

/// Outil d'écriture appliqué à la sélection (ou au curseur).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Inline(md_edit::Inline),
    Line(md_edit::Line),
    CodeBlock,
    Table(usize, usize),
    Rule,
    Date,
    /// Supprime la sélection (Couper).
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuChoice {
    Format(Format),
    InsertFile,
    Copy,
    Cut,
    /// Copier un lien vers la section ou le paragraphe du curseur.
    CopyLinkHere,
    /// Extraire la sélection dans une nouvelle fiche.
    Extract,
}

/// Menu contextuel du texte.
pub fn format_menu(ui: &mut egui::Ui, has_selection: bool) -> Option<MenuChoice> {
    use md_edit::{Inline as I, Line as L};
    let mut choice = None;
    let mut item = |ui: &mut egui::Ui, label: &str, keys: &str, c: MenuChoice| {
        let button = if keys.is_empty() {
            egui::Button::new(label)
        } else {
            egui::Button::new(label).shortcut_text(keys)
        };
        if ui.add(button).clicked() {
            choice = Some(c);
            ui.close();
        }
    };
    let mut picked = None;
    if has_selection {
        item(ui, "Couper", "Ctrl+X", MenuChoice::Cut);
        item(ui, "Copier", "Ctrl+C", MenuChoice::Copy);
        item(
            ui,
            "Extraire dans une nouvelle fiche…",
            "",
            MenuChoice::Extract,
        );
        ui.separator();
    }
    item(
        ui,
        "Gras",
        "Ctrl+B",
        MenuChoice::Format(Format::Inline(I::Bold)),
    );
    item(
        ui,
        "Italique",
        "Ctrl+I",
        MenuChoice::Format(Format::Inline(I::Italic)),
    );
    item(
        ui,
        "Barré",
        "Ctrl+Maj+X",
        MenuChoice::Format(Format::Inline(I::Strike)),
    );
    item(
        ui,
        "Code",
        "Ctrl+Maj+C",
        MenuChoice::Format(Format::Inline(I::Code)),
    );
    item(
        ui,
        "Lien vers une fiche [[ ]]",
        "Ctrl+K",
        MenuChoice::Format(Format::Inline(I::WikiLink)),
    );
    ui.separator();
    ui.menu_button("Titre", |ui| {
        for (h, k) in [(1u8, "Ctrl+1"), (2, "Ctrl+2"), (3, "Ctrl+3")] {
            item(
                ui,
                &format!("Titre {h}"),
                k,
                MenuChoice::Format(Format::Line(L::Heading(h))),
            );
        }
    });
    ui.menu_button("Liste", |ui| {
        item(
            ui,
            "Liste à puces",
            "",
            MenuChoice::Format(Format::Line(L::Bullet)),
        );
        item(
            ui,
            "Liste numérotée",
            "",
            MenuChoice::Format(Format::Line(L::Numbered)),
        );
        item(
            ui,
            "Liste de tâches",
            "",
            MenuChoice::Format(Format::Line(L::Task)),
        );
    });
    item(
        ui,
        "Citation",
        "",
        MenuChoice::Format(Format::Line(L::Quote)),
    );
    item(
        ui,
        "Bloc de code",
        "",
        MenuChoice::Format(Format::CodeBlock),
    );
    item(
        ui,
        "Copier le lien vers ce paragraphe",
        "",
        MenuChoice::CopyLinkHere,
    );
    ui.separator();
    ui.menu_button("Insérer", |ui| {
        ui.menu_button("Tableau", |ui| {
            if let Some(size) = table_picker(ui) {
                picked = Some(size);
                ui.close();
            }
        });
        item(ui, "Séparateur", "", MenuChoice::Format(Format::Rule));
        item(ui, "Date du jour", "", MenuChoice::Format(Format::Date));
        item(ui, "Image ou fichier…", "", MenuChoice::InsertFile);
    });
    if let Some((r, c)) = picked {
        choice = Some(MenuChoice::Format(Format::Table(r, c)));
    }
    choice
}

/// Grille de choix de la taille d'un tableau (survol puis clic) :
/// (lignes de données, colonnes).
pub fn table_picker(ui: &mut egui::Ui) -> Option<(usize, usize)> {
    const N: usize = 8;
    const CELL: f32 = 18.0;
    let id = egui::Id::unique("table_picker");
    let mut hover: (usize, usize) = ui.data(|d| d.get_temp(id)).unwrap_or((2, 3));
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(CELL * N as f32, CELL * (N + 1) as f32),
        egui::Sense::click(),
    );
    if let Some(p) = resp.hover_pos() {
        let c = (((p.x - rect.left()) / CELL) as usize).min(N - 1) + 1;
        let r = (((p.y - rect.top()) / CELL) as usize).min(N);
        hover = (r.max(1), c);
        ui.data_mut(|d| d.insert_temp(id, hover));
    }
    let v = ui.visuals().clone();
    let accent = crate::theme::accent(&v);
    for r in 0..=N {
        for c in 0..N {
            let cell = egui::Rect::from_min_size(
                rect.min + egui::vec2(c as f32 * CELL, r as f32 * CELL),
                egui::Vec2::splat(CELL),
            )
            .shrink(1.5);
            let on = r <= hover.0 && c < hover.1;
            let fill = match (on, r == 0) {
                (true, _) => accent.gamma_multiply(if r == 0 { 0.9 } else { 0.55 }),
                (false, true) => v.widgets.inactive.bg_fill,
                (false, false) => v.extreme_bg_color,
            };
            ui.painter().rect_filled(cell, 0.0, fill);
            ui.painter().rect_stroke(
                cell,
                0.0,
                Stroke::new(1.0, v.widgets.inactive.bg_stroke.color),
                egui::StrokeKind::Inside,
            );
        }
    }
    ui.label(
        RichText::new(format!(
            "{} colonnes × {} lignes (+ en-tête)",
            hover.1, hover.0
        ))
        .small()
        .weak(),
    );
    resp.clicked().then_some(hover)
}

/// Outils du tableau en cours de modification.
fn table_toolbar(ui: &mut egui::Ui) -> Option<table::Op> {
    use table::{Align, Op};
    let mut op = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.spacing_mut().button_padding = egui::vec2(7.0, 2.0);
        let mut b = |ui: &mut egui::Ui, label: &str, hint: &str, o: Op| {
            if ui
                .add(egui::Button::new(RichText::new(label).small()))
                .on_hover_text(hint)
                .clicked()
            {
                op = Some(o);
            }
        };
        b(
            ui,
            "+ Ligne avant",
            "Insérer une ligne au-dessus",
            Op::RowAbove,
        );
        b(
            ui,
            "+ Ligne après",
            "Insérer une ligne en dessous (Entrée sur la dernière ligne)",
            Op::RowBelow,
        );
        b(
            ui,
            "+ Colonne avant",
            "Insérer une colonne à gauche",
            Op::ColLeft,
        );
        b(
            ui,
            "+ Colonne après",
            "Insérer une colonne à droite",
            Op::ColRight,
        );
        ui.add_space(6.0);
        b(
            ui,
            "Suppr. ligne",
            "Supprimer la ligne du curseur",
            Op::DeleteRow,
        );
        b(
            ui,
            "Suppr. colonne",
            "Supprimer la colonne du curseur",
            Op::DeleteCol,
        );
        ui.add_space(6.0);
        b(
            ui,
            "Gauche",
            "Aligner la colonne à gauche",
            Op::Align(Align::Left),
        );
        b(ui, "Centre", "Centrer la colonne", Op::Align(Align::Center));
        b(
            ui,
            "Droite",
            "Aligner la colonne à droite",
            Op::Align(Align::Right),
        );
        ui.add_space(6.0);
        ui.weak(RichText::new("Tab : cellule suivante · Entrée : ligne suivante").small());
    });
    op
}

/// Texte d'un tableau ou d'un bloc de code en cours de modification :
/// police à chasse fixe (colonnes alignées), syntaxe atténuée.
fn mono_job(text: &str, kind: UnitKind, v: &egui::Visuals) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let font = FontId::monospace(14.0);
    let plain = TextFormat::simple(font.clone(), v.text_color());
    let weak = TextFormat::simple(font, v.weak_text_color());
    let mut job = LayoutJob::default();
    let lines: Vec<&str> = text.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let syntax = match kind {
            UnitKind::Code => i == 0 || (i + 1 == lines.len() && i > 0 && line.trim().len() >= 3),
            _ => table::is_delimiter_row(line),
        };
        if syntax {
            job.append(line, 0.0, weak.clone());
        } else if kind == UnitKind::Table {
            let cells = table::split_cells(line);
            let mut at = 0;
            for r in cells {
                job.append(&line[at..r.start], 0.0, weak.clone());
                job.append(&line[r.clone()], 0.0, plain.clone());
                at = r.end;
            }
            job.append(&line[at..], 0.0, weak.clone());
        } else {
            job.append(line, 0.0, plain.clone());
        }
        if i + 1 < lines.len() {
            job.append("\n", 0.0, plain.clone());
        }
    }
    job
}

impl Editor {
    pub fn open(project: &Project, id: &NoteId) -> Option<Editor> {
        let note = project.note(id)?;
        let mut editor = Editor {
            id: id.clone(),
            body: note.body.clone(),
            snap: Snapshot {
                meta: note.meta.clone(),
                body: note.body.clone(),
            },
            history: History::default(),
            title_buf: note.meta.title.clone(),
            field_bufs: BTreeMap::new(),
            highlighter: Highlighter::default(),
            focus_title: false,
            popup: None,
            popup_rect: None,
            dismissed: None,
            pending_cursor: None,
            last_cursor: None,
            last_sel: None,
            pending_anchor: None,
            preview: Preview::default(),
            active: None,
            last_galley: None,
            last_galley_pos: egui::Pos2::ZERO,
            drag_anchor: None,
            unit_rects: Vec::new(),
            reveal: false,
            find: None,
            slash: None,
            slash_rect: None,
            slash_dismissed: None,
            want_template: false,
            salt: 0,
            quick_category: QuickAdd::default(),
            quick_status: QuickAdd::default(),
            quick_field: QuickAdd::default(),
        };
        editor.refresh_field_bufs();
        Some(editor)
    }

    /// Reprend l'état du projet (rechargement externe, renommage, conflit
    /// résolu…). L'état précédent reste accessible par « annuler ».
    pub fn sync_from_project(&mut self, project: &Project, now_ms: u64) {
        let Some(note) = project.note(&self.id) else {
            return;
        };
        self.title_buf = note.meta.title.clone();
        self.snap.meta.title = note.meta.title.clone();
        let fresh = Snapshot {
            meta: note.meta.clone(),
            body: note.body.clone(),
        };
        if fresh != self.snap {
            let old = std::mem::replace(&mut self.snap, fresh);
            self.history.record(old, EditKind::Discrete, now_ms);
            self.body = self.snap.body.clone();
            self.refresh_field_bufs();
        }
    }

    #[cfg(test)]
    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn set_salt(&mut self, salt: u64) {
        self.salt = salt;
    }

    /// Texte de la fiche.
    pub fn body_text(&self) -> &str {
        &self.body
    }

    pub fn undo(&mut self, project: &mut Project) -> bool {
        match self.history.undo(self.snap.clone()) {
            Some(prev) => {
                self.apply(prev, project);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, project: &mut Project) -> bool {
        match self.history.redo(self.snap.clone()) {
            Some(next) => {
                self.apply(next, project);
                true
            }
            None => false,
        }
    }

    fn apply(&mut self, snap: Snapshot, project: &mut Project) {
        if let Err(e) = project
            .set_meta(&self.id, snap.meta.clone())
            .and_then(|()| project.set_body(&self.id, snap.body.clone()))
        {
            tracing::warn!("annuler/refaire : {e}");
            return;
        }
        self.snap = snap;
        if let Some(n) = project.note(&self.id) {
            self.snap.meta.title = n.meta.title.clone();
        }
        self.body = self.snap.body.clone();
        self.refresh_field_bufs();
    }

    /// Applique de nouvelles métadonnées comme une étape d'historique.
    fn commit_meta(&mut self, project: &mut Project, meta: NoteMeta, now_ms: u64) -> bool {
        if meta == self.snap.meta {
            return false;
        }
        if let Err(e) = project.set_meta(&self.id, meta.clone()) {
            tracing::warn!("modification des métadonnées : {e}");
            return false;
        }
        let old = std::mem::replace(&mut self.snap.meta, meta);
        self.history.record(
            Snapshot {
                meta: old,
                body: self.snap.body.clone(),
            },
            EditKind::Discrete,
            now_ms,
        );
        self.refresh_field_bufs();
        true
    }

    fn refresh_field_bufs(&mut self) {
        self.field_bufs = self
            .snap
            .meta
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), v.display()))
            .collect();
    }

    /// Titre, pastilles de catégories et statut. Renvoie `true` si la fiche a
    /// été modifiée.
    pub fn header_ui(
        &mut self,
        ui: &mut egui::Ui,
        project: &mut Project,
        now_ms: u64,
        notices: &mut Notices,
        user_packs: Option<&std::path::Path>,
    ) -> bool {
        let mut edited = false;

        // Titre (renommage à la validation).
        let current_title = project
            .note(&self.id)
            .map(|n| n.meta.title.clone())
            .unwrap_or_default();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.title_buf)
                .font(FontId::proportional(24.0))
                .desired_width(f32::INFINITY)
                .hint_text("Titre de la fiche")
                .frame(egui::Frame::NONE),
        );
        if self.focus_title {
            resp.request_focus();
            self.focus_title = false;
        }
        if resp.lost_focus() {
            let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if cancelled || self.title_buf.trim() == current_title {
                self.title_buf = current_title.clone();
            } else {
                match project.rename(&self.id, &self.title_buf) {
                    Ok(updated) => {
                        let others = updated.iter().filter(|i| **i != self.id).count();
                        if others > 0 {
                            notices.info(format!(
                                "Fiche renommée ; liens mis à jour dans {others} fiche(s)."
                            ));
                        }
                        self.sync_from_project(project, now_ms);
                    }
                    Err(e) => {
                        notices.error(format!("Renommage impossible : {e}"));
                        self.title_buf = current_title.clone();
                    }
                }
            }
        }
        ui.add_space(4.0);

        // Catégories et statut.
        let mut toggle: Option<String> = None;
        let mut new_status: Option<Option<String>> = None;
        let mut add_category: Option<NewItem> = None;
        let mut add_status: Option<NewItem> = None;
        ui.horizontal_wrapped(|ui| {
            let pack = project.pack();
            for cat in &pack.categories {
                let on = self.snap.meta.has_category(&cat.id);
                let color = c32(cat.color);
                let text = match &cat.icon {
                    Some(icon) => format!("{icon} {}", cat.label),
                    None => cat.label.clone(),
                };
                let button = egui::Button::new(RichText::new(text).color(if on {
                    text_on(color)
                } else {
                    color
                }))
                .fill(if on { color } else { Color32::TRANSPARENT })
                .stroke(Stroke::new(1.0, color))
                .corner_radius(0.0);
                let hint = if on {
                    "Retirer cette catégorie"
                } else {
                    "Ajouter cette catégorie"
                };
                if ui.add(button).on_hover_text(hint).clicked() {
                    toggle = Some(cat.id.clone());
                }
            }
            for id in &self.snap.meta.categories {
                if pack.category(id).is_none() {
                    let button = egui::Button::new(format!("{id} ✕"))
                        .fill(Color32::from_gray(90))
                        .corner_radius(0.0);
                    if ui
                        .add(button)
                        .on_hover_text("Catégorie absente du pack : cliquer pour la retirer")
                        .clicked()
                    {
                        toggle = Some(id.clone());
                    }
                }
            }
            add_category = self.quick_category.menu(
                ui,
                "+ Catégorie",
                "Créer une catégorie (ajoutée au pack du projet) et l'attribuer à la fiche",
                quick_add::Kind::Category,
            );

            ui.separator();
            let status = self.snap.meta.status.clone();
            let (label, color) = match &status {
                None => ("Aucun statut".to_owned(), ui.visuals().weak_text_color()),
                Some(s) => match pack.status(s) {
                    Some(def) => (def.label.clone(), c32(def.color)),
                    None => (format!("{s} (inconnu)"), ui.visuals().error_fg_color),
                },
            };
            egui::ComboBox::new("status", "Statut")
                .selected_text(RichText::new(format!("• {label}")).color(color))
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(status.is_none(), "Aucun statut")
                        .clicked()
                    {
                        new_status = Some(None);
                    }
                    for s in &pack.statuses {
                        let selected = status.as_deref() == Some(s.id.as_str());
                        let text = RichText::new(format!("• {}", s.label)).color(c32(s.color));
                        if ui.selectable_label(selected, text).clicked() {
                            new_status = Some(Some(s.id.clone()));
                        }
                    }
                });
            add_status = self.quick_status.menu(
                ui,
                "+ Statut",
                "Créer un statut (ajouté au pack du projet) et l'attribuer à la fiche",
                quick_add::Kind::Status,
            );
        });

        if let Some(item) = add_category {
            let mut id = String::new();
            match project.update_pack(user_packs, |p| id = p.add_category(&item.label, item.color))
            {
                Ok(()) => {
                    let mut meta = self.snap.meta.clone();
                    if !meta.has_category(&id) {
                        meta.toggle_category(&id);
                    }
                    edited |= self.commit_meta(project, meta, now_ms);
                    notices.info(format!("Catégorie « {} » ajoutée au pack.", item.label));
                }
                Err(e) => notices.error(format!("Catégorie non créée : {e}")),
            }
        }
        if let Some(item) = add_status {
            let mut id = String::new();
            match project.update_pack(user_packs, |p| id = p.add_status(&item.label, item.color)) {
                Ok(()) => new_status = Some(Some(id)),
                Err(e) => notices.error(format!("Statut non créé : {e}")),
            }
        }

        if let Some(cat) = toggle {
            let mut meta = self.snap.meta.clone();
            meta.toggle_category(&cat);
            edited |= self.commit_meta(project, meta, now_ms);
        }
        if let Some(status) = new_status {
            let mut meta = self.snap.meta.clone();
            meta.status = status;
            edited |= self.commit_meta(project, meta, now_ms);
        }
        edited
    }

    /// Corps de la fiche, toujours mis en forme ; seule l'unité en cours de
    /// modification (ligne, tableau, bloc de code) montre son Markdown, mis
    /// en forme pendant la frappe. Suggestions de liens, Ctrl+clic pour
    /// suivre un lien depuis le texte.
    pub fn body_ui(
        &mut self,
        ui: &mut egui::Ui,
        project: &mut Project,
        now_ms: u64,
        notices: &mut Notices,
    ) -> BodyOutcome {
        let mut outcome = BodyOutcome::default();
        let te_id = egui::Id::unique(("body", self.id.as_str(), self.salt));
        let ctx = ui.ctx().clone();
        self.preview.sync(&self.body);

        // 1. Édition demandée (insertion, mise en forme, annuler…).
        if let Some(ci) = self.pending_cursor.take() {
            let b = char_to_byte(&self.body, ci);
            let anchor = self
                .pending_anchor
                .take()
                .map(|a| char_to_byte(&self.body, a));
            self.activate(&ctx, te_id, b, anchor);
        }
        self.resolve_active();
        let focused = ctx.memory(|m| m.has_focus(te_id));

        if self.find.is_some() {
            outcome.edited |= self.find_ui(ui, &ctx, te_id, project, now_ms);
        }

        // 2. Touches consommées avant le TextEdit : suggestions, passage
        //    d'une unité à l'autre, cellules de tableau.
        if focused && let Some(sp) = &self.slash {
            let n = crate::slash::items(&sp.query).len().max(1);
            let nav = ctx.input_mut(|i| {
                use egui::{Key, Modifiers};
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    Some(Nav::Down)
                } else if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    Some(Nav::Up)
                } else if i.consume_key(Modifiers::NONE, Key::Enter)
                    || i.consume_key(Modifiers::NONE, Key::Tab)
                {
                    Some(Nav::Accept)
                } else if i.consume_key(Modifiers::NONE, Key::Escape) {
                    Some(Nav::Dismiss)
                } else {
                    None
                }
            });
            match nav {
                Some(Nav::Down) => {
                    if let Some(sp) = &mut self.slash {
                        sp.selected = (sp.selected + 1) % n;
                    }
                }
                Some(Nav::Up) => {
                    if let Some(sp) = &mut self.slash {
                        sp.selected = (sp.selected + n - 1) % n;
                    }
                }
                Some(Nav::Dismiss) => {
                    self.slash_dismissed = Some(sp.start);
                    self.slash = None;
                }
                Some(_) => {
                    let i = sp.selected;
                    outcome.edited |=
                        self.accept_slash(i, project, now_ms, &mut outcome.insert_file);
                }
                None => {}
            }
        } else if focused && self.popup.is_some() {
            let nav = ctx.input_mut(|i| {
                use egui::{Key, Modifiers};
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    Some(Nav::Down)
                } else if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    Some(Nav::Up)
                } else if i.consume_key(Modifiers::NONE, Key::Enter) {
                    Some(Nav::Accept)
                } else if i.consume_key(Modifiers::NONE, Key::Tab) {
                    Some(Nav::AcceptThenType)
                } else if i.consume_key(Modifiers::NONE, Key::Escape) {
                    Some(Nav::Dismiss)
                } else {
                    None
                }
            });
            if let Some(nav) = nav {
                outcome.edited |= self.navigate(nav, project, now_ms, notices);
            }
        } else if focused {
            outcome.edited |= self.unit_keys(&ctx, te_id, project, now_ms);
        }
        if let Some(ci) = self.pending_cursor.take() {
            let b = char_to_byte(&self.body, ci);
            self.activate(&ctx, te_id, b, None);
        }

        // 3. Les unités : mises en forme, sauf celle qu'on modifie.
        let units = std::mem::take(&mut self.preview.units);
        let active = self.active.clone();
        let popup_open = self.popup.is_some();
        let note_path = project
            .entry(&self.id)
            .map(|e| e.path.clone())
            .unwrap_or_default();
        let mut action: Option<ViewAction> = None;
        let mut edit: Option<(
            egui::text_edit::TextEditOutput,
            String,
            Range<usize>,
            UnitKind,
        )> = None;
        let mut active_rect: Option<egui::Rect> = None;
        let mut table_op: Option<table::Op> = None;
        let mut menu = |ui: &mut egui::Ui| format_menu(ui, false);
        let mut unit_rects: Vec<(Range<usize>, egui::Rangef)> = Vec::new();
        let reveal = std::mem::take(&mut self.reveal);
        {
            let project_ref: &Project = project;
            let body = &self.body;
            let highlighter = &mut self.highlighter;
            let drags = &mut self.preview.drags;
            let empty = body.is_empty();
            egui::ScrollArea::vertical()
                .id_salt("body_scroll")
                .auto_shrink(false)
                .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: 16,
                            ..Default::default()
                        })
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 3.0;
                            // Section repliée : ses unités sont sautées jusqu'au
                            // titre suivant de même niveau ou plus haut.
                            let mut fold_level: Option<usize> = None;
                            for cu in &units {
                                let range = cu.unit.range.clone();
                                let heading = match &cu.content {
                                    reading::Content::Line { source, .. }
                                        if source.starts_with('#') =>
                                    {
                                        let level =
                                            source.bytes().take_while(|&b| b == b'#').count();
                                        source[level..].starts_with(' ').then(|| {
                                            (
                                                level,
                                                source[level..]
                                                    .trim()
                                                    .trim_end_matches('#')
                                                    .trim()
                                                    .to_owned(),
                                            )
                                        })
                                    }
                                    _ => None,
                                };
                                if let (Some(l), Some((h, _))) = (fold_level, &heading)
                                    && *h <= l
                                {
                                    fold_level = None;
                                }
                                let touches_active = active.as_ref().is_some_and(|a| {
                                    a.start <= range.start && range.start <= a.end
                                });
                                if fold_level.is_some() && !touches_active {
                                    continue;
                                }
                                if let Some((h, text)) = &heading
                                    && drags.folded.contains(text)
                                {
                                    fold_level = Some(*h);
                                }
                                let top = ui.cursor().top();
                                let covered = active
                                    .as_ref()
                                    .is_some_and(|a| a.start <= range.start && range.end <= a.end);
                                if !covered {
                                    if action.is_none() {
                                        action = drags.unit_ui(
                                            ui,
                                            cu,
                                            project_ref,
                                            &note_path,
                                            &mut menu,
                                        );
                                    } else {
                                        drags.unit_ui(ui, cu, project_ref, &note_path, &mut menu);
                                    }
                                    unit_rects
                                        .push((range, egui::Rangef::new(top, ui.cursor().top())));
                                    continue;
                                }
                                let Some(range) = active.clone().filter(|a| a.start == range.start)
                                else {
                                    continue;
                                };
                                // Plusieurs unités sélectionnées : texte Markdown ordinaire.
                                let single = range == cu.unit.range;
                                let kind = if single { cu.unit.kind } else { UnitKind::Line };
                                let scope =
                                    ui.scope(|ui| {
                                        if kind == UnitKind::Table {
                                            table_op = table_toolbar(ui);
                                        }
                                        let mut buf = body[range.clone()].to_owned();
                                        let visuals = ui.visuals().clone();
                                        let mut layouter =
                                |ui: &egui::Ui, b: &dyn egui::TextBuffer, wrap_width: f32| {
                                    let mut job = match kind {
                                        UnitKind::Line | UnitKind::Callout | UnitKind::Comment => {
                                            highlighter.job(b.as_str(), project_ref, &visuals)
                                        }
                                        _ => mono_job(b.as_str(), kind, &visuals),
                                    };
                                    job.wrap.max_width = wrap_width;
                                    ui.fonts_mut(|f| f.layout_job(job))
                                };
                                        let bg = ui.painter().add(egui::Shape::Noop);
                                        let mut te = egui::TextEdit::multiline(&mut buf)
                                            .id(te_id)
                                            .layouter(&mut layouter)
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(1)
                                            .frame(egui::Frame::NONE)
                                            // Tab : cellules de tableau ou indentation ;
                                            // Échap ferme les suggestions sans faire
                                            // perdre le focus quand elles sont ouvertes.
                                            .event_filter(egui::EventFilter {
                                                tab: true,
                                                horizontal_arrows: true,
                                                vertical_arrows: true,
                                                escape: popup_open,
                                            });
                                        if empty {
                                            te = te.hint_text(
                                    "Écrivez ici… Markdown accepté. Tapez [[ pour lier une \
                                     fiche, Ctrl+clic sur un lien pour l'ouvrir.",
                                );
                                        }
                                        let output = te.show(ui);
                                        if reveal {
                                            ui.scroll_to_rect(
                                                output.response.rect,
                                                Some(egui::Align::Center),
                                            );
                                        }
                                        let r = output.response.rect.expand2(egui::vec2(6.0, 2.0));
                                        ui.painter().set(
                                            bg,
                                            egui::Shape::Vec(vec![
                                                egui::Shape::rect_filled(
                                                    r,
                                                    0.0,
                                                    visuals.extreme_bg_color,
                                                ),
                                                egui::Shape::rect_filled(
                                                    egui::Rect::from_min_size(
                                                        r.min,
                                                        egui::vec2(2.0, r.height()),
                                                    ),
                                                    0.0,
                                                    crate::theme::accent(&visuals),
                                                ),
                                            ]),
                                        );
                                        // Images de la ligne, toujours visibles et
                                        // redimensionnables pendant la modification.
                                        if single
                                            && cu.has_images()
                                            && let reading::Content::Line { blocks, .. } =
                                                &cu.content
                                        {
                                            ui.horizontal_wrapped(|ui| {
                                                for b in blocks {
                                                    if let enigma_core::render::Block::Image(img) =
                                                        b
                                                        && let Some(a) = drags.image_ui(
                                                            ui,
                                                            img,
                                                            project_ref,
                                                            &note_path,
                                                        )
                                                    {
                                                        action = Some(a);
                                                    }
                                                }
                                            });
                                        }
                                        (output, buf)
                                    });
                                active_rect = Some(scope.response.rect);
                                unit_rects.push((range.clone(), scope.response.rect.y_range()));
                                let (output, buf) = scope.inner;
                                edit = Some((output, buf, range, kind));
                            }
                            // Sous le texte : clic = écrire à la fin.
                            let rest = ui.available_size_before_wrap();
                            let (_, resp) = ui.allocate_exact_size(
                                egui::vec2(rest.x, rest.y.max(40.0)),
                                egui::Sense::click(),
                            );
                            if resp.on_hover_cursor(egui::CursorIcon::Text).clicked() {
                                action = Some(ViewAction::EditAt(body.len()));
                            }
                        });
                });
        }
        self.preview.units = units;
        self.unit_rects = unit_rects;

        // 4. Résultat de la modification.
        let mut cursor_byte = None;
        if let Some((output, buf, range, kind)) = edit {
            self.last_galley = Some(output.galley.clone());
            self.last_galley_pos = output.galley_pos;
            let local = |c: usize| range.start + char_to_byte(&buf, c);
            let sel = output
                .cursor_range
                .map(|r| (local(r.primary.index.0), local(r.secondary.index.0)));
            if output.response.changed() {
                self.body.replace_range(range.clone(), &buf);
                let mut cursor = sel.map_or(range.start + buf.len(), |(p, _)| p);
                let mut moved = false;
                if kind == UnitKind::Table
                    && let Some((t, c)) = table::edit(&buf, cursor - range.start, table::Op::Format)
                    && t != buf
                {
                    self.body
                        .replace_range(range.start..range.start + buf.len(), &t);
                    cursor = range.start + c;
                    moved = true;
                }
                self.commit_body(project, EditKind::Typing, now_ms);
                outcome.edited = true;
                self.preview.sync(&self.body);
                let idx = self.preview.unit_at(cursor);
                let now = self.preview.units[idx].unit.range.clone();
                if moved || now.start != range.start {
                    self.activate(&ctx, te_id, cursor, None);
                } else {
                    self.active = Some(now);
                }
                self.last_cursor = Some(cursor);
                self.last_sel = Some(cursor..cursor);
                cursor_byte = Some(cursor);
            } else if let Some((p, s)) = sel {
                self.last_cursor = Some(p);
                self.last_sel = Some(p.min(s)..p.max(s));
                cursor_byte = (p == s).then_some(p);
                // Sélection retirée : seule la ligne du curseur reste en
                // édition.
                let idx = self.preview.unit_at(p);
                let single = self.preview.units.get(idx).map(|u| u.unit.range.clone());
                if p == s
                    && self.drag_anchor.is_none()
                    && single.as_ref().is_some_and(|u| *u != range)
                    && !ctx.input(|i| i.pointer.any_down())
                {
                    self.activate(&ctx, te_id, p, None);
                }
            }
            // Glisser depuis le texte au-delà de ses bords : les lignes
            // survolées passent en édition.
            if output.response.dragged()
                && self.drag_anchor.is_none()
                && let Some((_, s)) = sel
                && ctx
                    .input(|i| i.pointer.interact_pos())
                    .is_some_and(|p| !output.response.rect.y_range().contains(p.y))
            {
                self.drag_anchor = Some(s);
            }

            // Clic droit : mise en forme de la sélection, ou insertions.
            let has_selection = self.last_sel.as_ref().is_some_and(|r| !r.is_empty());
            let mut chosen = None;
            output.response.context_menu(|ui| {
                chosen = format_menu(ui, has_selection);
            });
            if let Some(c) = chosen {
                self.apply_menu(c, &ctx, project, now_ms, &mut outcome);
            }

            // Ctrl+clic sur un lien.
            if output.response.clicked()
                && ctx.input(|i| i.modifiers.command)
                && let Some(link) = cursor_byte.and_then(|b| markdown::wiki_link_at(&self.body, b))
            {
                outcome.open = self.open_or_create(&link.target, project, notices);
                outcome.open_section = link.section.clone();
            }

            // Suggestions.
            let pointer_on_popup = self
                .popup_rect
                .zip(ctx.input(|i| i.pointer.hover_pos()))
                .is_some_and(|(r, p)| r.contains(p));
            if output.response.has_focus() {
                self.update_popup(project, cursor_byte, &output);
            } else if !pointer_on_popup {
                self.popup = None;
            }
            let pointer_on_slash = self
                .slash_rect
                .zip(ctx.input(|i| i.pointer.hover_pos()))
                .is_some_and(|(r, p)| r.contains(p));
            let slash_ctx = cursor_byte
                .filter(|_| self.popup.is_none() && output.response.has_focus())
                .filter(|_| matches!(kind, UnitKind::Line | UnitKind::Callout))
                .and_then(|b| crate::slash::context(&self.body, b));
            match slash_ctx {
                Some((start, query))
                    if self.slash_dismissed != Some(start)
                        && !crate::slash::items(&query).is_empty() =>
                {
                    let anchor = output
                        .cursor_range
                        .map(|r| {
                            output.galley_pos
                                + output
                                    .galley
                                    .pos_from_cursor(r.primary)
                                    .left_bottom()
                                    .to_vec2()
                        })
                        .unwrap_or(output.galley_pos);
                    let selected = match &self.slash {
                        Some(s) if s.start == start && s.query == query => s.selected,
                        _ => 0,
                    };
                    self.slash = Some(crate::slash::SlashPopup {
                        start,
                        query,
                        selected,
                        anchor: anchor + egui::vec2(0.0, 2.0),
                    });
                }
                _ if pointer_on_slash => {}
                Some(_) => self.slash = None,
                None => {
                    self.slash = None;
                    if cursor_byte.is_some() {
                        self.slash_dismissed = None;
                    }
                }
            }

            // Fin de la modification : clic ailleurs dans la fiche (pas
            // dans un menu), Échap, ou focus donné à un autre champ.
            let layer = ui.layer_id();
            let press = ctx.input(|i| {
                i.pointer
                    .any_pressed()
                    .then(|| i.pointer.press_origin())
                    .flatten()
            });
            let pressed_outside = press.is_some_and(|p| {
                !active_rect.is_some_and(|r| r.contains(p))
                    && ctx.layer_id_at(p).is_none_or(|l| l == layer)
            });
            let escape = !popup_open && ctx.input(|i| i.key_pressed(egui::Key::Escape));
            let elsewhere = ctx.memory(|m| m.focused().is_some_and(|f| f != te_id));
            if (pressed_outside || escape || elsewhere) && self.pending_cursor.is_none() {
                self.active = None;
                self.popup = None;
            }
        }

        // Sélection à la souris sur plusieurs lignes.
        if let Some(anchor) = self.drag_anchor {
            if ctx.input(|i| i.pointer.primary_down()) {
                if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                    let b = self.byte_at_pointer(p, anchor);
                    self.activate(&ctx, te_id, b, Some(anchor));
                    ctx.request_repaint();
                }
            } else {
                self.drag_anchor = None;
            }
        }

        // Fenêtre des commandes « / ».
        self.slash_rect = None;
        if let Some(sp) = &self.slash {
            let (clicked, rect) = sp.show(&ctx);
            self.slash_rect = Some(rect);
            if let Some(i) = clicked {
                outcome.edited |= self.accept_slash(i, project, now_ms, &mut outcome.insert_file);
                ctx.request_repaint();
            }
        }

        // 5. Fenêtre de suggestions (clic souris).
        self.popup_rect = None;
        if let Some(popup) = &self.popup {
            let (clicked, rect) = popup.show(&ctx);
            self.popup_rect = Some(rect);
            if let Some(i) = clicked {
                if let Some(p) = &mut self.popup {
                    p.selected = i;
                }
                outcome.edited |= self.navigate(Nav::Accept, project, now_ms, notices);
                ctx.request_repaint();
            }
        }

        // 6. Barre d'outils du tableau en cours de modification.
        if let (Some(op), Some(range)) = (table_op, self.active.clone()) {
            let cursor = self
                .last_cursor
                .filter(|c| range.contains(c) || *c == range.end)
                .unwrap_or(range.start);
            if let Some((t, c)) = table::edit(&self.body[range.clone()], cursor - range.start, op) {
                self.body.replace_range(range.clone(), &t);
                self.commit_body(project, EditKind::Discrete, now_ms);
                outcome.edited = true;
                self.activate(&ctx, te_id, range.start + c, None);
            }
        }

        // 7. Actions sur la fiche mise en forme.
        match action {
            Some(ViewAction::EditAt(b)) => {
                // Maj+clic : sélection depuis le curseur précédent.
                let anchor = ctx
                    .input(|i| i.modifiers.shift)
                    .then(|| self.selection_anchor())
                    .flatten();
                self.activate(&ctx, te_id, b, anchor);
            }
            Some(ViewAction::DragFrom(b)) => {
                self.activate(&ctx, te_id, b, None);
                self.drag_anchor = Some(b);
            }
            Some(ViewAction::OpenTitle(title, section)) => {
                outcome.open = self.open_or_create(&title, project, notices);
                outcome.open_section = section;
            }
            Some(ViewAction::SearchTag(t)) => outcome.search_tag = Some(t),
            Some(ViewAction::ToggleFold(key)) => {
                if !self.preview.drags.folded.remove(&key) {
                    self.preview.drags.folded.insert(key);
                }
            }
            Some(ViewAction::OpenFile(path)) => {
                if let Err(e) = crate::system_open::open(&path) {
                    notices.error(format!("Impossible d'ouvrir « {} » : {e}", path.display()));
                }
            }
            Some(ViewAction::Resize { image, width }) => {
                self.body = enigma_core::render::set_image_width(&self.body, &image, Some(width));
                self.realign_table_at(image.range.start);
                self.commit_body(project, EditKind::Discrete, now_ms);
                outcome.edited = true;
            }
            Some(ViewAction::ToggleTask(range)) => {
                if let Some(line) = self.body.get(range.clone()).and_then(live::toggle_task) {
                    self.body.replace_range(range, &line);
                    self.commit_body(project, EditKind::Discrete, now_ms);
                    outcome.edited = true;
                }
            }
            Some(ViewAction::TableResize { range, rows, cols }) => {
                if let Some((t, _)) = self
                    .body
                    .get(range.clone())
                    .and_then(|t| table::edit(t, 0, table::Op::Resize { rows, cols }))
                {
                    self.body.replace_range(range, &t);
                    self.commit_body(project, EditKind::Discrete, now_ms);
                    outcome.edited = true;
                }
            }
            Some(ViewAction::Menu(choice, b)) => {
                self.last_cursor = Some(b);
                self.last_sel = Some(b..b);
                self.apply_menu(choice, &ctx, project, now_ms, &mut outcome);
            }
            None => {}
        }
        outcome.insert_template = std::mem::take(&mut self.want_template);
        outcome
    }

    /// Applique la commande « / » numéro `i` : retire `/requête` puis
    /// insère. Rend `true` si le texte a changé.
    fn accept_slash(
        &mut self,
        i: usize,
        project: &mut Project,
        now_ms: u64,
        insert_file: &mut bool,
    ) -> bool {
        let Some(sp) = self.slash.take() else {
            return false;
        };
        let Some(item) = crate::slash::items(&sp.query).into_iter().nth(i) else {
            return false;
        };
        let end = (sp.start + 1 + sp.query.len()).min(self.body.len());
        self.body.replace_range(sp.start..end, "");
        self.history.break_group();
        let at = sp.start;
        match item.action {
            crate::slash::SlashAction::Format(f) => {
                self.commit_body(project, EditKind::Discrete, now_ms);
                self.last_sel = Some(at..at);
                self.last_cursor = Some(at);
                self.format(f, project, now_ms);
            }
            crate::slash::SlashAction::Snippet { text, cursor } => {
                self.body.insert_str(at, text);
                self.commit_body(project, EditKind::Discrete, now_ms);
                self.pending_cursor = Some(self.body[..at + cursor].chars().count());
            }
            crate::slash::SlashAction::InsertTemplate => {
                self.commit_body(project, EditKind::Discrete, now_ms);
                self.last_cursor = Some(at);
                self.pending_cursor = Some(self.body[..at].chars().count());
                self.want_template = true;
            }
            crate::slash::SlashAction::InsertFile => {
                self.commit_body(project, EditKind::Discrete, now_ms);
                self.last_cursor = Some(at);
                self.pending_cursor = Some(self.body[..at].chars().count());
                *insert_file = true;
            }
        }
        true
    }

    /// Ouvre la barre « Rechercher / remplacer dans la fiche ».
    pub fn open_find(&mut self) {
        let sel = self.selected_text();
        let find = self.find.get_or_insert_with(FindBar::default);
        if !sel.is_empty() && !sel.contains('\n') {
            find.query = sel;
        }
        find.focus = true;
    }

    /// Correspondances de la recherche dans la fiche (octets).
    fn find_matches(&self) -> Vec<Range<usize>> {
        let Some(f) = &self.find else {
            return Vec::new();
        };
        find_all(&self.body, &f.query, f.case)
    }

    /// Barre de recherche/remplacement ; rend `true` si le texte a changé.
    fn find_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        te_id: egui::Id,
        project: &mut Project,
        now_ms: u64,
    ) -> bool {
        let matches = self.find_matches();
        let Some(f) = &mut self.find else {
            return false;
        };
        enum Act {
            Go(usize),
            Replace,
            ReplaceAll,
            Close,
        }
        let mut act = None;
        let n = matches.len();
        if n > 0 {
            f.current = f.current.min(n - 1);
        }
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .inner_margin(6)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let q = ui.add(
                        egui::TextEdit::singleline(&mut f.query)
                            .hint_text("Rechercher dans la fiche")
                            .desired_width(180.0),
                    );
                    if f.focus {
                        q.request_focus();
                        f.focus = false;
                    }
                    if q.changed() {
                        f.current = 0;
                    }
                    let enter = q.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let shift = ui.input(|i| i.modifiers.shift);
                    if q.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        act = Some(Act::Close);
                    }
                    ui.label(if f.query.is_empty() {
                        String::new()
                    } else if n == 0 {
                        "aucun résultat".to_owned()
                    } else {
                        format!("{} / {n}", f.current + 1)
                    });
                    if (ui.button("Préc.").clicked() || (enter && shift)) && n > 0 {
                        act = Some(Act::Go((f.current + n - 1) % n));
                    }
                    if (ui.button("Suiv.").clicked() || (enter && !shift)) && n > 0 {
                        act = Some(Act::Go((f.current + 1) % n));
                    }
                    ui.toggle_value(&mut f.case, "Aa")
                        .on_hover_text("Respecter la casse");
                    ui.separator();
                    ui.add(
                        egui::TextEdit::singleline(&mut f.replace)
                            .hint_text("Remplacer par")
                            .desired_width(140.0),
                    );
                    if ui
                        .add_enabled(n > 0, egui::Button::new("Remplacer"))
                        .clicked()
                    {
                        act = Some(Act::Replace);
                    }
                    if ui
                        .add_enabled(n > 0, egui::Button::new("Tout"))
                        .on_hover_text("Tout remplacer")
                        .clicked()
                    {
                        act = Some(Act::ReplaceAll);
                    }
                    if ui.button("Fermer").clicked() {
                        act = Some(Act::Close);
                    }
                });
            });
        let replace = f.replace.clone();
        match act {
            None => false,
            Some(Act::Close) => {
                self.find = None;
                false
            }
            Some(Act::Go(i)) => {
                f.current = i;
                if let Some(r) = matches.get(i) {
                    self.activate(ctx, te_id, r.end, Some(r.start));
                    self.last_cursor = Some(r.end);
                    self.last_sel = Some(r.clone());
                    self.reveal = true;
                }
                false
            }
            Some(Act::Replace) => {
                let Some(r) = matches.get(f.current).cloned() else {
                    return false;
                };
                self.body.replace_range(r.clone(), &replace);
                self.commit_body(project, EditKind::Discrete, now_ms);
                let next = r.start + replace.len();
                self.activate(ctx, te_id, next, Some(r.start));
                self.reveal = true;
                true
            }
            Some(Act::ReplaceAll) => {
                for r in matches.iter().rev() {
                    self.body.replace_range(r.clone(), &replace);
                }
                self.commit_body(project, EditKind::Discrete, now_ms);
                true
            }
        }
    }

    /// Plan de la fiche (titres de section) ; un clic place l'éditeur sur
    /// la section.
    pub fn outline_ui(&mut self, ui: &mut egui::Ui) {
        let heads = enigma_core::sections::headings(&self.body);
        egui::CollapsingHeader::new(format!("Plan ({})", heads.len()))
            .id_salt("outline")
            .default_open(true)
            .show(ui, |ui| {
                if heads.is_empty() {
                    ui.weak("Aucun titre (# Titre).");
                }
                let mut go = None;
                for h in &heads {
                    ui.horizontal(|ui| {
                        ui.add_space(12.0 * f32::from(h.level.saturating_sub(1)));
                        let folded = self.preview.drags.folded.contains(&h.text);
                        let text = if folded {
                            format!("{} (replié)", h.text)
                        } else {
                            h.text.clone()
                        };
                        if ui.link(text).clicked() {
                            go = Some(h.line.end);
                        }
                    });
                }
                if let Some(b) = go {
                    self.reveal(b);
                }
            });
    }

    /// Fiche liée : ouverte, ou créée si elle n'existe pas.
    fn open_or_create(
        &self,
        title: &str,
        project: &mut Project,
        notices: &mut Notices,
    ) -> Option<NoteId> {
        match project.resolve_title(title) {
            Some(id) => Some(id.clone()),
            None => match project.create_note(title, Vec::new(), String::new()) {
                Ok(id) => {
                    notices.info(format!("Fiche « {title} » créée."));
                    Some(id)
                }
                Err(e) => {
                    notices.error(format!("Création impossible : {e}"));
                    None
                }
            },
        }
    }

    fn apply_menu(
        &mut self,
        choice: MenuChoice,
        ctx: &egui::Context,
        project: &mut Project,
        now_ms: u64,
        outcome: &mut BodyOutcome,
    ) {
        match choice {
            MenuChoice::Format(f) => {
                self.format(f, project, now_ms);
                outcome.edited = true;
            }
            MenuChoice::InsertFile => outcome.insert_file = true,
            MenuChoice::Extract => outcome.extract = true,
            MenuChoice::CopyLinkHere => {
                let title = project
                    .note(&self.id)
                    .map(|n| n.meta.title.clone())
                    .unwrap_or_default();
                let at = self.last_cursor.unwrap_or(0).min(self.body.len());
                let line_start = self.body[..at].rfind('\n').map_or(0, |i| i + 1);
                let heading = enigma_core::sections::headings(&self.body)
                    .into_iter()
                    .find(|h| h.line.start == line_start);
                let link = match heading {
                    Some(h) => format!("[[{title}#{}]]", h.text),
                    None => {
                        let (text, id) = enigma_core::sections::ensure_block_id(&self.body, at);
                        if text != self.body {
                            self.body = text;
                            self.commit_body(project, EditKind::Discrete, now_ms);
                            outcome.edited = true;
                        }
                        format!("[[{title}#^{id}]]")
                    }
                };
                ctx.copy_text(link);
            }
            MenuChoice::Copy => ctx.copy_text(self.selected_text()),
            MenuChoice::Cut => {
                ctx.copy_text(self.selected_text());
                self.format(Format::Delete, project, now_ms);
                outcome.edited = true;
            }
        }
    }

    /// Modifie l'unité contenant l'octet `b`, curseur à cet endroit. Avec
    /// `anchor` : sélection depuis cet octet, toutes les unités entre les
    /// deux passent en édition.
    fn activate(&mut self, ctx: &egui::Context, te_id: egui::Id, b: usize, anchor: Option<usize>) {
        self.preview.sync(&self.body);
        let mut b = b.min(self.body.len());
        while !self.body.is_char_boundary(b) {
            b -= 1;
        }
        let idx = self.preview.unit_at(b);
        let ia = anchor.map_or(idx, |a| self.preview.unit_at(a.min(self.body.len())));
        let (Some(first), Some(last)) = (
            self.preview.units.get(idx.min(ia)),
            self.preview.units.get(idx.max(ia)),
        ) else {
            return;
        };
        let range = first.unit.range.start..last.unit.range.end;
        let local = |x: usize| {
            let x = x.clamp(range.start, range.end);
            self.body
                .get(range.start..x)
                .map_or(0, |s| s.chars().count())
        };
        let cursor = egui::text::CCursor::new(local(b));
        let sel = match anchor.map(local) {
            Some(a) if a != cursor.index.0 => {
                egui::text::CCursorRange::two(egui::text::CCursor::new(a), cursor)
            }
            _ => egui::text::CCursorRange::one(cursor),
        };
        let mut state = egui::TextEdit::load_state(ctx, te_id).unwrap_or_default();
        state.cursor.set_char_range(Some(sel));
        state.store(ctx, te_id);
        ctx.memory_mut(|m| m.request_focus(te_id));
        self.active = Some(range);
        self.last_cursor = Some(b);
    }

    /// Recale l'unité modifiée après un changement venu d'ailleurs
    /// (annuler, rechargement…). Fiche vide : toujours modifiable.
    fn resolve_active(&mut self) {
        if self.body.is_empty() {
            self.active = Some(0..0);
            return;
        }
        if let Some(r) = self.active.clone() {
            let units = &self.preview.units;
            let ok = units.iter().any(|u| u.unit.range.start == r.start)
                && units.iter().any(|u| u.unit.range.end == r.end);
            if !ok {
                let len = self.body.len();
                let a = units.get(self.preview.unit_at(r.start.min(len)));
                let b = units.get(self.preview.unit_at(r.end.min(len)));
                self.active = a.zip(b).map(|(a, b)| a.unit.range.start..b.unit.range.end);
            }
        }
    }

    /// Bout fixe de la sélection courante (octet).
    fn selection_anchor(&self) -> Option<usize> {
        let sel = self.last_sel.clone()?;
        Some(if self.last_cursor == Some(sel.start) {
            sel.end
        } else {
            sel.start
        })
    }

    /// Octet sous le pointeur pendant une sélection à la souris : dans le
    /// texte en édition, la position exacte ; sur une autre ligne, son début
    /// ou sa fin selon le sens.
    fn byte_at_pointer(&self, p: egui::Pos2, anchor: usize) -> usize {
        if let (Some(range), Some(g)) = (&self.active, &self.last_galley)
            && let Some((_, y)) = self.unit_rects.iter().find(|(r, _)| r == range)
            && y.contains(p.y)
        {
            let c = g.cursor_from_pos(p - self.last_galley_pos).index.0;
            return range.start + char_to_byte(&self.body[range.clone()], c);
        }
        let hit = self
            .unit_rects
            .iter()
            .find(|(_, y)| y.contains(p.y))
            .or_else(|| {
                let first = self.unit_rects.first()?;
                if p.y < first.1.min {
                    Some(first)
                } else {
                    self.unit_rects.last()
                }
            });
        match hit {
            Some((r, _)) if r.start >= anchor => r.end,
            Some((r, _)) => r.start,
            None => anchor,
        }
    }

    /// Réaligne le tableau contenant l'octet `b` (après un redimensionnement
    /// d'image dans une cellule).
    fn realign_table_at(&mut self, b: usize) {
        let units = live::units(&self.body);
        let Some(u) = units.get(live::unit_at(&units, b)) else {
            return;
        };
        if u.kind == UnitKind::Table
            && let Some((t, _)) = table::edit(&self.body[u.range.clone()], 0, table::Op::Format)
        {
            self.body.replace_range(u.range.clone(), &t);
        }
    }

    /// Touches traitées avant le `TextEdit` :
    /// - flèches, Retour arrière et Suppr aux bords : passage à l'unité
    ///   voisine (ou fusion des deux lignes) ; avec Maj, la sélection
    ///   s'étend et les lignes traversées passent en édition ; Ctrl+A
    ///   sélectionne toute la fiche ;
    /// - listes : Entrée reprend la puce, Tab / Maj+Tab changent le niveau ;
    /// - tableau : Tab, Maj+Tab et Entrée passent d'une cellule à l'autre.
    ///
    /// Rend `true` si le texte a changé.
    fn unit_keys(
        &mut self,
        ctx: &egui::Context,
        te_id: egui::Id,
        project: &mut Project,
        now_ms: u64,
    ) -> bool {
        use egui::{Key, Modifiers};
        enum Move {
            To(usize),
            /// Échange le texte en édition avec l'unité voisine (plage).
            Swap(Range<usize>),
            Select(usize, usize),
            Join(Range<usize>, usize),
            Table(table::Op),
            Text(md_edit::Edit),
        }
        let Some(range) = self.active.clone() else {
            return false;
        };
        let Some(cr) = egui::TextEdit::load_state(ctx, te_id).and_then(|s| s.cursor.char_range())
        else {
            return false;
        };
        let first = self.preview.unit_at(range.start);
        let last = self.preview.unit_at(range.end);
        let units = &self.preview.units;
        let Some(kind) = units.get(first).map(|u| u.unit.kind) else {
            return false;
        };
        let kind = if first == last { kind } else { UnitKind::Line };
        let text = &self.body[range.clone()];
        let n = text.chars().count();
        let c = cr.primary.index.0.min(n);
        let selecting = cr.primary.index != cr.secondary.index;
        let cursor = range.start + char_to_byte(text, c);
        let anchor = range.start + char_to_byte(text, cr.secondary.index.0.min(n));
        let prev = first
            .checked_sub(1)
            .and_then(|i| units.get(i))
            .map(|u| u.unit.range.clone());
        let next = units.get(last + 1).map(|u| u.unit.range.clone());
        // Ligne affichée du curseur (texte replié compris).
        let (row, rows, col) = match &self.last_galley {
            Some(g) if g.job.text == text => {
                let l = g.layout_from_cursor(cr.primary);
                (l.row, g.rows.len(), l.column.0)
            }
            _ => {
                let before: String = text.chars().take(c).collect();
                let row = before.matches('\n').count();
                let col = before.rsplit('\n').next().map_or(0, |s| s.chars().count());
                (row, text.matches('\n').count() + 1, col)
            }
        };
        let body = &self.body;
        let line_col = |r: &Range<usize>, last_line: bool| {
            let t = &body[r.clone()];
            let start = if last_line {
                t.rfind('\n').map_or(0, |i| i + 1)
            } else {
                0
            };
            let line = &t[start..];
            let line = line.split('\n').next().unwrap_or("");
            let off = line.char_indices().nth(col).map_or(line.len(), |(i, _)| i);
            r.start + start + off
        };
        let in_list = matches!(kind, UnitKind::Line | UnitKind::Callout) && !selecting;
        let enter = in_list.then(|| md_edit::list_enter(body, cursor)).flatten();
        let indent = in_list
            .then(|| md_edit::list_indent(body, cursor, false))
            .flatten();
        let outdent = in_list
            .then(|| md_edit::list_indent(body, cursor, true))
            .flatten();
        let shift = Modifiers::SHIFT;
        let shift_cmd = Modifiers::SHIFT | Modifiers::COMMAND;
        let mv = ctx.input_mut(|i| {
            if i.consume_key(Modifiers::COMMAND, Key::A) {
                return Some(Move::Select(0, body.len()));
            }
            if let Some(p) = &prev
                && i.consume_key(Modifiers::ALT, Key::ArrowUp)
            {
                return Some(Move::Swap(p.clone()));
            }
            if let Some(nx) = &next
                && i.consume_key(Modifiers::ALT, Key::ArrowDown)
            {
                return Some(Move::Swap(nx.clone()));
            }
            if i.consume_key(shift_cmd, Key::Home) {
                return Some(Move::Select(anchor, 0));
            }
            if i.consume_key(shift_cmd, Key::End) {
                return Some(Move::Select(anchor, body.len()));
            }
            if i.consume_key(Modifiers::COMMAND, Key::Home) {
                return Some(Move::To(0));
            }
            if i.consume_key(Modifiers::COMMAND, Key::End) {
                return Some(Move::To(body.len()));
            }
            // Maj+flèches au bord : la sélection gagne l'unité voisine.
            if let Some(p) = &prev {
                if row == 0 && i.consume_key(shift, Key::ArrowUp) {
                    return Some(Move::Select(anchor, line_col(p, true)));
                }
                if c == 0 && i.consume_key(shift, Key::ArrowLeft) {
                    return Some(Move::Select(anchor, p.end));
                }
            }
            if let Some(nx) = &next {
                if row + 1 >= rows && i.consume_key(shift, Key::ArrowDown) {
                    return Some(Move::Select(anchor, line_col(nx, false)));
                }
                if c == n && i.consume_key(shift, Key::ArrowRight) {
                    return Some(Move::Select(anchor, nx.end.min(nx.start)));
                }
            }
            if selecting {
                return None;
            }
            if kind == UnitKind::Table {
                if i.consume_key(Modifiers::SHIFT, Key::Tab) {
                    return Some(Move::Table(table::Op::PrevCell));
                }
                if i.consume_key(Modifiers::NONE, Key::Tab) {
                    return Some(Move::Table(table::Op::NextCell));
                }
                if i.consume_key(Modifiers::NONE, Key::Enter) {
                    return Some(Move::Table(table::Op::NextRow));
                }
            }
            if let Some(e) = &enter
                && i.consume_key(Modifiers::NONE, Key::Enter)
            {
                return Some(Move::Text(e.clone()));
            }
            if let Some(e) = &outdent
                && i.consume_key(Modifiers::SHIFT, Key::Tab)
            {
                return Some(Move::Text(e.clone()));
            }
            if let Some(e) = &indent
                && i.consume_key(Modifiers::NONE, Key::Tab)
            {
                return Some(Move::Text(e.clone()));
            }
            if let Some(p) = &prev {
                if row == 0 && i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    return Some(Move::To(line_col(p, true)));
                }
                if c == 0 && i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                    return Some(Move::To(p.end));
                }
                if c == 0 && i.consume_key(Modifiers::NONE, Key::Backspace) {
                    return Some(Move::Join(p.end..range.start, p.end));
                }
            }
            if let Some(nx) = &next {
                if row + 1 >= rows && i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    return Some(Move::To(line_col(nx, false)));
                }
                if c == n && i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                    return Some(Move::To(nx.start));
                }
                if c == n && i.consume_key(Modifiers::NONE, Key::Delete) {
                    return Some(Move::Join(range.end..nx.start, range.end));
                }
            }
            None
        });
        match mv {
            None => false,
            Some(Move::To(b)) => {
                self.activate(ctx, te_id, b, None);
                false
            }
            Some(Move::Swap(other)) => {
                let mine = self.body[range.clone()].to_owned();
                let theirs = self.body[other.clone()].to_owned();
                let offset = cursor - range.start;
                let new_start = if other.start < range.start {
                    let sep = self.body[other.end..range.start].to_owned();
                    self.body
                        .replace_range(other.start..range.end, &format!("{mine}{sep}{theirs}"));
                    other.start
                } else {
                    let sep = self.body[range.end..other.start].to_owned();
                    self.body
                        .replace_range(range.start..other.end, &format!("{theirs}{sep}{mine}"));
                    range.start + theirs.len() + sep.len()
                };
                self.history.break_group();
                self.commit_body(project, EditKind::Discrete, now_ms);
                self.activate(ctx, te_id, new_start + offset, None);
                true
            }
            Some(Move::Select(a, b)) => {
                self.activate(ctx, te_id, b, Some(a));
                self.last_cursor = Some(b);
                self.last_sel = Some(a.min(b)..a.max(b));
                false
            }
            Some(Move::Join(sep, b)) => {
                self.body.replace_range(sep, "");
                self.commit_body(project, EditKind::Typing, now_ms);
                self.activate(ctx, te_id, b, None);
                true
            }
            Some(Move::Text(e)) => {
                self.body = e.text;
                self.history.break_group();
                self.commit_body(project, EditKind::Discrete, now_ms);
                self.activate(ctx, te_id, e.selection.end, None);
                true
            }
            Some(Move::Table(op)) => match table::edit(text, cursor - range.start, op) {
                Some((t, pos)) => {
                    self.body.replace_range(range.clone(), &t);
                    self.commit_body(project, EditKind::Discrete, now_ms);
                    self.activate(ctx, te_id, range.start + pos, None);
                    true
                }
                None => false,
            },
        }
    }

    /// Place le curseur à cet octet et fait défiler jusqu'à lui.
    pub fn reveal(&mut self, b: usize) {
        let b = b.min(self.body.len());
        if self.body.is_char_boundary(b) {
            self.pending_cursor = Some(self.body[..b].chars().count());
            self.reveal = true;
        }
    }

    /// Commence à écrire à la fin de la fiche (tests).
    #[cfg(test)]
    pub fn edit_at_end(&mut self) {
        self.pending_cursor = Some(self.body.chars().count());
    }

    /// Replie ou déplie une section (tests).
    #[cfg(test)]
    pub fn toggle_fold(&mut self, title: &str) {
        if !self.preview.drags.folded.remove(title) {
            self.preview.drags.folded.insert(title.to_owned());
        }
    }

    /// Sélectionne une plage (octets) en édition (tests).
    #[cfg(test)]
    pub fn select_bytes(&mut self, a: usize, b: usize) {
        self.pending_anchor = Some(self.body[..a].chars().count());
        self.pending_cursor = Some(self.body[..b].chars().count());
    }

    /// Commence à écrire à cet octet (tests).
    #[cfg(test)]
    pub fn edit_at(&mut self, b: usize) {
        self.pending_cursor = Some(self.body[..b].chars().count());
    }

    /// Une unité est-elle en cours de modification ? (tests)
    #[cfg(test)]
    pub fn is_editing(&self) -> bool {
        self.active.is_some()
    }

    /// Texte sélectionné (vide sans sélection).
    pub fn selection_text(&self) -> String {
        self.selected_text()
    }

    /// Remplace la sélection par `text` (étape d'historique).
    pub fn replace_selection(&mut self, text: &str, project: &mut Project, now_ms: u64) {
        let len = self.body.len();
        let Some(sel) = self
            .last_sel
            .clone()
            .filter(|r| r.end <= len && !r.is_empty())
        else {
            return;
        };
        self.body.replace_range(sel.clone(), text);
        let end = sel.start + text.len();
        self.last_sel = Some(end..end);
        self.last_cursor = Some(end);
        self.pending_cursor = Some(self.body[..end].chars().count());
        self.commit_body(project, EditKind::Discrete, now_ms);
    }

    /// Texte sélectionné (vide sans sélection).
    fn selected_text(&self) -> String {
        self.last_sel
            .clone()
            .and_then(|r| self.body.get(r))
            .unwrap_or_default()
            .to_owned()
    }

    /// Applique un outil d'écriture à la sélection, comme une étape
    /// d'historique, et rétablit la sélection.
    pub fn format(&mut self, f: Format, project: &mut Project, now_ms: u64) {
        let len = self.body.len();
        let sel = self
            .last_sel
            .clone()
            .filter(|r| r.end <= len)
            .unwrap_or(len..len);
        let text = self.body.as_str();
        let edit = match f {
            Format::Inline(k) => md_edit::toggle_inline(text, sel, k),
            Format::Line(k) => md_edit::toggle_line(text, sel, k),
            Format::CodeBlock => md_edit::code_block(text, sel),
            Format::Table(r, c) => md_edit::insert_block(text, sel, &md_edit::table(r, c), 2),
            Format::Rule => md_edit::insert_block(text, sel, "---", 3),
            Format::Date => {
                let date = crate::project_file::file_date(std::time::SystemTime::now());
                md_edit::insert_inline(text, sel, date.get(..10).unwrap_or(&date))
            }
            Format::Delete => md_edit::insert_inline(text, sel, ""),
        };
        if edit.text == self.body {
            return;
        }
        self.body = edit.text;
        let a = self.body[..edit.selection.start].chars().count();
        let b = self.body[..edit.selection.end].chars().count();
        self.last_sel = Some(edit.selection.clone());
        self.last_cursor = Some(edit.selection.end);
        self.pending_anchor = Some(a);
        self.pending_cursor = Some(b);
        self.commit_body(project, EditKind::Discrete, now_ms);
    }

    /// Insère du texte au curseur (ou à la fin), comme une étape
    /// d'historique. `block` : le texte forme un paragraphe à part (lignes
    /// vides autour), comme une image ou un tableau.
    pub fn insert_text(&mut self, text: &str, block: bool, project: &mut Project, now_ms: u64) {
        let len = self.body.len();
        let pos = self
            .last_cursor
            .filter(|&p| p <= len && self.body.is_char_boundary(p))
            .unwrap_or(len);
        let mut insert = String::new();
        if block {
            let before = &self.body[..pos];
            if !before.is_empty() && !before.ends_with("\n\n") {
                insert.push_str(if before.ends_with('\n') { "\n" } else { "\n\n" });
            }
        }
        insert.push_str(text);
        if block {
            let after = &self.body[pos..];
            if !after.starts_with("\n\n") {
                insert.push_str(if after.starts_with('\n') {
                    "\n"
                } else {
                    "\n\n"
                });
            }
        }
        self.body.insert_str(pos, &insert);
        let end = pos + insert.len();
        self.last_cursor = Some(end);
        self.pending_cursor = Some(self.body[..end].chars().count());
        self.commit_body(project, EditKind::Discrete, now_ms);
    }

    /// Insère un modèle (déjà rempli) en bloc au curseur ; `cursor` :
    /// position dans `text` où placer le curseur ensuite.
    pub fn insert_template(
        &mut self,
        text: &str,
        cursor: Option<usize>,
        project: &mut Project,
        now_ms: u64,
    ) {
        let len = self.body.len();
        let pos = self
            .last_cursor
            .filter(|&p| p <= len && self.body.is_char_boundary(p))
            .unwrap_or(len);
        self.insert_text(text, true, project, now_ms);
        if let Some(c) = cursor
            && let Some(start) = self.body[pos..].find(text).map(|i| pos + i)
        {
            self.pending_cursor = Some(self.body[..start + c].chars().count());
        }
    }

    /// Enregistre le tampon courant comme nouvel état (historique + projet).
    fn commit_body(&mut self, project: &mut Project, kind: EditKind, now_ms: u64) {
        let old_body = std::mem::replace(&mut self.snap.body, self.body.clone());
        self.history.record(
            Snapshot {
                meta: self.snap.meta.clone(),
                body: old_body,
            },
            kind,
            now_ms,
        );
        if let Err(e) = project.set_body(&self.id, self.body.clone()) {
            tracing::warn!("modification du texte : {e}");
        }
    }

    fn update_popup(
        &mut self,
        project: &Project,
        cursor_byte: Option<usize>,
        output: &egui::text_edit::TextEditOutput,
    ) {
        let context =
            cursor_byte.and_then(|b| markdown::completion_at(&self.body, b).map(|c| (b, c)));
        let Some((byte, context)) = context else {
            self.popup = None;
            self.dismissed = None;
            return;
        };
        if self.dismissed == Some(context.start()) {
            self.popup = None;
            return;
        }
        self.dismissed = None;
        // Suggestions recalculées seulement si la saisie ou le projet changent.
        let generation = project.generation();
        let items = match &self.popup {
            Some(p) if p.context == context && p.generation == generation => p.items.clone(),
            _ => completion::items_for(&context, project),
        };
        if items.is_empty() {
            self.popup = None;
            return;
        }
        let selected = match &self.popup {
            Some(p) if p.context.start() == context.start() => p.selected.min(items.len() - 1),
            _ => 0,
        };
        let anchor = output
            .cursor_range
            .map(|r| {
                output.galley_pos
                    + output
                        .galley
                        .pos_from_cursor(r.primary)
                        .left_bottom()
                        .to_vec2()
            })
            .unwrap_or(output.galley_pos);
        self.popup = Some(Popup {
            context,
            cursor_byte: byte,
            items,
            selected,
            anchor: anchor + egui::vec2(0.0, 2.0),
            generation,
        });
    }

    /// Applique une action de navigation ; renvoie `true` si le texte a changé.
    fn navigate(
        &mut self,
        nav: Nav,
        project: &mut Project,
        now_ms: u64,
        notices: &mut Notices,
    ) -> bool {
        let Some(popup) = &mut self.popup else {
            return false;
        };
        let n = popup.items.len();
        match nav {
            Nav::Down => {
                popup.selected = (popup.selected + 1) % n;
                false
            }
            Nav::Up => {
                popup.selected = (popup.selected + n - 1) % n;
                false
            }
            Nav::Dismiss => {
                self.dismissed = Some(popup.context.start());
                self.popup = None;
                false
            }
            Nav::Accept | Nav::AcceptThenType => {
                let Some(popup) = self.popup.take() else {
                    return false;
                };
                let Some(item) = popup.items.get(popup.selected) else {
                    return false;
                };
                let choice = match &item.kind {
                    ItemKind::Note(id) => match project.note(id) {
                        Some(n) => n.meta.title.clone(),
                        None => return false,
                    },
                    ItemKind::Create(title) => {
                        match project.create_note(title, Vec::new(), String::new()) {
                            Ok(_) => {
                                notices.info(format!("Fiche « {title} » créée."));
                                title.clone()
                            }
                            Err(e) => {
                                notices.error(format!("Création impossible : {e}"));
                                return false;
                            }
                        }
                    }
                    ItemKind::LinkType(id) | ItemKind::Section(id) => id.clone(),
                };
                let then_type = nav == Nav::AcceptThenType && !project.pack().link_types.is_empty();
                let (text, cursor) = markdown::apply_completion(
                    &self.body,
                    popup.cursor_byte.min(self.body.len()),
                    &popup.context,
                    &choice,
                    then_type,
                );
                self.body = text;
                self.history.break_group();
                self.commit_body(project, EditKind::Discrete, now_ms);
                self.pending_cursor = Some(self.body[..cursor].chars().count());
                true
            }
        }
    }

    /// Panneau latéral : champs personnalisés du pack et informations.
    pub fn fields_ui(
        &mut self,
        ui: &mut egui::Ui,
        project: &mut Project,
        now_ms: u64,
        notices: &mut Notices,
        user_packs: Option<&std::path::Path>,
    ) -> bool {
        let mut new_meta: Option<NoteMeta> = None;

        let mut add_field = None;
        ui.horizontal(|ui| {
            ui.heading("Champs");
            add_field = self.quick_field.menu(
                ui,
                "+ Champ",
                "Créer un champ (ajouté au pack du projet, pour toutes les fiches)",
                quick_add::Kind::Field,
            );
        });
        if let Some(item) = add_field {
            match project.update_pack(user_packs, |p| {
                p.add_field(&item.label, item.field_kind);
            }) {
                Ok(()) => notices.info(format!("Champ « {} » ajouté au pack.", item.label)),
                Err(e) => notices.error(format!("Champ non créé : {e}")),
            }
        }
        let defs = project.pack().fields.clone();
        if defs.is_empty() {
            ui.weak("Aucun champ : « + » pour en créer un.");
        }
        egui::Grid::new("fields")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                for def in &defs {
                    ui.label(&def.label);
                    let current = self.snap.meta.fields.get(&def.id);
                    match def.kind {
                        FieldKind::Bool => {
                            let mut v = matches!(current, Some(FieldValue::Bool(true)));
                            if ui.checkbox(&mut v, "").changed() {
                                let mut m = self.snap.meta.clone();
                                m.fields.insert(def.id.clone(), FieldValue::Bool(v));
                                new_meta = Some(m);
                            }
                        }
                        FieldKind::List => {
                            let items: Vec<String> = match current {
                                Some(FieldValue::List(l)) => l.clone(),
                                Some(v) if !v.display().is_empty() => vec![v.display()],
                                _ => Vec::new(),
                            };
                            let key = format!("{}#nouveau", def.id);
                            ui.vertical(|ui| {
                                let mut remove = None;
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    for (i, it) in items.iter().enumerate() {
                                        if ui
                                            .add(egui::Button::new(format!("{it} ×")).small())
                                            .on_hover_text("Retirer")
                                            .clicked()
                                        {
                                            remove = Some(i);
                                        }
                                    }
                                });
                                let buf = self.field_bufs.entry(key).or_default();
                                let r = ui.add(
                                    egui::TextEdit::singleline(buf)
                                        .desired_width(140.0)
                                        .hint_text("+ ajouter (Entrée)"),
                                );
                                let mut list = items.clone();
                                if let Some(i) = remove {
                                    list.remove(i);
                                }
                                if r.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                                    && !buf.trim().is_empty()
                                {
                                    list.push(buf.trim().to_owned());
                                    buf.clear();
                                    r.request_focus();
                                }
                                if list != items {
                                    let mut m = self.snap.meta.clone();
                                    if list.is_empty() {
                                        m.fields.remove(&def.id);
                                    } else {
                                        m.fields.insert(def.id.clone(), FieldValue::List(list));
                                    }
                                    new_meta = Some(m);
                                }
                            });
                        }
                        FieldKind::Link => {
                            let current_id = match current {
                                Some(FieldValue::Text(s)) => NoteId::parse(s),
                                _ => None,
                            };
                            let title = current_id
                                .as_ref()
                                .and_then(|id| project.note(id))
                                .map(|n| n.meta.title.clone());
                            let shown = match (&title, &current_id) {
                                (Some(t), _) => t.clone(),
                                (None, Some(id)) => format!("{id} (introuvable)"),
                                (None, None) => "—".to_owned(),
                            };
                            let qkey = format!("{}#filtre", def.id);
                            egui::ComboBox::from_id_salt(("field_link", def.id.as_str()))
                                .selected_text(shown)
                                .width(140.0)
                                .show_ui(ui, |ui| {
                                    let q = self.field_bufs.entry(qkey).or_default();
                                    ui.add(
                                        egui::TextEdit::singleline(q)
                                            .hint_text("filtrer…")
                                            .desired_width(160.0),
                                    )
                                    .request_focus();
                                    if ui
                                        .selectable_label(current_id.is_none(), "Aucune")
                                        .clicked()
                                    {
                                        let mut m = self.snap.meta.clone();
                                        m.fields.remove(&def.id);
                                        new_meta = Some(m);
                                    }
                                    for nid in project.search(q).into_iter().take(30) {
                                        if nid == self.id {
                                            continue;
                                        }
                                        let Some(n) = project.note(&nid) else {
                                            continue;
                                        };
                                        let on = current_id.as_ref() == Some(&nid);
                                        if ui.selectable_label(on, &n.meta.title).clicked() {
                                            let mut m = self.snap.meta.clone();
                                            m.fields.insert(
                                                def.id.clone(),
                                                FieldValue::Text(nid.as_str().to_owned()),
                                            );
                                            new_meta = Some(m);
                                        }
                                    }
                                });
                        }
                        FieldKind::Number | FieldKind::Text | FieldKind::Date => {
                            let buf = self.field_bufs.entry(def.id.clone()).or_default();
                            let mut today = false;
                            let resp = ui
                                .horizontal(|ui| {
                                    let r = ui.add(
                                        egui::TextEdit::singleline(buf)
                                            .desired_width(if def.kind == FieldKind::Date {
                                                96.0
                                            } else {
                                                140.0
                                            })
                                            .hint_text(match def.kind {
                                                FieldKind::Number => "nombre",
                                                FieldKind::Date => "AAAA-MM-JJ",
                                                _ => "texte",
                                            }),
                                    );
                                    if def.kind == FieldKind::Date
                                        && ui
                                            .small_button("Auj.")
                                            .on_hover_text("Date du jour")
                                            .clicked()
                                    {
                                        today = true;
                                    }
                                    r
                                })
                                .inner;
                            if today {
                                let d =
                                    crate::project_file::file_date(std::time::SystemTime::now());
                                *buf = d.get(..10).unwrap_or(&d).to_owned();
                            }
                            if resp.lost_focus() || today {
                                match parse_field(def.kind, buf) {
                                    Ok(value) => {
                                        let mut m = self.snap.meta.clone();
                                        match value {
                                            Some(v) => m.fields.insert(def.id.clone(), v),
                                            None => m.fields.remove(&def.id),
                                        };
                                        new_meta = Some(m);
                                    }
                                    Err(()) => {
                                        *buf = current.map(FieldValue::display).unwrap_or_default();
                                    }
                                }
                            }
                        }
                    }
                    ui.end_row();
                }
            });

        // Champs présents dans la fiche mais inconnus du pack.
        let extras: Vec<(String, String)> = self
            .snap
            .meta
            .fields
            .iter()
            .filter(|(k, _)| !defs.iter().any(|d| &d.id == *k))
            .map(|(k, v)| (k.clone(), v.display()))
            .collect();
        if !extras.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("Hors pack").weak());
            for (k, v) in extras {
                ui.horizontal(|ui| {
                    if ui
                        .small_button("✕")
                        .on_hover_text("Retirer ce champ")
                        .clicked()
                    {
                        let mut m = self.snap.meta.clone();
                        m.fields.remove(&k);
                        new_meta = Some(m);
                    }
                    ui.label(format!("{k} : {v}"));
                });
            }
        }

        ui.add_space(16.0);
        ui.separator();
        if let Some(entry) = project.entry(&self.id) {
            let rel = entry
                .path
                .strip_prefix(project.root())
                .unwrap_or(&entry.path)
                .display()
                .to_string();
            ui.weak(format!("Fichier : {rel}"));
            ui.weak(format!("Identifiant : {}", self.id));
        }

        match new_meta {
            Some(m) => self.commit_meta(project, m, now_ms),
            None => false,
        }
    }
}

/// Convertit un index de caractère (egui) en position d'octet.
fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(b, _)| b)
}

/// `Ok(None)` : champ vidé (à retirer) ; `Err` : valeur invalide.
fn parse_field(kind: FieldKind, text: &str) -> Result<Option<FieldValue>, ()> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    match kind {
        FieldKind::Number => {
            let normalized = t.replace(',', ".");
            if let Ok(i) = normalized.parse::<i64>() {
                Ok(Some(FieldValue::Int(i)))
            } else {
                normalized
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite())
                    .map(|f| Some(FieldValue::Float(f)))
                    .ok_or(())
            }
        }
        FieldKind::Text | FieldKind::Link => Ok(Some(FieldValue::Text(t.to_owned()))),
        FieldKind::Date => {
            let parts: Vec<&str> = t.split('-').collect();
            let ok = matches!(parts.as_slice(), [y, m, d]
                if y.len() == 4
                    && y.parse::<u16>().is_ok()
                    && m.parse::<u8>().is_ok_and(|m| (1..=12).contains(&m))
                    && d.parse::<u8>().is_ok_and(|d| (1..=31).contains(&d)));
            if ok {
                Ok(Some(FieldValue::Text(t.to_owned())))
            } else {
                Err(())
            }
        }
        FieldKind::List => Ok(Some(FieldValue::List(
            t.split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect(),
        ))),
        FieldKind::Bool => Ok(Some(FieldValue::Bool(matches!(
            t.to_lowercase().as_str(),
            "oui" | "true" | "1"
        )))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numbers() {
        assert_eq!(
            parse_field(FieldKind::Number, " 3 "),
            Ok(Some(FieldValue::Int(3)))
        );
        assert_eq!(
            parse_field(FieldKind::Number, "2,5"),
            Ok(Some(FieldValue::Float(2.5)))
        );
        assert_eq!(parse_field(FieldKind::Number, "abc"), Err(()));
        assert_eq!(parse_field(FieldKind::Number, "inf"), Err(()));
        assert_eq!(parse_field(FieldKind::Number, ""), Ok(None));
        assert_eq!(
            parse_field(FieldKind::Date, "2026-11-18"),
            Ok(Some(FieldValue::Text("2026-11-18".into())))
        );
        assert_eq!(parse_field(FieldKind::Date, "18/11/2026"), Err(()));
        assert_eq!(parse_field(FieldKind::Date, "2026-13-01"), Err(()));
    }
}
