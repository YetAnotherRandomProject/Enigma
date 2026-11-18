//! Ouvre un fichier avec l'application par défaut du système (lecteur
//! audio/vidéo, visionneuse PDF…). Une commande par OS, sans dépendance.

use std::path::Path;
use std::process::Command;

/// Lance l'application associée au fichier (sans attendre).
pub fn open(path: &Path) -> Result<(), String> {
    let mut cmd = command(path);
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(windows)]
fn command(path: &Path) -> Command {
    // `start` est une commande de cmd.exe ; le premier argument entre
    // guillemets est le titre de la fenêtre.
    let mut c = Command::new("cmd");
    c.args(["/C", "start", ""]).arg(path);
    c
}

#[cfg(target_os = "macos")]
fn command(path: &Path) -> Command {
    let mut c = Command::new("open");
    c.arg(path);
    c
}

#[cfg(all(unix, not(target_os = "macos")))]
fn command(path: &Path) -> Command {
    let mut c = Command::new("xdg-open");
    c.arg(path);
    c
}
