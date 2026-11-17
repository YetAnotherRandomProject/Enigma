//! Mise à jour automatique depuis les « Releases » GitHub du dépôt.
//!
//! 1. Vérification (en arrière-plan) : dernière version publiée via l'API
//!    GitHub, comparée à la version compilée (`CARGO_PKG_VERSION`).
//! 2. Si une version plus récente existe, l'utilisateur décide : rien n'est
//!    téléchargé sans son accord.
//! 3. Installation : téléchargement de l'exécutable de la plateforme et de
//!    son empreinte `.sha256`, vérification, remplacement de l'exécutable en
//!    cours (`self-replace`), puis redémarrage.
//!
//! Noms des fichiers publiés (voir `.github/workflows/release.yml`) :
//! `enigma-windows-x86_64.exe`, `enigma-linux-x86_64`, `enigma-macos-aarch64`,
//! `enigma-macos-x86_64`, chacun accompagné de `<nom>.sha256`.

use std::sync::mpsc::{Receiver, channel};

use eframe::egui;
use sha2::{Digest, Sha256};

/// Dépôt GitHub (propriétaire/nom).
pub const REPO: &str = "YetAnotherRandomProject/Enigma";
/// Version de cet exécutable.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Taille maximale acceptée pour un exécutable téléchargé.
const MAX_DOWNLOAD: u64 = 300 * 1024 * 1024;
/// Identification générique auprès de l'API GitHub (obligatoire).
const USER_AGENT: &str = "enigma-notes-updater";

/// Nom du fichier publié pour la plateforme courante.
pub fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("enigma-windows-x86_64.exe"),
        ("linux", "x86_64") => Some("enigma-linux-x86_64"),
        ("macos", "aarch64") => Some("enigma-macos-aarch64"),
        ("macos", "x86_64") => Some("enigma-macos-x86_64"),
        _ => None,
    }
}

/// `v1.2.3` ou `1.2.3` (suffixe `-beta…` ignoré) → (1, 2, 3).
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let v = (
        it.next()??,
        it.next().flatten().unwrap_or(0),
        it.next().flatten().unwrap_or(0),
    );
    Some(v)
}

/// `candidate` est-elle strictement plus récente que `current` ?
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(c), Some(k)) => c > k,
        _ => false,
    }
}

/// Version publiée plus récente que celle en cours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub page_url: String,
    pub notes: String,
    pub asset_url: String,
    pub checksum_url: String,
}

/// Extrait la version utile de la réponse de l'API `releases/latest`.
/// `Ok(None)` : pas plus récente que `current`.
pub fn parse_latest(json: &str, current: &str, asset: &str) -> Result<Option<Release>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("réponse illisible : {e}"))?;
    let tag = v["tag_name"]
        .as_str()
        .ok_or("réponse sans numéro de version")?;
    if !is_newer(tag, current) {
        return Ok(None);
    }
    let assets = v["assets"].as_array().ok_or("réponse sans fichiers")?;
    let url_of = |name: &str| {
        assets
            .iter()
            .find(|a| a["name"].as_str() == Some(name))
            .and_then(|a| a["browser_download_url"].as_str())
            .map(str::to_owned)
    };
    let asset_url =
        url_of(asset).ok_or_else(|| format!("la version {tag} ne contient pas « {asset} »"))?;
    let checksum_url = url_of(&format!("{asset}.sha256"))
        .ok_or_else(|| format!("la version {tag} n'a pas d'empreinte pour « {asset} »"))?;
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_owned(),
        page_url: v["html_url"].as_str().unwrap_or_default().to_owned(),
        notes: v["body"].as_str().unwrap_or_default().to_owned(),
        asset_url,
        checksum_url,
    }))
}

/// Vérifie qu'un fichier `.sha256` (« <hex>  nom » ou « <hex> ») correspond
/// aux octets téléchargés.
pub fn checksum_matches(bytes: &[u8], checksum_file: &str) -> bool {
    let Some(expected) = checksum_file.split_whitespace().next() else {
        return false;
    };
    let actual = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    expected.eq_ignore_ascii_case(&actual)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .build()
        .into()
}

fn check_blocking() -> Result<Option<Release>, String> {
    let asset = asset_name().ok_or("mise à jour automatique indisponible sur cette plateforme")?;
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut resp = agent()
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => "aucune version publiée pour l'instant".to_owned(),
            e => format!("connexion à GitHub impossible : {e}"),
        })?;
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("réponse de GitHub illisible : {e}"))?;
    parse_latest(&body, CURRENT, asset)
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut resp = agent()
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("téléchargement impossible : {e}"))?;
    resp.body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| format!("téléchargement interrompu : {e}"))
}

