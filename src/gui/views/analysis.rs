//! Ansicht „Analyse“: Tabs aus dem Berichtsmodell der gewählten Wurzel, Export.

use std::path::PathBuf;

use eframe::egui;

use super::analysis_tabs as tabs;
use crate::gui::shell::{Choice, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::texts;
use crate::ops::report::{export_report, report_model, ReportRequest, ReportView};
use crate::report::Format;

const TASK_LOAD: &str = "Analyse laden";
const TASK_EXPORT: &str = "Bericht exportieren";
const TASK_PICK: &str = "Zielordner wählen";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    SizeTree,
    TypesAge,
    Duplicates,
    Problems,
    Content,
    Health,
    Template,
    Export,
}

impl Tab {
    const ALL: [(Tab, &'static str); 8] = [
        (Tab::SizeTree, "Größenbaum"),
        (Tab::TypesAge, "Typen & Alter"),
        (Tab::Duplicates, "Duplikate"),
        (Tab::Problems, "Probleme & Struktur"),
        (Tab::Content, "Inhalte"),
        (Tab::Health, "Health & Verlauf"),
        (Tab::Template, "Soll/Ist"),
        (Tab::Export, "Exportieren"),
    ];
}

pub struct AnalysisView {
    view: Option<ReportView>,
    /// Wofür `view` geladen wurde
    loaded_for: Option<(Choice, u64)>,
    requested: Option<(Choice, u64)>,
    tab: Tab,
    html: bool,
    json: bool,
    csv: bool,
    out_dir: Option<PathBuf>,
    old_after: String,
    top: String,
    written: Vec<PathBuf>,
    error: Option<String>,
}

impl Default for AnalysisView {
    fn default() -> Self {
        Self {
            view: None,
            loaded_for: None,
            requested: None,
            tab: Tab::default(),
            html: true,
            json: false,
            csv: false,
            out_dir: None,
            old_after: String::new(),
            top: String::new(),
            written: Vec::new(),
            error: None,
        }
    }
}

fn request_for(choice: &Choice, old_after: &str, top: &str) -> ReportRequest {
    let mut req = ReportRequest {
        old_after: Some(old_after.trim().to_string()).filter(|t| !t.is_empty()),
        top: top.trim().parse().ok(),
        ..ReportRequest::default()
    };
    match choice {
        Choice::Folder(path) => req.path = Some(path.clone()),
        Choice::Profile(name) => req.profile = Some(name.clone()),
    }
    req
}

impl AnalysisView {
    /// Öffnet einen Tab von außen (`problems` oder `content`); unbekannte Schlüssel ändern nichts.
    pub fn preselect_tab(&mut self, key: &str) {
        match key {
            "problems" => self.tab = Tab::Problems,
            "content" => self.tab = Tab::Content,
            _ => {}
        }
    }

    fn load(&mut self, shell: &mut Shell, choice: Choice) {
        let key = (choice.clone(), shell.generation);
        self.requested = Some(key);
        let req = request_for(&choice, &self.old_after, &self.top);
        shell.spawn(Route::Analysis, TASK_LOAD, TaskKind::Read, move |ctx| {
            report_model(&req, ctx)
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        ui.heading(texts::NAV_ANALYSIS);
        let Some(choice) = shell.target.clone() else {
            ui.label(texts::NO_ROOT_YET);
            return;
        };
        let wanted = (choice.clone(), shell.generation);
        if self.requested.as_ref() != Some(&wanted) && !shell.is_running(Route::Analysis) {
            self.load(shell, choice.clone());
        }
        ui.horizontal(|ui| {
            ui.label(format!("Ziel: {}", choice.label()));
            if ui.button("Neu laden").clicked() && !shell.is_running(Route::Analysis) {
                self.load(shell, choice.clone());
            }
            if shell.is_running(Route::Analysis) {
                ui.spinner();
            }
        });
        let Some(view) = &self.view else {
            if let Some(e) = &self.error {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            } else if !shell.is_running(Route::Analysis) {
                ui.label("Für dieses Ziel gibt es noch keine Analyse. Erst scannen.");
            }
            return;
        };
        for w in &view.notes.warnings {
            ui.colored_label(egui::Color32::YELLOW, w);
        }
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in Tab::ALL {
                ui.selectable_value(&mut self.tab, tab, label);
            }
        });
        ui.separator();
        let tab = self.tab;
        if tab == Tab::Export {
            self.export_ui(ui, shell);
            return;
        }
        let view = self.view.as_ref().map(|v| &v.model);
        let Some(report) = view else { return };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match tab {
                Tab::SizeTree => tabs::size_tree(ui, report, shell),
                Tab::TypesAge => tabs::types_and_age(ui, report, shell),
                Tab::Duplicates => tabs::duplicates(ui, report, shell),
                Tab::Problems => tabs::problems(ui, report, shell),
                Tab::Content => tabs::content(ui, report, shell),
                Tab::Health => tabs::health(ui, report),
                Tab::Template => tabs::template(ui, report, shell),
                Tab::Export => {}
            });
    }

