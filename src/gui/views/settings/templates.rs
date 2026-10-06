//! Zielstruktur-Vorlagen: eingebaute ansehen und als eigene kopieren, eigene bearbeiten.

use std::path::PathBuf;

use eframe::egui;

use super::doc_editor::DocEditor;
use crate::gui::shell::Shell;
use crate::paths;
use crate::settings::validate::template_text;
use crate::template::{builtin_names, builtin_text};

/// Ordner der eigenen Vorlagen neben der Config.
pub fn templates_dir() -> Option<PathBuf> {
    paths::config_dir().map(|d| d.join("templates"))
}

/// Eigene Vorlagendateien (`*.toml`), nach Namen sortiert.
pub fn own_templates() -> Vec<PathBuf> {
    let Some(dir) = templates_dir() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// Kopiert eine eingebaute Vorlage nach `<dir>\<name>.toml` (ohne etwas zu überschreiben; bei
/// vorhandener Datei wird durchnummeriert).
pub fn copy_builtin(name: &str, dir: &std::path::Path) -> Result<PathBuf, String> {
    let text = builtin_text(name).ok_or_else(|| format!("Keine eingebaute Vorlage „{name}“"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", paths::display(dir)))?;
    for n in 0.. {
        let file = dir.join(if n == 0 {
            format!("{name}.toml")
        } else {
            format!("{name}-{}.toml", n + 1)
        });
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
        {
            Ok(mut f) => {
                use std::io::Write;
                f.write_all(text.as_bytes())
                    .map_err(|e| format!("{}: {e}", paths::display(&file)))?;
                return Ok(file);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{}: {e}", paths::display(&file))),
        }
    }
    unreachable!("unendliche Schleife endet mit return")
}

#[derive(Default)]
pub struct TemplatesTab {
    viewing: Option<&'static str>,
    editor: Option<DocEditor>,
    message: Option<String>,
}

impl TemplatesTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Eingebaute Vorlagen");
            for name in builtin_names() {
                ui.horizontal(|ui| {
                    ui.label(name);
                    if ui.button("Ansehen").clicked() {
                        self.viewing = Some(name);
                    }
                    if ui.button("Als eigene Vorlage kopieren").clicked() {
                        match templates_dir()
                            .ok_or("Config-Ordner nicht ermittelbar".to_string())
                            .and_then(|d| copy_builtin(name, &d))
                        {
                            Ok(file) => {
                                self.message =
                                    Some(format!("Kopiert nach {}", paths::display(&file)));
                                self.editor = Some(DocEditor::new("Eigene Vorlage", file));
                            }
                            Err(e) => shell.message("Vorlage", &e),
                        }
                    }
                });
            }
            if let Some(name) = self.viewing {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(builtin_text(name).unwrap_or_default()).monospace());
            }
            ui.add_space(10.0);
            ui.heading("Eigene Vorlagen");
            let own = own_templates();
            if own.is_empty() {
                ui.label("Noch keine eigenen Vorlagen.");
            }
            for file in own {
                if ui.button(paths::display(&file)).clicked() {
                    self.editor = Some(DocEditor::new("Eigene Vorlage", file));
                }
            }
            if let Some(m) = &self.message {
                ui.label(egui::RichText::new(m).weak());
            }
            if let Some(ed) = &mut self.editor {
                ui.separator();
                ed.toolbar(ui, shell, &template_text);
                if ed.is_loaded() {
                    if !ed.raw_mode {
                        ed.raw.set_text(ed.current_text());
                        ed.raw_mode = true;
                    }
                    ed.raw.ui(ui, &template_text, 22);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kopie_ueberschreibt_nie_und_nummeriert_durch() {
        let dir = tempfile::tempdir().unwrap();
        let first = copy_builtin("para", dir.path()).unwrap();
        assert_eq!(first, dir.path().join("para.toml"));
        std::fs::write(&first, "# meine Änderung\n").unwrap();
        let second = copy_builtin("para", dir.path()).unwrap();
        assert_eq!(second, dir.path().join("para-2.toml"));
        assert_eq!(
            std::fs::read_to_string(&first).unwrap(),
            "# meine Änderung\n"
        );
        assert!(crate::settings::validate::template_text(
            &std::fs::read_to_string(&second).unwrap()
        )
        .is_ok());
        assert!(copy_builtin("gibt-es-nicht", dir.path()).is_err());
    }
}