fn install_blocking(release: &Release) -> Result<(), String> {
    let bytes = download(&release.asset_url, MAX_DOWNLOAD)?;
    let checksum = download(&release.checksum_url, 4096)?;
    let checksum = String::from_utf8_lossy(&checksum);
    if !checksum_matches(&bytes, &checksum) {
        return Err("empreinte SHA-256 incorrecte : fichier refusé".to_owned());
    }
    let exe = std::env::current_exe().map_err(|e| format!("exécutable introuvable : {e}"))?;
    let dir = exe
        .parent()
        .ok_or("dossier de l'exécutable introuvable")?
        .to_path_buf();
    // Fichier temporaire dans le même dossier (même disque) puis remplacement.
    let tmp = tempfile::Builder::new()
        .prefix(".enigma-maj-")
        .tempfile_in(&dir)
        .map_err(|e| format!("écriture impossible dans {} : {e}", dir.display()))?;
    std::fs::write(tmp.path(), &bytes).map_err(|e| format!("écriture impossible : {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("droits d'exécution impossibles : {e}"))?;
    }
    self_replace::self_replace(tmp.path())
        .map_err(|e| format!("remplacement de l'exécutable impossible : {e}"))?;
    Ok(())
}

/// Relance l'application (après installation), avec les mêmes arguments.
pub fn restart() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("redémarrage impossible : {e}"))
}

/// État de la mise à jour, affiché dans l'interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Installing(Release),
    /// Installée : il reste à redémarrer.
    Installed(String),
    Failed(String),
}

enum Msg {
    Checked(Result<Option<Release>, String>),
    Installed(Result<String, String>),
}

/// Vérifications et installations en arrière-plan (l'interface ne se fige
/// jamais pendant les échanges réseau).
pub struct Updater {
    pub status: Status,
    /// La vérification a été lancée par l'utilisateur (afficher aussi
    /// « à jour » et les erreurs).
    pub manual: bool,
    /// Le bandeau a été fermé (« Plus tard »).
    pub dismissed: bool,
    rx: Option<Receiver<Msg>>,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            manual: false,
            dismissed: false,
            rx: None,
        }
    }
}

impl Updater {
    pub fn busy(&self) -> bool {
        matches!(self.status, Status::Checking | Status::Installing(_))
    }

    pub fn check(&mut self, ctx: &egui::Context, manual: bool) {
        if self.busy() {
            return;
        }
        self.manual = manual;
        self.dismissed = false;
        self.status = Status::Checking;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Checked(check_blocking()));
            ctx.request_repaint();
        });
    }

    pub fn install(&mut self, ctx: &egui::Context) {
        let Status::Available(release) = self.status.clone() else {
            return;
        };
        self.status = Status::Installing(release.clone());
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = install_blocking(&release).map(|()| release.version.clone());
            let _ = tx.send(Msg::Installed(result));
            ctx.request_repaint();
        });
    }

    /// Récupère les résultats des tâches en arrière-plan.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else {
            return;
        };
        let Ok(msg) = rx.try_recv() else {
            return;
        };
        self.rx = None;
        self.status = match msg {
            Msg::Checked(Ok(Some(r))) => Status::Available(r),
            Msg::Checked(Ok(None)) => Status::UpToDate,
            Msg::Checked(Err(e)) | Msg::Installed(Err(e)) => Status::Failed(e),
            Msg::Installed(Ok(v)) => Status::Installed(v),
        };
        if let Status::Failed(e) = &self.status {
            tracing::warn!("mise à jour : {e}");
        }
    }
}

/// Action demandée depuis l'interface de mise à jour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Enregistrer puis relancer l'application (mise à jour installée).
    Restart,
}

impl Updater {
    /// Menu « Aide » : version, vérification manuelle, réglage automatique.
    pub fn menu_ui(&mut self, ui: &mut egui::Ui, auto_check: &mut bool) {
        ui.menu_button("Aide", |ui| {
            ui.weak(format!("Enigma Notes {CURRENT}"));
            ui.separator();
            let label = if self.busy() {
                "Vérification en cours…"
            } else {
                "Vérifier les mises à jour"
            };
            if ui
                .add_enabled(!self.busy(), egui::Button::new(label))
                .clicked()
            {
                self.check(ui.ctx(), true);
                ui.close();
            }
            ui.checkbox(auto_check, "Vérifier au démarrage");
            if ui.button("Page du projet sur GitHub").clicked() {
                ui.ctx()
                    .open_url(egui::OpenUrl::new_tab(format!("https://github.com/{REPO}")));
                ui.close();
            }
        });
    }

