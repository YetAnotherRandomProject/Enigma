//! Éditeur d'une fiche : titre, pastilles de catégories, statut, corps
//! Markdown mis en forme, champs personnalisés. L'en-tête YAML n'est jamais
//! montré en texte brut.

use std::collections::BTreeMap;

use eframe::egui::{self, Color32, FontId, RichText, Stroke};
use enigma_core::history::{EditKind, History};
use enigma_core::markdown;
use enigma_core::pack::FieldKind;
use enigma_core::{FieldValue, NoteId, NoteMeta, Project};

use crate::colors::{c32, text_on};
use crate::completion::{self, ItemKind, Nav, Popup};
use crate::highlight::Highlighter;
use crate::notices::Notices;

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
}

/// Résultat de l'affichage du corps.
#[derive(Default)]
pub struct BodyOutcome {
    pub edited: bool,
    /// Fiche à ouvrir (Ctrl+clic sur un lien).
    pub open: Option<NoteId>,
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
                .corner_radius(10.0);
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
                        .corner_radius(10.0);
                    if ui
                        .add(button)
                        .on_hover_text("Catégorie absente du pack : cliquer pour la retirer")
                        .clicked()
                    {
                        toggle = Some(id.clone());
                    }
                }
            }
            if pack.categories.is_empty() && self.snap.meta.categories.is_empty() {
                ui.weak("Aucun pack : choisissez-en un en haut pour avoir des catégories.");
            }

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
        });

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

    /// Corps Markdown mis en forme pendant la frappe, avec suggestions de
    /// liens et Ctrl+clic pour suivre un lien.
    pub fn body_ui(
        &mut self,
        ui: &mut egui::Ui,
        project: &mut Project,
        now_ms: u64,
        notices: &mut Notices,
    ) -> BodyOutcome {
        let mut outcome = BodyOutcome::default();
        let te_id = egui::Id::unique(("body", self.id.as_str()));
        let focused = ui.memory(|m| m.has_focus(te_id));

        // 1. Touches de navigation, consommées avant le TextEdit.
        if focused && self.popup.is_some() {
            let nav = ui.input_mut(|i| {
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
        }

        // 2. Position du curseur après une insertion.
        if let Some(ci) = self.pending_cursor.take() {
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), te_id) {
                let cursor = egui::text::CCursor::new(ci);
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(cursor)));
                state.store(ui.ctx(), te_id);
            }
            ui.memory_mut(|m| m.request_focus(te_id));
        }

        // 3. Le texte.
        let popup_open = self.popup.is_some();
        let output = {
            let project_ref: &Project = project;
            let visuals = ui.visuals().clone();
            let highlighter = &mut self.highlighter;
            let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                let mut job = highlighter.job(buf.as_str(), project_ref, &visuals);
                job.wrap.max_width = wrap_width;
                ui.fonts_mut(|f| f.layout_job(job))
            };
            egui::ScrollArea::vertical()
                .id_salt("body_scroll")
                .auto_shrink(false)
                .show(ui, |ui| {
                    egui::TextEdit::multiline(&mut self.body)
                        .id(te_id)
                        .layouter(&mut layouter)
                        .desired_width(f32::INFINITY)
                        .min_size(ui.available_size())
                        .frame(egui::Frame::NONE)
                        // Tab indente ; Échap ferme les suggestions sans
                        // faire perdre le focus quand elles sont ouvertes.
                        .event_filter(egui::EventFilter {
                            tab: true,
                            horizontal_arrows: true,
                            vertical_arrows: true,
                            escape: popup_open,
                        })
                        .hint_text(
                            "Écrivez ici… Markdown accepté. Tapez [[ pour lier une fiche, \
                             Ctrl+clic sur un lien pour l'ouvrir.",
                        )
                        .show(ui)
                })
                .inner
        };

        if output.response.changed() {
            self.commit_body(project, EditKind::Typing, now_ms);
            outcome.edited = true;
        }

        // 4. Ctrl+clic sur un lien.
        let cursor_byte = output
            .cursor_range
            .filter(|r| r.primary.index == r.secondary.index)
            .map(|r| char_to_byte(&self.body, r.primary.index.0));
        if output.response.clicked()
            && ui.input(|i| i.modifiers.command)
            && let Some(link) = cursor_byte.and_then(|b| markdown::wiki_link_at(&self.body, b))
        {
            outcome.open = match project.resolve_title(&link.target) {
                Some(id) => Some(id.clone()),
                None => match project.create_note(&link.target, Vec::new(), String::new()) {
                    Ok(id) => {
                        notices.info(format!("Fiche « {} » créée.", link.target));
                        Some(id)
                    }
                    Err(e) => {
                        notices.error(format!("Création impossible : {e}"));
                        None
                    }
                },
            };
        }

        // 5. Suggestions.
        let has_focus = output.response.has_focus();
        let pointer_on_popup = self
            .popup_rect
            .zip(ui.input(|i| i.pointer.hover_pos()))
            .is_some_and(|(r, p)| r.contains(p));
        if has_focus {
            self.update_popup(project, cursor_byte, &output);
        } else if !pointer_on_popup {
            self.popup = None;
        }

        // 6. Fenêtre de suggestions (clic souris).
        self.popup_rect = None;
        if let Some(popup) = &self.popup {
            let (clicked, rect) = popup.show(ui.ctx());
            self.popup_rect = Some(rect);
            if let Some(i) = clicked {
                if let Some(p) = &mut self.popup {
                    p.selected = i;
                }
                outcome.edited |= self.navigate(Nav::Accept, project, now_ms, notices);
                ui.ctx().request_repaint();
            }
        }
        outcome
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
                    ItemKind::LinkType(id) => id.clone(),
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
    pub fn fields_ui(&mut self, ui: &mut egui::Ui, project: &mut Project, now_ms: u64) -> bool {
        let defs = project.pack().fields.clone();
        let mut new_meta: Option<NoteMeta> = None;

        ui.heading("Champs");
        if defs.is_empty() {
            ui.weak("Le pack ne définit aucun champ.");
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
                        FieldKind::Number | FieldKind::Text => {
                            let buf = self.field_bufs.entry(def.id.clone()).or_default();
                            let resp = ui.add(
                                egui::TextEdit::singleline(buf)
                                    .desired_width(140.0)
                                    .hint_text(if def.kind == FieldKind::Number {
                                        "nombre"
                                    } else {
                                        "texte"
                                    }),
                            );
                            if resp.lost_focus() {
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
        FieldKind::Text => Ok(Some(FieldValue::Text(t.to_owned()))),
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
    }
}
