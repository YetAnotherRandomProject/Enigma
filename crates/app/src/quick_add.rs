//! Petit formulaire « + » pour enrichir le pack de jeu là où on en a
//! besoin : nouvelle catégorie depuis les pastilles d'une fiche, nouveau
//! statut, nouveau type de lien depuis le graphe, nouveau champ.

use eframe::egui::{self, RichText, Stroke};
use enigma_core::Color;
use enigma_core::pack::FieldKind;

use crate::colors::c32;

/// Couleurs proposées (lisibles en thème clair et sombre).
pub const SWATCHES: [&str; 10] = [
    "#4aa3ff", "#ff7a59", "#ffd23f", "#6ee7a8", "#c084fc", "#f472b6", "#38bdf8", "#a3e635",
    "#f97316", "#9ca3af",
];

/// Élément du pack à créer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Category,
    Status,
    LinkType,
    Field,
}

impl Kind {
    fn title(self) -> &'static str {
        match self {
            Kind::Category => "Nouvelle catégorie",
            Kind::Status => "Nouveau statut",
            Kind::LinkType => "Nouveau type de lien",
            Kind::Field => "Nouveau champ",
        }
    }
}

/// Saisie validée.
#[derive(Clone, Debug, PartialEq)]
pub struct NewItem {
    pub label: String,
    pub color: Color,
    pub directed: bool,
    pub field_kind: FieldKind,
}

pub struct QuickAdd {
    label: String,
    color: usize,
    directed: bool,
    field_kind: FieldKind,
}

impl Default for QuickAdd {
    fn default() -> Self {
        QuickAdd {
            label: String::new(),
            color: 0,
            directed: true,
            field_kind: FieldKind::Text,
        }
    }
}

fn swatch(i: usize) -> Color {
    SWATCHES
        .get(i)
        .and_then(|s| Color::parse(s))
        .unwrap_or(Color([128, 128, 128, 255]))
}

impl QuickAdd {
    /// Bouton qui ouvre le formulaire ; rend l'élément à créer une fois
    /// validé (Entrée ou « Créer »).
    pub fn menu(
        &mut self,
        ui: &mut egui::Ui,
        button: &str,
        hint: &str,
        kind: Kind,
    ) -> Option<NewItem> {
        let mut out = None;
        let resp = ui.menu_button(button, |ui| {
            ui.set_min_width(240.0);
            ui.label(RichText::new(kind.title()).strong());
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.label)
                    .hint_text("Libellé")
                    .desired_width(230.0),
            );
            if self.label.is_empty() && !edit.has_focus() {
                edit.request_focus();
            }
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            match kind {
                Kind::Field => {
                    ui.horizontal_wrapped(|ui| {
                        for k in FieldKind::ALL {
                            ui.selectable_value(&mut self.field_kind, k, k.label());
                        }
                    });
                }
                _ => {
                    ui.horizontal_wrapped(|ui| {
                        for i in 0..SWATCHES.len() {
                            let c = c32(swatch(i));
                            let (rect, r) = ui
                                .allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
                            ui.painter().circle_filled(rect.center(), 8.0, c);
                            if self.color == i {
                                ui.painter().circle_stroke(
                                    rect.center(),
                                    9.0,
                                    Stroke::new(2.0, ui.visuals().strong_text_color()),
                                );
                            }
                            if r.clicked() {
                                self.color = i;
                            }
                        }
                    });
                }
            }
            if kind == Kind::LinkType {
                ui.checkbox(&mut self.directed, "Orienté (flèche)");
            }
            ui.horizontal(|ui| {
                let ok = !self.label.trim().is_empty();
                let create = ui.add_enabled(ok, egui::Button::new("Créer")).clicked();
                if ok && (create || enter) {
                    out = Some(NewItem {
                        label: self.label.trim().to_owned(),
                        color: swatch(self.color),
                        directed: self.directed,
                        field_kind: self.field_kind,
                    });
                    self.label.clear();
                    ui.close();
                }
                if ui.button("Annuler").clicked() {
                    self.label.clear();
                    ui.close();
                }
            });
        });
        resp.response.on_hover_text(hint);
        out
    }
}
