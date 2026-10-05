//! Eine bearbeitbare TOML-Datei mit Formular- und Roh-Ansicht, Speichern mit Prüfung, Sicherung
//! und Konflikterkennung.

use std::path::PathBuf;

use eframe::egui;
use toml_edit::DocumentMut;

use crate::gui::shell::Shell;
use crate::gui::widgets::form::FormState;
use crate::gui::widgets::toml_editor::TomlEditor;
use crate::paths;
use crate::settings::{EditableFile, SaveError, ValidationError};

pub struct DocEditor {
    pub label: &'static str,
    pub path: PathBuf,
    file: Option<EditableFile>,
    /// Die Datei ließ sich nicht laden (Syntaxfehler o. Ä.)
    pub load_error: Option<ValidationError>,
    pub form: FormState,
    pub raw: TomlEditor,
    pub raw_mode: bool,
    /// Die Datei wurde seit dem Laden von außen geändert
    pub conflict: bool,
    notice: Option<String>,
    pub save_error: Option<ValidationError>,
    saved_text: String,
}

impl DocEditor {
    pub fn new(label: &'static str, path: PathBuf) -> Self {
        let mut editor = Self {
            label,
            path,
            file: None,
            load_error: None,
            form: FormState::default(),
            raw: TomlEditor::default(),
            raw_mode: false,
            conflict: false,
            notice: None,
            save_error: None,
            saved_text: String::new(),
        };
        editor.reload();
        editor
    }

    pub fn reload(&mut self) {
        self.form.reset();
        self.conflict = false;
        self.save_error = None;
        match EditableFile::load_or_empty(&self.path) {
            Ok(file) => {
                self.saved_text = file.text();
                self.raw.set_text(self.saved_text.clone());
                self.file = Some(file);
                self.load_error = None;
            }
            Err(e) => {
                self.file = None;
                self.load_error = Some(e);
            }
        }
    }

    pub fn doc(&mut self) -> Option<&mut DocumentMut> {
        self.file.as_mut().map(EditableFile::doc_mut)
    }

    pub fn doc_ref(&self) -> Option<&DocumentMut> {
        self.file.as_ref().map(EditableFile::doc)
    }

