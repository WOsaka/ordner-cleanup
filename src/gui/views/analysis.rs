//! Ansicht „Analyse“.

use eframe::egui;

use crate::gui::shell::{Shell, TaskResult};

#[derive(Default)]
pub struct AnalysisView {}

impl AnalysisView {
    pub fn ui(&mut self, ui: &mut egui::Ui, _shell: &mut Shell) {
        ui.heading("Analyse");
    }

    pub fn on_finished(&mut self, _name: &str, _result: TaskResult, _shell: &mut Shell) {}
}
