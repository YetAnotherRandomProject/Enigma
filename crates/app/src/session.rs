//! Projet ouvert : fichier `.enigma` (voir `project_file`) et espace de
//! travail qui l'affiche. Ouverture, création, conversion d'un ancien
//! dossier de fiches, enregistrement en arrière-plan, fermeture.

use std::path::{Path, PathBuf};

use eframe::egui;
use enigma_core::container;
use enigma_core::import::import_folder;

use crate::notices::Notices;
use crate::project_file::{Commit, ProjectFile};
use crate::workspace::Workspace;

pub struct Session {
    pub ws: Workspace,
    file: ProjectFile,
}

/// Le chemin désigne-t-il un projet `.enigma` ?
pub fn is_project_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(container::EXTENSION))
}

/// Fichier `.enigma` proposé pour convertir un dossier : `Dossier.enigma`
/// à côté de lui (numéroté s'il existe déjà).
pub fn converted_path(folder: &Path) -> PathBuf {
    let name = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Projet".to_owned());
    let dir = folder.parent().unwrap_or(Path::new("."));
    let ext = container::EXTENSION;
    let first = dir.join(format!("{name}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|k| dir.join(format!("{name} {k}.{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

impl Session {
    fn start(
        file: ProjectFile,
        pack_roots: &[PathBuf],
        ctx: &egui::Context,
        notices: &mut Notices,
        new_project: bool,
    ) -> Option<Session> {
        for r in &file.recovered {
            notices.error(format!(
                "Des modifications d'une session interrompue ont été sauvées dans « {} ».",
                r.display()
            ));
        }
        match Workspace::open(file.workdir(), pack_roots, ctx, notices) {
            Ok(mut ws) => {
                if new_project && ws.project.config().pack.is_none() {
                    // Nouveau projet : le pack Blue Prince s'il est là, sinon le premier.
                    let packs: Vec<String> = ws
                        .project
                        .available_packs()
                        .into_iter()
                        .filter_map(|d| d.result.ok().map(|p| p.id))
                        .collect();
                    let choice = packs
                        .iter()
                        .find(|id| *id == "blue_prince")
                        .or(packs.first())
                        .cloned();
                    let meta = ws.project.root().join(enigma_core::project::META_DIR);
                    let _ = std::fs::create_dir_all(&meta);
                    if let Some(id) = choice
                        && let Err(e) = ws.project.set_pack(Some(id))
                    {
                        notices.error(format!("Pack de jeu : {e}"));
                    }
                }
                Some(Session { ws, file })
            }
            Err(e) => {
                notices.error(format!("Impossible d'ouvrir le projet : {e}"));
                file.close();
                None
            }
        }
    }

    /// Ouvre un fichier `.enigma`.
    pub fn open(
        path: &Path,
        work_root: &Path,
        pack_roots: &[PathBuf],
        ctx: &egui::Context,
        notices: &mut Notices,
    ) -> Option<Session> {
        match ProjectFile::open(path, work_root) {
            Ok(file) => Session::start(file, pack_roots, ctx, notices, false),
            Err(e) => {
                notices.error(format!("Impossible d'ouvrir « {} » : {e}", path.display()));
                None
            }
        }
    }

    /// Crée un nouveau projet vide.
    pub fn create(
        path: &Path,
        work_root: &Path,
        pack_roots: &[PathBuf],
        ctx: &egui::Context,
        notices: &mut Notices,
    ) -> Option<Session> {
        match ProjectFile::create(path, work_root, |_| Ok(())) {
            Ok(file) => {
                let mut s = Session::start(file, pack_roots, ctx, notices, true)?;
                s.save_now(notices);
                Some(s)
            }
            Err(e) => {
                notices.error(format!("Création impossible : {e}"));
                None
            }
        }
    }

    /// Convertit un dossier de fiches `.md` en projet `.enigma` (le dossier
    /// n'est pas modifié).
    pub fn convert_folder(
        folder: &Path,
        target: &Path,
        work_root: &Path,
        pack_roots: &[PathBuf],
        ctx: &egui::Context,
        notices: &mut Notices,
    ) -> Option<Session> {
        let mut report = None;
        let created = ProjectFile::create(target, work_root, |dir| {
            report = Some(import_folder(folder, dir).map_err(|e| e.to_string())?);
            Ok(())
        });
        match created {
            Ok(file) => {
                if let Some(r) = report {
                    let mut msg = format!(
                        "Dossier converti en « {} » : {} fiche(s), {} image(s).",
                        target.display(),
                        r.notes.len(),
                        r.images
                    );
                    if !r.missing.is_empty() {
                        msg.push_str(&format!(" Images introuvables : {}.", r.missing.join(", ")));
                    }
                    notices.info(msg);
                }
                Session::start(file, pack_roots, ctx, notices, true)
            }
            Err(e) => {
                notices.error(format!("Conversion impossible : {e}"));
                None
            }
        }
    }

    /// Fichier `.enigma` du projet.
    pub fn path(&self) -> PathBuf {
        self.file.file()
    }

    pub fn name(&self) -> String {
        self.file.name()
    }

    /// À appeler après chaque image : lance l'enregistrement en arrière-plan
    /// si le projet a changé, et signale le résultat.
    pub fn tick(&mut self, ctx: &egui::Context, notices: &mut Notices) {
        if self.ws.take_disk_changed() {
            self.file.request_commit();
        }
        match self.file.poll() {
            Some(result) => report(result, notices),
            None => {
                if self.file.busy() {
                    ctx.request_repaint_after(std::time::Duration::from_millis(30));
                }
            }
        }
    }

    /// Enregistre tout maintenant (fiches puis fichier `.enigma`).
    pub fn save_now(&mut self, notices: &mut Notices) {
        self.ws.flush(notices);
        let _ = self.ws.take_disk_changed();
        report(self.file.commit(), notices);
    }

    /// Enregistre et ferme (supprime le dossier de travail).
    pub fn close(mut self, notices: &mut Notices) {
        self.save_now(notices);
        self.file.close();
    }
}

fn report(result: Result<Commit, String>, notices: &mut Notices) {
    match result {
        Ok(Commit::Unchanged | Commit::Written) => {}
        Ok(Commit::Conflict(path)) => notices.error(format!(
            "Le fichier du projet a été modifié par ailleurs : vos modifications sont \
             enregistrées dans « {} », qui devient le fichier du projet.",
            path.display()
        )),
        Err(e) => notices.error(format!("Enregistrement du projet impossible : {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[derive(Default)]
    struct State {
        session: Option<Session>,
        notices: Notices,
    }

    fn snapshot(harness: &mut Harness<'_, State>, name: &str) {
        let Ok(dir) = std::env::var("ENIGMA_SNAPSHOT_DIR") else {
            return;
        };
        if let Ok(img) = harness.render() {
            img.save(Path::new(&dir).join(format!("{name}.png")))
                .unwrap();
        }
    }

    fn harness() -> Harness<'static, State> {
        let h = Harness::builder()
            .with_size([1200.0, 760.0])
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, state: &mut State| {
                    if let Some(s) = &mut state.session {
                        s.ws.ui(ui, &mut state.notices, 0);
                        let ctx = ui.ctx().clone();
                        s.tick(&ctx, &mut state.notices);
                    }
                },
                State::default(),
            );
        egui_extras::install_image_loaders(&h.ctx);
        h
    }

    /// Attend (en images) qu'une condition soit vraie.
    fn wait(harness: &mut Harness<'_, State>, mut ok: impl FnMut(&mut Harness<'_, State>) -> bool) {
        for _ in 0..200 {
            harness.run_steps(2);
            if ok(harness) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("condition jamais remplie");
    }

    #[test]
    fn convert_open_insert_svg_resize_and_reopen() {
        let d = tempfile::tempdir().unwrap();
        let work = d.path().join("work");
        let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../exemples/blue_prince_demo");
        let packs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")];
        let file = d.path().join("Démo.enigma");
        let svg = d.path().join("plan.svg");
        std::fs::write(
            &svg,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="200" height="100" fill="#4aa3ff"/><circle cx="100" cy="50" r="40" fill="#ffd23f"/></svg>"##,
        )
        .unwrap();

        let mut harness = harness();
        let ctx = harness.ctx.clone();
        {
            let state = harness.state_mut();
            state.session =
                Session::convert_folder(&demo, &file, &work, &packs, &ctx, &mut state.notices);
            assert!(state.session.is_some(), "conversion");
            state.session.as_mut().unwrap().ws.group_by_category = false;
        }
        assert!(file.is_file());
        harness.run_steps(8);
        harness.get_by_label("Salle des Archives").click();
        harness.run_steps(8);

        // Insertion de l'image SVG (copiée dans assets/).
        {
            let state = harness.state_mut();
            let s = state.session.as_mut().unwrap();
            s.ws.insert_image(&svg, &mut state.notices);
            let body = s.ws.editor_body().unwrap();
            assert!(body.contains("![plan](assets/plan.svg)"), "{body}");
        }

        // Mode Lecture : l'image est chargée (rendu SVG), puis redimensionnée.
        harness.state_mut().session.as_mut().unwrap().ws.reading = true;
        wait(&mut harness, |h| h.query_by_label("plan").is_some());
        snapshot(&mut harness, "20_lecture_svg");
        let rect = harness.get_by_label("plan").rect();
        assert!(
            (rect.width() - 200.0).abs() < 1.0,
            "taille d'origine : {rect:?}"
        );
        let from = rect.right_bottom() - egui::vec2(4.0, 4.0);
        let to = from + egui::vec2(120.0, 0.0);
        harness.hover_at(from);
        harness.run_steps(2);
        harness.drag_at(from);
        harness.run_steps(1);
        for k in 1..=6 {
            harness.hover_at(from + (to - from) * (k as f32 / 6.0));
            harness.run_steps(1);
        }
        harness.drop_at(to);
        harness.run_steps(4);
        let body = harness
            .state()
            .session
            .as_ref()
            .unwrap()
            .ws
            .editor_body()
            .unwrap()
            .to_owned();
        let width: u32 = body
            .split("![plan|")
            .nth(1)
            .and_then(|r| r.split(']').next())
            .and_then(|w| w.parse().ok())
            .unwrap_or_else(|| panic!("largeur écrite : {body}"));
        assert!((310..=330).contains(&width), "largeur {width}");
        snapshot(&mut harness, "21_image_redimensionnee");

        // Ctrl+Z revient à la taille d'origine.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        harness.run_steps(3);
        let body = harness
            .state()
            .session
            .as_ref()
            .unwrap()
            .ws
            .editor_body()
            .unwrap()
            .to_owned();
        assert!(body.contains("![plan](assets/plan.svg)"), "{body}");
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Y);
        harness.run_steps(3);

        // Enregistrement puis réouverture du .enigma.
        let state = harness.state_mut();
        let session = state.session.take().unwrap();
        session.close(&mut state.notices);
        let entries = container::read_file(&file).unwrap();
        assert!(entries.iter().any(|e| e.path == "assets/plan.svg"));
        let salle = entries
            .iter()
            .find(|e| e.path == "Salle des Archives.md")
            .unwrap();
        let text = String::from_utf8_lossy(&salle.data);
        assert!(
            text.contains(&format!("![plan|{width}](assets/plan.svg)")),
            "{text}"
        );
        let reopened = Session::open(&file, &work, &packs, &ctx, &mut state.notices).unwrap();
        assert!(reopened.ws.project.resolve_title("Majordome").is_some());
        reopened.close(&mut state.notices);
        assert!(
            std::fs::read_dir(&work).unwrap().next().is_none(),
            "dossier de travail supprimé"
        );
    }

    #[test]
    fn converted_names() {
        let d = tempfile::tempdir().unwrap();
        let folder = d.path().join("Blue Prince");
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(converted_path(&folder), d.path().join("Blue Prince.enigma"));
        std::fs::write(d.path().join("Blue Prince.enigma"), "x").unwrap();
        assert_eq!(
            converted_path(&folder),
            d.path().join("Blue Prince 2.enigma")
        );
        assert!(is_project_file(Path::new("a/B.ENIGMA")));
        assert!(!is_project_file(Path::new("a/B")));
    }
}
