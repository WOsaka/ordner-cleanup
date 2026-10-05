//! Ansicht „Übersicht“.

use eframe::egui;

use crate::gui::shell::{Shell, TaskResult};

#[derive(Default)]
pub struct OverviewView {}

impl OverviewView {
    pub fn refresh(&mut self, _shell: &mut Shell) {}

    pub fn ui(&mut self, ui: &mut egui::Ui, _shell: &mut Shell) {
        ui.heading("Übersicht");
    }

    pub fn on_finished(&mut self, _name: &str, _result: TaskResult, _shell: &mut Shell) {}
}
