// Sürüm derlemesinde arka planda konsol penceresi açılmasın
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod gm;
mod piano_roll;
mod player;

use eframe::egui;

fn main() -> eframe::Result<()> {
    // "Birlikte aç" / sürükle-bırak ile exe'ye verilen dosya
    let initial = std::env::args_os().nth(1).map(std::path::PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("WAV2MIDI — Ses → MIDI Dönüştürücü")
            .with_inner_size([1320.0, 820.0])
            .with_min_inner_size([1000.0, 620.0])
            .with_drag_and_drop(true)
            .with_icon(std::sync::Arc::new(app::make_icon())),
        ..Default::default()
    };

    eframe::run_native(
        "WAV2MIDI",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)))),
    )
}
