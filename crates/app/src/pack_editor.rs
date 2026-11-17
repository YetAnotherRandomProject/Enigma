//! Fenêtre de création et de modification d'un pack de jeu.
//!
//! - Un pack du projet (`.enigma/packs`) ou du dossier utilisateur est
//!   modifié sur place.
//! - Un pack fourni avec l'application est enregistré en copie dans le
//!   projet : la copie est prioritaire et l'original n'est jamais modifié.
//! - Les identifiants existant à l'ouverture sont verrouillés (les fiches y
//!   font référence) ; seuls les nouveaux éléments ont un identifiant éditable.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, RichText};
use enigma_core::pack::{
    Category, FieldDef, FieldKind, LinkType, PACK_FILE, PACK_FORMAT_VERSION, Status, Template,
};
use enigma_core::{Color, Pack, Project};

use crate::notices::Notices;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Section {
    General,
    Categories,
    LinkTypes,
    Statuses,
    Fields,
    Templates,
}

impl Section {
    const ALL: [Section; 6] = [
        Section::General,
        Section::Categories,
        Section::LinkTypes,
        Section::Statuses,
        Section::Fields,
        Section::Templates,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::General => "Général",
            Section::Categories => "Catégories",
            Section::LinkTypes => "Types de liens",
            Section::Statuses => "Statuts",
            Section::Fields => "Champs",
            Section::Templates => "Modèles",
        }
    }
}

/// Où enregistrer un nouveau pack.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Location {
    /// `.enigma/packs` du projet (versionné avec les fiches).
    Project,
    /// Dossier utilisateur : disponible pour tous les projets.
    User,
}

/// Couleurs proposées aux nouveaux éléments, à tour de rôle.
const PALETTE: [[u8; 3]; 8] = [
    [0x4a, 0xa3, 0xff],
    [0xff, 0x7a, 0x59],
    [0xff, 0xd2, 0x3f],
    [0x6e, 0xe7, 0xa8],
    [0xc0, 0x84, 0xfc],
    [0xf4, 0x72, 0xb6],
    [0x2d, 0xd4, 0xbf],
    [0xa3, 0xa3, 0xa3],
];

pub struct PackEditor {
    pack: Pack,
    is_new: bool,
    /// Dossier d'enregistrement pour un pack existant.
    existing_dir: Option<PathBuf>,
    location: Location,
    user_dir: Option<PathBuf>,
    use_for_project: bool,
    locked: HashSet<(Section, String)>,
    section: Section,
    info: Option<String>,
}

/// Résultat de l'affichage de la fenêtre.
pub enum Outcome {
    Open,
    Closed,
    Saved,
}

impl PackEditor {
    /// Modifier le pack actuel du projet.
    pub fn edit_current(project: &Project, user_dir: Option<&Path>) -> PackEditor {
        let pack = project.pack().clone();
        let local = project.local_packs_dir();
        let in_place = project
            .pack_dir()
            .filter(|d| d.starts_with(&local) || user_dir.is_some_and(|u| d.starts_with(u)))
            .map(Path::to_path_buf);
        let info = match &in_place {
            Some(_) => None,
            None => Some(format!(
                "Ce pack est fourni avec l'application : vos modifications seront \
                 enregistrées dans une copie propre au projet (.enigma/packs/{}), \
                 prioritaire sur l'original.",
                pack.id
            )),
        };
        let existing_dir = in_place.or_else(|| Some(local.join(&pack.id)));
        let mut locked = HashSet::new();
        locked.extend(
            pack.categories
                .iter()
                .map(|c| (Section::Categories, c.id.clone())),
        );
        locked.extend(
            pack.link_types
                .iter()
                .map(|c| (Section::LinkTypes, c.id.clone())),
        );
        locked.extend(
            pack.statuses
                .iter()
                .map(|c| (Section::Statuses, c.id.clone())),
        );
        locked.extend(pack.fields.iter().map(|c| (Section::Fields, c.id.clone())));
        locked.extend(
            pack.templates
                .iter()
                .map(|c| (Section::Templates, c.id.clone())),
        );
        PackEditor {
            pack,
            is_new: false,
            existing_dir,
            location: Location::Project,
            user_dir: user_dir.map(Path::to_path_buf),
            use_for_project: true,
            locked,
            section: Section::Categories,
            info,
        }
    }

    /// Créer un nouveau pack (vide, ou à partir du pack actuel).
    pub fn new_pack(project: &Project, user_dir: Option<&Path>, from_current: bool) -> PackEditor {
        let mut pack = if from_current && !project.pack().id.is_empty() {
            project.pack().clone()
        } else {
            Pack::empty()
        };
        pack.version = PACK_FORMAT_VERSION;
        pack.id = String::new();
        pack.name = String::new();
        PackEditor {
            pack,
            is_new: true,
            existing_dir: None,
            location: Location::Project,
            user_dir: user_dir.map(Path::to_path_buf),
            use_for_project: true,
            locked: HashSet::new(),
            section: Section::General,
            info: None,
        }
    }

