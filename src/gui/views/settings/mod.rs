//! Ansicht „Einstellungen“: Allgemein, Profile (mit Zeitplänen), Regeln, Klassifikation und
//! LLM, Kategorien, Vorlagen und Dateien.

mod content;
mod doc_editor;
mod general;
mod list;
mod profiles;
mod rules;
mod templates;

use std::path::PathBuf;

use eframe::egui;

use crate::gui::help::Topic;
use crate::gui::shell::{Answer, Page, Shell, TaskResult};
use crate::gui::texts;
use crate::gui::widgets::help_button::heading_with_help;
use crate::paths;
use doc_editor::DocEditor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    General,
    Profiles,
    Rules,
    Content,
    Categories,
    Templates,
    Files,
}

impl Tab {
    const ALL: [(Tab, &'static str); 7] = [
        (Tab::General, "Allgemein"),
        (Tab::Profiles, "Profile & Zeitpläne"),
        (Tab::Rules, "Regeln"),
        (Tab::Content, "Klassifikation & LLM"),
        (Tab::Categories, "Kategorien"),
        (Tab::Templates, "Vorlagen"),
        (Tab::Files, "Dateien"),
    ];
}

fn config_path() -> PathBuf {
    paths::config_dir()
        .map(|d| d.join("config.toml"))
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}

fn categories_path() -> PathBuf {
    let config = crate::ops::load_config().unwrap_or_default();
    match (&config.classify.categories_file, paths::config_dir()) {
        (Some(f), dir) => PathBuf::from(crate::config::resolve_in(f, dir.as_deref())),
        (None, Some(dir)) => dir.join("categories.toml"),
        (None, None) => PathBuf::from("categories.toml"),
    }
}

pub struct SettingsView {
    tab: Tab,
    config: Option<DocEditor>,
    rules_ed: Option<DocEditor>,
    categories_ed: Option<DocEditor>,
    general: general::GeneralTab,
    profiles: profiles::ProfilesTab,
    rules: rules::RulesTab,
    content: content::ContentTab,
    templates: templates::TemplatesTab,
}

impl Default for SettingsView {
    fn default() -> Self {
        Self {
            tab: Tab::default(),
            config: None,
            rules_ed: None,
            categories_ed: None,
            general: general::GeneralTab::default(),
            profiles: profiles::ProfilesTab::default(),
            rules: rules::RulesTab::new(),
            content: content::ContentTab::default(),
            templates: templates::TemplatesTab::default(),
        }
    }
}

impl SettingsView {
    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        heading_with_help(ui, texts::NAV_SETTINGS, Topic::Page(Page::Settings), shell);
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in Tab::ALL {
                ui.selectable_value(&mut self.tab, tab, label);
            }
        });
        ui.separator();
        match self.tab {
            Tab::General => {
                let ed = self
                    .config
                    .get_or_insert_with(|| DocEditor::new("config.toml", config_path()));
                self.general.ui(ui, ed, shell);
            }
            Tab::Profiles => {
                let ed = self
                    .config
                    .get_or_insert_with(|| DocEditor::new("config.toml", config_path()));
                self.profiles.ui(ui, ed, shell);
            }
            Tab::Content => {
                let ed = self
                    .config
                    .get_or_insert_with(|| DocEditor::new("config.toml", config_path()));
                self.content.ui(ui, ed, shell);
            }
            Tab::Rules => {
                let ed = self
                    .rules_ed
                    .get_or_insert_with(|| DocEditor::new("Regeldatei", rules::RulesTab::path()));
                self.rules.ui(ui, ed, shell);
            }
            Tab::Categories => {
                let ed = self
                    .categories_ed
                    .get_or_insert_with(|| DocEditor::new("Eigene Kategorien", categories_path()));
                content::categories_ui(ui, ed, shell);
            }
            Tab::Templates => self.templates.ui(ui, shell),
            Tab::Files => files_ui(ui, shell),
        }
    }

    pub fn on_answer(&mut self, id: &str, answer: Answer, _shell: &mut Shell) {
        if let Some(ed) = &mut self.config {
            self.profiles.on_answer(id, answer, ed);
        }
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        match name {
            "Treffer berechnen" => self.rules.on_finished(name, result, shell),
            "Verbindung testen" => self.content.on_finished(name, result, shell),
            _ => self.profiles.on_finished(name, result, shell),
        }
    }
}

/// Dateien im Editor öffnen und die Startmenü-Verknüpfung (wie in Stufe 1).
fn files_ui(ui: &mut egui::Ui, shell: &mut Shell) {
    let dir = paths::config_dir();
    let at = |name: &str| dir.as_ref().map(|d| d.join(name));
    for (label, path) in [
        ("Konfiguration", at("config.toml")),
        ("Regeln", at("rules.toml")),
        ("Eigene Kategorien", at("categories.toml")),
    ] {
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
        });
    }
    ui.add_space(crate::gui::theme::SPACE_L);
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
