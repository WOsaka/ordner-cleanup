//! Klassifikation und lokales LLM (mit Verbindungstest) sowie eigene Kategorien (Roh-TOML).

use eframe::egui;

use super::doc_editor::DocEditor;
use super::general::raw;
use crate::config::Config;
use crate::content::classify::defs::CategoryDefs;
use crate::gui::shell::{Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::widgets::form;
use crate::ops::admin::llm_check;
use crate::settings::fields::{Loc, CLASSIFY_FIELDS, LLM_FIELDS};
use crate::settings::validate::{categories_text, config_text};

const TASK_LLM: &str = "Verbindung testen";

#[derive(Default)]
pub struct ContentTab {
    llm_result: Option<Result<(), String>>,
}

impl ContentTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, ed: &mut DocEditor, shell: &mut Shell) {
        ed.toolbar(ui, shell, &config_text);
        ui.separator();
        if ed.raw_mode {
            raw(ui, ed);
            return;
        }
        let defaults = Config::default();
        let mut form_state = std::mem::take(&mut ed.form);
        let text = ed.current_text();
        if let Some(doc) = ed.doc() {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Klassifikation");
                form::show(
                    ui,
                    doc,
                    Loc::Path(&["classify"]),
                    CLASSIFY_FIELDS,
                    &defaults,
                    &mut form_state,
                );
                ui.add_space(10.0);
                ui.heading("Lokales LLM (optional)");
                form::show(
                    ui,
                    doc,
                    Loc::Path(&["llm"]),
                    LLM_FIELDS,
                    &defaults,
                    &mut form_state,
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !shell.is_running(Route::Settings),
                            egui::Button::new("Verbindung testen"),
                        )
                        .clicked()
                    {
                        match Config::parse(&text) {
                            Ok(config) => {
                                let llm = config.llm;
                                shell.spawn(Route::Settings, TASK_LLM, TaskKind::Read, move |_| {
                                    Ok(llm_check(&llm))
                                });
                            }
                            Err(e) => shell.message(TASK_LLM, &format!("Config ungültig: {e:#}")),
                        }
                    }
                    match &self.llm_result {
                        Some(Ok(())) => {
                            ui.colored_label(
                                crate::gui::theme::tone_color(ui, crate::gui::format::Tone::Ok),
                                "Erreichbar, Modell vorhanden",
                            );
                        }
                        Some(Err(e)) => {
                            ui.colored_label(
                                crate::gui::theme::tone_color(ui, crate::gui::format::Tone::Error),
                                e,
                            );
                        }
                        None => {}
                    }
                });
            });
        }
        ed.form = form_state;
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        if name != TASK_LLM {
            return;
        }
        match result.map(|b| b.downcast::<Result<(), String>>()) {
            Ok(Ok(r)) => self.llm_result = Some(*r),
            Ok(Err(_)) => {}
            Err(e) => shell.show_error(name, &e),
        }
    }
}

/// Kategorien: Roh-TOML mit Prüfung, darüber die Liste (eingebaut oder eigene).
pub fn categories_ui(ui: &mut egui::Ui, ed: &mut DocEditor, shell: &mut Shell) {
    ed.toolbar(ui, shell, &categories_text);
    ui.separator();
    if !ed.is_loaded() {
        return;
    }
    if !ed.raw_mode {
        // Diese Datei bearbeitet man nur als Text
        ed.raw.set_text(ed.current_text());
        ed.raw_mode = true;
    }
    let builtin: std::collections::BTreeSet<String> = CategoryDefs::builtin().names();
    let custom: Vec<String> = CategoryDefs::load(&ed.raw.text, "categories.toml")
        .map(|d| {
            d.names()
                .into_iter()
                .filter(|n| !builtin.contains(n))
                .collect()
        })
        .unwrap_or_default();
    ui.label(format!(
        "{} eingebaute Kategorien, {} eigene{}",
        builtin.len(),
        custom.len(),
        if custom.is_empty() {
            String::new()
        } else {
            format!(": {}", custom.join(", "))
        }
    ));
    ui.collapsing("Eingebaute Kategorien", |ui| {
        ui.label(builtin.iter().cloned().collect::<Vec<_>>().join(", "));
    });
    ed.raw.ui(ui, &categories_text, 22);
}
