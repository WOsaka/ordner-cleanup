//! Ansicht „Verlauf“.

use eframe::egui;

use crate::gui::shell::{Shell, TaskResult};

#[derive(Default)]
pub struct HistoryView {}

impl HistoryView {
    pub fn ui(&mut self, ui: &mut egui::Ui, _shell: &mut Shell) {
        ui.heading("Verlauf");
    }

    pub fn on_finished(&mut self, _name: &str, _result: TaskResult, _shell: &mut Shell) {}
}
