//! Ansicht „Einstellungen“.

use eframe::egui;

use crate::gui::shell::{Shell, TaskResult};

#[derive(Default)]
pub struct SettingsView {}

impl SettingsView {
    pub fn ui(&mut self, ui: &mut egui::Ui, _shell: &mut Shell) {
        ui.heading("Einstellungen");
    }

    pub fn on_finished(&mut self, _name: &str, _result: TaskResult, _shell: &mut Shell) {}
}