    /// Le bandeau doit-il être affiché ?
    pub fn wants_banner(&self) -> bool {
        if self.dismissed {
            return false;
        }
        match self.status {
            Status::Idle => false,
            Status::Checking | Status::UpToDate | Status::Failed(_) => self.manual,
            Status::Available(_) | Status::Installing(_) | Status::Installed(_) => true,
        }
    }

    /// Bandeau d'information (en haut de la fenêtre).
    pub fn banner_ui(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        ui.horizontal_wrapped(|ui| match self.status.clone() {
            Status::Idle => {}
            Status::Checking => {
                ui.spinner();
                ui.label("Recherche d'une mise à jour…");
            }
            Status::UpToDate => {
                ui.label(format!("Enigma Notes {CURRENT} est à jour."));
                if ui.small_button("OK").clicked() {
                    self.dismissed = true;
                }
            }
            Status::Failed(e) => {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Mise à jour impossible : {e}"),
                );
                if ui.small_button("Fermer").clicked() {
                    self.dismissed = true;
                }
            }
            Status::Available(r) => {
                ui.strong(format!("Version {} disponible", r.version));
                ui.weak(format!("(vous avez la {CURRENT})"));
                if ui.button("Mettre à jour et redémarrer").clicked() {
                    self.install(ui.ctx());
                }
                if !r.page_url.is_empty() && ui.button("Nouveautés").clicked() {
                    ui.ctx()
                        .open_url(egui::OpenUrl::new_tab(r.page_url.clone()));
                }
                if ui.button("Plus tard").clicked() {
                    self.dismissed = true;
                }
            }
            Status::Installing(r) => {
                ui.spinner();
                ui.label(format!("Téléchargement de la version {}…", r.version));
            }
            Status::Installed(v) => {
                ui.label(format!("Version {v} installée."));
                if ui.button("Redémarrer maintenant").clicked() {
                    action = Action::Restart;
                }
                ui.weak("(sinon elle sera utilisée au prochain lancement)");
            }
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.4"), Some((0, 4, 0)));
        assert_eq!(parse_version("2.0.0-beta.1"), Some((2, 0, 0)));
        assert_eq!(parse_version("abc"), None);
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("n'importe", "0.1.0"));
        assert!(
            asset_name().is_some(),
            "plateforme de développement prise en charge"
        );
    }

    fn api(tag: &str, with_checksum: bool) -> String {
        let mut assets = vec![serde_json::json!({
            "name": "enigma-windows-x86_64.exe",
            "browser_download_url": "https://example.invalid/enigma.exe"
        })];
        if with_checksum {
            assets.push(serde_json::json!({
                "name": "enigma-windows-x86_64.exe.sha256",
                "browser_download_url": "https://example.invalid/enigma.exe.sha256"
            }));
        }
        serde_json::json!({
            "tag_name": tag,
            "html_url": "https://github.com/x/y/releases/tag/v9",
            "body": "Nouveautés",
            "assets": assets,
        })
        .to_string()
    }

    #[test]
    fn latest_release_parsing() {
        let asset = "enigma-windows-x86_64.exe";
        let r = parse_latest(&api("v9.0.0", true), "0.1.0", asset)
            .unwrap()
            .unwrap();
        assert_eq!(r.version, "9.0.0");
        assert!(r.checksum_url.ends_with(".sha256"));
        assert_eq!(r.notes, "Nouveautés");
        assert_eq!(parse_latest(&api("v0.1.0", true), "0.1.0", asset), Ok(None));
        let err = parse_latest(&api("v9.0.0", false), "0.1.0", asset).unwrap_err();
        assert!(err.contains("empreinte"), "{err}");
        let err = parse_latest(&api("v9.0.0", true), "0.1.0", "enigma-linux-x86_64").unwrap_err();
        assert!(err.contains("ne contient pas"), "{err}");
        assert!(parse_latest("pas du json", "0.1.0", asset).is_err());
    }

    #[test]
    fn checksum_check() {
        // SHA-256 de « abc ».
        let hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(checksum_matches(b"abc", &format!("{hex}  enigma.exe\n")));
        assert!(checksum_matches(b"abc", &hex.to_uppercase()));
        assert!(!checksum_matches(b"abd", hex));
        assert!(!checksum_matches(b"abc", ""));
    }
}