    fn target_dir(&self, project: &Project) -> Option<PathBuf> {
        if !self.is_new {
            return self.existing_dir.clone();
        }
        let base = match self.location {
            Location::Project => project.local_packs_dir(),
            Location::User => self.user_dir.clone()?,
        };
        Some(base.join(&self.pack.id))
    }

    /// Problèmes empêchant l'enregistrement (ceux du pack + emplacement).
    fn problems(&self, project: &Project) -> Vec<String> {
        let mut problems = self.pack.problems();
        if self.is_new
            && !self.pack.id.is_empty()
            && let Some(dir) = self.target_dir(project)
            && dir.join(PACK_FILE).exists()
        {
            problems.push(format!(
                "un pack « {} » existe déjà à cet endroit",
                self.pack.id
            ));
        }
        problems
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        project: &mut Project,
        notices: &mut Notices,
    ) -> Outcome {
        let mut open = true;
        let mut outcome = Outcome::Open;
        let title = if self.is_new {
            "Nouveau pack de jeu"
        } else {
            "Modifier le pack de jeu"
        };
        egui::Window::new(title)
            .id(egui::Id::unique("pack_editor"))
            .open(&mut open)
            .default_size([640.0, 520.0])
            .collapsible(false)
            .show(ctx, |ui| {
                if let Some(info) = &self.info {
                    ui.label(RichText::new(info).small().weak());
                    ui.add_space(4.0);
                }
                ui.horizontal_wrapped(|ui| {
                    for s in Section::ALL {
                        ui.selectable_value(&mut self.section, s, s.label());
                    }
                });
                ui.separator();

                let problems = self.problems(project);
                let footer_h = 40.0 + 16.0 * problems.len().min(6) as f32;
                egui::ScrollArea::vertical()
                    .id_salt("pack_editor_scroll")
                    .max_height((ui.available_height() - footer_h).max(120.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.section {
                        Section::General => self.general_ui(ui),
                        Section::Categories => self.categories_ui(ui),
                        Section::LinkTypes => self.link_types_ui(ui),
                        Section::Statuses => self.statuses_ui(ui),
                        Section::Fields => self.fields_ui(ui),
                        Section::Templates => self.templates_ui(ui),
                    });

                ui.separator();
                for p in problems.iter().take(6) {
                    ui.colored_label(ui.visuals().error_fg_color, format!("• {p}"));
                }
                if problems.len() > 6 {
                    ui.weak(format!("… et {} autre(s)", problems.len() - 6));
                }
                ui.horizontal(|ui| {
                    let save =
                        ui.add_enabled(problems.is_empty(), egui::Button::new("Enregistrer"));
                    if save.clicked() {
                        match self.save(project) {
                            Ok(dir) => {
                                notices.info(format!("Pack enregistré dans {}.", dir.display()));
                                outcome = Outcome::Saved;
                            }
                            Err(e) => notices.error(format!("Enregistrement impossible : {e}")),
                        }
                    }
                    if ui.button("Annuler").clicked() {
                        outcome = Outcome::Closed;
                    }
                });
            });
        if !open {
            outcome = Outcome::Closed;
        }
        outcome
    }

    fn save(&self, project: &mut Project) -> Result<PathBuf, String> {
        let dir = self
            .target_dir(project)
            .ok_or_else(|| "dossier utilisateur introuvable".to_owned())?;
        project
            .save_pack(&self.pack, &dir)
            .map_err(|e| e.to_string())?;
        if self.is_new && self.use_for_project {
            project
                .set_pack(Some(self.pack.id.clone()))
                .map_err(|e| e.to_string())?;
        }
        Ok(dir)
    }

    fn general_ui(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("pack_general")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Identifiant");
                if self.is_new {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pack.id)
                            .hint_text("ex. mon_jeu")
                            .desired_width(200.0),
                    );
                } else {
                    ui.monospace(&self.pack.id);
                }
                ui.end_row();
                ui.label("Nom");
                ui.add_sized(
                    [300.0, 20.0],
                    egui::TextEdit::singleline(&mut self.pack.name).hint_text("nom"),
                );
                ui.end_row();
                ui.label("Description");
                let mut desc = self.pack.description.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut desc)
                            .desired_rows(3)
                            .desired_width(300.0),
                    )
                    .changed()
                {
                    self.pack.description = (!desc.trim().is_empty()).then_some(desc);
                }
                ui.end_row();
            });
        if self.is_new {
            ui.add_space(8.0);
            ui.label("Enregistrer :");
            ui.radio_value(
                &mut self.location,
                Location::Project,
                "dans ce projet (.enigma/packs, versionné avec les fiches)",
            );
            ui.add_enabled_ui(self.user_dir.is_some(), |ui| {
                ui.radio_value(
                    &mut self.location,
                    Location::User,
                    "pour tous mes projets (dossier utilisateur)",
                );
            });
            ui.checkbox(&mut self.use_for_project, "Utiliser ce pack pour le projet");
        }
    }

    fn id_cell(ui: &mut egui::Ui, locked: bool, id: &mut String) {
        if locked {
            ui.monospace(id.as_str())
                .on_hover_text("Identifiant utilisé par les fiches : non modifiable.");
        } else {
            ui.add_sized(
                [110.0, 20.0],
                egui::TextEdit::singleline(id).hint_text("id"),
            );
        }
    }

    fn color_cell(ui: &mut egui::Ui, color: &mut Color) {
        let [r, g, b, a] = color.0;
        let mut c = Color32::from_rgba_unmultiplied(r, g, b, a);
        if ui.color_edit_button_srgba(&mut c).changed() {
            *color = Color(c.to_srgba_unmultiplied());
        }
    }

    fn next_color(n: usize) -> Color {
        let [r, g, b] = PALETTE[n % PALETTE.len()];
        Color([r, g, b, 0xff])
    }

    fn categories_ui(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        egui::Grid::new("pack_categories")
            .num_columns(5)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                for h in ["Identifiant", "Libellé", "Couleur", "Icône", ""] {
                    ui.label(RichText::new(h).small().weak());
                }
                ui.end_row();
                for (i, c) in self.pack.categories.iter_mut().enumerate() {
                    let locked = self.locked.contains(&(Section::Categories, c.id.clone()));
                    Self::id_cell(ui, locked, &mut c.id);
                    ui.add_sized(
                        [150.0, 20.0],
                        egui::TextEdit::singleline(&mut c.label).hint_text("libellé"),
                    );
                    Self::color_cell(ui, &mut c.color);
                    let mut icon = c.icon.clone().unwrap_or_default();
                    if ui
                        .add_sized(
                            [36.0, 20.0],
                            egui::TextEdit::singleline(&mut icon).hint_text("icône"),
                        )
                        .changed()
                    {
                        c.icon = (!icon.trim().is_empty()).then(|| icon.trim().to_owned());
                    }
                    if remove_button(ui, locked) {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        if let Some(i) = remove {
            self.pack.categories.remove(i);
        }
        if ui.button("+ Ajouter une catégorie").clicked() {
            let color = Self::next_color(self.pack.categories.len());
            self.pack.categories.push(Category {
                id: String::new(),
                label: String::new(),
                color,
                icon: None,
            });
        }
    }

    fn link_types_ui(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        egui::Grid::new("pack_link_types")
            .num_columns(5)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                for h in ["Identifiant", "Libellé", "Couleur", "Orienté", ""] {
                    ui.label(RichText::new(h).small().weak());
                }
                ui.end_row();
                for (i, t) in self.pack.link_types.iter_mut().enumerate() {
                    let locked = self.locked.contains(&(Section::LinkTypes, t.id.clone()));
                    Self::id_cell(ui, locked, &mut t.id);
                    ui.add_sized(
                        [150.0, 20.0],
                        egui::TextEdit::singleline(&mut t.label).hint_text("libellé"),
                    );
                    Self::color_cell(ui, &mut t.color);
                    ui.checkbox(&mut t.directed, "")
                        .on_hover_text("Un lien orienté a une flèche (A indique B).");
                    if remove_button(ui, locked) {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        if let Some(i) = remove {
            self.pack.link_types.remove(i);
        }
        if ui.button("+ Ajouter un type de lien").clicked() {
            let color = Self::next_color(self.pack.link_types.len());
            self.pack.link_types.push(LinkType {
                id: String::new(),
                label: String::new(),
                color,
                directed: true,
            });
        }
    }

    fn statuses_ui(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        egui::Grid::new("pack_statuses")
            .num_columns(4)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                for h in ["Identifiant", "Libellé", "Couleur", ""] {
                    ui.label(RichText::new(h).small().weak());
                }
                ui.end_row();
                for (i, s) in self.pack.statuses.iter_mut().enumerate() {
                    let locked = self.locked.contains(&(Section::Statuses, s.id.clone()));
                    Self::id_cell(ui, locked, &mut s.id);
                    ui.add_sized(
                        [150.0, 20.0],
                        egui::TextEdit::singleline(&mut s.label).hint_text("libellé"),
                    );
                    Self::color_cell(ui, &mut s.color);
                    if remove_button(ui, locked) {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        if let Some(i) = remove {
            self.pack.statuses.remove(i);
        }
        if ui.button("+ Ajouter un statut").clicked() {
            let color = Self::next_color(self.pack.statuses.len());
            self.pack.statuses.push(Status {
                id: String::new(),
                label: String::new(),
                color,
            });
        }
    }

    fn fields_ui(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        egui::Grid::new("pack_fields")
            .num_columns(4)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                for h in ["Identifiant", "Libellé", "Type", ""] {
                    ui.label(RichText::new(h).small().weak());
                }
                ui.end_row();
                for (i, f) in self.pack.fields.iter_mut().enumerate() {
                    let locked = self.locked.contains(&(Section::Fields, f.id.clone()));
                    Self::id_cell(ui, locked, &mut f.id);
                    ui.add_sized(
                        [150.0, 20.0],
                        egui::TextEdit::singleline(&mut f.label).hint_text("libellé"),
                    );
                    egui::ComboBox::new(("field_kind", i), "")
                        .selected_text(kind_label(f.kind))
                        .show_ui(ui, |ui| {
                            for k in [FieldKind::Number, FieldKind::Text, FieldKind::Bool] {
                                ui.selectable_value(&mut f.kind, k, kind_label(k));
                            }
                        });
                    if remove_button(ui, locked) {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        if let Some(i) = remove {
            self.pack.fields.remove(i);
        }
        if ui.button("+ Ajouter un champ").clicked() {
            self.pack.fields.push(FieldDef {
                id: String::new(),
                label: String::new(),
                kind: FieldKind::Text,
            });
        }
    }

    fn templates_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Dans le corps, {{title}} est remplacé par le titre de la fiche.")
                .small()
                .weak(),
        );
        let categories = self.pack.categories.clone();
        let statuses = self.pack.statuses.clone();
        let mut remove = None;
        for (i, t) in self.pack.templates.iter_mut().enumerate() {
            let locked = self.locked.contains(&(Section::Templates, t.id.clone()));
            let header = if t.label.is_empty() {
                format!("Modèle {}", i + 1)
            } else {
                t.label.clone()
            };
            egui::CollapsingHeader::new(header)
                .id_salt(("template", i))
                .default_open(t.id.is_empty())
                .show(ui, |ui| {
                    egui::Grid::new(("template_grid", i))
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Identifiant");
                            Self::id_cell(ui, locked, &mut t.id);
                            ui.end_row();
                            ui.label("Libellé");
                            ui.add_sized(
                                [220.0, 20.0],
                                egui::TextEdit::singleline(&mut t.label).hint_text("libellé"),
                            );
                            ui.end_row();
                            ui.label("Catégories");
                            ui.horizontal_wrapped(|ui| {
                                for c in &categories {
                                    let mut on = t.categories.contains(&c.id);
                                    let label = if c.label.is_empty() { &c.id } else { &c.label };
                                    if ui.checkbox(&mut on, label.as_str()).changed() {
                                        enigma_core::filter::NoteFilter::toggle(
                                            &mut t.categories,
                                            c.id.clone(),
                                        );
                                    }
                                }
                            });
                            ui.end_row();
                            ui.label("Statut");
                            let current = t
                                .status
                                .as_deref()
                                .and_then(|s| statuses.iter().find(|x| x.id == s))
                                .map_or("Aucun", |s| s.label.as_str())
                                .to_owned();
                            egui::ComboBox::new(("template_status", i), "")
                                .selected_text(current)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut t.status, None, "Aucun");
                                    for s in &statuses {
                                        ui.selectable_value(
                                            &mut t.status,
                                            Some(s.id.clone()),
                                            &s.label,
                                        );
                                    }
                                });
                            ui.end_row();
                        });
                    ui.label("Corps");
                    ui.add(
                        egui::TextEdit::multiline(&mut t.body)
                            .code_editor()
                            .desired_rows(5)
                            .desired_width(f32::INFINITY),
                    );
                    if remove_button(ui, locked) {
                        remove = Some(i);
                    }
                });
        }
        if let Some(i) = remove {
            self.pack.templates.remove(i);
        }
        if ui.button("+ Ajouter un modèle").clicked() {
            self.pack.templates.push(Template::default());
        }
    }
}

fn kind_label(k: FieldKind) -> &'static str {
    match k {
        FieldKind::Number => "Nombre",
        FieldKind::Text => "Texte",
        FieldKind::Bool => "Oui / non",
    }
}

fn remove_button(ui: &mut egui::Ui, locked: bool) -> bool {
    let hint = if locked {
        "Supprimer : les fiches qui l'utilisent le garderont (affiché en gris)."
    } else {
        "Supprimer"
    };
    ui.small_button("Supprimer").on_hover_text(hint).clicked()
}
