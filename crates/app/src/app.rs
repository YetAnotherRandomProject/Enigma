//! Application eframe : barre du haut, écran d'accueil, projets récents.
//!
//! Un projet est un fichier `.enigma` (voir `session`) ; un ancien dossier
//! de fiches `.md` est proposé à la conversion.

use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, RichText};
use enigma_core::shortcut::Keymap;

use crate::commands::{self, Command};
use crate::notices::Notices;
use crate::paths;
use crate::session::{Session, converted_path, is_project_file};
use crate::update::{self, Updater};
use crate::workspace::AppRequest;

const RECENT_KEY: &str = "recent_projects";
const MAX_RECENT: usize = 8;
const AUTO_UPDATE_KEY: &str = "auto_update";

pub struct EnigmaApp {
    recent: Vec<PathBuf>,
    session: Option<Session>,
    notices: Notices,
    pack_roots: Vec<PathBuf>,
    /// Dossiers de travail des projets ouverts.
    work_root: PathBuf,
    start: Instant,
    keymap: Keymap,
    /// Fichier des raccourcis (`None` : illisible, donc jamais écrasé).
    keymap_path: Option<PathBuf>,
    updater: Updater,
    /// Chercher une mise à jour au démarrage.
    auto_update: bool,
    /// Ancien dossier de fiches dont on propose la conversion.
    convert: Option<PathBuf>,
}

impl EnigmaApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> EnigmaApp {
        egui_extras::install_image_loaders(&cc.egui_ctx);
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
            session: None,
            notices,
            pack_roots: paths::pack_roots(),
            work_root: paths::work_root(),
            start: Instant::now(),
            keymap,
            keymap_path,
            updater: Updater::default(),
            auto_update: cc
                .storage
                .and_then(|s| eframe::get_value(s, AUTO_UPDATE_KEY))
                .unwrap_or(true),
            convert: None,
        };
        // Pas de vérification automatique en développement.
        if app.auto_update && !cfg!(debug_assertions) {
            app.updater.check(&cc.egui_ctx, false);
        }
        // Fichier passé en argument (double-clic sur un `.enigma`), sinon
        // le dernier projet ouvert s'il existe encore.
        let from_args = std::env::args_os().nth(1).map(PathBuf::from);
        let initial = from_args.or_else(|| {
            app.recent
                .first()
                .cloned()
                .filter(|p| p.is_file() && is_project_file(p))
        });
        if let Some(path) = initial {
            app.open_path(path, &cc.egui_ctx);
        }
        app
    }

    /// Ouvre un `.enigma`, ou propose de convertir un dossier.
    fn open_path(&mut self, path: PathBuf, ctx: &egui::Context) {
        if path.is_dir() {
            self.convert = Some(path);
            return;
        }
        self.close_session();
        let session = Session::open(
            &path,
            &self.work_root,
            &self.pack_roots,
            ctx,
            &mut self.notices,
        );
        self.install(session, ctx);
    }

    /// Ferme le projet ouvert (après l'avoir enregistré).
    fn close_session(&mut self) {
        if let Some(s) = self.session.take() {
            // Raccourcis éventuellement modifiés dans ce projet.
            self.keymap = s.ws.keymap().clone();
            s.close(&mut self.notices);
        }
    }

    fn install(&mut self, session: Option<Session>, ctx: &egui::Context) {
        let Some(mut s) = session else {
            return;
        };
        s.ws.set_keymap(self.keymap.clone(), self.keymap_path.clone());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "{} — Enigma Notes",
            s.name()
        )));
        self.remember(s.path());
        self.session = Some(s);
    }

    fn remember(&mut self, path: PathBuf) {
        self.recent.retain(|p| p != &path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
    }

    fn pick_project(&mut self, ctx: &egui::Context) {
        if let Some(file) = rfd::FileDialog::new()
            .set_title("Ouvrir un projet")
            .add_filter("Projet Enigma Notes", &["enigma"])
            .pick_file()
        {
            self.open_path(file, ctx);
        }
    }

    fn new_project(&mut self, ctx: &egui::Context) {
        let Some(mut file) = rfd::FileDialog::new()
            .set_title("Nouveau projet")
            .add_filter("Projet Enigma Notes", &["enigma"])
            .set_file_name("Nouvelle enquête.enigma")
            .save_file()
        else {
            return;
        };
        if !is_project_file(&file) {
            file.set_extension(enigma_core::container::EXTENSION);
        }
        self.close_session();
        let s = Session::create(
            &file,
            &self.work_root,
            &self.pack_roots,
            ctx,
            &mut self.notices,
        );
        self.install(s, ctx);
    }

    fn pick_folder_to_convert(&mut self) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("Dossier de fiches .md à convertir")
            .pick_folder()
        {
            self.convert = Some(dir);
        }
    }

    fn convert_folder(&mut self, folder: &Path, ctx: &egui::Context) {
        let target = converted_path(folder);
        self.close_session();
        let s = Session::convert_folder(
            folder,
            &target,
            &self.work_root,
            &self.pack_roots,
            ctx,
            &mut self.notices,
        );
        if s.is_some() {
            self.recent.retain(|p| p != folder);
        }
        self.install(s, ctx);
    }

    fn handle_request(&mut self, req: AppRequest, ctx: &egui::Context) {
        match req {
            AppRequest::OpenProject => self.pick_project(ctx),
            AppRequest::NewProject => self.new_project(ctx),
            AppRequest::ImportFolder => self.pick_folder_to_convert(),
            AppRequest::AssociateFiles => match crate::file_assoc::associate() {
                Ok(()) => self.notices.info(
                    "Les fichiers .enigma s'ouvrent maintenant avec Enigma Notes \
                     (double-clic dans l'explorateur).",
                ),
                Err(e) => self.notices.error(format!("Association impossible : {e}")),
            },
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut open: Option<PathBuf> = None;
        let mut request = None;
        ui.horizontal(|ui| {
            ui.menu_button("Projet", |ui| {
                for (cmd, req) in [
                    (Command::NewProject, AppRequest::NewProject),
                    (Command::OpenProject, AppRequest::OpenProject),
                    (Command::ImportFolder, AppRequest::ImportFolder),
                ] {
                    if ui.button(cmd.label()).clicked() {
                        request = Some(req);
                        ui.close();
                    }
                }
                ui.menu_button("Récents", |ui| {
                    if self.recent.is_empty() {
                        ui.weak("Aucun");
                    }
                    for p in &self.recent {
                        if ui.button(recent_label(p)).clicked() {
                            open = Some(p.clone());
                            ui.close();
                        }
                    }
                });
                if cfg!(windows) {
                    ui.separator();
                    if ui.button(Command::AssociateFiles.label()).clicked() {
                        request = Some(AppRequest::AssociateFiles);
                        ui.close();
                    }
                }
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
                if let Some(s) = &self.session {
                    ui.separator();
                    for cmd in [Command::Palette, Command::Shortcuts] {
                        let keys = cmd.bindings_text(s.ws.keymap());
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
            self.updater.menu_ui(ui, &mut self.auto_update);
            if let Some(cmd) = run {
                match &mut self.session {
                    Some(s) => s.ws.run_command(cmd, &ctx, &mut self.notices),
                    None => commands::apply_theme(&ctx, cmd),
                }
            }
            if let Some(s) = &mut self.session {
                ui.separator();
                s.ws.pack_selector_ui(ui, &mut self.notices);
                ui.separator();
                ui.weak(s.path().display().to_string());
            }
        });
        if let Some(req) = request {
            self.handle_request(req, &ctx);
        }
        if let Some(p) = open {
            self.open_path(p, &ctx);
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut open: Option<PathBuf> = None;
        let mut request = None;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.18);
            ui.heading("Enigma Notes");
            ui.label(
                "Un projet est un fichier .enigma : fiches, images et réglages réunis \
                 dans un seul fichier.",
            );
            ui.add_space(12.0);
            for (cmd, req) in [
                (Command::NewProject, AppRequest::NewProject),
                (Command::OpenProject, AppRequest::OpenProject),
                (Command::ImportFolder, AppRequest::ImportFolder),
            ] {
                if ui.button(cmd.label()).clicked() {
                    request = Some(req);
                }
            }
            if !self.recent.is_empty() {
                ui.add_space(16.0);
                ui.label("Récents :");
                for p in &self.recent {
                    if ui.link(recent_label(p)).clicked() {
                        open = Some(p.clone());
                    }
                }
            }
        });
        if let Some(req) = request {
            self.handle_request(req, &ctx);
        }
        if let Some(p) = open {
            self.open_path(p, &ctx);
        }
    }

    /// Confirmation de la conversion d'un ancien dossier.
    fn convert_dialog(&mut self, ctx: &egui::Context) {
        let Some(folder) = self.convert.clone() else {
            return;
        };
        let target = converted_path(&folder);
        let mut go = false;
        let mut cancel = false;
        let resp = egui::Modal::new(egui::Id::unique("convert_folder")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading("Convertir le dossier en projet ?");
            ui.label(format!(
                "Les fiches, images et réglages de « {} » seront copiés dans le \
                 nouveau projet « {} ». Le dossier d'origine n'est pas modifié.",
                folder.display(),
                target.display()
            ));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Convertir").strong()).clicked() {
                    go = true;
                }
                if ui.button("Annuler").clicked() {
                    cancel = true;
                }
            });
        });
        if go {
            self.convert = None;
            self.convert_folder(&folder, ctx);
        } else if cancel || resp.should_close() {
            self.convert = None;
        }
    }
}

