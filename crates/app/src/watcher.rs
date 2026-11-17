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
                        paths.extend(ev.event.paths.iter().cloned());
                    }
                }
                Err(errs) => errors.extend(errs.into_iter().map(|e| e.to_string())),
            }
        }
        (paths, errors)
    }
}
