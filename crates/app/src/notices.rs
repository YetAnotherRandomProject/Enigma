//! Messages affichés à l'utilisateur (informations et erreurs lisibles).

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};

const INFO_TTL: Duration = Duration::from_secs(6);
const MAX_SHOWN: usize = 4;

pub struct Notice {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

#[derive(Default)]
pub struct Notices(Vec<Notice>);

impl Notices {
    pub fn info(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::info!("{text}");
        self.push(text, false);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        let text = text.into();
        tracing::warn!("{text}");
        self.push(text, true);
    }

    fn push(&mut self, text: String, error: bool) {
        self.0.push(Notice {
            text,
            error,
            at: Instant::now(),
        });
        if self.0.len() > 20 {
            self.0.remove(0);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Bandeau des derniers messages. Les informations disparaissent seules,
    /// les erreurs restent jusqu'à ce qu'on les ferme.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.0.retain(|n| n.error || n.at.elapsed() < INFO_TTL);
        if let Some(next) = self
            .0
            .iter()
            .filter(|n| !n.error)
            .map(|n| INFO_TTL.saturating_sub(n.at.elapsed()))
            .min()
        {
            ui.ctx().request_repaint_after(next);
        }
        let start = self.0.len().saturating_sub(MAX_SHOWN);
        let mut close = None;
        for (i, n) in self.0.iter().enumerate().skip(start) {
            ui.horizontal(|ui| {
                if ui.small_button("✕").on_hover_text("Fermer").clicked() {
                    close = Some(i);
                }
                let color = if n.error {
                    ui.visuals().error_fg_color
                } else {
                    Color32::from_rgb(0x6e, 0xe7, 0xa8)
                };
                ui.label(RichText::new(if n.error { "⚠" } else { "ℹ" }).color(color));
                ui.label(&n.text);
            });
        }
        if let Some(i) = close {
            self.0.remove(i);
        }
    }
}
