//! Association des fichiers `.enigma` à l'application (double-clic dans
//! l'explorateur).
//!
//! Windows : clés de l'utilisateur courant (`HKCU\Software\Classes`, sans
//! droits administrateur), écrites avec `reg.exe` fourni par le système.
//! macOS et Linux : non pris en charge pour l'instant (sur macOS il faut un
//! paquet `.app` avec `Info.plist`, sur Linux un fichier `.desktop` et un
//! type MIME) ; la commande y est masquée.

use std::path::Path;

/// ProgID Windows des projets.
#[cfg_attr(not(windows), allow(dead_code))] // utilisé seulement sous Windows (et par les tests)
pub const PROG_ID: &str = "EnigmaNotes.Projet";

/// Clés et valeurs par défaut à écrire pour associer `.enigma` à `exe`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn registry_entries(exe: &Path) -> Vec<(String, String)> {
    let exe = exe.display().to_string();
    let base = r"HKCU\Software\Classes";
    vec![
        (format!(r"{base}\.enigma"), PROG_ID.to_owned()),
        (
            format!(r"{base}\{PROG_ID}"),
            "Projet Enigma Notes".to_owned(),
        ),
        (
            format!(r"{base}\{PROG_ID}\DefaultIcon"),
            format!("\"{exe}\",0"),
        ),
        (
            format!(r"{base}\{PROG_ID}\shell\open\command"),
            format!("\"{exe}\" \"%1\""),
        ),
    ]
}

/// Associe les fichiers `.enigma` à l'exécutable courant.
#[cfg(windows)]
pub fn associate() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("exécutable introuvable : {e}"))?;
    for (key, value) in registry_entries(&exe) {
        let out = std::process::Command::new("reg")
            .args(["add", &key, "/ve", "/d", &value, "/f"])
            .output()
            .map_err(|e| format!("reg.exe : {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "écriture de {key} refusée : {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn associate() -> Result<(), String> {
    Err("association de fichiers disponible seulement sous Windows pour l'instant".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_point_to_exe() {
        let e = registry_entries(Path::new(r"C:\Apps\Enigma\enigma.exe"));
        assert_eq!(
            e[0],
            (
                r"HKCU\Software\Classes\.enigma".to_owned(),
                PROG_ID.to_owned()
            )
        );
        assert_eq!(
            e[3].1, r#""C:\Apps\Enigma\enigma.exe" "%1""#,
            "commande d'ouverture"
        );
        assert!(e[3].0.ends_with(r"shell\open\command"));
    }
}
