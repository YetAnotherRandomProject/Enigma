//! Emplacements où chercher les packs de jeu.

use std::path::PathBuf;

/// Dossiers de packs, par ordre de priorité (le dossier `.enigma/packs` du
/// projet passe encore avant, voir `Project::available_packs`) :
/// 1. dossier utilisateur (ex. `%APPDATA%\Enigma Notes\data\packs`) ;
/// 2. `packs/` à côté de l'exécutable ;
/// 3. `packs/` du dossier courant ;
/// 4. en développement, `packs/` du dépôt.
///
/// Aucun pack n'est intégré à l'exécutable : ceux du dépôt officiel sont
/// proposés puis installés dans le dossier utilisateur (voir `pack_catalog`).
pub fn pack_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(dirs) = directories::ProjectDirs::from("", "", "Enigma Notes") {
        roots.push(dirs.data_dir().join("packs"));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("packs")))
    {
        roots.push(dir);
    }
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd.join("packs"));
    }
    #[cfg(debug_assertions)]
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packs"));
    let mut seen = std::collections::HashSet::new();
    roots.retain(|r| seen.insert(r.clone()));
    roots
}

/// Fichier des raccourcis clavier de l'utilisateur
/// (ex. `%APPDATA%\Enigma Notes\config\raccourcis.json`).
pub fn keymap_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Enigma Notes")
        .map(|d| d.config_dir().join("raccourcis.json"))
}

/// Dossier des dossiers de travail des projets ouverts (cache de
/// l'utilisateur ; un sous-dossier par projet ouvert, supprimé à la
/// fermeture).
pub fn work_root() -> PathBuf {
    directories::ProjectDirs::from("", "", "Enigma Notes")
        .map(|d| d.cache_dir().join("projets-ouverts"))
        .unwrap_or_else(|| std::env::temp_dir().join("enigma-notes-projets"))
}
