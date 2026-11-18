//! Pastilles de filtre réutilisables (liste des fiches et graphe).

use eframe::egui::{self, Color32, RichText};
use enigma_core::Pack;
use enigma_core::filter::{CategoryMode, NoteFilter};

use crate::colors::{c32, text_on};

/// État d'une pastille de filtre.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    Off,
    On,
    Excluded,
}

impl From<bool> for Chip {
    fn from(on: bool) -> Self {
        if on { Chip::On } else { Chip::Off }
    }
}

pub fn small_heading(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_space(2.0);
    ui.label(RichText::new(text).small().weak())
}

/// Pastille : contour (neutre), remplie (active), barrée (exclue).
pub fn chip(ui: &mut egui::Ui, label: &str, color: Color32, state: Chip) -> egui::Response {
    let text = match state {
        Chip::Off => RichText::new(label).color(color),
        Chip::On => RichText::new(label).color(text_on(color)),
        Chip::Excluded => RichText::new(format!("− {label}"))
            .color(ui.visuals().error_fg_color)
            .strikethrough(),
    };
    let fill = if state == Chip::On {
        color
    } else {
        Color32::TRANSPARENT
    };
    let stroke_color = if state == Chip::Excluded {
        ui.visuals().error_fg_color
    } else {
        color
    };
    ui.add(
        egui::Button::new(text.small())
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, stroke_color))
            .corner_radius(0.0),
    )
}

/// Catégories : clic = inclure, 2e clic = exclure, 3e = neutre ; choix
/// ET / OU dès que deux catégories sont incluses.
pub fn categories_ui(ui: &mut egui::Ui, f: &mut NoteFilter, pack: &Pack) {
    if pack.categories.is_empty() {
        return;
    }
    small_heading(ui, "Catégories")
        .on_hover_text("Clic : inclure · 2e clic : exclure · 3e clic : neutre");
    ui.horizontal_wrapped(|ui| {
        for cat in &pack.categories {
            let state = if f.include_categories.contains(&cat.id) {
                Chip::On
            } else if f.exclude_categories.contains(&cat.id) {
                Chip::Excluded
            } else {
                Chip::Off
            };
            if chip(ui, &cat.label, c32(cat.color), state).clicked() {
                f.cycle_category(&cat.id);
            }
        }
    });
    if f.include_categories.len() >= 2 {
        ui.horizontal(|ui| {
            ui.radio_value(&mut f.category_mode, CategoryMode::All, "Toutes (ET)");
            ui.radio_value(&mut f.category_mode, CategoryMode::Any, "Au moins une (OU)");
        });
    }
}

/// Statuts (OU), avec « Sans statut ».
pub fn statuses_ui(ui: &mut egui::Ui, f: &mut NoteFilter, pack: &Pack) {
    small_heading(ui, "Statut");
    ui.horizontal_wrapped(|ui| {
        let grey = ui.visuals().weak_text_color();
        let on = f.statuses.contains(&None);
        if chip(ui, "Sans statut", grey, Chip::from(on)).clicked() {
            NoteFilter::toggle(&mut f.statuses, None);
        }
        for st in &pack.statuses {
            let value = Some(st.id.clone());
            let on = f.statuses.contains(&value);
            if chip(ui, &st.label, c32(st.color), Chip::from(on)).clicked() {
                NoteFilter::toggle(&mut f.statuses, value);
            }
        }
    });
}

/// Pastilles des types de liens (dont « Sans type ») ; bascule la valeur
/// cliquée dans `selected`.
pub fn link_types_ui(
    ui: &mut egui::Ui,
    title: &str,
    selected: &mut Vec<Option<String>>,
    pack: &Pack,
    invert: bool,
) {
    small_heading(ui, title);
    ui.horizontal_wrapped(|ui| {
        let grey = ui.visuals().weak_text_color();
        let mut values: Vec<(Option<String>, String, Color32)> =
            vec![(None, "Sans type".to_owned(), grey)];
        values.extend(
            pack.link_types
                .iter()
                .map(|lt| (Some(lt.id.clone()), lt.label.clone(), c32(lt.color))),
        );
        for (value, label, color) in values {
            let on = selected.contains(&value) != invert;
            if chip(ui, &label, color, Chip::from(on)).clicked() {
                NoteFilter::toggle(selected, value);
            }
        }
    });
}