    /// Aktueller Text (Formular- oder Roh-Ansicht).
    pub fn current_text(&self) -> String {
        if self.raw_mode {
            self.raw.text.clone()
        } else {
            self.file
                .as_ref()
                .map(EditableFile::text)
                .unwrap_or_default()
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.current_text() != self.saved_text
    }

    pub fn is_loaded(&self) -> bool {
        self.file.is_some()
    }

    /// Wechselt zwischen Formular und Roh-TOML; der Roh-Text muss dafür gültige Syntax haben.
    fn toggle_raw(&mut self) {
        if self.raw_mode {
            let text = self.raw.text.clone();
            match self.file.as_mut().map(|f| f.set_text(&text)) {
                Some(Ok(())) | None => {
                    self.raw_mode = false;
                    self.form.reset();
                }
                Some(Err(e)) => self.save_error = Some(e),
            }
        } else {
            let text = self
                .file
                .as_ref()
                .map(EditableFile::text)
                .unwrap_or_default();
            self.raw.set_text(text);
            self.raw_mode = true;
            self.save_error = None;
        }
    }

    fn save(&mut self, validate: &dyn Fn(&str) -> Result<(), ValidationError>, force: bool) {
        self.save_error = None;
        if self.raw_mode {
            let text = self.raw.text.clone();
            if let Some(Err(e)) = self.file.as_mut().map(|f| f.set_text(&text)) {
                self.save_error = Some(e);
                return;
            }
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        match file.save(validate, force) {
            Ok(result) => {
                self.saved_text = file.text();
                self.conflict = false;
                self.notice = Some(match result.backup {
                    Some(b) => format!("Gespeichert. Sicherung: {}", paths::display(&b)),
                    None => "Gespeichert.".into(),
                });
            }
            Err(SaveError::Invalid(e)) => self.save_error = Some(e),
            Err(SaveError::Conflict) => self.conflict = true,
            Err(SaveError::Io(e)) => self.save_error = Some(ValidationError::new(e)),
        }
    }

    /// Kopfleiste: Speichern, Neu laden, Roh-TOML, Editor; Meldungen und Konflikthinweis.
    pub fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        shell: &mut Shell,
        validate: &dyn Fn(&str) -> Result<(), ValidationError>,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(self.label).strong());
            ui.label(
                egui::RichText::new(paths::display(&self.path))
                    .monospace()
                    .weak(),
            );
        });
        if let Some(e) = &self.load_error {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!("Die Datei lässt sich nicht lesen: {e}"),
            );
        }
        ui.horizontal_wrapped(|ui| {
            let dirty = self.is_dirty();
            if ui
                .add_enabled(dirty && self.is_loaded(), egui::Button::new("Speichern"))
                .clicked()
            {
                self.save(validate, false);
            }
            if ui.button("Neu laden").clicked() {
                self.reload();
                self.notice = None;
            }
            if self.is_loaded() && ui.selectable_label(self.raw_mode, "Roh-TOML").clicked() {
                self.toggle_raw();
            }
            if ui.button("Im Editor öffnen").clicked() {
                if let Err(e) = crate::platform::shell::edit(&self.path) {
                    shell.message("Editor", &format!("{e:#}"));
                }
            }
            if dirty {
                ui.label(egui::RichText::new("ungespeicherte Änderungen").italics());
            }
        });
        if self.conflict {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(egui::Color32::YELLOW, "Die Datei wurde extern geändert.");
                if ui.button("Neu laden (Änderungen verwerfen)").clicked() {
                    self.reload();
                }
                if ui.button("Überschreiben").clicked() {
                    self.save(validate, true);
                }
            });
        }
        if let Some(e) = &self.save_error {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Nicht gespeichert: {e}"));
        }
        if let Some(n) = &self.notice {
            ui.label(egui::RichText::new(n).weak());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(_: &str) -> Result<(), ValidationError> {
        Ok(())
    }

    #[test]
    fn speichern_schreibt_und_ungueltig_schreibt_nichts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "top = 5\n").unwrap();
        let mut ed = DocEditor::new("Config", path.clone());
        assert!(!ed.is_dirty());
        crate::settings::fields::set(
            ed.doc().unwrap(),
            &[],
            "top",
            &crate::settings::fields::Value::Int(9),
        );
        assert!(ed.is_dirty());
        let reject = |_: &str| Err(ValidationError::new("nein").at_line(1));
        ed.save(&reject, false);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "top = 5\n");
        assert_eq!(ed.save_error.as_ref().unwrap().line, Some(1));
        ed.save(&ok, false);
        assert!(std::fs::read_to_string(&path).unwrap().contains("top = 9"));
        assert!(!ed.is_dirty() && ed.save_error.is_none());
    }

    #[test]
    fn externe_aenderung_wird_zum_konflikt_und_ueberschreiben_loest_ihn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "top = 5\n").unwrap();
        let mut ed = DocEditor::new("Config", path.clone());
        std::fs::write(&path, "top = 7\n").unwrap();
        crate::settings::fields::set(
            ed.doc().unwrap(),
            &[],
            "top",
            &crate::settings::fields::Value::Int(9),
        );
        ed.save(&ok, false);
        assert!(ed.conflict);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "top = 7\n");
        ed.save(&ok, true);
        assert!(!ed.conflict);
        assert!(std::fs::read_to_string(&path).unwrap().contains("top = 9"));
    }

    #[test]
    fn roh_umschalter_sperrt_bei_ungueltiger_syntax() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "top = 5\n").unwrap();
        let mut ed = DocEditor::new("Config", path);
        ed.toggle_raw();
        assert!(ed.raw_mode);
        ed.raw.text = "top = \n".into();
        ed.toggle_raw();
        assert!(ed.raw_mode, "bleibt im Roh-Modus");
        assert!(ed.save_error.is_some());
        ed.raw.text = "top = 6\n".into();
        ed.toggle_raw();
        assert!(!ed.raw_mode);
        assert!(ed.current_text().contains("top = 6"));
    }

    #[test]
    fn defekte_datei_zeigt_fehler_statt_zu_laden() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "top = \n").unwrap();
        let ed = DocEditor::new("Config", path);
        assert!(!ed.is_loaded());
        assert_eq!(ed.load_error.as_ref().unwrap().line, Some(1));
    }
}
