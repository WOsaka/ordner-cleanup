//! Profile: Liste, Formular, anlegen, umbenennen, löschen und der Zeitplan je Profil.

use eframe::egui;

use super::doc_editor::DocEditor;
use super::general::raw;
use crate::config::Config;
use crate::gui::shell::{Answer, Dialog, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::widgets::form;
use crate::ops::local_time;
use crate::ops::schedule::{schedule_add, schedule_list, schedule_remove, ScheduleAdded};
use crate::schedule::{task_name, Row, Trigger};
use crate::settings::config_doc;
use crate::settings::fields::{Loc, PROFILE_FIELDS};
use crate::settings::validate::config_text;

pub const ID_DELETE: &str = "settings.profile-delete";
const TASK_LIST: &str = "Zeitpläne laden";
const TASK_ADD: &str = "Zeitplan anlegen";
const TASK_REMOVE: &str = "Zeitplan entfernen";

const DAYS: [(&str, &str); 7] = [
    ("MO", "Montag"),
    ("DI", "Dienstag"),
    ("MI", "Mittwoch"),
    ("DO", "Donnerstag"),
    ("FR", "Freitag"),
    ("SA", "Samstag"),
    ("SO", "Sonntag"),
];

pub struct ProfilesTab {
    selected: Option<String>,
    new_name: String,
    new_root: String,
    rename_to: String,
    error: Option<String>,
    schedules: Vec<Row>,
    schedules_loaded: bool,
    weekly: bool,
    day: usize,
    time: String,
    pending_delete: Option<String>,
}

impl Default for ProfilesTab {
    fn default() -> Self {
        Self {
            selected: None,
            new_name: String::new(),
            new_root: String::new(),
            rename_to: String::new(),
            error: None,
            schedules: Vec::new(),
            schedules_loaded: false,
            weekly: false,
            day: 0,
            time: "09:00".into(),
            pending_delete: None,
        }
    }
}

impl ProfilesTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, ed: &mut DocEditor, shell: &mut Shell) {
        ed.toolbar(ui, shell, &config_text);
        ui.separator();
        if ed.raw_mode {
            raw(ui, ed);
            return;
        }
        if !self.schedules_loaded {
            self.schedules_loaded = true;
            shell.spawn(Route::Settings, TASK_LIST, TaskKind::Read, |_| {
                schedule_list()
            });
        }
        let Some(names) = ed.doc_ref().map(config_doc::profile_names) else {
            return;
        };
        if self.selected.as_ref().is_none_or(|s| !names.contains(s)) {
            self.selected = names.first().cloned();
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for n in &names {
                    if ui
                        .selectable_label(self.selected.as_deref() == Some(n), n)
                        .clicked()
                    {
                        self.selected = Some(n.clone());
                        self.rename_to = n.clone();
                    }
                }
            });
            self.manage_ui(ui, ed, shell);
            if let Some(name) = self.selected.clone() {
                ui.add_space(8.0);
                ui.heading(format!("Profil {name}"));
                let defaults = Config::default();
                let mut form_state = std::mem::take(&mut ed.form);
                if let Some(doc) = ed.doc() {
                    let base = ["profiles", name.as_str()];
                    form::show(
                        ui,
                        doc,
                        Loc::Path(&base),
                        PROFILE_FIELDS,
                        &defaults,
                        &mut form_state,
                    );
                }
                ed.form = form_state;
                self.schedule_ui(ui, ed, shell, &name);
            }
        });
    }

    fn manage_ui(&mut self, ui: &mut egui::Ui, ed: &mut DocEditor, shell: &mut Shell) {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Neues Profil:");
            ui.add(
                egui::TextEdit::singleline(&mut self.new_name)
                    .hint_text("name")
                    .desired_width(120.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.new_root)
                    .hint_text("Wurzel (Ordner)")
                    .desired_width(260.0),
            );
            if ui.button("Anlegen").clicked() {
                let (name, root) = (self.new_name.trim().to_string(), self.new_root.clone());
                match ed.doc().map(|d| config_doc::add_profile(d, &name, &root)) {
                    Some(Ok(())) => {
                        self.selected = Some(name);
                        self.new_name.clear();
                        self.new_root.clear();
                        self.error = None;
                    }
                    Some(Err(e)) => self.error = Some(e),
                    None => {}
                }
            }
        });
        if let Some(selected) = self.selected.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.label("Umbenennen in:");
                ui.add(egui::TextEdit::singleline(&mut self.rename_to).desired_width(120.0));
                if ui.button("Umbenennen").clicked() {
                    let to = self.rename_to.trim().to_string();
                    match ed
                        .doc()
                        .map(|d| config_doc::rename_profile(d, &selected, &to))
                    {
                        Some(Ok(())) => {
                            self.selected = Some(to);
                            self.error = None;
                            if self.has_schedule(&selected) {
                                shell.message(
                                    "Zeitplan",
                                    "Der Zeitplan gehört zum alten Namen. Bitte unter dem neuen \
                                     Namen neu anlegen und den alten entfernen.",
                                );
                            }
                        }
                        Some(Err(e)) => self.error = Some(e),
                        None => {}
                    }
                }
                if ui.button("Löschen …").clicked() {
                    self.pending_delete = Some(selected.clone());
                    let hint = if self.has_schedule(&selected) {
                        "\n\nFür dieses Profil besteht ein Zeitplan; er bleibt bestehen und \
                         lässt sich unten entfernen."
                    } else {
                        ""
                    };
                    shell.confirm(Dialog::Confirm {
                        id: ID_DELETE,
                        title: "Profil löschen".into(),
                        text: format!("Profil „{selected}“ aus der Config entfernen?{hint}"),
                        ok_label: "Löschen".into(),
                        must_check: None,
                        checked: false,
                        danger: true,
                    });
                }
            });
        }
        if let Some(e) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, e);
        }
    }

    fn has_schedule(&self, profile: &str) -> bool {
        self.schedules.iter().any(|r| r.entry.profile == profile)
    }

    fn schedule_ui(&mut self, ui: &mut egui::Ui, ed: &DocEditor, shell: &mut Shell, profile: &str) {
        ui.add_space(10.0);
        ui.heading("Geplanter Lauf");
        let now = chrono::Local::now().naive_local();
        match self.schedules.iter().find(|r| r.entry.profile == profile) {
            Some(row) => {
                let when = row.entry.trigger.to_trigger().map_or_else(
                    |_| "unbekannter Auslöser".to_string(),
                    |t| {
                        format!(
                            "{}, nächster Lauf {}",
                            t.describe(),
                            t.next_run(now).format("%d.%m.%Y %H:%M")
                        )
                    },
                );
                ui.label(format!("{} – {when}", task_name(profile)));
                let last = crate::paths::runs_log(profile)
                    .ok()
                    .and_then(|log| crate::runlog::read_all(&log).pop());
                ui.label(match last {
                    Some(r) => format!(
                        "Letzter Lauf: {} – {}",
                        local_time(&r.started),
                        r.status.label()
                    ),
                    None => "Letzter Lauf: noch keiner".into(),
                });
                if !matches!(row.state, crate::schedule::TaskState::Ok) {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Die Aufgabe fehlt in der Aufgabenplanung oder zeigt auf ein fehlendes Programm; neu anlegen.",
                    );
                }
                if ui.button("Zeitplan entfernen").clicked() {
                    let name = profile.to_string();
                    shell.spawn(Route::Settings, TASK_REMOVE, TaskKind::Write, move |_| {
                        schedule_remove(&name)
                    });
                }
            }
            None => {
                ui.label("Kein Zeitplan.");
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.weekly, false, "täglich");
            ui.selectable_value(&mut self.weekly, true, "wöchentlich");
            if self.weekly {
                egui::ComboBox::from_id_salt("sched-day")
                    .selected_text(DAYS[self.day].1)
                    .show_ui(ui, |ui| {
                        for (i, (_, label)) in DAYS.iter().enumerate() {
                            ui.selectable_value(&mut self.day, i, *label);
                        }
                    });
            }
            ui.label("Uhrzeit:");
            ui.add(egui::TextEdit::singleline(&mut self.time).desired_width(50.0));
            if ui.button("Zeitplan anlegen / ersetzen").clicked() {
                if ed.is_dirty() {
                    shell.message(
                        "Zeitplan",
                        "Das Profil muss gespeichert sein, bevor ein Zeitplan angelegt wird.",
                    );
                    return;
                }
                let trigger = if self.weekly {
                    Trigger::weekly(DAYS[self.day].0, &self.time)
                } else {
                    Trigger::daily(&self.time)
                };
                match trigger {
                    Ok(trigger) => {
                        let name = profile.to_string();
                        shell.spawn(Route::Settings, TASK_ADD, TaskKind::Write, move |_| {
                            let added = schedule_add(&name, &trigger)?;
                            Ok((added, trigger.describe()))
                        });
                    }
                    Err(e) => shell.message("Zeitplan", &e),
                }
            }
        });
        ui.label(
            egui::RichText::new(
                "Der Lauf erzeugt Bericht und Pläne und führt nie „Anwenden“ aus; \
                 Benachrichtigungen steuert [notify] in der Config.",
            )
            .weak(),
        );
    }

    pub fn on_answer(&mut self, id: &str, answer: Answer, ed: &mut DocEditor) {
        if id == ID_DELETE {
            if let (true, Some(name)) = (answer.ok, self.pending_delete.take()) {
                if let Some(doc) = ed.doc() {
                    config_doc::delete_profile(doc, &name);
                }
                self.selected = None;
            }
        }
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        match name {
            TASK_LIST => {
                if let Ok(Ok(rows)) = result.map(|b| b.downcast::<Vec<Row>>()) {
                    self.schedules = *rows;
                }
            }
            TASK_ADD => match result.map(|b| b.downcast::<(ScheduleAdded, String)>()) {
                Ok(Ok(done)) => {
                    let (added, when) = *done;
                    let mut text = format!(
                        "Aufgabe {}: {when}, nächster Lauf {}.",
                        if added.added.replaced {
                            "ersetzt"
                        } else {
                            "angelegt"
                        },
                        added.added.next_run.format("%d.%m.%Y %H:%M")
                    );
                    if let Some(w) = added.aumid_warning {
                        text.push_str(&format!(
                            "\nHinweis: Absender für Benachrichtigungen nicht registriert: {w}"
                        ));
                    }
                    shell.message(TASK_ADD, &text);
                    self.schedules_loaded = false;
                }
                Ok(Err(_)) => {}
                Err(e) => shell.show_error(name, &e),
            },
            TASK_REMOVE => match result {
                Ok(_) => self.schedules_loaded = false,
                Err(e) => shell.show_error(name, &e),
            },
            _ => {}
        }
    }
}
