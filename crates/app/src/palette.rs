//! Palette de commandes (Ctrl+P) : une seule zone de saisie pour lancer une
//! commande ou ouvrir une fiche, par recherche approximative.

use eframe::egui::{self, RichText};
use enigma_core::fuzzy;
use enigma_core::shortcut::Keymap;
use enigma_core::{NoteId, Project};

use crate::commands::Command;

/// Nombre maximal de résultats affichés.
const MAX_ITEMS: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteItem {
    Command(Command),
    Note(NoteId),
    /// Créer une fiche de ce titre (ouverture rapide sans résultat exact).
    Create(String),
}

pub enum PaletteOutcome {
    Open,
    Closed,
    Run(PaletteItem),
}

#[derive(Default)]
pub struct Palette {
    pub query: String,
    selected: usize,
    focused: bool,
    /// Ouverture rapide : fiches seulement (et création).
    notes_only: bool,
}

impl Palette {
    pub fn notes_only() -> Palette {
        Palette {
            notes_only: true,
            ..Palette::default()
        }
    }
}

/// Résultats pour une requête : commandes disponibles et fiches, triés par
/// score (à égalité, commandes d'abord). Requête vide : toutes les
/// commandes, puis les fiches par ordre alphabétique.
pub fn items(
    query: &str,
    project: &Project,
    available: &impl Fn(Command) -> bool,
) -> Vec<(PaletteItem, String)> {
    let mut scored: Vec<(i64, u8, PaletteItem, String)> = Vec::new();
    for &c in Command::ALL {
        if matches!(c, Command::Palette | Command::QuickOpen) || !available(c) {
            continue;
        }
        if let Some(s) = fuzzy::score(query, c.label()) {
            scored.push((s, 0, PaletteItem::Command(c), c.label().to_owned()));
        }
    }
    for id in project.ids() {
        let Some(note) = project.note(id) else {
            continue;
        };
        if let Some(s) = fuzzy::score(query, &note.meta.title) {
            scored.push((s, 1, PaletteItem::Note(id.clone()), note.meta.title.clone()));
        }
    }
    if query.trim().is_empty() {
        scored.sort_by(|a, b| {
            a.1.cmp(&b.1).then_with(|| match a.1 {
                0 => std::cmp::Ordering::Equal,
                _ => a.3.to_lowercase().cmp(&b.3.to_lowercase()),
            })
        });
    } else {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.3.cmp(&b.3)));
    }
    scored
        .into_iter()
        .take(MAX_ITEMS)
        .map(|(_, _, item, label)| (item, label))
        .collect()
}

impl Palette {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        project: &Project,
        keymap: &Keymap,
        available: impl Fn(Command) -> bool,
    ) -> PaletteOutcome {
        let mut results = items(&self.query, project, &available);
        if self.notes_only {
            results.retain(|(i, _)| matches!(i, PaletteItem::Note(_)));
            let q = self.query.trim();
            if !q.is_empty() && project.resolve_title(q).is_none() {
                results.push((PaletteItem::Create(q.to_owned()), format!("Créer « {q} »")));
            }
        }
        // Navigation consommée avant la zone de saisie.
        let (up, down, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if escape {
            return PaletteOutcome::Closed;
        }
        let n = results.len();
        if n > 0 {
            if down {
                self.selected = (self.selected + 1) % n;
            }
            if up {
                self.selected = (self.selected + n - 1) % n;
            }
        }
        self.selected = self.selected.min(n.saturating_sub(1));
        if enter && let Some((item, _)) = results.get(self.selected) {
            return PaletteOutcome::Run(item.clone());
        }

        let mut run = None;
        let resp = egui::Modal::new(egui::Id::unique("command_palette")).show(ctx, |ui| {
            ui.set_width(460.0);
            let before = self.query.clone();
            let edit = ui.add_sized(
                [460.0, 24.0],
                egui::TextEdit::singleline(&mut self.query).hint_text(if self.notes_only {
                    "Titre de la fiche à ouvrir…"
                } else {
                    "Commande ou titre de fiche…"
                }),
            );
            if !self.focused {
                edit.request_focus();
                self.focused = true;
            }
            if self.query != before {
                self.selected = 0;
            }
            ui.add_space(4.0);
            if results.is_empty() {
                ui.weak("Aucun résultat");
            }
            for (k, (item, label)) in results.iter().enumerate() {
                let (tag, extra) = match item {
                    PaletteItem::Command(c) => ("", c.bindings_text(keymap)),
                    PaletteItem::Note(_) => ("fiche", String::new()),
                    PaletteItem::Create(_) => ("nouvelle", String::new()),
                };
                let row = ui.horizontal(|ui| {
                    let r = ui.selectable_label(k == self.selected, label);
                    if !tag.is_empty() {
                        ui.label(RichText::new(tag).small().weak());
                    }
                    if !extra.is_empty() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(extra).small().weak());
                        });
                    }
                    r
                });
                if row.inner.clicked() {
                    run = Some(item.clone());
                }
                if k == self.selected && (up || down) {
                    row.inner.scroll_to_me(None);
                }
            }
            ui.add_space(2.0);
            ui.label(
                RichText::new("Flèches : choisir · Entrée : lancer · Échap : fermer")
                    .small()
                    .weak(),
            );
        });
        if let Some(item) = run {
            return PaletteOutcome::Run(item);
        }
        if resp.should_close() {
            return PaletteOutcome::Closed;
        }
        PaletteOutcome::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn project() -> (tempfile::TempDir, Project) {
        let d = tempfile::tempdir().unwrap();
        let mut p = Project::open(d.path(), &[Path::new("x").to_path_buf()]).unwrap();
        for t in ["Graphe des salles", "Majordome", "Coffre"] {
            p.create_note(t, vec![], String::new()).unwrap();
        }
        (d, p)
    }

    #[test]
    fn empty_query_lists_commands_then_notes() {
        let (_d, p) = project();
        let all = items("", &p, &|_| true);
        assert_eq!(all.len(), MAX_ITEMS);
        assert!(matches!(all[0].0, PaletteItem::Command(_)));
        assert!(
            !all.iter()
                .any(|(i, _)| *i == PaletteItem::Command(Command::Palette))
        );
    }

    #[test]
    fn query_mixes_commands_and_notes() {
        let (_d, p) = project();
        let found = items("graphe", &p, &|_| true);
        let labels: Vec<&str> = found.iter().map(|(_, l)| l.as_str()).collect();
        assert!(labels.contains(&"Graphe en 3D"), "{labels:?}");
        assert!(labels.contains(&"Graphe des salles"), "{labels:?}");
        let found = items("major", &p, &|_| true);
        assert!(matches!(&found[0].0, PaletteItem::Note(_)));
        assert_eq!(found[0].1, "Majordome");
        // Commandes indisponibles masquées.
        let found = items("3d", &p, &|c| c != Command::Graph3D);
        assert!(!found.iter().any(|(_, l)| l == "Graphe en 3D"));
    }
}
