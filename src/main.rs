#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use sqlite_editor::app::SqliteEditorApp;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1150.0, 750.0])
            .with_min_inner_size([700.0, 450.0])
            .with_title("Rust SQLite Editor"),
        ..Default::default()
    };

    eframe::run_native(
        "Rust SQLite Editor",
        native_options,
        Box::new(|cc| Ok(Box::new(SqliteEditorApp::new(cc)))),
    )
}
