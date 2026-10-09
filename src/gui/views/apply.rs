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
use crate::ops::apply::{apply_check, apply_execute, plan_file_changed, ApplyCheck, ApplyResult};
use crate::ops::plan::save_subset;
use crate::ops::text::{apply_headline, status_line};
use crate::ops::{index_age_note, load_config};
use crate::paths;

pub const ID_APPLY: &str = "cleanup.apply";
pub const ID_RELOAD: &str = "cleanup.reload";
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

/// Ergebnis des Anwenden-Tasks.
enum Applied {
    /// Ausgeführt: Ergebnis, Plan-Datei (Original oder Teilplan), Wurzel
    Done(ApplyResult, Option<PathBuf>, String),
    /// Die Plan-Datei hat sich seit dem Laden verändert; nichts ausgeführt
    Changed(PathBuf),
}

#[derive(Default)]
pub struct ApplyFlow {
    pending: Option<Pending>,
    check_for: Option<Pending>,
    pub result: Option<ApplyView>,
    /// Plan-Datei für „Plan neu laden“, nachdem sie sich vor dem Anwenden verändert hatte
    pub reload: Option<PathBuf>,
}

/// Ziel der Aktionen als Text für den Dialog.
fn target_text(kind: PlanKind, quarantine_days: u32) -> String {
    match kind {
        PlanKind::Archive | PlanKind::Versions => "den Ordner „_Archiv“ in der Wurzel".into(),
        PlanKind::Rules => "ihre Zielordner unter der Wurzel".into(),
        PlanKind::EmptyDirs => "das Entfernen (Ordner werden bei „Rückgängig“ neu angelegt)".into(),
        _ => {
            let days = if quarantine_days == 1 { "Tag" } else { "Tage" };
            format!(
                "die Quarantäne (.ordner-cleanup in der Wurzel; {quarantine_days} {days}, \
                 per „Rückgängig“ holbar)"
            )
        }
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
        target_text(kind, check.quarantine_days)
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

/// Rumpf des Anwenden-Tasks: Plan-Datei prüfen, bei Teilauswahl den Teilplan speichern, ausführen.
fn execute(p: Pending, allow_large: bool, ctx: &crate::ops::OpCtx) -> anyhow::Result<Applied> {
    if let Some(path) = &p.original_path {
        if plan_file_changed(path, &p.original)? {
            return Ok(Applied::Changed(path.clone()));
        }
    }
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
    Ok(Applied::Done(result, file, p.plan.root.clone()))
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
            execute(p, allow_large, ctx)
        });
    }

    pub fn on_applied(&mut self, result: TaskResult, shell: &mut Shell) {
        match result {
            Ok(b) => match b.downcast::<Applied>().map(|a| *a) {
                Ok(Applied::Changed(path)) => {
                    shell.confirm(Dialog::Confirm {
                        id: ID_RELOAD,
                        title: "Plan wurde verändert".into(),
                        text: format!(
                            "Die Plan-Datei {} wurde seit dem Laden verändert. Es wurde nichts \
                             ausgeführt.\n\nPlan neu laden und erneut prüfen?",
                            paths::display(&path)
                        ),
                        ok_label: "Plan neu laden".into(),
                        must_check: None,
                        checked: false,
                        danger: false,
                    });
                    self.reload = Some(path);
                }
                Ok(Applied::Done(applied, file, root)) => {
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
                Err(_) => {}
            },
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
                    crate::gui::theme::tone_color(ui, crate::gui::format::Tone::Warn),
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
            quarantine_days: 30,
        }
    }

    #[test]
    fn dialogtext_nennt_die_konfigurierte_quarantaene_frist() {
        let with_days = |days| ApplyCheck {
            quarantine_days: days,
            ..check(None, None)
        };
        let text = confirm_text(PlanKind::Junk, &with_days(7), None, None);
        assert!(
            text.contains("7 Tage") && !text.contains("30 Tage"),
            "{text}"
        );
        let text = confirm_text(PlanKind::Junk, &with_days(1), None, None);
        assert!(text.contains("1 Tag,"), "{text}");
        let text = confirm_text(PlanKind::Archive, &with_days(7), None, None);
        assert!(!text.contains("7 Tage"), "{text}");
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
    fn veraenderte_plan_datei_wird_nicht_angewendet() {
        use crate::change::plan::{ActionType, PlannedAction, PLAN_VERSION};
        let dir = tempfile::tempdir().unwrap();
        let victims: Vec<PathBuf> = (1..=2)
            .map(|i| {
                let path = dir.path().join(format!("x{i}.tmp"));
                std::fs::write(&path, "x").unwrap();
                path
            })
            .collect();
        let action = |id: u32, path: &PathBuf| PlannedAction {
            id,
            action: ActionType::Quarantine,
            path: path.display().to_string(),
            size: 1,
            mtime_ticks: 0,
            mtime: String::new(),
            hash: None,
            keep: None,
            keep_hash: None,
            reason: "junk:temp".into(),
            target: None,
            is_dir: false,
            keep_fingerprint: None,
            source_fingerprint: None,
            files: None,
            rule: None,
        };
        let plan = Plan {
            version: PLAN_VERSION,
            created: "2026-10-09T10:00:00+02:00".into(),
            kind: PlanKind::Junk,
            root: dir.path().display().to_string(),
            keep_strategy: None,
            params: Default::default(),
            protected_paths: Vec::new(),
            actions: vec![action(1, &victims[0]), action(2, &victims[1])],
            skipped: vec![],
        };
        let file = dir.path().join("plan.json");
        let mut edited = plan.clone();
        edited.actions.pop();
        edited.save(&file).unwrap();

        for subset in [false, true] {
            let pending = Pending {
                plan: plan.clone(),
                original: plan.clone(),
                original_path: Some(file.clone()),
                ids: [1].into(),
                subset,
            };
            match execute(pending, false, &crate::ops::OpCtx::default()).unwrap() {
                Applied::Changed(path) => assert_eq!(path, file),
                Applied::Done(..) => panic!("veränderter Plan wurde angewendet"),
            }
        }
        assert!(victims.iter().all(|v| v.exists()), "nichts ausgeführt");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| !n.contains("auswahl")),
            "kein Teilplan gespeichert: {names:?}"
        );
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
