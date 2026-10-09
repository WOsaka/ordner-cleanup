//! Ansicht „Verlauf“: Läufe mit Details und Rückgängig, Quarantäne leeren, Index-Wurzeln.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use chrono::Local;
use eframe::egui;

use super::undo_flow::{UndoFlow, TASK_CHECK as UNDO_CHECK, TASK_UNDO};
use crate::change::journal::{self, Entry};
use crate::change::quarantine;
use crate::change::undo::RunSummary;
use crate::change::RunId;
use crate::gui::format::{root_status, run_row};
use crate::gui::help::Topic;
use crate::gui::shell::{Answer, Dialog, Page, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::texts;
use crate::gui::widgets::help_button::heading_with_help;
use crate::gui::widgets::table::{short_path_cell, status_cell};
use crate::index::RootInfo;
use crate::ops::admin::{index_remove, index_roots};
use crate::ops::runs::{purge_candidates, purge_execute, runs, PurgeCandidate, PurgeCheck};
use crate::ops::status_label;
use crate::paths;

const TASK_LOAD: &str = "Läufe laden";
const TASK_DETAIL: &str = "Journal lesen";
const TASK_PURGE_CHECK: &str = "Quarantäne prüfen";
const TASK_PURGE: &str = "Quarantäne leeren";
const TASK_INDEX_REMOVE: &str = "Wurzel entfernen";
const ID_UNDO: &str = "history.undo";
const ID_PURGE: &str = "history.purge";
const ID_INDEX_REMOVE: &str = "history.index-remove";

/// Eine Zeile der Journal-Details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailRow {
    pub action: u32,
    pub text: String,
    pub status: String,
}

fn push(rows: &mut Vec<DetailRow>, index: &mut HashMap<u32, usize>, action: u32, text: String) {
    index.insert(action, rows.len());
    rows.push(DetailRow {
        action,
        text,
        status: "offen".into(),
    });
}

/// Fasst die Einträge eines Journals je Aktion zusammen (Absicht, dann Ergebnis).
pub fn detail_rows(entries: &[Entry]) -> Vec<DetailRow> {
    let mut rows: Vec<DetailRow> = Vec::new();
    let mut index: HashMap<u32, usize> = HashMap::new();
    for entry in entries {
        match entry {
            Entry::Intent {
                action, from, to, ..
            } => push(&mut rows, &mut index, *action, format!("{from} → {to}")),
            Entry::IntentRemoveDir { action, path, .. } => {
                push(
                    &mut rows,
                    &mut index,
                    *action,
                    format!("Ordner entfernt: {path}"),
                );
            }
            Entry::Done { action, .. } => {
                if let Some(&i) = index.get(action) {
                    rows[i].status = "ausgeführt".into();
                }
            }
            Entry::Skip { action, reason, .. } => {
                if let Some(&i) = index.get(action) {
                    rows[i].status = format!("übersprungen: {reason}");
                }
            }
            Entry::Fail { action, error, .. } => {
                if let Some(&i) = index.get(action) {
                    rows[i].status = format!("Fehler: {error}");
                }
            }
            Entry::UndoDone { action, .. } => {
                if let Some(&i) = index.get(action) {
                    rows[i].status = "zurückgedreht".into();
                }
            }
            Entry::UndoConflict { action, reason, .. } => {
                if let Some(&i) = index.get(action) {
                    rows[i].status = format!("Kollision beim Zurückdrehen: {reason}");
                }
            }
            _ => {}
        }
    }
    rows
}

struct HistoryData {
    runs: Vec<(PathBuf, Vec<RunSummary>)>,
    roots: Vec<RootInfo>,
}

fn load() -> Result<HistoryData> {
    let roots = match paths::index_path() {
        Ok(f) if f.exists() => index_roots().unwrap_or_default(),
        _ => Vec::new(),
    };
    Ok(HistoryData {
        runs: runs(None)?,
        roots,
    })
}

