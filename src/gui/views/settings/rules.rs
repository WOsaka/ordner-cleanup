//! Regel-Editor: Liste, Formular mit Platzhalter-Auswahl, Vorschau der Treffer, Roh-TOML.

use eframe::egui;

use super::doc_editor::DocEditor;
use super::list::{self, ListState};
use crate::config::Config;
use crate::gui::review::ReviewModel;
use crate::gui::shell::{Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::views::review::ReviewState;
use crate::gui::widgets::form;
use crate::ops::plan::{plan, PlanKindRequest, PlanOut, PlanOutcome, PlanRequest};
use crate::settings::fields::{self, Loc, Value};
use crate::settings::rules_doc::{self, KEY, PLACEHOLDERS, RULE_FIELDS};
use crate::settings::tables;
use crate::settings::validate::rules_text;

const TASK_PREVIEW: &str = "Treffer berechnen";

#[derive(Default)]
pub struct RulesTab {
    list: ListState,
    /// Config für die Prüfung (Kategorien); wird beim Laden gelesen
    config: Option<Config>,
    /// Eingabepuffer der Textfelder der gewählten Regel
    name: String,
    target: String,
    fields: String,
    fields_regex: String,
    synced_for: Option<(usize, String)>,
    preview: Option<ReviewState>,
    no_classify: bool,
    map_error: Option<String>,
}

impl RulesTab {
    pub fn new() -> Self {
        Self {
            no_classify: true,
            ..Self::default()
        }
    }

    fn config(&mut self) -> &Config {
        self.config
            .get_or_insert_with(|| crate::ops::load_config().unwrap_or_default())
    }

    /// Pfad der Regeldatei wie bei `plan rules`.
    pub fn path() -> std::path::PathBuf {
        let config = crate::ops::load_config().unwrap_or_default();
        crate::ops::plan::resolve_rules_path(None, &config, crate::paths::config_dir().as_deref())
            .unwrap_or_else(|_| std::path::PathBuf::from("rules.toml"))
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ed: &mut DocEditor, shell: &mut Shell) {
        let config = self.config().clone();
        let validate = move |t: &str| rules_text(t, &config);
        ed.toolbar(ui, shell, &validate);
        ui.separator();
        if ed.raw_mode {
            ed.raw.ui(ui, &validate, 26);
            return;
        }
        if !ed.is_loaded() {
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.editor_ui(ui, ed);
            ui.add_space(crate::gui::theme::SPACE_L);
            self.preview_ui(ui, ed, shell);
        });
    }

    fn editor_ui(&mut self, ui: &mut egui::Ui, ed: &mut DocEditor) {
        let defaults = Config::default();
        let mut form_state = std::mem::take(&mut ed.form);
        let Some(doc) = ed.doc() else { return };
        if list::ui(
            ui,
            doc,
            KEY,
            &mut self.list,
            "neue-regel",
            &rules_doc::new_rule_defaults(),
        ) {
            self.synced_for = None;
            form_state.reset();
        }
        if tables::len(doc, KEY) == 0 {
            ui.label("Noch keine Regeln. Mit „Neu“ anlegen.");
            ed.form = form_state;
            return;
        }
        let index = self.list.selected;
        let current = tables::names(doc, KEY)
            .get(index)
            .cloned()
            .unwrap_or_default();
        if self.synced_for.as_ref() != Some(&(index, current.clone())) {
            self.sync_buffers(doc, index);
            self.synced_for = Some((index, current));
        }
        ui.separator();
        egui::Grid::new("rule-main")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Name").strong());
                if ui
                    .add(egui::TextEdit::singleline(&mut self.name).desired_width(320.0))
                    .changed()
                {
                    fields::set_at(
                        doc,
                        Loc::Entry { list: KEY, index },
                        "name",
                        &Value::Text(self.name.clone()),
                    );
                    self.synced_for = Some((index, self.name.clone()));
                }
                ui.end_row();
                ui.label(egui::RichText::new("Ziel").strong());
                ui.vertical(|ui| {
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.target)
                                .desired_width(420.0)
                                .font(egui::TextStyle::Monospace),
                        )
                        .changed()
                    {
                        fields::set_at(
                            doc,
                            Loc::Entry { list: KEY, index },
                            "target",
                            &Value::Text(self.target.clone()),
                        );
                    }
                    egui::ComboBox::from_id_salt("placeholder")
                        .selected_text("Platzhalter einfügen …")
                        .show_ui(ui, |ui| {
                            for (text, help) in PLACEHOLDERS {
                                if ui
                                    .selectable_label(false, format!("{text}  –  {help}"))
                                    .clicked()
                                {
                                    self.target.push_str(text);
                                    fields::set_at(
                                        doc,
                                        Loc::Entry { list: KEY, index },
                                        "target",
                                        &Value::Text(self.target.clone()),
                                    );
                                }
                            }
                        });
                });
                ui.end_row();
            });
        ui.add_space(crate::gui::theme::SPACE_M);
        form::show(
            ui,
            doc,
            Loc::Entry { list: KEY, index },
            RULE_FIELDS,
            &defaults,
            &mut form_state,
        );
        ui.add_space(crate::gui::theme::SPACE_M);
        ui.label(egui::RichText::new("Felder (Inhaltsbedingungen: schlüssel = wert)").strong());
        if ui
            .add(
                egui::TextEdit::multiline(&mut self.fields)
                    .desired_rows(2)
                    .desired_width(420.0)
                    .font(egui::TextStyle::Monospace),
            )
            .changed()
        {
            self.map_error = write_map(doc, index, "fields", &self.fields);
        }
        ui.label(egui::RichText::new("Felder per Regex (schlüssel = regex)").strong());
        if ui
            .add(
                egui::TextEdit::multiline(&mut self.fields_regex)
                    .desired_rows(2)
                    .desired_width(420.0)
                    .font(egui::TextStyle::Monospace),
            )
            .changed()
        {
            self.map_error = write_map(doc, index, "fields_regex", &self.fields_regex);
        }
        if let Some(e) = &self.map_error {
            ui.colored_label(
                crate::gui::theme::tone_color(ui, crate::gui::format::Tone::Error),
                e,
            );
        }
        ed.form = form_state;
    }

    fn sync_buffers(&mut self, doc: &toml_edit::DocumentMut, index: usize) {
        let text = |key: &str| match fields::get_at(doc, Loc::Entry { list: KEY, index }, key) {
            Some(Value::Text(t)) => t,
            _ => String::new(),
        };
        self.name = text("name");
        self.target = text("target");
        self.fields = tables::get_map(doc, KEY, index, "fields").join("\n");
        self.fields_regex = tables::get_map(doc, KEY, index, "fields_regex").join("\n");
        self.map_error = None;
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui, ed: &mut DocEditor, shell: &mut Shell) {
        ui.separator();
        ui.heading("Vorschau: Treffer");
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.no_classify, "Ohne Nachklassifizieren");
            let ready = shell.target.is_some() && !shell.is_running(Route::Settings);
            if ui
                .add_enabled(ready, egui::Button::new("Treffer anzeigen"))
                .clicked()
            {
                self.start_preview(ed, shell);
            }
            match &shell.target {
                Some(t) => ui.label(format!("Auf: {}", t.label())),
                None => ui.label("Erst auf der Übersicht einen gescannten Ordner wählen."),
            };
        });
        ui.label(
            egui::RichText::new(
                "Die Vorschau verändert nichts: sie ist ein „Plan nach Regeln“, der nicht gespeichert wird.",
            )
            .weak(),
        );
        if let Some(preview) = &mut self.preview {
            preview.ui(ui, shell);
        }
    }

    fn start_preview(&mut self, ed: &DocEditor, shell: &mut Shell) {
        let Some(target) = shell.target.as_ref().map(|t| t.spec()) else {
            return;
        };
        let text = ed.current_text();
        let req = PlanRequest {
            target,
            kind: PlanKindRequest::Rules {
                rules_file: None,
                rules_text: Some(text),
                only: Vec::new(),
                no_classify: self.no_classify,
                lock_held: false,
            },
            out: PlanOut::DontSave,
        };
        let kind = if self.no_classify {
            TaskKind::Read
        } else {
            TaskKind::Write
        };
        shell.spawn(Route::Settings, TASK_PREVIEW, kind, move |ctx| {
            plan(&req, ctx)
        });
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        if name != TASK_PREVIEW {
            return;
        }
        match result.map(|b| b.downcast::<PlanOutcome>()) {
            Ok(Ok(outcome)) => {
                let mut notes: Vec<String> = outcome.notes.clone();
                notes.extend(outcome.prep.hints.iter().cloned());
                let mut state = ReviewState::new(
                    ReviewModel::new(outcome.plan),
                    None,
                    outcome.headline,
                    notes,
                );
                state.read_only = true;
                self.preview = Some(state);
            }
            Ok(Err(_)) => {}
            Err(e) => shell.show_error(name, &e),
        }
    }
}

fn write_map(
    doc: &mut toml_edit::DocumentMut,
    index: usize,
    field: &str,
    text: &str,
) -> Option<String> {
    let lines: Vec<String> = text.lines().map(String::from).collect();
    tables::set_map(doc, KEY, index, field, &lines).err()
}
