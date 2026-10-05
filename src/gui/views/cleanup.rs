//! Ansicht „Aufräumen“.

use eframe::egui;

use crate::gui::shell::{Shell, TaskResult};

#[derive(Default)]
pub struct CleanupView {}

impl CleanupView {
    pub fn preselect(&mut self, _kind: &'static str) {}

    pub fn ui(&mut self, ui: &mut egui::Ui, _shell: &mut Shell) {
        ui.heading("Aufräumen");
    }

    pub fn on_finished(&mut self, _name: &str, _result: TaskResult, _shell: &mut Shell) {}
}
