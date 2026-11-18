//! Commandes « / » : en tapant `/` en début de ligne ou après une espace,
//! une liste d'insertions (titres, listes, tableau, encadré…) filtrée par
//! le texte tapé ensuite.

use eframe::egui::{self, RichText};
use enigma_core::md_edit::Line;
use enigma_core::search::fold;

use crate::editor::Format;

/// Ce qu'insère une commande.
#[derive(Clone, Debug, PartialEq)]
pub enum SlashAction {
    Format(Format),
    /// Texte inséré tel quel ; le curseur se place à `cursor` octets après
    /// son début.
    Snippet {
        text: &'static str,
        cursor: usize,
    },
    InsertFile,
    /// Choisir un modèle du pack.
    InsertTemplate,
}

pub struct SlashItem {
    pub label: &'static str,
    /// Mots-clés supplémentaires pour le filtre.
    pub keys: &'static str,
    pub action: SlashAction,
}

fn all() -> Vec<SlashItem> {
    use SlashAction::{Format as F, InsertFile, InsertTemplate, Snippet};
    let s = |label, keys, action| SlashItem {
        label,
        keys,
        action,
    };
    vec![
        s("Titre 1", "h1 heading", F(Format::Line(Line::Heading(1)))),
        s("Titre 2", "h2 heading", F(Format::Line(Line::Heading(2)))),
        s("Titre 3", "h3 heading", F(Format::Line(Line::Heading(3)))),
        s("Liste à puces", "bullet ul", F(Format::Line(Line::Bullet))),
        s(
            "Liste numérotée",
            "ol numero",
            F(Format::Line(Line::Numbered)),
        ),
        s(
            "Liste de tâches",
            "todo case task",
            F(Format::Line(Line::Task)),
        ),
        s("Citation", "quote", F(Format::Line(Line::Quote))),
        s("Tableau", "table grille", F(Format::Table(2, 3))),
        s("Bloc de code", "code", F(Format::CodeBlock)),
        s(
            "Encadré indice",
            "callout hint astuce",
            Snippet {
                text: "> [!indice] ",
                cursor: 12,
            },
        ),
        s(
            "Encadré question",
            "callout enigme",
            Snippet {
                text: "> [!question] ",
                cursor: 14,
            },
        ),
        s(
            "Encadré solution (replié)",
            "callout spoiler reponse",
            Snippet {
                text: "> [!solution]- Solution\n> ",
                cursor: 26,
            },
        ),
        s(
            "Encadré attention",
            "callout warning piege",
            Snippet {
                text: "> [!attention] ",
                cursor: 15,
            },
        ),
        s(
            "Lien vers une fiche",
            "link wiki",
            Snippet {
                text: "[[",
                cursor: 2,
            },
        ),
        s(
            "Insérer une fiche",
            "embed transclusion",
            Snippet {
                text: "![[",
                cursor: 3,
            },
        ),
        s(
            "Commentaire",
            "comment cache",
            Snippet {
                text: "%%  %%",
                cursor: 3,
            },
        ),
        s(
            "Surlignage",
            "highlight",
            Snippet {
                text: "====",
                cursor: 2,
            },
        ),
        s(
            "Note de bas de page",
            "footnote",
            Snippet {
                text: "[^1]",
                cursor: 4,
            },
        ),
        s("Séparateur", "hr rule ligne", F(Format::Rule)),
        s("Date du jour", "date aujourd'hui", F(Format::Date)),
        s("Image ou fichier…", "image media son", InsertFile),
        s("Modèle…", "template modele", InsertTemplate),
    ]
}

/// Commandes correspondant au texte tapé après `/`.
pub fn items(query: &str) -> Vec<SlashItem> {
    let q = fold(query.trim());
    all()
        .into_iter()
        .filter(|i| {
            if q.is_empty() {
                return true;
            }
            let hay = fold(&format!("{} {}", i.label, i.keys));
            hay.windows(q.len()).any(|w| w == q.as_slice())
        })
        .collect()
}

/// `/requête` juste avant le curseur : (octet du `/`, requête).
pub fn context(text: &str, cursor: usize) -> Option<(usize, String)> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let line_start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[line_start..cursor];
    let slash = before.rfind('/')?;
    let query = &before[slash + 1..];
    if query.chars().any(char::is_whitespace) || query.chars().count() > 20 {
        return None;
    }
    let prev_ok = before[..slash]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    prev_ok.then(|| (line_start + slash, query.to_owned()))
}

/// Fenêtre des commandes « / ».
pub struct SlashPopup {
    /// Octet du `/`.
    pub start: usize,
    pub query: String,
    pub selected: usize,
    pub anchor: egui::Pos2,
}

impl SlashPopup {
    /// Dessine la fenêtre ; rend l'élément cliqué et le rectangle occupé.
    pub fn show(&self, ctx: &egui::Context) -> (Option<usize>, egui::Rect) {
        let mut clicked = None;
        let list = items(&self.query);
        let area = egui::Area::new(egui::Id::unique("slash_commands"))
            .order(egui::Order::Foreground)
            .fixed_pos(self.anchor)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(220.0);
                    ui.label(RichText::new("Insérer").weak().small());
                    for (i, item) in list.iter().enumerate() {
                        if ui
                            .selectable_label(i == self.selected, item.label)
                            .clicked()
                        {
                            clicked = Some(i);
                        }
                    }
                    ui.separator();
                    ui.label(
                        RichText::new("Entrée : insérer · Échap : fermer")
                            .weak()
                            .small(),
                    );
                });
            });
        (clicked, area.response.rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_context_and_filter() {
        assert_eq!(context("/ta", 3), Some((0, "ta".into())));
        assert_eq!(context("Texte /tab", 10), Some((6, "tab".into())));
        assert_eq!(context("a/b", 3), None, "pas après une lettre");
        assert_eq!(context("/a b", 4), None);
        let labels: Vec<&str> = items("tabl").iter().map(|i| i.label).collect();
        assert_eq!(labels, ["Tableau"]);
        assert!(items("encadre").len() >= 4);
        assert_eq!(items("").len(), all().len());
    }
}
