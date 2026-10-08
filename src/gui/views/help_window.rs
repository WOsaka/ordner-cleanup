//! Hilfefenster: Reiter „Erklärung“ mit dem Kurztext zum gewählten Thema und Reiter „Tastenkürzel“.
//! Es blockiert nichts; `Esc` schließt es zuerst und wird dabei verbraucht, damit ein Dialog
//! darunter ihn nicht auch noch sieht.

use eframe::egui;

use crate::gui::format;
use crate::gui::help::{self, DocFile, HelpEntry, Topic};
use crate::gui::keys;
use crate::gui::theme;
use crate::paths;

/// Öffnet eine Doku-Datei; `Err` trägt die Meldung für die Anzeige. Austauschbar für Tests.
pub type DocOpener = fn(DocFile) -> Result<(), String>;

/// Öffnet die Doku-Datei im Standardprogramm (Rückfall: Notepad).
pub fn open_doc(file: DocFile) -> Result<(), String> {
    match help::doc_path(file) {
        Some(path) => crate::platform::shell::open_text(&path).map_err(|e| format!("{e:#}")),
        None => Err(format!(
            "Die Datei „{}“ wurde nicht gefunden. Sie gehört neben die Programmdatei \
             (Ordner „docs“). Erwarteter Pfad: {}",
            file.label(),
            paths::display(&help::expected_path(file))
        )),
    }
}

pub struct HelpState {
    pub open: bool,
    pub topic: Topic,
    /// Das zuletzt erklärte Thema; der Reiter „Erklärung“ kehrt dorthin zurück
    explain: Topic,
    /// Fehlermeldung vom letzten Versuch, die Doku zu öffnen
    notice: Option<(DocFile, String)>,
    opener: DocOpener,
}

impl Default for HelpState {
    fn default() -> Self {
        Self::with_opener(open_doc)
    }
}

impl HelpState {
    pub fn with_opener(opener: DocOpener) -> Self {
        let first = Topic::Page(crate::gui::shell::Page::Overview);
        Self {
            open: false,
            topic: first,
            explain: first,
            notice: None,
            opener,
        }
    }

    /// Öffnet das Fenster mit dem Kurztext zu diesem Thema.
    pub fn show_topic(&mut self, topic: Topic) {
        self.open = true;
        self.topic = topic;
        if topic != Topic::Keys {
            self.explain = topic;
        }
        self.notice = None;
    }

    /// `?` und „Kürzel“: Liste öffnen, bei geöffneter Liste wieder schließen.
    pub fn toggle_keys(&mut self) {
        if self.open && self.topic == Topic::Keys {
            self.open = false;
        } else {
            self.show_topic(Topic::Keys);
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.open = false;
            return;
        }
        let mut open = true;
        let max_height = (ctx.content_rect().height() * 0.7).max(200.0);
        egui::Window::new("Hilfe")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(480.0)
            .max_width(640.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                self.tabs(ui);
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .max_height(max_height)
                    .show(ui, |ui| self.body(ui));
            });
        self.open &= open;
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.topic != Topic::Keys, "Erklärung")
                .clicked()
            {
                self.topic = self.explain;
            }
            if ui
                .selectable_label(self.topic == Topic::Keys, "Tastenkürzel")
                .clicked()
            {
                self.topic = Topic::Keys;
            }
        });
    }

    fn body(&mut self, ui: &mut egui::Ui) {
        let entry = help::entry(self.topic);
        ui.heading(entry.title);
        if self.topic == Topic::Keys {
            ui.label(entry.what);
            ui.add_space(theme::SPACE_M);
            egui::Grid::new("keys-help")
                .num_columns(2)
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    for (key, effect) in keys::HELP {
                        ui.label(egui::RichText::new(key).monospace());
                        ui.label(effect);
                        ui.end_row();
                    }
                });
            return;
        }
        section(ui, "Was es tut", entry.what);
        section(ui, "Was sich ändert", entry.changes);
        section(ui, "Rückgängig machen", entry.undo);
        options(ui, entry);
        self.doc_link(ui, entry);
    }

    fn doc_link(&mut self, ui: &mut egui::Ui, entry: &HelpEntry) {
        let Some(doc) = entry.doc else { return };
        ui.add_space(theme::SPACE_M);
        ui.separator();
        ui.label(format!(
            "Ausführlich: {}, Abschnitt „{}“",
            doc.file.label(),
            doc.heading
        ));
        if ui.button("Ausführliche Doku öffnen").clicked() {
            self.notice = (self.opener)(doc.file).err().map(|e| (doc.file, e));
        }
        if let Some((file, message)) = &self.notice {
            let error = theme::tone_color(ui, format::Tone::Error);
            ui.colored_label(error, message);
            if ui.button("Pfad kopieren").clicked() {
                let path = paths::display(&help::expected_path(*file));
                ui.ctx().copy_text(path);
            }
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str, text: &str) {
    ui.add_space(theme::SPACE_M);
    ui.label(egui::RichText::new(title).strong());
    ui.add(egui::Label::new(text).wrap());
}

fn options(ui: &mut egui::Ui, entry: &HelpEntry) {
    if entry.options.is_empty() {
        return;
    }
    ui.add_space(theme::SPACE_M);
    ui.label(egui::RichText::new("Wichtige Optionen").strong());
    for (name, text) in entry.options {
        ui.add(egui::Label::new(format!("• {name}: {text}")).wrap());
    }
}
