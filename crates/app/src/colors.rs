//! Conversions de couleurs entre le modèle et egui.

use eframe::egui::Color32;

pub fn c32(c: enigma_core::Color) -> Color32 {
    let [r, g, b, a] = c.0;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Couleur de texte lisible sur un fond donné.
pub fn text_on(bg: Color32) -> Color32 {
    let lum = 0.299 * f32::from(bg.r()) + 0.587 * f32::from(bg.g()) + 0.114 * f32::from(bg.b());
    if lum > 150.0 {
        Color32::from_rgb(20, 20, 24)
    } else {
        Color32::WHITE
    }
}

/// Couleur des liens `[[...]]` valides.
pub const WIKI_LINK: Color32 = Color32::from_rgb(0x8b, 0x9c, 0xff);
