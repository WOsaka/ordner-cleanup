//! Ansicht „Quarantäne“: Inhalt, Größe und Ablauf; einzelne Einträge zurückholen.

use eframe::egui;

use crate::gui::help::Topic;
use crate::gui::shell::{Answer, Page, Shell, TaskResult};
use crate::gui::texts;
use crate::gui::widgets::help_button::heading_with_help;

#[derive(Default)]
pub struct QuarantineView {}

impl QuarantineView {
    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        heading_with_help(
            ui,
            texts::NAV_QUARANTINE,
            Topic::Page(Page::Quarantine),
            shell,
        );
    }

    pub fn on_answer(&mut self, _id: &str, _answer: Answer, _shell: &mut Shell) {}

    pub fn on_finished(&mut self, _name: &str, _result: TaskResult, _shell: &mut Shell) {}
}
