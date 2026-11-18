//! Packs de jeu intégrés à l'exécutable : l'application tient en un seul
//! fichier (pratique pour la distribution et la mise à jour automatique).
//!
//! Au démarrage, chaque pack intégré est écrit (s'il a changé) dans un
//! dossier de l'application, ajouté aux racines de packs. Comme ce dossier
//! n'est ni celui du projet ni celui de l'utilisateur, l'éditeur de packs
//! le traite comme « fourni avec l'application » : il n'est jamais modifié
//! sur place (copie dans le projet).

use std::path::{Path, PathBuf};

/// (identifiant, contenu de `pack.json`).
const BUNDLED: &[(&str, &str)] = &[(
    "blue_prince",
    include_str!("../../../packs/blue_prince/pack.json"),
)];

/// Dossier où les packs intégrés sont déposés.
pub fn root() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Enigma Notes")
        .map(|d| d.data_local_dir().join("packs-integres"))
}

/// Écrit les packs intégrés dans `dir` (seulement ceux qui ont changé).
pub fn install(dir: &Path) -> std::io::Result<()> {
    for (id, json) in BUNDLED {
        let path = dir.join(id).join("pack.json");
        if std::fs::read_to_string(&path).is_ok_and(|current| current == *json) {
            continue;
        }
        std::fs::create_dir_all(dir.join(id))?;
        enigma_core::storage::write_atomic(&path, json.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_packs_are_valid_and_installable() {
        for (id, json) in BUNDLED {
            let pack = enigma_core::Pack::from_json(json, Path::new("pack.json")).unwrap();
            assert_eq!(pack.id, *id);
            assert!(pack.problems().is_empty(), "{:?}", pack.problems());
        }
        let d = tempfile::tempdir().unwrap();
        install(d.path()).unwrap();
        install(d.path()).unwrap();
        let found = enigma_core::pack::discover_packs(&[d.path().to_path_buf()]);
        assert_eq!(found.len(), BUNDLED.len());
    }
}