/// Libellé d'un projet récent (un dossier est à l'ancien format).
fn recent_label(p: &Path) -> String {
    if p.is_dir() {
        format!("{} (dossier, à convertir)", p.display())
    } else {
        p.display().to_string()
    }
}

impl eframe::App for EnigmaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now_ms = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let ctx = ui.ctx().clone();

        egui::Panel::top("top_bar").show(ui, |ui| self.top_bar(ui));
        self.updater.poll();
        if self.updater.wants_banner() {
            let action = egui::Panel::top("update_banner")
                .show(ui, |ui| self.updater.banner_ui(ui))
                .inner;
            if action == update::Action::Restart {
                if let Some(s) = &mut self.session {
                    s.save_now(&mut self.notices);
                }
                match update::restart() {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => self.notices.error(e),
                }
            }
        }
        if !self.notices.is_empty() {
            egui::Panel::bottom("notices").show(ui, |ui| self.notices.ui(ui));
        }
        let mut requests = Vec::new();
        match &mut self.session {
            Some(s) => {
                s.ws.ui(ui, &mut self.notices, now_ms);
                requests = s.ws.take_requests();
                s.tick(&ctx, &mut self.notices);
            }
            None => {
                egui::CentralPanel::default_margins().show(ui, |ui| self.welcome(ui));
            }
        }
        for req in requests {
            self.handle_request(req, &ctx);
        }
        self.convert_dialog(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, RECENT_KEY, &self.recent);
        eframe::set_value(storage, AUTO_UPDATE_KEY, &self.auto_update);
        if let Some(s) = &mut self.session {
            s.save_now(&mut self.notices);
        }
    }

    fn on_exit(&mut self) {
        self.close_session();
    }
}
