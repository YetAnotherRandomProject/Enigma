//! Surveillance du dossier du projet (modifications externes des fiches).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui;
use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

pub struct FsWatcher {
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    rx: Receiver<DebounceEventResult>,
    root: PathBuf,
    /// Racine résolue (liens symboliques suivis) : sous macOS, `/var/…`
    /// est un lien vers `/private/var/…` et les événements arrivent avec
    /// le chemin résolu.
    canonical_root: Option<PathBuf>,
}

/// Ramène un chemin signalé sous la racine telle que le projet la connaît.
pub fn under_root(path: &Path, root: &Path, canonical_root: Option<&Path>) -> PathBuf {
    if path.starts_with(root) {
        return path.to_owned();
    }
    match canonical_root.and_then(|c| path.strip_prefix(c).ok()) {
        Some(rel) => root.join(rel),
        None => path.to_owned(),
    }
}

impl FsWatcher {
    /// Surveille `root` récursivement ; chaque lot d'événements réveille l'interface.
    pub fn new(root: &Path, ctx: egui::Context) -> anyhow::Result<FsWatcher> {
        let (tx, rx) = channel();
        let mut debouncer = new_debouncer(
            Duration::from_millis(250),
            None,
            move |res: DebounceEventResult| {
                if tx.send(res).is_ok() {
                    ctx.request_repaint();
                }
            },
        )?;
        debouncer.watch(root, RecursiveMode::Recursive)?;
        Ok(FsWatcher {
            _debouncer: debouncer,
            rx,
            root: root.to_owned(),
            canonical_root: root.canonicalize().ok(),
        })
    }

    /// Chemins touchés depuis le dernier appel (sans doublon), et erreurs.
    pub fn drain(&self) -> (BTreeSet<PathBuf>, Vec<String>) {
        let mut paths = BTreeSet::new();
        let mut errors = Vec::new();
        while let Ok(res) = self.rx.try_recv() {
            match res {
                Ok(events) => {
                    for ev in events {
                        paths.extend(
                            ev.event
                                .paths
                                .iter()
                                .map(|p| under_root(p, &self.root, self.canonical_root.as_deref())),
                        );
                    }
                }
                Err(errs) => errors.extend(errs.into_iter().map(|e| e.to_string())),
            }
        }
        (paths, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_paths_are_mapped_back_to_root() {
        let root = Path::new("/var/folders/x/projet");
        let canon = Path::new("/private/var/folders/x/projet");
        assert_eq!(
            under_root(&canon.join("Salle.md"), root, Some(canon)),
            root.join("Salle.md")
        );
        assert_eq!(
            under_root(&root.join("a.md"), root, Some(canon)),
            root.join("a.md")
        );
        assert_eq!(
            under_root(Path::new("/ailleurs/b.md"), root, Some(canon)),
            Path::new("/ailleurs/b.md")
        );
    }
}