    fn export_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        ui.label("Formate:");
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.html, "HTML");
            ui.checkbox(&mut self.json, "JSON");
            ui.checkbox(&mut self.csv, "CSV");
        });
        ui.horizontal(|ui| {
            ui.label("Alt ab (z. B. 1y, 18m, 90d):");
            ui.add(egui::TextEdit::singleline(&mut self.old_after).desired_width(70.0));
            ui.label("Top-N:");
            ui.add(egui::TextEdit::singleline(&mut self.top).desired_width(50.0));
        });
        ui.horizontal(|ui| {
            ui.label(match &self.out_dir {
                Some(d) => crate::paths::display(d),
                None => "Zielordner: Standard (Datenordner)".into(),
            });
            if ui.button("Ordner wählen …").clicked() {
                shell.spawn(Route::Analysis, TASK_PICK, TaskKind::Read, |_| {
                    Ok(crate::platform::shell::pick_folder(None))
                });
            }
        });
        let formats = self.formats();
        if ui
            .add_enabled(
                !formats.is_empty() && !shell.is_running(Route::Analysis),
                egui::Button::new("Exportieren"),
            )
            .clicked()
        {
            self.start_export(shell, formats);
        }
        for file in self.written.clone() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(crate::paths::display(&file)).monospace());
                if file.extension().is_some_and(|e| e == "html")
                    && ui.button("Im Browser öffnen").clicked()
                {
                    if let Err(e) = crate::platform::shell::open(&file) {
                        shell.message("Öffnen", &format!("{e:#}"));
                    }
                }
            });
        }
    }

    fn formats(&self) -> Vec<Format> {
        [
            (self.html, Format::Html),
            (self.json, Format::Json),
            (self.csv, Format::Csv),
        ]
        .into_iter()
        .filter_map(|(on, f)| on.then_some(f))
        .collect()
    }

    fn start_export(&mut self, shell: &mut Shell, formats: Vec<Format>) {
        let Some(choice) = shell.target.clone() else {
            return;
        };
        let req = request_for(&choice, &self.old_after, &self.top);
        let out = self.out_dir.clone();
        shell.spawn(Route::Analysis, TASK_EXPORT, TaskKind::Read, move |ctx| {
            let view = report_model(&req, ctx)?;
            export_report(&view, &formats, out.as_deref())
        });
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        match (name, result) {
            (TASK_LOAD, Ok(b)) => {
                if let Ok(view) = b.downcast::<ReportView>() {
                    self.error = None;
                    self.view = Some(*view);
                    self.loaded_for = self.requested.clone();
                }
            }
            (TASK_EXPORT, Ok(b)) => {
                if let Ok(files) = b.downcast::<Vec<PathBuf>>() {
                    self.written = *files;
                }
            }
            (TASK_PICK, Ok(b)) => {
                if let Ok(dir) = b.downcast::<Option<PathBuf>>() {
                    if dir.is_some() {
                        self.out_dir = *dir;
                    }
                }
            }
            (TASK_LOAD, Err(e)) => {
                self.view = None;
                self.error = Some(e.to_string());
            }
            (_, Err(e)) => shell.show_error(name, &e),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_laesst_sich_von_aussen_vorwaehlen() {
        let mut view = AnalysisView::default();
        assert_eq!(view.tab, Tab::SizeTree);
        view.preselect_tab("problems");
        assert_eq!(view.tab, Tab::Problems);
        view.preselect_tab("content");
        assert_eq!(view.tab, Tab::Content);
    }

    #[test]
    fn unbekannter_tab_schluessel_aendert_nichts() {
        let mut view = AnalysisView::default();
        view.preselect_tab("content");
        view.preselect_tab("gibt-es-nicht");
        assert_eq!(view.tab, Tab::Content);
    }
}
