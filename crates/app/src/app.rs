//! Application eframe : barre du haut, écran d'accueil, projets récents.

use std::path::PathBuf;
use std::time::Instant;

use eframe::egui;
use enigma_core::shortcut::Keymap;

use crate::commands::{self, Command};
use crate::notices::Notices;
use crate::paths;
use crate::workspace::{AppRequest, Workspace};

const RECENT_KEY: &str = "recent_projects";
const MAX_RECENT: usize = 8;

pub struct EnigmaApp {
    recent: Vec<PathBuf>,
    workspace: Option<Workspace>,
    notices: Notices,
    pack_roots: Vec<PathBuf>,
    start: Instant,
    keymap: Keymap,
    /// Fichier des raccourcis (`None` : illisible, donc jamais écrasé).
    keymap_path: Option<PathBuf>,
}

impl EnigmaApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> EnigmaApp {
        let recent: Vec<PathBuf> = cc
            .storage
            .and_then(|s| eframe::get_value(s, RECENT_KEY))
            .unwrap_or_default();
        let mut notices = Notices::default();
        let keymap_path = paths::keymap_path();
        let (keymap, keymap_path) = match keymap_path.as_deref().map(Keymap::load) {
            Some(Ok(k)) => (k, keymap_path),
            Some(Err(e)) => {
                notices.error(format!(
                    "Raccourcis : {e} (valeurs par défaut, fichier laissé intact)"
                ));
                (Keymap::default(), None)
            }
            None => (Keymap::default(), None),
        };
        let mut app = EnigmaApp {
            recent,
            workspace: None,
            notices,
            pack_roots: paths::pack_roots(),
            start: Instant::now(),
            keymap,
            keymap_path,
        };
        // Dossier passé en argument (`enigma <dossier>`), sinon le dernier
        // projet ouvert s'il existe encore.
        let from_args = std::env::args_os().nth(1).map(PathBuf::from);
        let initial = from_args.or_else(|| app.recent.first().cloned().filter(|p| p.is_dir()));
        if let Some(root) = initial {
            app.open_project(root, &cc.egui_ctx);
        }
        app
    }

    fn open_project(&mut self, root: PathBuf, ctx: &egui::Context) {
        if let Some(ws) = &mut self.workspace {
            ws.flush(&mut self.notices);
            // Raccourcis éventuellement modifiés dans ce projet.
            self.keymap = ws.keymap().clone();
        }
        match Workspace::open(&root, &self.pack_roots, ctx, &mut self.notices) {
            Ok(mut ws) => {
                ws.set_keymap(self.keymap.clone(), self.keymap_path.clone());
                let name = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.display().to_string());
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "{name} — Enigma Notes"
                )));
                self.workspace = Some(ws);
                self.recent.retain(|p| p != &root);
                self.recent.insert(0, root);
                self.recent.truncate(MAX_RECENT);
            }
            Err(e) => self
                .notices
                .error(format!("Impossible d'ouvrir le projet : {e}")),
        }
    }

    fn pick_folder(&mut self, ctx: &egui::Context) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("Choisir le dossier du projet")
            .pick_folder()
        {
            self.open_project(dir, ctx);
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut open: Option<PathBuf> = None;
        let mut pick = false;
        ui.horizontal(|ui| {
            ui.menu_button("Projet", |ui| {
                if ui.button("Ouvrir un dossier…").clicked() {
                    pick = true;
                    ui.close();
                }
                ui.menu_button("Récents", |ui| {
                    if self.recent.is_empty() {
                        ui.weak("Aucun");
                    }
                    for p in &self.recent {
                        if ui.button(p.display().to_string()).clicked() {
                            open = Some(p.clone());
                            ui.close();
                        }
                    }
                });
            });
            let mut run = None;
            ui.menu_button("Affichage", |ui| {
                for cmd in [
                    Command::ThemeLight,
                    Command::ThemeDark,
                    Command::ThemeSystem,
                ] {
                    if ui.button(cmd.label()).clicked() {
                        run = Some(cmd);
                        ui.close();
                    }
                }
                if self.workspace.is_some() {
                    ui.separator();
                    for cmd in [Command::Palette, Command::Shortcuts] {
                        let keys = self
                            .workspace
                            .as_ref()
                            .map(|ws| cmd.bindings_text(ws.keymap()))
                            .unwrap_or_default();
                        if ui
                            .add(egui::Button::new(cmd.label()).shortcut_text(keys))
                            .clicked()
                        {
                            run = Some(cmd);
                            ui.close();
                        }
                    }
                }
            });
            if let Some(cmd) = run {
                match &mut self.workspace {
                    Some(ws) => ws.run_command(cmd, &ctx, &mut self.notices),
                    None => commands::apply_theme(&ctx, cmd),
                }
            }
            if let Some(ws) = &mut self.workspace {
                ui.separator();
                ws.pack_selector_ui(ui, &mut self.notices);
                ui.separator();
                ui.weak(ws.root().display().to_string());
            }
        });
        if pick {
            self.pick_folder(&ctx);
        }
        if let Some(p) = open {
            self.open_project(p, &ctx);
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut open: Option<PathBuf> = None;
        let mut pick = false;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.2);
            ui.heading("Enigma Notes");
            ui.label("Un projet est un dossier de fiches .md (une fiche par fichier).");
            ui.add_space(12.0);
            if ui.button("Ouvrir un dossier…").clicked() {
                pick = true;
            }
            if !self.recent.is_empty() {
                ui.add_space(16.0);
                ui.label("Récents :");
                for p in &self.recent {
                    if ui.link(p.display().to_string()).clicked() {
                        open = Some(p.clone());
                    }
                }
            }
        });
        if pick {
            self.pick_folder(&ctx);
        }
        if let Some(p) = open {
            self.open_project(p, &ctx);
        }
    }
}

impl eframe::App for EnigmaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now_ms = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);

        egui::Panel::top("top_bar").show(ui, |ui| self.top_bar(ui));
        if !self.notices.is_empty() {
            egui::Panel::bottom("notices").show(ui, |ui| self.notices.ui(ui));
        }
        let mut requests = Vec::new();
        match &mut self.workspace {
            Some(ws) => {
                ws.ui(ui, &mut self.notices, now_ms);
                requests = ws.take_requests();
            }
            None => {
                egui::CentralPanel::default_margins().show(ui, |ui| self.welcome(ui));
            }
        }
        for req in requests {
            match req {
                AppRequest::OpenProject => self.pick_folder(&ui.ctx().clone()),
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, RECENT_KEY, &self.recent);
        if let Some(ws) = &mut self.workspace {
            ws.flush(&mut self.notices);
        }
    }

    fn on_exit(&mut self) {
        if let Some(ws) = &mut self.workspace {
            ws.flush(&mut self.notices);
        }
    }
}
