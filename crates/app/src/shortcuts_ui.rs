//! Éditeur des raccourcis clavier : liste des commandes, saisie d'un
//! nouveau raccourci, conflits, retour aux valeurs par défaut.

use eframe::egui::{self, RichText};
use enigma_core::shortcut::{Keymap, conflicts};

use crate::commands::{Command, from_egui};

#[derive(Default)]
pub struct ShortcutEditor {
    /// Commande dont on attend le nouveau raccourci.
    capturing: Option<Command>,
}

/// Résultat d'une image de l'éditeur.
pub struct ShortcutOutcome {
    pub open: bool,
    /// Le jeu de raccourcis a changé (à enregistrer).
    pub changed: bool,
}

impl ShortcutEditor {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        keymap: &mut Keymap,
        file: Option<&str>,
    ) -> ShortcutOutcome {
        let mut changed = false;
        // Saisie : la prochaine touche (avec ses modificateurs) devient le
        // raccourci ; Échap annule.
        if let Some(cmd) = self.capturing {
            let pressed = ctx.input_mut(|i| {
                let mut found = None;
                i.events.retain(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } if found.is_none() => {
                        found = Some((*key, *modifiers));
                        false
                    }
                    egui::Event::Key { .. } | egui::Event::Text(_) => false,
                    _ => true,
                });
                found
            });
            match pressed {
                Some((egui::Key::Escape, m)) if m.is_none() => self.capturing = None,
                Some((key, m)) => {
                    keymap.set(cmd.id(), vec![from_egui(m, key)]);
                    self.capturing = None;
                    changed = true;
                }
                None => {}
            }
        }

        let effective: Vec<(&str, Vec<_>)> = Command::ALL
            .iter()
            .map(|c| (c.id(), c.bindings(keymap)))
            .collect();
        let clashes = conflicts(effective.iter().map(|(id, s)| (*id, s.clone())));
        let in_conflict = |id: &str| clashes.iter().any(|(_, ids)| ids.contains(&id));

        let mut close = false;
        let capturing = self.capturing;
        let resp = egui::Modal::new(egui::Id::unique("shortcut_editor")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading("Raccourcis clavier");
            if let Some(f) = file {
                ui.label(
                    RichText::new(format!("Enregistrés dans {f}"))
                        .small()
                        .weak(),
                );
            }
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    egui::Grid::new("shortcuts_grid")
                        .num_columns(3)
                        .striped(true)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            for &cmd in Command::ALL {
                                ui.label(cmd.label());
                                let text = if capturing == Some(cmd) {
                                    RichText::new("Appuyez sur une touche… (Échap : annuler)")
                                        .italics()
                                } else {
                                    let t = cmd.bindings_text(keymap);
                                    let t = if t.is_empty() { "—".to_owned() } else { t };
                                    if in_conflict(cmd.id()) {
                                        RichText::new(t).color(ui.visuals().error_fg_color)
                                    } else {
                                        RichText::new(t)
                                    }
                                };
                                ui.label(text);
                                ui.horizontal(|ui| {
                                    if ui.small_button("Modifier").clicked() {
                                        self.capturing = Some(cmd);
                                    }
                                    if !cmd.bindings(keymap).is_empty()
                                        && ui.small_button("Désactiver").clicked()
                                    {
                                        keymap.set(cmd.id(), Vec::new());
                                        changed = true;
                                    }
                                    if keymap.is_overridden(cmd.id())
                                        && ui.small_button("Par défaut").clicked()
                                    {
                                        keymap.reset(cmd.id());
                                        changed = true;
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });
            if !clashes.is_empty() {
                ui.add_space(4.0);
                for (s, ids) in &clashes {
                    let labels: Vec<&str> = ids
                        .iter()
                        .filter_map(|id| Command::ALL.iter().find(|c| c.id() == *id))
                        .map(|c| c.label())
                        .collect();
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("{s} est utilisé par : {}", labels.join(", ")),
                    );
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Tout réinitialiser").clicked() {
                    keymap.reset_all();
                    changed = true;
                }
                if ui.button("Fermer").clicked() {
                    close = true;
                }
            });
        });
        if resp.should_close() && self.capturing.is_none() {
            close = true;
        }
        ShortcutOutcome {
            open: !close,
            changed,
        }
    }
}
