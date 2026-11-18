//! Enigma Notes : éditeur de notes visuel pour jeux d'énigmes.

// Pas de console en version publiée sous Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod board_view;
mod bundled_packs;
mod clipboard;
mod colors;
mod commands;
mod completion;
mod editor;
mod file_assoc;
mod filter_ui;
mod graph_export;
mod graph_view;
mod highlight;
mod notices;
mod pack_editor;
mod palette;
mod paths;
mod project_file;
mod quick_add;
mod reading;
mod search_ui;
mod session;
mod shortcuts_ui;
mod slash;
mod system_open;
mod theme;
mod update;
mod watcher;
mod workspace;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu=warn,naga=warn".into()),
        )
        .init();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Enigma Notes")
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([640.0, 400.0]),
        wgpu_options: wgpu_options(),
        // Copier/coller géré par l'application (fichiers et images, voir `clipboard`).
        clipboard_shortcuts: false,
        ..Default::default()
    };
    eframe::run_native(
        "Enigma Notes",
        options,
        Box::new(|cc| Ok(Box::new(app::EnigmaApp::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("impossible de démarrer l'interface : {e}"))
}

/// Sous Windows, DirectX 12 est utilisé par défaut plutôt que Vulkan : des
/// couches Vulkan tierces (overlays, ReShade…) peuvent faire planter la
/// création de l'instance Vulkan. La variable `WGPU_BACKEND` reste prioritaire.
#[cfg(windows)]
fn wgpu_options() -> eframe::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuSetup, WgpuSetupCreateNew};
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    if eframe::wgpu::Backends::from_env().is_none() {
        setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
    }
    eframe::WgpuConfiguration {
        wgpu_setup: WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}

#[cfg(not(windows))]
fn wgpu_options() -> eframe::WgpuConfiguration {
    eframe::WgpuConfiguration::default()
}
