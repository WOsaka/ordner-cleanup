//! Anwenden: prüfen, Bestätigungsdialog (mit Haken für die Obergrenze), Teilplan, ausführen,
//! Ergebnisansicht.

use std::collections::HashSet;
use std::path::PathBuf;

use eframe::egui;

use crate::change::plan::{Plan, PlanKind};
use crate::change::RunId;
use crate::gui::shell::{Answer, Dialog, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::texts;
use crate::index::Index;
use crate::ops::apply::{apply_check, apply_execute, ApplyCheck, ApplyResult};
use crate::ops::plan::save_subset;
use crate::ops::text::{apply_headline, status_line};
use crate::ops::{index_age_note, load_config};
use crate::paths;

pub const ID_APPLY: &str = "cleanup.apply";
pub const TASK_CHECK: &str = "Plan prüfen";
pub const TASK_APPLY: &str = "Anwenden";

/// Ergebnis der Prüfung vor dem Dialog.
struct Checked {
    plan: Plan,
    check: ApplyCheck,
    index_note: Option<String>,
}

struct Pending {
    plan: Plan,
    original: Plan,
    original_path: Option<PathBuf>,
    ids: HashSet<u32>,
    subset: bool,
}

/// Ergebnisansicht eines Laufs.
pub struct ApplyView {
    pub headline: String,
    pub lines: Vec<String>,
    pub run: RunId,
    pub root: PathBuf,
    pub executed: usize,
    pub aborted: bool,
    pub register_warning: Option<String>,
    pub plan_file: Option<PathBuf>,
}

#[derive(Default)]
pub struct ApplyFlow {
    pending: Option<Pending>,
    check_for: Option<Pending>,
    pub result: Option<ApplyView>,
}

/// Ziel der Aktionen als Text für den Dialog.
fn target_text(kind: PlanKind) -> &'static str {
    match kind {
        PlanKind::Archive | PlanKind::Versions => "den Ordner „_Archiv“ in der Wurzel",
        PlanKind::Rules => "ihre Zielordner unter der Wurzel",
        PlanKind::EmptyDirs => "das Entfernen (Ordner werden bei „Rückgängig“ neu angelegt)",
        _ => "die Quarantäne (.ordner-cleanup in der Wurzel; 30 Tage, per „Rückgängig“ holbar)",
    }
}

/// Text des Bestätigungsdialogs.
pub fn confirm_text(
    kind: PlanKind,
    check: &ApplyCheck,
    index_note: Option<&str>,
    subset: Option<(usize, usize)>,
) -> String {
    let mut text = format!(
        "{}\nGröße: {}\nZiel: {}",
        check.counts.plan_text(),
        texts::bytes(check.bytes),
        target_text(kind)
    );
    if let Some((chosen, total)) = subset {
        text.push_str(&format!(
            "\n\nEs wird eine neue Plan-Datei mit {chosen} von {total} Einträgen angelegt; \
             das Original bleibt unverändert."
        ));
    }
    for w in &check.notes.warnings {
        text.push_str(&format!("\n\n{w}"));
    }
    if let Some(note) = index_note {
        text.push_str(&format!("\n\n{note}"));
    }
    text.push_str("\n\nCloud-only-Dateien werden nie gelesen und übersprungen.");
    if let Some(limit) = &check.limit {
        text.push_str(&format!("\n\n{limit}"));
    }
    text
}

impl ApplyFlow {
    /// Startet die Prüfung für die gewählten Einträge.
    pub fn begin(
        &mut self,
        shell: &mut Shell,
        plan: &Plan,
        plan_path: Option<PathBuf>,
        ids: HashSet<u32>,
    ) {
        if ids.is_empty() {
            return shell.message("Anwenden", "Es ist nichts ausgewählt.");
        }
        let subset = ids.len() < plan.actions.len();
        let candidate = if subset {
            plan.subset(&ids)
        } else {
            plan.clone()
        };
        let task_plan = candidate.clone();
        self.check_for = Some(Pending {
            plan: candidate,
            original: plan.clone(),
            original_path: plan_path,
            ids,
            subset,
        });
        shell.spawn(Route::Cleanup, TASK_CHECK, TaskKind::Read, move |_| {
            let check = apply_check(&task_plan)?;
            let index_note = paths::index_path()
                .ok()
                .filter(|p| p.exists())
                .and_then(|p| Index::open(&p).ok())
                .and_then(|index| {
                    crate::report::select_root(&index, Some(std::path::Path::new(&task_plan.root)))
                        .ok()
                })
                .and_then(|root| index_age_note(&root, chrono::Utc::now()));
            Ok(Checked {
                plan: task_plan,
                check,
                index_note,
            })
        });
    }

