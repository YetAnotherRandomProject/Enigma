//! Commandes de l'application : identifiant stable (fichier de raccourcis),
//! libellé (palette), raccourcis par défaut, et conversion des raccourcis
//! vers egui.

use eframe::egui::{self, KeyboardShortcut, Modifiers};
use enigma_core::shortcut::{Keymap, Shortcut};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    Palette,
    NewNote,
    Save,
    Undo,
    Redo,
    Find,
    ShowNote,
    ShowGraph,
    ShowBoard,
    ToggleGraph,
    Graph2D,
    Graph3D,
    GraphRecenter,
    ExportSvg,
    ExportPng,
    AddToBoard,
    RemoveFromBoard,
    ToggleTheme,
    ThemeDark,
    ThemeLight,
    ThemeSystem,
    Shortcuts,
    EditPack,
    OpenProject,
    DeleteNote,
    NewProject,
    ImportFolder,
    ImportNotes,
    InsertImage,
    ToggleReading,
    AssociateFiles,
}

impl Command {
    pub const ALL: &'static [Command] = &[
        Command::Palette,
        Command::NewNote,
        Command::Save,
        Command::Undo,
        Command::Redo,
        Command::Find,
        Command::ShowNote,
        Command::ShowGraph,
        Command::ShowBoard,
        Command::ToggleGraph,
        Command::Graph2D,
        Command::Graph3D,
        Command::GraphRecenter,
        Command::ExportSvg,
        Command::ExportPng,
        Command::AddToBoard,
        Command::RemoveFromBoard,
        Command::ToggleTheme,
        Command::ThemeDark,
        Command::ThemeLight,
        Command::ThemeSystem,
        Command::Shortcuts,
        Command::EditPack,
        Command::OpenProject,
        Command::DeleteNote,
        Command::NewProject,
        Command::ImportFolder,
        Command::ImportNotes,
        Command::InsertImage,
        Command::ToggleReading,
        Command::AssociateFiles,
    ];

    /// Identifiant stable (fichier de raccourcis).
    pub fn id(self) -> &'static str {
        match self {
            Command::Palette => "palette",
            Command::NewNote => "new_note",
            Command::Save => "save",
            Command::Undo => "undo",
            Command::Redo => "redo",
            Command::Find => "find",
            Command::ShowNote => "show_note",
            Command::ShowGraph => "show_graph",
            Command::ShowBoard => "show_board",
            Command::ToggleGraph => "toggle_graph",
            Command::Graph2D => "graph_2d",
            Command::Graph3D => "graph_3d",
            Command::GraphRecenter => "graph_recenter",
            Command::ExportSvg => "export_svg",
            Command::ExportPng => "export_png",
            Command::AddToBoard => "board_add",
            Command::RemoveFromBoard => "board_remove",
            Command::ToggleTheme => "toggle_theme",
            Command::ThemeDark => "theme_dark",
            Command::ThemeLight => "theme_light",
            Command::ThemeSystem => "theme_system",
            Command::Shortcuts => "shortcuts",
            Command::EditPack => "edit_pack",
            Command::OpenProject => "open_project",
            Command::DeleteNote => "delete_note",
            Command::NewProject => "new_project",
            Command::ImportFolder => "import_folder",
            Command::ImportNotes => "import_notes",
            Command::InsertImage => "insert_image",
            Command::ToggleReading => "toggle_reading",
            Command::AssociateFiles => "associate_files",
        }
    }

    /// Libellé (palette, éditeur de raccourcis).
    pub fn label(self) -> &'static str {
        match self {
            Command::Palette => "Palette de commandes",
            Command::NewNote => "Nouvelle fiche",
            Command::Save => "Enregistrer",
            Command::Undo => "Annuler",
            Command::Redo => "Rétablir",
            Command::Find => "Rechercher dans la liste",
            Command::ShowNote => "Afficher : fiche",
            Command::ShowGraph => "Afficher : graphe",
            Command::ShowBoard => "Afficher : tableau d'enquête",
            Command::ToggleGraph => "Basculer fiche / graphe",
            Command::Graph2D => "Graphe en 2D",
            Command::Graph3D => "Graphe en 3D",
            Command::GraphRecenter => "Recentrer le graphe",
            Command::ExportSvg => "Exporter le graphe en SVG…",
            Command::ExportPng => "Exporter le graphe en PNG…",
            Command::AddToBoard => "Ajouter la fiche au tableau",
            Command::RemoveFromBoard => "Retirer la fiche du tableau",
            Command::ToggleTheme => "Basculer thème clair / sombre",
            Command::ThemeDark => "Thème sombre",
            Command::ThemeLight => "Thème clair",
            Command::ThemeSystem => "Thème du système",
            Command::Shortcuts => "Raccourcis clavier…",
            Command::EditPack => "Modifier le pack de jeu…",
            Command::OpenProject => "Ouvrir un projet…",
            Command::DeleteNote => "Supprimer la fiche…",
            Command::NewProject => "Nouveau projet…",
            Command::ImportFolder => "Importer un dossier de fiches .md…",
            Command::ImportNotes => "Importer des fiches .md…",
            Command::InsertImage => "Insérer une image…",
            Command::ToggleReading => "Basculer lecture / édition",
            Command::AssociateFiles => "Ouvrir les fichiers .enigma avec Enigma Notes",
        }
    }

    /// Raccourcis par défaut.
    pub fn default_shortcuts(self) -> &'static [&'static str] {
        match self {
            Command::Palette => &["Ctrl+P"],
            Command::NewNote => &["Ctrl+N"],
            Command::Save => &["Ctrl+S"],
            Command::Undo => &["Ctrl+Z"],
            Command::Redo => &["Ctrl+Y", "Ctrl+Maj+Z"],
            Command::Find => &["Ctrl+F"],
            Command::ToggleGraph => &["Ctrl+G"],
            Command::ShowBoard => &["Ctrl+T"],
            Command::ToggleTheme => &["Ctrl+Maj+L"],
            Command::ToggleReading => &["Ctrl+E"],
            Command::OpenProject => &["Ctrl+O"],
            _ => &[],
        }
    }

    pub fn defaults(self) -> Vec<Shortcut> {
        self.default_shortcuts()
            .iter()
            .filter_map(|s| s.parse().ok())
            .collect()
    }

    /// Raccourcis effectifs.
    pub fn bindings(self, keymap: &Keymap) -> Vec<Shortcut> {
        keymap.bindings(self.id(), &self.defaults())
    }

    /// Texte des raccourcis effectifs (« Ctrl+Y, Ctrl+Maj+Z »).
    pub fn bindings_text(self, keymap: &Keymap) -> String {
        self.bindings(keymap)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Applique une commande de thème (préférence mémorisée par egui, donc
/// conservée d'une session à l'autre).
pub fn apply_theme(ctx: &egui::Context, cmd: Command) {
    let pref = match cmd {
        Command::ThemeDark => egui::ThemePreference::Dark,
        Command::ThemeLight => egui::ThemePreference::Light,
        Command::ThemeSystem => egui::ThemePreference::System,
        Command::ToggleTheme => {
            if ctx.global_style().visuals.dark_mode {
                egui::ThemePreference::Light
            } else {
                egui::ThemePreference::Dark
            }
        }
        _ => return,
    };
    ctx.set_theme(pref);
}

/// Raccourci egui correspondant (`None` si la touche est inconnue).
pub fn to_egui(s: &Shortcut) -> Option<KeyboardShortcut> {
    let key = egui::Key::from_name(&s.key)?;
    let mut m = Modifiers::NONE;
    if s.ctrl {
        m = m.plus(Modifiers::COMMAND);
    }
    if s.shift {
        m = m.plus(Modifiers::SHIFT);
    }
    if s.alt {
        m = m.plus(Modifiers::ALT);
    }
    Some(KeyboardShortcut::new(m, key))
}

/// Raccourci saisi au clavier (éditeur de raccourcis).
pub fn from_egui(modifiers: Modifiers, key: egui::Key) -> Shortcut {
    Shortcut {
        ctrl: modifiers.command || modifiers.ctrl,
        shift: modifiers.shift,
        alt: modifiers.alt,
        key: key.name().to_owned(),
    }
}

/// Consomme les raccourcis pressés pendant cette image et rend les
/// commandes correspondantes. Les combinaisons les plus précises sont
/// testées d'abord (`Ctrl+Maj+Z` avant `Ctrl+Z`, qu'egui accepterait aussi).
pub fn consume_shortcuts(ctx: &egui::Context, keymap: &Keymap) -> Vec<Command> {
    let mut all: Vec<(Command, Shortcut)> = Command::ALL
        .iter()
        .flat_map(|&c| c.bindings(keymap).into_iter().map(move |s| (c, s)))
        .collect();
    all.sort_by_key(|(_, s)| std::cmp::Reverse(s.modifier_count()));
    let mut out = Vec::new();
    ctx.input_mut(|i| {
        for (cmd, s) in &all {
            if let Some(ks) = to_egui(s)
                && i.consume_shortcut(&ks)
                && !out.contains(cmd)
            {
                out.push(*cmd);
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_unique_and_defaults_valid() {
        let mut ids: Vec<&str> = Command::ALL.iter().map(|c| c.id()).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "identifiants uniques");
        for c in Command::ALL {
            assert_eq!(c.defaults().len(), c.default_shortcuts().len(), "{c:?}");
            for s in c.defaults() {
                assert!(to_egui(&s).is_some(), "touche inconnue pour {c:?}");
            }
        }
        let conflicts =
            enigma_core::shortcut::conflicts(Command::ALL.iter().map(|c| (c.id(), c.defaults())));
        assert!(conflicts.is_empty(), "{conflicts:?}");
    }

    #[test]
    fn egui_round_trip() {
        let s: Shortcut = "Ctrl+Maj+F2".parse().unwrap();
        let ks = to_egui(&s).unwrap();
        assert_eq!(from_egui(ks.modifiers, ks.logical_key), s);
    }
}
