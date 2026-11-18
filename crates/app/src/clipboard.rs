//! Copier / couper / coller gérés par l'application.
//!
//! egui-winit ne transmet que le texte ou une image collés, et rien du
//! tout quand on colle des fichiers copiés dans l'explorateur. Les
//! raccourcis presse-papiers automatiques sont donc désactivés
//! (`NativeOptions::clipboard_shortcuts = false`) et les touches
//! Ctrl+C / Ctrl+X / Ctrl+V sont traduites ici, avant qu'egui ne voie
//! l'image : texte et images deviennent les événements habituels d'egui,
//! les fichiers sont rendus à l'appelant pour être joints à la fiche.

use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, ColorImage, Event, Key, RawInput};

/// Contenu du presse-papiers au moment de coller.
#[derive(Debug)]
pub enum Clip {
    Text(String),
    Image(Arc<ColorImage>),
    Files(Vec<PathBuf>),
    Empty,
}

/// Lit le presse-papiers du système : fichiers d'abord (copie depuis
/// l'explorateur), puis texte, puis image.
pub fn read_system() -> Clip {
    let Ok(mut cb) = arboard::Clipboard::new() else {
        return Clip::Empty;
    };
    if let Ok(files) = cb.get().file_list()
        && !files.is_empty()
    {
        return Clip::Files(files);
    }
    if let Ok(text) = cb.get_text()
        && !text.is_empty()
    {
        return Clip::Text(text.replace("\r\n", "\n"));
    }
    if let Ok(img) = cb.get_image() {
        let image = ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.bytes);
        return Clip::Image(Arc::new(image));
    }
    Clip::Empty
}

/// Remplace les touches Ctrl+C / X / V par les événements correspondants ;
/// rend les fichiers collés. `read` lit le presse-papiers (injecté pour les
/// tests).
pub fn translate(raw: &mut RawInput, read: impl Fn() -> Clip) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut out = Vec::with_capacity(raw.events.len());
    for event in raw.events.drain(..) {
        match &event {
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if modifiers.command && !modifiers.alt => match key {
                Key::C if !modifiers.shift => out.push(Event::Copy),
                Key::X if !modifiers.shift => out.push(Event::Cut),
                Key::V if !modifiers.shift => match read() {
                    Clip::Text(t) => out.push(Event::Paste(t)),
                    Clip::Image(img) => out.push(Event::PasteImage(img)),
                    Clip::Files(f) => files.extend(f),
                    Clip::Empty => {}
                },
                _ => out.push(event),
            },
            // Relâchement de C/X/V avec Ctrl : sans objet une fois traduits.
            Event::Key {
                key: Key::C | Key::X | Key::V,
                pressed: false,
                modifiers,
                ..
            } if modifiers.command && !modifiers.shift => {}
            _ => out.push(event),
        }
    }
    raw.events = out;
    files
}

/// Image collée encodée en PNG.
pub fn image_png(image: &ColorImage) -> Result<Vec<u8>, String> {
    let [w, h] = image.size;
    crate::graph_export::encode_png_region(
        image,
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Modifiers;

    fn key(k: Key, pressed: bool) -> Event {
        Event::Key {
            key: k,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }
    }

    #[test]
    fn shortcuts_become_clipboard_events() {
        let mut raw = RawInput {
            events: vec![
                key(Key::C, true),
                key(Key::C, false),
                key(Key::V, true),
                Event::Text("a".into()),
                key(Key::Z, true),
            ],
            ..Default::default()
        };
        let files = translate(&mut raw, || Clip::Text("collé".into()));
        assert!(files.is_empty());
        assert_eq!(raw.events.len(), 4);
        assert!(matches!(raw.events[0], Event::Copy));
        assert!(matches!(&raw.events[1], Event::Paste(t) if t == "collé"));
        assert!(matches!(raw.events[3], Event::Key { key: Key::Z, .. }));
    }

    #[test]
    fn files_are_returned_and_images_forwarded() {
        let mut raw = RawInput {
            events: vec![key(Key::V, true)],
            ..Default::default()
        };
        let files = translate(&mut raw, || Clip::Files(vec![PathBuf::from("a.mp3")]));
        assert_eq!(files, [PathBuf::from("a.mp3")]);
        assert!(raw.events.is_empty());

        let mut raw = RawInput {
            events: vec![key(Key::V, true)],
            ..Default::default()
        };
        let img = Arc::new(ColorImage::filled([2, 2], egui::Color32::RED));
        translate(&mut raw, || Clip::Image(img.clone()));
        assert!(matches!(raw.events[0], Event::PasteImage(_)));
        let png = image_png(&img).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }
}