#[derive(Default)]
pub struct HistoryView {
    data: Option<HistoryData>,
    loaded_generation: Option<u64>,
    filter_root: Option<PathBuf>,
    detail: Option<(RunId, Vec<DetailRow>)>,
    purge_age: String,
    pending_purge: Vec<PurgeCandidate>,
    pending_index_remove: Option<PathBuf>,
    undo: UndoFlow,
}

impl HistoryView {
    fn reload(&mut self, shell: &mut Shell) {
        self.loaded_generation = Some(shell.generation);
        shell.spawn(Route::History, TASK_LOAD, TaskKind::Read, |_| load());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        heading_with_help(ui, texts::NAV_HISTORY, Topic::Page(Page::History), shell);
        if self.loaded_generation != Some(shell.generation) && !shell.is_running(Route::History) {
            self.reload(shell);
        }
        let Some(data) = self.data.take() else {
            ui.label("Wird geladen …");
            return;
        };
        let roots: Vec<PathBuf> = data.runs.iter().map(|(r, _)| r.clone()).collect();
        ui.horizontal(|ui| {
            ui.label("Wurzel:");
            egui::ComboBox::from_id_salt("hist-root")
                .selected_text(
                    self.filter_root
                        .as_ref()
                        .map_or("Alle".to_string(), |p| paths::display(p)),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filter_root, None, "Alle");
                    for r in &roots {
                        ui.selectable_value(
                            &mut self.filter_root,
                            Some(r.clone()),
                            paths::display(r),
                        );
                    }
                });
        });
        let mut want_detail: Option<(PathBuf, RunId)> = None;
        let mut want_undo: Option<(PathBuf, RunId)> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let now = Local::now();
            egui::Grid::new("runs").striped(true).show(ui, |ui| {
                for h in ["Wann", "Ordner", "Was", "Ergebnis", "Größe", "Läuft ab", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for (root, list) in &data.runs {
                    if self.filter_root.as_ref().is_some_and(|f| f != root) {
                        continue;
                    }
                    for r in list {
                        let row = run_row(now, root, r);
                        ui.label(&row.when)
                            .on_hover_text(format!("{}\nLauf {}", row.when_tooltip, row.run_id));
                        short_path_cell(ui, &row.root_full, 40, shell);
                        ui.label(&row.kind);
                        status_cell(ui, &row.status);
                        ui.label(&row.size);
                        ui.label(&row.expires);
                        ui.horizontal(|ui| {
                            if ui.button("Details").clicked() {
                                want_detail = Some((root.clone(), r.run.clone()));
                            }
                            if ui
                                .add_enabled(row.can_undo, egui::Button::new("Rückgängig …"))
                                .clicked()
                            {
                                want_undo = Some((root.clone(), r.run.clone()));
                            }
                        });
                        ui.end_row();
                    }
                }
            });
            if data.runs.is_empty() {
                ui.label("Noch keine Läufe.");
            }
            if let Some((run, rows)) = &self.detail {
                ui.add_space(crate::gui::theme::SPACE_M);
                ui.heading(format!("Lauf {run}"));
                for row in rows {
                    ui.label(
                        egui::RichText::new(format!("{}  [{}]", row.text, row.status)).monospace(),
                    );
                }
            }
            ui.add_space(crate::gui::theme::SPACE_L);
            ui.separator();
            self.purge_ui(ui, shell);
            ui.add_space(crate::gui::theme::SPACE_L);
            ui.separator();
            self.index_ui(ui, shell, &data.roots);
        });
        self.data = Some(data);
        if let Some((root, run)) = want_detail {
            shell.spawn(Route::History, TASK_DETAIL, TaskKind::Read, move |_| {
                let entries = journal::read(&quarantine::journal_path(&root, &run))?;
                Ok((run, detail_rows(&entries)))
            });
        }
        if let Some((root, run)) = want_undo {
            self.undo.begin(shell, Route::History, run, Some(root));
        }
    }

    fn purge_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        ui.heading("Quarantäne leeren");
        ui.horizontal(|ui| {
            ui.label("Mindestalter (leer = Frist aus der Config, z. B. 30d):");
            ui.add(egui::TextEdit::singleline(&mut self.purge_age).desired_width(70.0));
            if ui.button("Vorschau …").clicked() {
                let age = Some(self.purge_age.trim().to_string()).filter(|a| !a.is_empty());
                let root = self.filter_root.clone();
                shell.spawn(
                    Route::History,
                    TASK_PURGE_CHECK,
                    TaskKind::Read,
                    move |_| purge_candidates(age.as_deref(), root.as_deref()),
                );
            }
        });
    }

    fn index_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell, roots: &[RootInfo]) {
        ui.heading("Gescannte Wurzeln im Index");
        if roots.is_empty() {
            ui.label("Keine gescannten Wurzeln.");
        }
        let mut remove: Option<PathBuf> = None;
        for r in roots {
            ui.horizontal(|ui| {
                short_path_cell(ui, &r.path, 56, shell);
                status_cell(ui, &root_status(r.status, r.error_count));
                if ui.button("Aus dem Index entfernen …").clicked() {
                    remove = Some(PathBuf::from(&r.path));
                }
            });
        }
        if let Some(path) = remove {
            self.pending_index_remove = Some(path.clone());
            shell.confirm(Dialog::Confirm {
                id: ID_INDEX_REMOVE,
                title: "Wurzel entfernen".into(),
                text: format!(
                    "{} aus dem Index entfernen? Die Dateien selbst bleiben unberührt; \
                     für Analysen muss neu gescannt werden.",
                    paths::display(&path)
                ),
                ok_label: "Entfernen".into(),
                must_check: None,
                checked: false,
                danger: true,
            });
        }
    }

    pub fn on_answer(&mut self, id: &str, answer: Answer, shell: &mut Shell) {
        match id {
            ID_UNDO => self.undo.on_answer(shell, Route::History, answer),
            ID_PURGE => {
                let candidates = std::mem::take(&mut self.pending_purge);
                if answer.ok && !candidates.is_empty() {
                    shell.spawn(Route::History, TASK_PURGE, TaskKind::Write, move |ctx| {
                        let results = purge_execute(&candidates, ctx)?;
                        let failures: Vec<String> = results
                            .iter()
                            .filter_map(|(run, r)| r.as_ref().err().map(|e| format!("{run}: {e}")))
                            .collect();
                        Ok((results.len(), failures))
                    });
                }
            }
            ID_INDEX_REMOVE => {
                if let (true, Some(path)) = (answer.ok, self.pending_index_remove.take()) {
                    shell.spawn(
                        Route::History,
                        TASK_INDEX_REMOVE,
                        TaskKind::Write,
                        move |_| index_remove(&path),
                    );
                }
            }
            _ => {}
        }
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        match name {
            UNDO_CHECK => self.undo.on_checked(result, shell, ID_UNDO),
            TASK_UNDO => self.undo.on_done(result, shell),
            TASK_LOAD => match result.map(|b| b.downcast::<HistoryData>()) {
                Ok(Ok(data)) => self.data = Some(*data),
                Ok(Err(_)) => {}
                Err(e) => shell.show_error(name, &e),
            },
            TASK_DETAIL => {
                if let Ok(Ok(detail)) = result.map(|b| b.downcast::<(RunId, Vec<DetailRow>)>()) {
                    self.detail = Some(*detail);
                }
            }
            TASK_PURGE_CHECK => match result.map(|b| b.downcast::<PurgeCheck>()) {
                Ok(Ok(check)) => self.ask_purge(*check, shell),
                Ok(Err(_)) => {}
                Err(e) => shell.show_error(name, &e),
            },
            TASK_PURGE => match result.map(|b| b.downcast::<(usize, Vec<String>)>()) {
                Ok(Ok(done)) => {
                    let (n, failures) = *done;
                    let mut text = format!("{n} Läufe bearbeitet.");
                    for f in failures {
                        text.push_str(&format!("\nFehler bei {f}"));
                    }
                    shell.message(TASK_PURGE, &text);
                    shell.generation += 1;
                }
                Ok(Err(_)) => {}
                Err(e) => shell.show_error(name, &e),
            },
            TASK_INDEX_REMOVE => match result {
                Ok(_) => shell.generation += 1,
                Err(e) => shell.show_error(name, &e),
            },
            _ => {}
        }
    }

    fn ask_purge(&mut self, check: PurgeCheck, shell: &mut Shell) {
        if check.candidates.is_empty() {
            return shell.message(
                TASK_PURGE,
                &format!("Keine abgelaufenen Läufe (älter als {} Tage).", check.days),
            );
        }
        let mut text = format!(
            "{} Läufe ({}) endgültig löschen? Das lässt sich nicht rückgängig machen.\n",
            check.candidates.len(),
            texts::bytes(check.total_bytes())
        );
        for c in check.candidates.iter().take(15) {
            text.push_str(&format!(
                "\n{}  {}  {}  ({})",
                c.run.run,
                texts::bytes(c.run.bytes),
                status_label(c.run.status),
                paths::display(&c.root)
            ));
        }
        self.pending_purge = check.candidates;
        shell.confirm(Dialog::Confirm {
            id: ID_PURGE,
            title: "Quarantäne endgültig leeren".into(),
            text,
            ok_label: "Endgültig löschen".into(),
            must_check: None,
            checked: false,
            danger: true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::journal::EndStatus;
    use crate::change::SkipReason;

    fn run() -> RunId {
        RunId::parse("20261003-120000-ab12").unwrap()
    }

    #[test]
    fn details_fassen_absicht_und_ergebnis_je_aktion_zusammen() {
        let entries = vec![
            Entry::Intent {
                run: run(),
                action: 1,
                from: r"D:\a.txt".into(),
                to: r"D:\q\a.txt".into(),
                size: 1,
                hash: None,
                dest: Default::default(),
                is_dir: false,
            },
            Entry::Done {
                run: run(),
                action: 1,
            },
            Entry::IntentRemoveDir {
                run: run(),
                action: 2,
                path: r"D:\leer".into(),
                attrs: 0,
                mtime_ticks: 0,
                ctime_ticks: 0,
            },
            Entry::Skip {
                run: run(),
                action: 2,
                reason: SkipReason::NotEmpty,
            },
            Entry::Intent {
                run: run(),
                action: 3,
                from: r"D:\c.txt".into(),
                to: r"D:\q\c.txt".into(),
                size: 1,
                hash: None,
                dest: Default::default(),
                is_dir: false,
            },
            Entry::Fail {
                run: run(),
                action: 3,
                error: "gesperrt".into(),
            },
            Entry::UndoDone {
                run: run(),
                action: 1,
                to: None,
            },
            Entry::RunEnd {
                run: run(),
                status: EndStatus::Complete,
            },
        ];
        let rows = detail_rows(&entries);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, "zurückgedreht");
        assert_eq!(rows[0].text, r"D:\a.txt → D:\q\a.txt");
        assert!(rows[1].status.starts_with("übersprungen"), "{:?}", rows[1]);
        assert_eq!(rows[2].status, "Fehler: gesperrt");
    }

    #[test]
    fn aktion_ohne_ergebnis_bleibt_offen() {
        let entries = vec![Entry::Intent {
            run: run(),
            action: 1,
            from: "a".into(),
            to: "b".into(),
            size: 0,
            hash: None,
            dest: Default::default(),
            is_dir: false,
        }];
        assert_eq!(detail_rows(&entries)[0].status, "offen");
    }
}
