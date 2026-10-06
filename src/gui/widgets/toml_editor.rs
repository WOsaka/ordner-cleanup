//! Roh-TOML-Editor: Monospace, Zeilennummern, Validierung beim Tippen (entprellt, 300 ms) und
//! markierte Fehlerzeile.

use std::time::{Duration, Instant};

use eframe::egui::{self, text::LayoutJob, Color32, FontId, TextFormat};

use crate::settings::ValidationError;

const DEBOUNCE: Duration = Duration::from_millis(300);

pub struct TomlEditor {
    pub text: String,
    error: Option<ValidationError>,
    validated: Option<String>,
    last_edit: Instant,
}

impl Default for TomlEditor {
    fn default() -> Self {
        Self {
            text: String::new(),
            error: None,
            validated: None,
            last_edit: Instant::now(),
        }
    }
}

impl TomlEditor {
    pub fn with_text(text: String) -> Self {
        Self {
            text,
            ..Self::default()
        }
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.validated = None;
        self.error = None;
    }

    pub fn error(&self) -> Option<&ValidationError> {
        self.error.as_ref()
    }

    /// Prüft sofort (z. B. vor dem Speichern).
    pub fn validate_now(&mut self, validate: &dyn Fn(&str) -> Result<(), ValidationError>) -> bool {
        self.error = validate(&self.text).err();
        self.validated = Some(self.text.clone());
        self.error.is_none()
    }

    /// Zeichnet den Editor; `true`, wenn der Text geändert wurde.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        validate: &dyn Fn(&str) -> Result<(), ValidationError>,
        rows: usize,
    ) -> bool {
        let lines = self.text.lines().count().max(1) + 1;
        let error_line = self.error.as_ref().and_then(|e| e.line);
        let mut changed = false;
        egui::ScrollArea::vertical()
            .id_salt("toml-scroll")
            .max_height(rows as f32 * 16.0)
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.add(egui::Label::new(gutter(lines, error_line)).selectable(false));
                    let response = ui.add(
                        egui::TextEdit::multiline(&mut self.text)
                            .font(egui::TextStyle::Monospace)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(rows),
                    );
                    if response.changed() {
                        changed = true;
                        self.last_edit = Instant::now();
                    }
                });
            });
        let pending = self.validated.as_deref() != Some(self.text.as_str());
        if pending {
            if self.last_edit.elapsed() >= DEBOUNCE {
                self.validate_now(validate);
            } else {
                ui.ctx().request_repaint_after(DEBOUNCE);
            }
        }
        match &self.error {
            Some(e) => {
                ui.colored_label(Color32::LIGHT_RED, e.to_string());
            }
            None if self.validated.is_some() => {
                ui.colored_label(Color32::LIGHT_GREEN, "Gültig");
            }
            None => {}
        }
        changed
    }
}

/// Zeilennummern als eine Spalte; die Fehlerzeile ist rot.
fn gutter(lines: usize, error_line: Option<usize>) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font = FontId::monospace(14.0);
    for n in 1..=lines {
        let color = if error_line == Some(n) {
            Color32::LIGHT_RED
        } else {
            Color32::GRAY
        };
        job.append(
            &format!("{n:>3}\n"),
            0.0,
            TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    }
    job
}
