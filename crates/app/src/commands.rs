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
    AssociateFiles,
    FormatBold,
    FormatItalic,
    FormatStrike,
    FormatCode,
    FormatLink,
    Heading1,
    Heading2,
    Heading3,
    BulletList,
    NumberedList,
    TaskList,
    QuoteBlock,
    CodeBlock,
    InsertTable,
    InsertRule,
    InsertDate,
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
        Command::AssociateFiles,
        Command::FormatBold,
        Command::FormatItalic,
        Command::FormatStrike,
        Command::FormatCode,
        Command::FormatLink,
        Command::Heading1,
        Command::Heading2,
        Command::Heading3,
        Command::BulletList,
        Command::NumberedList,
        Command::TaskList,
        Command::QuoteBlock,
        Command::CodeBlock,
        Command::InsertTable,
        Command::InsertRule,
        Command::InsertDate,
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
            Command::AssociateFiles => "associate_files",
            Command::FormatBold => "format_bold",
            Command::FormatItalic => "format_italic",
            Command::FormatStrike => "format_strike",
            Command::FormatCode => "format_code",
            Command::FormatLink => "format_link",
            Command::Heading1 => "heading_1",
            Command::Heading2 => "heading_2",
            Command::Heading3 => "heading_3",
            Command::BulletList => "bullet_list",
            Command::NumberedList => "numbered_list",
            Command::TaskList => "task_list",
            Command::QuoteBlock => "quote",
            Command::CodeBlock => "code_block",
            Command::InsertTable => "insert_table",
            Command::InsertRule => "insert_rule",
            Command::InsertDate => "insert_date",
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
            Command::InsertImage => "Insérer une image ou un fichier…",
            Command::AssociateFiles => "Ouvrir les fichiers .enigma avec Enigma Notes",
            Command::FormatBold => "Texte : gras",
            Command::FormatItalic => "Texte : italique",
            Command::FormatStrike => "Texte : barré",
            Command::FormatCode => "Texte : code",
            Command::FormatLink => "Texte : lien vers une fiche [[ ]]",
            Command::Heading1 => "Texte : titre 1",
            Command::Heading2 => "Texte : titre 2",
            Command::Heading3 => "Texte : titre 3",
            Command::BulletList => "Texte : liste à puces",
            Command::NumberedList => "Texte : liste numérotée",
            Command::TaskList => "Texte : liste de tâches",
            Command::QuoteBlock => "Texte : citation",
            Command::CodeBlock => "Texte : bloc de code",
            Command::InsertTable => "Insérer un tableau",
            Command::InsertRule => "Insérer un séparateur",
            Command::InsertDate => "Insérer la date du jour",
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
            Command::FormatBold => &["Ctrl+B"],
            Command::FormatItalic => &["Ctrl+I"],
            Command::FormatStrike => &["Ctrl+Maj+X"],
            Command::FormatCode => &["Ctrl+Maj+C"],
            Command::FormatLink => &["Ctrl+K"],
            Command::Heading1 => &["Ctrl+1"],
            Command::Heading2 => &["Ctrl+2"],
            Command::Heading3 => &["Ctrl+3"],
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

/// Outil d'écriture d'une commande de mise en forme.
pub fn format_of(cmd: Command) -> Option<crate::editor::Format> {
    use crate::editor::Format;
    use enigma_core::md_edit::{Inline as I, Line as L};
    Some(match cmd {
        Command::FormatBold => Format::Inline(I::Bold),
        Command::FormatItalic => Format::Inline(I::Italic),
        Command::FormatStrike => Format::Inline(I::Strike),
        Command::FormatCode => Format::Inline(I::Code),
        Command::FormatLink => Format::Inline(I::WikiLink),
        Command::Heading1 => Format::Line(L::Heading(1)),
        Command::Heading2 => Format::Line(L::Heading(2)),
        Command::Heading3 => Format::Line(L::Heading(3)),
        Command::BulletList => Format::Line(L::Bullet),
        Command::NumberedList => Format::Line(L::Numbered),
        Command::TaskList => Format::Line(L::Task),
        Command::QuoteBlock => Format::Line(L::Quote),
        Command::CodeBlock => Format::CodeBlock,
        Command::InsertTable => Format::Table(3, 3),
        Command::InsertRule => Format::Rule,
        Command::InsertDate => Format::Date,
        _ => return None,
    })
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
