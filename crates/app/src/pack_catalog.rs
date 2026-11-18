//! Catalogue des packs de jeu du dépôt officiel (dossier `packs/`).
//!
//! Les packs ne sont plus intégrés à l'exécutable : l'application interroge
//! le dépôt (API GitHub `contents/packs`, puis `pack.json` de chaque
//! dossier), valide chaque pack avec les mêmes règles qu'un pack local, et
//! le propose. **Rien n'est installé sans l'accord de l'utilisateur** ; un
//! pack installé est écrit dans le dossier de packs de l'utilisateur.
//!
//! La recherche automatique suit le réglage de mise à jour (désactivable,
//! jamais en développement) ; le bouton « Chercher des packs en ligne »
//! fonctionne toujours.

use std::path::Path;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui;
use enigma_core::Pack;

use crate::update::REPO;

/// Taille maximale d'un `pack.json` téléchargé.
const MAX_PACK_BYTES: u64 = 256 * 1024;
/// Nombre maximal de packs examinés.
const MAX_PACKS: usize = 40;
const USER_AGENT: &str = "enigma-notes-updater";

/// Un pack proposé par le dépôt, déjà validé.
#[derive(Clone, Debug, PartialEq)]
pub struct OnlinePack {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Contenu de `pack.json`, tel que publié.
    pub json: String,
}

/// Identifiants de dossiers de packs dans la réponse de l'API (`type: dir`),
/// limités à des noms sûrs (ils deviennent des noms de dossier).
pub fn parse_listing(json: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("réponse illisible : {e}"))?;
    let items = v.as_array().ok_or("réponse inattendue")?;
    Ok(items
        .iter()
        .filter(|i| i["type"].as_str() == Some("dir"))
        .filter_map(|i| i["name"].as_str())
        .filter(|n| safe_id(n))
        .map(str::to_owned)
        .take(MAX_PACKS)
        .collect())
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 60
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Valide un `pack.json` téléchargé pour le dossier `id`.
pub fn validate(id: &str, json: &str) -> Result<OnlinePack, String> {
    let pack = Pack::from_json(json, Path::new("pack.json")).map_err(|e| e.to_string())?;
    if pack.id != id {
        return Err(format!(
            "l'identifiant « {} » ne correspond pas au dossier « {id} »",
            pack.id
        ));
    }
    let problems = pack.problems();
    if !problems.is_empty() {
        return Err(problems.join(" ; "));
    }
    Ok(OnlinePack {
        id: pack.id,
        name: pack.name,
        description: pack.description,
        json: json.to_owned(),
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into()
}

fn get_text(url: &str, limit: u64) -> Result<String, String> {
    let mut resp = agent()
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("connexion impossible : {e}"))?;
    resp.body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .map_err(|e| format!("réponse illisible : {e}"))
}

fn fetch_blocking() -> Result<Vec<OnlinePack>, String> {
    let listing = get_text(
        &format!("https://api.github.com/repos/{REPO}/contents/packs"),
        1024 * 1024,
    )?;
    let mut out = Vec::new();
    for id in parse_listing(&listing)? {
        let url = format!("https://raw.githubusercontent.com/{REPO}/HEAD/packs/{id}/pack.json");
        match get_text(&url, MAX_PACK_BYTES).and_then(|json| validate(&id, &json)) {
            Ok(p) => out.push(p),
            Err(e) => tracing::warn!("pack en ligne « {id} » ignoré : {e}"),
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Loading,
    Ready,
    Failed(String),
}

/// Recherche en arrière-plan (l'interface ne se fige jamais).
pub struct PackCatalog {
    pub state: State,
    pub packs: Vec<OnlinePack>,
    /// Suggestion automatique fermée (« Plus tard »).
    pub dismissed: bool,
    rx: Option<Receiver<Result<Vec<OnlinePack>, String>>>,
}

impl Default for PackCatalog {
    fn default() -> Self {
        Self {
            state: State::Idle,
            packs: Vec::new(),
            dismissed: false,
            rx: None,
        }
    }
}

impl PackCatalog {
    pub fn fetch(&mut self, ctx: &egui::Context) {
        if self.state == State::Loading {
            return;
        }
        self.state = State::Loading;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_blocking());
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else {
            return;
        };
        let Ok(result) = rx.try_recv() else {
            return;
        };
        self.rx = None;
        match result {
            Ok(packs) => {
                self.packs = packs;
                self.state = State::Ready;
            }
            Err(e) => {
                tracing::warn!("catalogue de packs : {e}");
                self.state = State::Failed(e);
            }
        }
    }

    /// Packs du dépôt pas encore installés ; `wanted` (le pack que le projet
    /// réclame) passe en premier, et seul s'il est dans le catalogue.
    pub fn suggestions(&self, installed: &[String], wanted: Option<&str>) -> Vec<&OnlinePack> {
        let missing = self.packs.iter().filter(|p| !installed.contains(&p.id));
        match wanted {
            Some(w) if self.packs.iter().any(|p| p.id == w) => {
                missing.filter(|p| p.id == w).collect()
            }
            _ => missing.collect(),
        }
    }
}

/// Écrit un pack dans le dossier de packs de l'utilisateur.
pub fn install(pack: &OnlinePack, user_dir: &Path) -> std::io::Result<()> {
    if !safe_id(&pack.id) {
        return Err(std::io::Error::other("identifiant de pack invalide"));
    }
    let dir = user_dir.join(&pack.id);
    std::fs::create_dir_all(&dir)?;
    enigma_core::storage::write_atomic(&dir.join("pack.json"), pack.json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r##"{ "version": 1, "id": "essai", "name": "Essai",
        "categories": [ { "id": "a", "label": "A", "color": "#112233" } ] }"##;

    #[test]
    fn listing_keeps_only_safe_directories() {
        let json = r#"[
            {"name":"blue_prince","type":"dir"},
            {"name":"README.md","type":"file"},
            {"name":"../evil","type":"dir"},
            {"name":"Autre Pack","type":"dir"},
            {"name":"jeu-2","type":"dir"}
        ]"#;
        assert_eq!(parse_listing(json).unwrap(), ["blue_prince", "jeu-2"]);
        assert!(parse_listing("{}").is_err());
        assert!(parse_listing("pas du json").is_err());
    }

    #[test]
    fn validates_packs_like_local_ones() {
        assert_eq!(validate("essai", GOOD).unwrap().name, "Essai");
        assert!(
            validate("autre", GOOD)
                .unwrap_err()
                .contains("ne correspond pas")
        );
        assert!(validate("essai", r#"{"version":1,"id":"essai"}"#).is_err());
        assert!(validate("essai", r#"{"version":1,"id":"essai","name":"E","x":1}"#).is_err());
    }

    #[test]
    fn suggests_missing_packs_and_installs_them() {
        let c = PackCatalog {
            packs: vec![validate("essai", GOOD).unwrap()],
            ..Default::default()
        };
        assert_eq!(c.suggestions(&[], None).len(), 1);
        assert!(c.suggestions(&["essai".into()], None).is_empty());
        assert_eq!(c.suggestions(&[], Some("essai")).len(), 1);
        // Pack réclamé absent du catalogue : on propose tout le catalogue.
        assert_eq!(c.suggestions(&[], Some("inconnu")).len(), 1);

        let d = tempfile::tempdir().unwrap();
        install(&c.packs[0], d.path()).unwrap();
        let found = enigma_core::pack::discover_packs(&[d.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert!(found[0].result.is_ok());
    }
}
