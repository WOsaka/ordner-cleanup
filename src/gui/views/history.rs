//! Ansicht „Verlauf“: Läufe mit Details und Rückgängig, Quarantäne leeren, Index-Wurzeln.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use chrono::Local;
use eframe::egui;
use egui_extras::{Column, TableBuilder};

use super::undo_flow::{UndoFlow, TASK_CHECK as UNDO_CHECK, TASK_UNDO};
use crate::change::journal::{self, Entry};
use crate::change::quarantine;
use crate::change::undo::{RunStatus, RunSummary};
use crate::change::RunId;
use crate::gui::format::{root_status, run_row};
use crate::gui::help::Topic;
use crate::gui::shell::{Answer, Dialog, Goto, Page, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::texts;
use crate::gui::widgets::help_button::heading_with_help;
use crate::gui::widgets::table::{short_path_cell, status_cell, wrapped_mono};
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
            Entry::UndoDone { action, to, .. } => {
                if let Some(&i) = index.get(action) {
                    rows[i].status = match to {
                        Some(to) => format!("zurückgeholt als {to}"),
                        None => "zurückgedreht".into(),
                    };
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

/// Hat der Lauf noch Dateien in der Quarantäne, die sich ansehen lassen?
fn can_view_content(run: &RunSummary) -> bool {
    run.bytes > 0 && !matches!(run.status, RunStatus::Purged | RunStatus::Unreadable)
}

const RUN_ROW_H: f32 = 24.0;
const COL_WHEN: f32 = 130.0;
const COL_WHAT: f32 = 140.0;
const COL_RESULT: f32 = 150.0;
const COL_SIZE: f32 = 90.0;
const COL_EXPIRES: f32 = 100.0;
const COL_BUTTONS: f32 = 330.0;
const COL_ROOT_MIN: f32 = 220.0;
const COL_GAP: f32 = 8.0;
const COLUMN_COUNT: f32 = 7.0;
/// Ungefähre Zeichenbreite des Standardtexts, um die Pfadkürzung an die Spalte anzupassen.
const CHAR_WIDTH: f32 = 7.5;

/// Spaltenbreiten der Lauf-Tabelle: Alle Spalten bis auf „Ordner“ haben feste Breiten, „Ordner“
/// bekommt den Rest, mindestens aber `COL_ROOT_MIN`. Ist das Fenster schmaler als `total`,
/// scrollt die Tabelle waagerecht.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunColumns {
    pub root: f32,
    pub root_chars: usize,
    pub total: f32,
}

pub fn run_columns(avail: f32) -> RunColumns {
    let fixed = COL_WHEN + COL_WHAT + COL_RESULT + COL_SIZE + COL_EXPIRES + COL_BUTTONS;
    let gaps = COL_GAP * COLUMN_COUNT;
    let avail = if avail.is_finite() { avail } else { 0.0 };
    let root = (avail - fixed - gaps).max(COL_ROOT_MIN);
    RunColumns {
        root,
        root_chars: (((root - COL_GAP) / CHAR_WIDTH) as usize).max(20),
        total: fixed + gaps + root,
    }
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
    /// Setzt Läufe und Detailzeilen ohne Laden (Tests der Oberfläche).
    #[doc(hidden)]
    pub fn set_runs(
        &mut self,
        runs: Vec<(PathBuf, Vec<RunSummary>)>,
        detail: Option<(RunId, Vec<DetailRow>)>,
        generation: u64,
    ) {
        self.data = Some(HistoryData {
            runs,
            roots: Vec::new(),
        });
        self.detail = detail;
        self.loaded_generation = Some(generation);
    }

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
        let mut want_view: Option<(PathBuf, RunId)> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let now = Local::now();
            let shown: Vec<(&PathBuf, &RunSummary)> = data
                .runs
                .iter()
                .filter(|(root, _)| self.filter_root.as_ref().is_none_or(|f| f == root))
                .flat_map(|(root, list)| list.iter().map(move |r| (root, r)))
                .collect();
            if shown.is_empty() {
                ui.label("Noch keine Läufe.");
            } else {
                let cols = run_columns(ui.available_width());
                egui::ScrollArea::horizontal()
                    .id_salt("runs-h")
                    .show(ui, |ui| {
                        TableBuilder::new(ui)
                            .id_salt("runs-table")
                            .striped(true)
                            .vscroll(false)
                            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                            .column(Column::exact(COL_WHEN))
                            .column(Column::exact(cols.root))
                            .column(Column::exact(COL_WHAT))
                            .column(Column::exact(COL_RESULT))
                            .column(Column::exact(COL_SIZE))
                            .column(Column::exact(COL_EXPIRES))
                            .column(Column::exact(COL_BUTTONS))
                            .header(RUN_ROW_H, |mut h| {
                                for title in
                                    ["Wann", "Ordner", "Was", "Ergebnis", "Größe", "Läuft ab", ""]
                                {
                                    h.col(|ui| {
                                        ui.strong(title);
                                    });
                                }
                            })
                            .body(|body| {
                                body.rows(RUN_ROW_H, shown.len(), |mut table_row| {
                                    let (root, r) = shown[table_row.index()];
                                    let row = run_row(now, root, r);
                                    table_row.col(|ui| {
                                        ui.label(&row.when).on_hover_text(format!(
                                            "{}
Lauf {}",
                                            row.when_tooltip, row.run_id
                                        ));
                                    });
                                    table_row.col(|ui| {
                                        short_path_cell(ui, &row.root_full, cols.root_chars, shell);
                                    });
                                    table_row.col(|ui| {
                                        ui.label(&row.kind);
                                    });
                                    table_row.col(|ui| status_cell(ui, &row.status));
                                    table_row.col(|ui| {
                                        ui.label(&row.size);
                                    });
                                    table_row.col(|ui| {
                                        ui.label(&row.expires);
                                    });
                                    table_row.col(|ui| {
                                        if ui.button("Details").clicked() {
                                            want_detail = Some((root.clone(), r.run.clone()));
                                        }
                                        if ui
                                            .add_enabled(
                                                can_view_content(r),
                                                egui::Button::new(texts::HISTORY_VIEW_CONTENT),
                                            )
                                            .on_hover_text(texts::HISTORY_VIEW_CONTENT_TIP)
                                            .clicked()
                                        {
                                            want_view = Some((root.clone(), r.run.clone()));
                                        }
                                        if ui
                                            .add_enabled(
                                                row.can_undo,
                                                egui::Button::new("Rückgängig …"),
                                            )
                                            .clicked()
                                        {
                                            want_undo = Some((root.clone(), r.run.clone()));
                                        }
                                    });
                                });
                            });
                    });
            }
            if let Some((run, rows)) = &self.detail {
                ui.add_space(crate::gui::theme::SPACE_M);
                ui.heading(format!("Lauf {run}"));
                for row in rows {
                    wrapped_mono(ui, &format!("{}  [{}]", row.text, row.status));
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
        if let Some(target) = want_view {
            shell.goto = Some(Goto {
                page: Page::Quarantine,
                quarantine_run: Some(target),
                ..Goto::default()
            });
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
    fn einzeln_zurueckgeholte_datei_nennt_den_neuen_namen() {
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
            Entry::UndoDone {
                run: run(),
                action: 1,
                to: Some(r"D:\a (2).txt".into()),
            },
        ];
        assert_eq!(
            detail_rows(&entries)[0].status,
            r"zurückgeholt als D:\a (2).txt"
        );
    }

    fn summary(bytes: u64, status: RunStatus) -> RunSummary {
        RunSummary {
            run: run(),
            started: None,
            moved: 1,
            counts: Default::default(),
            bytes,
            status,
            expires: None,
        }
    }

    #[test]
    fn inhalt_ansehen_nur_bei_dateien_in_der_quarantaene() {
        assert!(can_view_content(&summary(10, RunStatus::Complete)));
        assert!(can_view_content(&summary(10, RunStatus::PartiallyUndone)));
        assert!(!can_view_content(&summary(0, RunStatus::Complete)));
        assert!(!can_view_content(&summary(10, RunStatus::Purged)));
        assert!(!can_view_content(&summary(10, RunStatus::Unreadable)));
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

    #[test]
    fn breite_spalte_bekommt_den_rest_schmale_scrollt() {
        let wide = run_columns(1400.0);
        assert!(wide.root > 400.0, "{wide:?}");
        assert!(wide.root_chars >= 50, "{wide:?}");
        assert!((wide.total - 1400.0).abs() < 1.0, "{wide:?}");
        let narrow = run_columns(400.0);
        assert_eq!(narrow.root, COL_ROOT_MIN);
        assert!(narrow.total > 400.0, "{narrow:?}");
        assert!(narrow.root_chars >= 20, "{narrow:?}");
    }

    #[test]
    fn spaltenrechnung_bleibt_bei_unsinnigen_breiten_im_mindestmass() {
        for avail in [0.0, -50.0, f32::NAN, f32::INFINITY] {
            let c = run_columns(avail);
            assert_eq!(c.root, COL_ROOT_MIN, "{avail}");
            assert!(c.total.is_finite() && c.root_chars >= 20, "{avail}: {c:?}");
        }
    }
}
