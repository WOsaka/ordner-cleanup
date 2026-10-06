//! Scannen und Klassifizieren: Ziel wählen, Optionen („Erweitert“), Fortschritt und Ergebnis.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use eframe::egui;

use crate::gui::shell::{Choice, Route, Shell, TaskResult};
use crate::gui::tasks::{TaskError, TaskKind};
use crate::gui::texts;
use crate::ops::classify::{classify, render_summary, ClassifyOutcome, ClassifyRequest};
use crate::ops::scan::{scan, ScanReport, ScanRequest};
use crate::ops::target::TargetSpec;
use crate::paths;

const TASK_PICK: &str = "Ordner wählen";
pub const TASK_SCAN: &str = "Scannen";
pub const TASK_CLASSIFY: &str = "Klassifizieren";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Retry {
    Scan,
    Classify,
}

#[derive(Default)]
pub struct ScanPanel {
    advanced: bool,
    exclude: String,
    summary_only: String,
    no_default_excludes: bool,
    reset_index: bool,
    threads: String,
    template: String,
    net_force: bool,
    no_llm: bool,
    force: bool,
    ext: String,
    only: String,
    result: String,
    retry: Option<Retry>,
}

fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

fn opt(text: &str) -> Option<String> {
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

/// Zusammenfassung eines Scans (wie in der Konsole).
pub fn scan_text(report: &ScanReport) -> String {
    let o = &report.outcome;
    let mut text = format!(
        "{} Dateien, {} Ordner, {} ({} Fehler/Warnungen)\n",
        texts::grouped(o.files),
        texts::grouped(o.dirs),
        texts::bytes(o.bytes),
        o.errors
    );
    for hint in &report.notes.hints {
        text.push_str(hint);
        text.push('\n');
    }
    if o.aborted {
        text.push_str("Scan abgebrochen; der Index bleibt konsistent, aber unvollständig.\n");
        return text;
    }
    if let Some(line) = &report.score_line {
        text.push_str(line);
        text.push('\n');
    }
    if let Some(w) = &report.history_warning {
        text.push_str(&format!("Warnung: Verlauf nicht aktualisiert: {w}\n"));
    }
    text
}

impl ScanPanel {
    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell, profile_names: &[String]) {
        let busy = shell.is_running(Route::Overview);
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label("Ziel:");
                match &shell.target {
                    Some(choice) => ui.strong(choice.label()),
                    None => ui.label("–"),
                };
                if ui
                    .add_enabled(!busy, egui::Button::new("Ordner wählen …"))
                    .clicked()
                {
                    shell.spawn(Route::Overview, TASK_PICK, TaskKind::Read, |_| {
                        Ok(crate::platform::shell::pick_folder(None))
                    });
                }
                if !profile_names.is_empty() {
                    egui::ComboBox::from_id_salt("profile-pick")
                        .selected_text("Profil …")
                        .show_ui(ui, |ui| {
                            for name in profile_names {
                                if ui.selectable_label(false, name).clicked() {
                                    shell.target = Some(Choice::Profile(name.clone()));
                                }
                            }
                        });
                }
            });
            ui.horizontal(|ui| {
                let has_target = shell.target.is_some();
                if ui
                    .add_enabled(!busy && has_target, egui::Button::new("Scannen"))
                    .clicked()
                {
                    self.start_scan(shell);
                }
                if ui
                    .add_enabled(!busy && has_target, egui::Button::new("Klassifizieren"))
                    .clicked()
                {
                    self.start_classify(shell, false);
                }
                if self.retry.is_some() && !busy && ui.button(texts::RETRY).clicked() {
                    match self.retry.take() {
                        Some(Retry::Scan) => self.start_scan(shell),
                        Some(Retry::Classify) => self.start_classify(shell, false),
                        None => {}
                    }
                }
            });
            ui.collapsing("Erweitert", |ui| {
                self.advanced = true;
                ui.checkbox(&mut self.net_force, "Netzlaufwerk trotzdem scannen");
                ui.checkbox(&mut self.no_default_excludes, "Standard-Ausschlüsse aus");
                ui.checkbox(&mut self.reset_index, "Index neu aufbauen");
                ui.horizontal(|ui| {
                    ui.label("Threads (leer = automatisch):");
                    ui.add(egui::TextEdit::singleline(&mut self.threads).desired_width(50.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Vorlage für Soll/Ist (para, johnny-decimal oder Pfad):");
                    ui.add(egui::TextEdit::singleline(&mut self.template).desired_width(220.0));
                });
                ui.label("Zusätzliche Ausschlüsse (ein Muster je Zeile):");
                ui.add(egui::TextEdit::multiline(&mut self.exclude).desired_rows(2));
                ui.label("Nur Summe (ein Muster je Zeile):");
                ui.add(egui::TextEdit::multiline(&mut self.summary_only).desired_rows(2));
                ui.separator();
                ui.label("Klassifizieren:");
                ui.checkbox(&mut self.no_llm, "Ohne LLM");
                ui.checkbox(&mut self.force, "Alles neu analysieren");
                ui.horizontal(|ui| {
                    ui.label("Nur Endungen (z. B. pdf,jpg):");
                    ui.add(egui::TextEdit::singleline(&mut self.ext).desired_width(140.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Nur Glob (relativ zur Wurzel):");
                    ui.add(egui::TextEdit::singleline(&mut self.only).desired_width(180.0));
                });
                if ui
                    .add_enabled(
                        !busy && shell.target.is_some(),
                        egui::Button::new("Text-Cache löschen"),
                    )
                    .clicked()
                {
                    self.start_classify(shell, true);
                }
            });
            if let Some((name, ctx)) = shell.running_ctx(Route::Overview) {
                ui.separator();
                let p = &ctx.progress;
                let (done, total) = (
                    p.done.load(Ordering::Relaxed),
                    p.total.load(Ordering::Relaxed),
                );
                ui.label(format!("{name}: {}", p.phase()));
                if total > 0 {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total as f32).text(format!(
                            "{} / {}",
                            texts::grouped(done),
                            texts::grouped(total)
                        )),
                    );
                } else {
                    ui.label(format!(
                        "{} Dateien, {} Ordner, {}, {} Fehler",
                        texts::grouped(done),
                        texts::grouped(p.dirs.load(Ordering::Relaxed)),
                        texts::bytes(p.bytes.load(Ordering::Relaxed)),
                        p.errors.load(Ordering::Relaxed)
                    ));
                }
                ui.label(egui::RichText::new(p.current()).monospace().weak());
            }
            if !self.result.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new(&self.result).monospace());
            }
        });
    }

    fn target_spec(&self, shell: &Shell) -> Option<TargetSpec> {
        shell.target.as_ref().map(|c| match c {
            Choice::Folder(path) => TargetSpec::Path {
                path: path.clone(),
                force: self.net_force,
            },
            Choice::Profile(name) => TargetSpec::Profile(name.clone()),
        })
    }

    fn start_scan(&mut self, shell: &mut Shell) {
        let Some(target) = self.target_spec(shell) else {
            return;
        };
        let request = ScanRequest {
            target,
            exclude: lines(&self.exclude),
            summary_only: lines(&self.summary_only),
            no_default_excludes: self.no_default_excludes,
            reset_index: self.reset_index,
            threads: self.threads.trim().parse().ok(),
            template: opt(&self.template),
        };
        self.result.clear();
        self.retry = Some(Retry::Scan);
        shell.spawn(Route::Overview, TASK_SCAN, TaskKind::Write, move |ctx| {
            scan(&request, ctx)
        });
    }

    fn start_classify(&mut self, shell: &mut Shell, clear: bool) {
        let Some(target) = self.target_spec(shell) else {
            return;
        };
        let request = ClassifyRequest {
            target,
            no_llm: self.no_llm,
            force: self.force,
            ext: self
                .ext
                .split(',')
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
                .collect(),
            only: opt(&self.only),
            clear,
        };
        self.result.clear();
        self.retry = Some(Retry::Classify);
        shell.spawn(
            Route::Overview,
            TASK_CLASSIFY,
            TaskKind::Write,
            move |ctx| classify(&request, ctx),
        );
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        match name {
            TASK_PICK => {
                if let Ok(Ok(path)) = result.map(|b| b.downcast::<Option<PathBuf>>()) {
                    if let Some(path) = *path {
                        shell.target = Some(Choice::Folder(path));
                    }
                }
            }
            TASK_SCAN => match result {
                Ok(boxed) => {
                    if let Ok(report) = boxed.downcast::<ScanReport>() {
                        self.result = scan_text(&report);
                        self.retry = None;
                    }
                    shell.generation += 1;
                }
                Err(e) => self.fail(name, &e, shell),
            },
            TASK_CLASSIFY => {
                match result {
                    Ok(boxed) => {
                        if let Ok(outcome) = boxed.downcast::<ClassifyOutcome>() {
                            self.result = match *outcome {
                                ClassifyOutcome::Cleared { root, entries } => format!(
                                "Inhalts- und OCR-Text-Cache von {} geleert ({entries} Einträge).",
                                paths::display(&root)
                            ),
                                ClassifyOutcome::Classified { root, run, summary } => {
                                    let mut text =
                                        render_summary(&paths::display(&root), &run, &summary);
                                    if run.aborted {
                                        text.push_str("Abgebrochen; bis dahin Analysiertes bleibt im Cache.\n");
                                    }
                                    text
                                }
                            };
                            self.retry = None;
                        }
                        shell.generation += 1;
                    }
                    Err(e) => self.fail(name, &e, shell),
                }
            }
            _ => {}
        }
    }

    fn fail(&mut self, name: &str, error: &TaskError, shell: &mut Shell) {
        if *error != TaskError::Busy {
            self.retry = None;
        }
        shell.show_error(name, error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Notes;
    use crate::scan::ScanOutcome;

    fn report(aborted: bool) -> ScanReport {
        let mut notes = Notes::default();
        notes.hint("Hinweis: x");
        ScanReport {
            root: PathBuf::from(r"D:\Daten"),
            outcome: ScanOutcome {
                files: 1234,
                dirs: 7,
                bytes: 2048,
                errors: 1,
                aborted,
                hash: Default::default(),
            },
            index_file: PathBuf::from("index.db"),
            score_line: Some("Health-Score 80 (erster Lauf)".into()),
            history_warning: None,
            notes,
        }
    }

    #[test]
    fn scan_text_nennt_zaehler_hinweise_und_score() {
        let text = scan_text(&report(false));
        assert!(text.contains("1.234 Dateien, 7 Ordner"), "{text}");
        assert!(
            text.contains("Hinweis: x") && text.contains("Health-Score 80"),
            "{text}"
        );
    }

    #[test]
    fn abgebrochener_scan_zeigt_keinen_score() {
        let text = scan_text(&report(true));
        assert!(
            text.contains("abgebrochen") && !text.contains("Health-Score"),
            "{text}"
        );
    }

    #[test]
    fn zeilen_und_optionen_werden_bereinigt() {
        assert_eq!(
            lines(" *.tmp \n\n  node_modules\n"),
            ["*.tmp", "node_modules"]
        );
        assert_eq!(opt("  "), None);
        assert_eq!(opt(" para "), Some("para".into()));
    }
}