    pub fn on_checked(&mut self, result: TaskResult, shell: &mut Shell) {
        let checked = match result.map(|b| b.downcast::<Checked>()) {
            Ok(Ok(c)) => *c,
            Ok(Err(_)) => return,
            Err(e) => {
                self.check_for = None;
                return shell.show_error(TASK_CHECK, &e);
            }
        };
        let Some(pending) = self.check_for.take() else {
            return;
        };
        if checked.check.empty {
            return shell.message("Anwenden", "Der Plan enthält keine Aktionen.");
        }
        let subset = pending
            .subset
            .then_some((checked.plan.actions.len(), pending.original.actions.len()));
        let text = confirm_text(
            checked.plan.kind,
            &checked.check,
            checked.index_note.as_deref(),
            subset,
        );
        shell.confirm(Dialog::Confirm {
            id: ID_APPLY,
            title: "Anwenden".into(),
            text,
            ok_label: "Anwenden".into(),
            must_check: checked
                .check
                .limit
                .is_some()
                .then(|| "Obergrenze für diesen Plan aufheben".to_string()),
            checked: false,
            danger: true,
        });
        self.pending = Some(pending);
    }

    pub fn on_answer(&mut self, shell: &mut Shell, answer: Answer) {
        let Some(p) = self.pending.take() else { return };
        if !answer.ok {
            return;
        }
        let allow_large = answer.checked;
        shell.spawn(Route::Cleanup, TASK_APPLY, TaskKind::Write, move |ctx| {
            let file = if p.subset {
                let config = load_config()?;
                Some(save_subset(
                    p.original_path
                        .as_deref()
                        .unwrap_or(std::path::Path::new("plan.json")),
                    &p.original,
                    &p.ids,
                    &config,
                )?)
            } else {
                p.original_path.clone()
            };
            let name = file
                .as_ref()
                .and_then(|f| f.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "plan".into());
            let result = apply_execute(&p.plan, &name, allow_large, ctx)?;
            Ok((result, file, p.plan.root.clone()))
        });
    }

    pub fn on_applied(&mut self, result: TaskResult, shell: &mut Shell) {
        match result {
            Ok(b) => {
                if let Ok(done) = b.downcast::<(ApplyResult, Option<PathBuf>, String)>() {
                    let (applied, file, root) = *done;
                    let o = &applied.outcome;
                    let lines: Vec<String> = o.results.iter().filter_map(status_line).collect();
                    self.result = Some(ApplyView {
                        headline: apply_headline(o),
                        lines,
                        run: o.run.clone(),
                        root: PathBuf::from(root),
                        executed: o.executed(),
                        aborted: o.aborted,
                        register_warning: applied.register_warning,
                        plan_file: file,
                    });
                    shell.generation += 1;
                }
            }
            Err(e) => shell.show_error(TASK_APPLY, &e),
        }
    }

    /// Ergebnisansicht; `Some(run)`, wenn „Rückgängig“ gedrückt wurde, `true` bei „Weiter“.
    pub fn result_ui(&mut self, ui: &mut egui::Ui) -> (Option<(RunId, PathBuf)>, bool) {
        let mut undo = None;
        let mut close = false;
        if let Some(r) = &self.result {
            ui.heading("Ergebnis");
            ui.label(egui::RichText::new(&r.headline).strong());
            if r.aborted {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Abgebrochen; der Lauf ist teilweise ausgeführt und lässt sich zurückdrehen.",
                );
            }
            if let Some(w) = &r.register_warning {
                ui.label(format!("Hinweis: {w}"));
            }
            if let Some(f) = &r.plan_file {
                ui.label(format!("Plan-Datei: {}", paths::display(f)));
            }
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for line in &r.lines {
                        ui.label(egui::RichText::new(line.trim_start()).monospace());
                    }
                });
            ui.horizontal(|ui| {
                if r.executed > 0 && ui.button("Rückgängig …").clicked() {
                    undo = Some((r.run.clone(), r.root.clone()));
                }
                if ui.button("Weiter").clicked() {
                    close = true;
                }
            });
        }
        (undo, close)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::ActionCounts;
    use crate::ops::Notes;

    fn check(limit: Option<&str>, warn: Option<&str>) -> ApplyCheck {
        let mut notes = Notes::default();
        if let Some(w) = warn {
            notes.warn(w);
        }
        ApplyCheck {
            counts: ActionCounts {
                quarantined: 3,
                ..ActionCounts::default()
            },
            actions: 3,
            bytes: 2048,
            limit: limit.map(String::from),
            notes,
            empty: false,
        }
    }

    #[test]
    fn dialogtext_nennt_anzahl_groesse_ziel_und_warnungen() {
        let text = confirm_text(
            PlanKind::Junk,
            &check(Some("Obergrenze überschritten"), Some("Warnung: OneDrive")),
            Some("Hinweis: Scan ist 9 Tage alt"),
            None,
        );
        assert!(
            text.contains("3 Dateien in die Quarantäne verschieben"),
            "{text}"
        );
        assert!(
            text.contains("2.0 KiB") && text.contains("Quarantäne"),
            "{text}"
        );
        assert!(
            text.contains("OneDrive") && text.contains("9 Tage"),
            "{text}"
        );
        assert!(text.contains("Obergrenze überschritten"), "{text}");
        assert!(text.contains("Cloud-only"), "{text}");
    }

    #[test]
    fn dialogtext_nennt_den_teilplan_und_das_archiv() {
        let text = confirm_text(
            PlanKind::Archive,
            &check(None, None),
            None,
            Some((480, 500)),
        );
        assert!(text.contains("480 von 500"), "{text}");
        assert!(text.contains("_Archiv"), "{text}");
        assert!(text.contains("Original bleibt unverändert"), "{text}");
    }
}
