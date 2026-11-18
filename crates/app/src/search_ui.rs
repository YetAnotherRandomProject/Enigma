//! Panneau « Recherche » : recherche dans tout le projet
//! (`enigma_core::search`), résultats par fiche avec les lignes trouvées.
//! Un clic sur une ligne ouvre la fiche à cet endroit.

use eframe::egui::{self, RichText, text::LayoutJob, text::TextFormat};
use enigma_core::search::{Hit, Query};
use enigma_core::{NoteId, Project};

const HELP: &str = "Mots : tous présents (ET), sans tenir compte des accents\n\
\"expression exacte\"\n\
a OR b : l'un ou l'autre\n\
-mot : exclure\n\
/regex/ : expression régulière\n\
titre:… cat:… statut:… tag:… lien:…\n\
<champ>:valeur (identifiant ou nom d'un champ du pack)";

#[derive(Default)]
pub struct SearchPanel {
    pub query: String,
    /// Requête et versions du projet des résultats affichés.
    key: Option<(String, u64, u64)>,
    hits: Vec<Hit>,
    error: Option<String>,
    /// Donner le focus au champ de recherche.
    pub focus: bool,
}

impl SearchPanel {
    fn refresh(&mut self, project: &Project) {
        let key = (
            self.query.clone(),
            project.generation(),
            project.content_version(),
        );
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.key = Some(key);
        match Query::parse(&self.query) {
            Ok(q) => {
                self.hits = project.search_text(&q);
                self.error = None;
            }
            Err(e) => {
                self.hits.clear();
                self.error = Some(e.to_string());
            }
        }
    }

    /// Rend la fiche à ouvrir et l'octet où placer le curseur.
    pub fn ui(&mut self, ui: &mut egui::Ui, project: &Project) -> Option<(NoteId, Option<usize>)> {
        let mut open = None;
        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Rechercher dans tout le projet…")
                    .desired_width(ui.available_width() - 28.0),
            );
            if self.focus {
                edit.request_focus();
                self.focus = false;
            }
            ui.label(RichText::new("?").strong()).on_hover_text(HELP);
        });
        self.refresh(project);
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return None;
        }
        if self.query.trim().is_empty() {
            ui.weak("Tapez des mots, une \"expression\", /regex/ ou un filtre (survolez « ? »).");
            return None;
        }
        let total: usize = self.hits.iter().map(|h| h.lines.len().max(1)).sum();
        ui.weak(format!("{} fiche(s), {total} résultat(s)", self.hits.len()));
        let accent = crate::theme::accent(ui.visuals());
        let text_color = ui.visuals().text_color();
        egui::ScrollArea::vertical()
            .id_salt("search_results")
            .auto_shrink(false)
            .show(ui, |ui| {
                for hit in &self.hits {
                    let title = ui.add(
                        egui::Label::new(
                            RichText::new(&hit.title)
                                .strong()
                                .color(ui.visuals().strong_text_color()),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if title
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        open = Some((hit.id.clone(), None));
                    }
                    for line in &hit.lines {
                        let mut job = LayoutJob::default();
                        let mut at = 0;
                        let plain =
                            TextFormat::simple(egui::FontId::proportional(12.5), text_color);
                        for r in &line.ranges {
                            if r.start < at {
                                continue;
                            }
                            job.append(&line.text[at..r.start], 0.0, plain.clone());
                            let mut hl = plain.clone();
                            hl.background = accent.gamma_multiply(0.45);
                            hl.color = ui.visuals().strong_text_color();
                            job.append(&line.text[r.clone()], 0.0, hl);
                            at = r.end;
                        }
                        job.append(&line.text[at..], 0.0, plain.clone());
                        job.wrap.max_width = ui.available_width() - 12.0;
                        let resp = ui
                            .horizontal(|ui| {
                                ui.add_space(10.0);
                                ui.add(egui::Label::new(job).sense(egui::Sense::click()))
                            })
                            .inner
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(format!("Ligne {}", line.line + 1));
                        if resp.clicked() {
                            open = Some((hit.id.clone(), Some(line.byte)));
                        }
                    }
                    ui.add_space(6.0);
                }
            });
        open
    }
}
