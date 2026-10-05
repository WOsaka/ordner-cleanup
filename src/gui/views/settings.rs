//! Ansicht „Einstellungen“ (Stufe 1): die Dateien im Editor öffnen; die Formulare folgen in
//! Stufe 2.

use std::path::PathBuf;

use eframe::egui;

use crate::gui::shell::{Shell, TaskResult};
use crate::gui::texts;
use crate::paths;

#[derive(Default)]
pub struct SettingsView {}

/// Die Dateien, die die GUI pflegt: Name und Pfad.
fn files() -> Vec<(&'static str, Option<PathBuf>)> {
    let dir = paths::config_dir();
    let at = |name: &str| dir.as_ref().map(|d| d.join(name));
    vec![
        (
            "Konfiguration (Profile, Schutz, Defaults)",
            at("config.toml"),
        ),
        ("Regeln für „Nach Regeln“", at("rules.toml")),
        ("Eigene Kategorien", at("categories.toml")),
    ]
}

impl SettingsView {
    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        ui.heading(texts::NAV_SETTINGS);
        ui.label("Die Dateien lassen sich im Editor öffnen; nach dem Speichern gelten sie sofort.");
        ui.add_space(8.0);
        for (label, path) in files() {
            let Some(path) = path else { continue };
            ui.horizontal(|ui| {
                ui.label(label);
                ui.label(
                    egui::RichText::new(paths::display(&path))
                        .monospace()
                        .weak(),
                );
                if ui.button("Im Editor öffnen").clicked() {
                    if let Err(e) = crate::platform::shell::edit(&path) {
                        shell.message("Einstellungen", &format!("{e:#}"));
                    }
                }
                if ui.button("Im Explorer zeigen").clicked() {
                    let target = if path.exists() {
                        path.clone()
                    } else {
                        path.parent().map(PathBuf::from).unwrap_or(path.clone())
                    };
                    if let Err(e) = crate::platform::shell::reveal(&target) {
                        shell.message("Einstellungen", &format!("{e:#}"));
                    }
                }
            });
        }
        ui.add_space(12.0);
        if ui.button("Verknüpfung im Startmenü anlegen").clicked() {
            match crate::platform::shell::create_start_menu_shortcut() {
                Ok(file) => shell.message(
                    "Startmenü",
                    &format!("Verknüpfung angelegt: {}", paths::display(&file)),
                ),
                Err(e) => shell.message("Startmenü", &format!("{e:#}")),
            }
        }
    }

    pub fn on_finished(&mut self, _name: &str, _result: TaskResult, _shell: &mut Shell) {}
}
