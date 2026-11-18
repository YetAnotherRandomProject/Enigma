//! Suggestions d'auto-complétion des liens : titres de fiches après `[[`,
//! types de liens du pack après `|`.

use eframe::egui::{self, Color32, RichText};
use enigma_core::markdown::Completion;
use enigma_core::{NoteId, Project};

use crate::colors::c32;

/// Nombre maximal de fiches proposées.
const MAX_TITLES: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    /// Fiche existante.
    Note(NoteId),
    /// Fiche à créer avec ce titre.
    Create(String),
    /// Type de lien (identifiant du pack).
    LinkType(String),
    /// Section de la fiche visée (texte du titre).
    Section(String),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub kind: ItemKind,
    pub label: String,
    pub detail: Option<String>,
    pub color: Option<Color32>,
}

/// Suggestions pour un contexte de saisie donné.
pub fn items_for(c: &Completion, project: &Project) -> Vec<Item> {
    match c {
        Completion::Title { query, .. } => {
            let mut items: Vec<Item> = project
                .search(query)
                .into_iter()
                .take(MAX_TITLES)
                .filter_map(|id| {
                    let note = project.note(&id)?;
                    let color = note
                        .meta
                        .categories
                        .first()
                        .and_then(|c| project.pack().category(c))
                        .map(|c| c32(c.color));
                    Some(Item {
                        label: note.meta.title.clone(),
                        detail: None,
                        color,
                        kind: ItemKind::Note(id),
                    })
                })
                .collect();
            let q = query.trim();
            if !q.is_empty()
                && project.resolve_title(q).is_none()
                && Project::validate_title(q).is_ok()
            {
                items.push(Item {
                    kind: ItemKind::Create(q.to_owned()),
                    label: format!("+ Créer « {q} »"),
                    detail: None,
                    color: None,
                });
            }
            items
        }
        Completion::Section { target, query, .. } => {
            let q = enigma_core::search::fold(query.trim());
            let Some(note) = project
                .resolve_title(target)
                .and_then(|id| project.note(id))
            else {
                return Vec::new();
            };
            enigma_core::sections::headings(&note.body)
                .into_iter()
                .filter(|h| {
                    let f = enigma_core::search::fold(&h.text);
                    q.is_empty() || f.windows(q.len()).any(|w| w == q.as_slice())
                })
                .map(|h| Item {
                    label: format!("{}{}", "  ".repeat(usize::from(h.level) - 1), h.text),
                    detail: None,
                    color: None,
                    kind: ItemKind::Section(h.text),
                })
                .collect()
        }
        Completion::LinkType { query, .. } => {
            let q = query.trim().to_lowercase();
            project
                .pack()
                .link_types
                .iter()
                .filter(|t| {
                    q.is_empty()
                        || t.id.to_lowercase().contains(&q)
                        || t.label.to_lowercase().contains(&q)
                })
                .map(|t| Item {
                    kind: ItemKind::LinkType(t.id.clone()),
                    label: t.label.clone(),
                    detail: (t.label != t.id).then(|| t.id.clone()),
                    color: Some(c32(t.color)),
                })
                .collect()
        }
    }
}

/// État de la fenêtre de suggestions.
pub struct Popup {
    pub context: Completion,
    /// Position du curseur (octets) lors du calcul.
    pub cursor_byte: usize,
    pub items: Vec<Item>,
    pub selected: usize,
    /// Position à l'écran (sous le curseur).
    pub anchor: egui::Pos2,
    /// Génération du projet lors du calcul des suggestions (cache).
    pub generation: u64,
}

/// Navigation clavier dans la fenêtre de suggestions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    /// Entrée : insérer.
    Accept,
    /// Tab : insérer le titre et enchaîner sur le type.
    AcceptThenType,
    Dismiss,
}

impl Popup {
    /// Dessine la fenêtre ; renvoie l'élément cliqué et le rectangle occupé.
    pub fn show(&self, ctx: &egui::Context) -> (Option<usize>, egui::Rect) {
        let mut clicked = None;
        let area = egui::Area::new(egui::Id::unique("link_completion"))
            .order(egui::Order::Foreground)
            .fixed_pos(self.anchor)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(240.0);
                    let title = match &self.context {
                        Completion::Title { .. } => "Lier à une fiche",
                        Completion::LinkType { .. } => "Type de lien",
                        Completion::Section { .. } => "Section de la fiche",
                    };
                    ui.label(RichText::new(title).weak().small());
                    for (i, item) in self.items.iter().enumerate() {
                        let mut text = RichText::new(&item.label);
                        if let Some(c) = item.color {
                            text = text.color(c);
                        }
                        let resp = ui.selectable_label(i == self.selected, text);
                        if let Some(d) = &item.detail {
                            resp.clone().on_hover_text(d);
                        }
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                    }
                    ui.separator();
                    let hint = match &self.context {
                        Completion::Title { .. } => "Entrée : insérer · Tab : puis un type · Échap",
                        Completion::LinkType { .. } | Completion::Section { .. } => {
                            "Entrée : insérer · Échap : fermer"
                        }
                    };
                    ui.label(RichText::new(hint).weak().small());
                });
            });
        (clicked, area.response.rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, Project) {
        let d = tempfile::tempdir().unwrap();
        let packs = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")];
        let mut p = Project::open(d.path(), &packs).unwrap();
        p.set_pack(Some("blue_prince".into())).unwrap();
        for t in ["Coffre", "Grand coffre", "Salle"] {
            p.create_note(t, vec![], String::new()).unwrap();
        }
        (d, p)
    }

    #[test]
    fn title_items_prefix_first_with_create() {
        let (_d, p) = project();
        let c = Completion::Title {
            start: 0,
            query: "cof".into(),
        };
        let labels: Vec<_> = items_for(&c, &p).into_iter().map(|i| i.label).collect();
        assert_eq!(labels, ["Coffre", "Grand coffre", "+ Créer « cof »"]);
        // Titre exact (casse ignorée) : pas de proposition de création.
        let c = Completion::Title {
            start: 0,
            query: "coffre".into(),
        };
        assert!(
            !items_for(&c, &p)
                .iter()
                .any(|i| matches!(i.kind, ItemKind::Create(_)))
        );
    }

    #[test]
    fn type_items_filtered_by_id_or_label() {
        let (_d, p) = project();
        let c = Completion::LinkType {
            start: 0,
            target: "Coffre".into(),
            query: "trouve".into(),
        };
        let items = items_for(&c, &p);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, ItemKind::LinkType("situe_dans".into()));
        let all = Completion::LinkType {
            start: 0,
            target: String::new(),
            query: String::new(),
        };
        assert_eq!(items_for(&all, &p).len(), 4);
    }
}
