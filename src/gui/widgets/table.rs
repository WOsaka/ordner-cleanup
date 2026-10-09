//! Gemeinsame Tabellenbausteine: Kontextmenü für Pfade und eine Pfadzelle.

use std::path::Path;

use eframe::egui;

use crate::gui::format::{short_path, StatusStyle};
use crate::gui::shell::Shell;
use crate::gui::theme;
use crate::platform::shell as explorer;

/// Kontextmenü „Im Explorer zeigen · Pfad kopieren · Öffnen“ für einen Pfad. „Öffnen“ ist für
/// Cloud-only-Dateien (und fehlende Dateien) deaktiviert, weil es einen Download auslösen würde.
pub fn path_menu(response: &egui::Response, path: &str, shell: &mut Shell) {
    response.context_menu(|ui| {
        if ui.button("Im Explorer zeigen").clicked() {
            if let Err(e) = explorer::reveal(Path::new(path)) {
                shell.message("Explorer", &format!("{e:#}"));
            }
            ui.close();
        }
        if ui.button("Pfad kopieren").clicked() {
            ui.ctx().copy_text(path.to_string());
            ui.close();
        }
        let can_open = explorer::can_open(Path::new(path));
        let open = ui.add_enabled(can_open, egui::Button::new("Mit Standardprogramm öffnen"));
        if !can_open {
            open.clone()
                .on_disabled_hover_text("Nur in der Cloud oder nicht mehr vorhanden");
        }
        if open.clicked() {
            if let Err(e) = explorer::open(Path::new(path)) {
                shell.message("Öffnen", &format!("{e:#}"));
            }
            ui.close();
        }
    });
}

/// Eine Pfadzelle in Monospace, abgeschnitten statt umbrochen, mit Kontextmenü.
pub fn path_cell(ui: &mut egui::Ui, path: &str, shell: &mut Shell) {
    let response = ui.add(
        egui::Label::new(egui::RichText::new(path).monospace())
            .truncate()
            .sense(egui::Sense::click()),
    );
    response.clone().on_hover_text(path);
    path_menu(&response, path, shell);
}

/// Anteil als Balken mit Prozenttext.
pub fn share_bar(ui: &mut egui::Ui, share: f32, width: f32) {
    ui.add(
        egui::ProgressBar::new(share.clamp(0.0, 1.0))
            .desired_width(width)
            .text(format!("{:.0} %", share * 100.0)),
    );
}

/// Pfad in der Mitte gekürzt; der volle Pfad steht im Tooltip, das Kontextmenü kopiert ihn.
pub fn short_path_cell(ui: &mut egui::Ui, path: &str, max_chars: usize, shell: &mut Shell) {
    let response = ui.add(
        egui::Label::new(short_path(path, max_chars))
            .truncate()
            .sense(egui::Sense::click()),
    );
    response.clone().on_hover_text(path);
    path_menu(&response, path, shell);
}

/// Status mit Symbol in der Farbe seines Tons.
pub fn status_cell(ui: &mut egui::Ui, status: &StatusStyle) {
    ui.colored_label(theme::tone_color(ui, status.tone), status.label());
}

/// Kurzer Zeittext, der genaue Wert steht im Tooltip.
pub fn time_cell(ui: &mut egui::Ui, time: &(String, String)) {
    let response = ui.label(&time.0);
    if !time.1.is_empty() {
        response.on_hover_text(&time.1);
    }
}
