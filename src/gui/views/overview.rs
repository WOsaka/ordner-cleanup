//! Ansicht „Übersicht“: gescannte Wurzeln und Profile mit Score, letzte Läufe, ausstehende
//! Quarantäne. Scannen und Klassifizieren liegen in der Kopfleiste.

use std::path::PathBuf;

use anyhow::Result;
use chrono::Local;
use eframe::egui;

use super::undo_flow::{UndoFlow, TASK_CHECK as UNDO_CHECK, TASK_UNDO};
use crate::change::undo::{RunStatus, RunSummary};
use crate::change::RunId;
use crate::gui::format::{
    quarantine_text, review_text, root_status, run_row, score_style, time_of, trend, Tone,
};
use crate::gui::shell::{Answer, Choice, Goto, HeaderRequest, Page, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::texts;
use crate::gui::theme;
use crate::gui::widgets::table::{short_path_cell, status_cell, time_cell};
use crate::history::History;
use crate::index::RootInfo;
use crate::ops::admin::{index_roots, profiles, ProfileInfo};
use crate::ops::runs::runs;
use crate::paths;

const TASK_LOAD: &str = "Übersicht laden";
pub const ID_UNDO: &str = "overview.undo";

/// Eine gescannte Wurzel mit dem letzten Score.
pub struct RootRow {
    pub info: RootInfo,
    pub score: Option<u8>,
    pub previous_score: Option<u8>,
}

/// Alles, was die Übersicht zeigt.
pub struct OverviewData {
    pub roots: Vec<RootRow>,
    pub profiles: Vec<ProfileInfo>,
    pub runs: Vec<(PathBuf, Vec<RunSummary>)>,
    /// Fehler beim Lesen der Config (die GUI startet trotzdem)
    pub profile_error: Option<String>,
}

/// Quarantäne, die noch auf ein `purge` wartet: Läufe, die nicht gelöscht oder zurückgedreht sind.
pub fn pending_quarantine(runs: &[(PathBuf, Vec<RunSummary>)]) -> (usize, u64) {
    runs.iter()
        .flat_map(|(_, r)| r)
        .filter(|r| !matches!(r.status, RunStatus::Purged | RunStatus::Undone) && r.bytes > 0)
        .fold((0, 0), |(n, b), r| (n + 1, b + r.bytes))
}

/// Alle gescannten Wurzeln mit dem letzten und vorletzten Score (liest nur).
pub fn root_rows() -> Result<Vec<RootRow>> {
    let roots = match paths::index_path() {
        Ok(file) if file.exists() => index_roots()?,
        _ => Vec::new(),
    };
    let history = paths::history_path()
        .ok()
        .filter(|p| p.exists())
        .and_then(|p| History::open(&p).ok());
    Ok(roots
        .into_iter()
        .map(|info| {
            let series = history
                .as_ref()
                .and_then(|h| h.series(&info.path_key, "", 2).ok())
                .unwrap_or_default();
            let score = series.last().map(|p| p.score);
            let previous_score = (series.len() > 1).then(|| series[series.len() - 2].score);
            RootRow {
                info,
                score,
                previous_score,
            }
        })
        .collect())
}

fn load() -> Result<OverviewData> {
    let roots = root_rows()?;
    let (profiles, profile_error) = match profiles() {
        Ok(p) => (p, None),
        Err(e) => (Vec::new(), Some(format!("{e:#}"))),
    };
    Ok(OverviewData {
        roots,
        profiles,
        runs: runs(None).unwrap_or_default(),
        profile_error,
    })
}

#[derive(Default)]
pub struct OverviewView {
    data: Option<OverviewData>,
    /// Stand von `Shell::generation`, für den `data` geladen wurde
    loaded_generation: u64,
    undo: UndoFlow,
}

/// Score mit Trend, in der Farbe seiner Einordnung; der Tooltip nennt sie in Worten.
fn score_cell(ui: &mut egui::Ui, score: Option<u8>, previous: Option<u8>) {
    match score {
        Some(s) => {
            let style = score_style(s);
            let color = theme::tone_color(ui, style.tone);
            ui.colored_label(color, trend(score, previous))
                .on_hover_text(format!("{s} – {}", style.text));
        }
        None => {
            ui.label("–");
        }
    }
}

impl OverviewView {
    pub fn refresh(&mut self, shell: &mut Shell) {
        self.loaded_generation = shell.generation;
        shell.spawn(Route::Overview, TASK_LOAD, TaskKind::Read, |_| load());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        if shell.generation != self.loaded_generation {
            self.refresh(shell);
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading(texts::NAV_OVERVIEW);
            self.data_ui(ui, shell);
        });
    }

    fn data_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let Some(data) = &self.data else {
            ui.label("Wird geladen …");
            return;
        };
        ui.separator();
        ui.heading("Gescannte Ordner");
        if data.roots.is_empty() {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                ui.heading(texts::EMPTY_TITLE);
                ui.label(texts::EMPTY_TEXT);
                ui.add_space(8.0);
                let button = egui::Button::new(texts::EMPTY_BUTTON);
                if ui.add_sized([260.0, 36.0], button).clicked() {
                    shell.header_request = Some(HeaderRequest::PickAndScan);
                }
            });
            ui.add_space(16.0);
        }
        let now = Local::now();
        let mut chosen: Option<(PathBuf, Page)> = None;
        egui::Grid::new("roots").striped(true).show(ui, |ui| {
            for h in ["Ordner", "Letzter Scan", "Score", "Status", ""] {
                ui.strong(h);
            }
            ui.end_row();
            for row in &data.roots {
                let r = &row.info;
                short_path_cell(ui, &r.path, 48, shell);
                let stamp = r.finished_at.as_deref().or(r.started_at.as_deref());
                time_cell(ui, &time_of(now, stamp));
                score_cell(ui, row.score, row.previous_score);
                status_cell(ui, &root_status(r.status, r.error_count));
                ui.horizontal(|ui| {
                    if ui.button("Analyse").clicked() {
                        chosen = Some((PathBuf::from(&r.path), Page::Analysis));
                    }
                    if ui.button("Aufräumen").clicked() {
                        chosen = Some((PathBuf::from(&r.path), Page::Cleanup));
                    }
                });
                ui.end_row();
            }
        });
        if let Some((path, page)) = chosen {
            shell.target = Some(Choice::Folder(path));
            shell.goto = Some(Goto {
                page,
                ..Goto::default()
            });
        }

        ui.add_space(12.0);
        ui.heading("Profile");
        if let Some(e) = &data.profile_error {
            ui.colored_label(
                theme::tone_color(ui, Tone::Error),
                format!("Config nicht lesbar: {e}"),
            );
        } else if data.profiles.is_empty() {
            ui.label("Keine Profile in der config.toml.");
        }
        let mut open_review: Option<String> = None;
        egui::Grid::new("profiles").striped(true).show(ui, |ui| {
            for p in &data.profiles {
                ui.label(&p.name);
                short_path_cell(ui, &p.profile.root, 40, shell);
                match &p.last_run {
                    Some(r) => {
                        time_cell(ui, &time_of(now, Some(&r.started)));
                        ui.label(r.status.label());
                    }
                    None => {
                        ui.label("noch kein Lauf");
                        ui.label("");
                    }
                }
                score_cell(ui, p.last_point.as_ref().map(|pt| pt.score), None);
                match review_text(p.last_run.as_ref()) {
                    Some(text) => {
                        if ui
                            .link(text)
                            .on_hover_text("Inhalte in der Analyse ansehen")
                            .clicked()
                        {
                            open_review = Some(p.name.clone());
                        }
                    }
                    None => {
                        ui.label("");
                    }
                }
                if ui.button("Öffnen").clicked() {
                    shell.target = Some(Choice::Profile(p.name.clone()));
                }
                ui.end_row();
            }
        });
        if let Some(name) = open_review {
            shell.target = Some(Choice::Profile(name));
            shell.goto = Some(Goto {
                page: Page::Analysis,
                analysis_tab: Some("content"),
                ..Goto::default()
            });
        }

        ui.add_space(12.0);
        ui.heading("Letzte Läufe");
        let mut recent: Vec<(&PathBuf, &RunSummary)> = data
            .runs
            .iter()
            .flat_map(|(root, rs)| rs.iter().map(move |r| (root, r)))
            .collect();
        recent.sort_by(|a, b| b.1.started.cmp(&a.1.started));
        if recent.is_empty() {
            ui.label("Noch keine Läufe.");
        }
        let mut want_undo: Option<(PathBuf, RunId)> = None;
        if !recent.is_empty() {
            egui::Grid::new("recent-runs").striped(true).show(ui, |ui| {
                for h in ["Wann", "Ordner", "Was", "Ergebnis", "Größe", ""] {
                    ui.strong(h);
                }
                ui.end_row();
                for (root, run) in recent.into_iter().take(5) {
                    let row = run_row(now, root, run);
                    ui.label(&row.when)
                        .on_hover_text(format!("{}\nLauf {}", row.when_tooltip, row.run_id));
                    ui.label(&row.root).on_hover_text(&row.root_full);
                    ui.label(&row.kind);
                    status_cell(ui, &row.status);
                    ui.label(&row.size);
                    if ui
                        .add_enabled(row.can_undo, egui::Button::new("Rückgängig …"))
                        .clicked()
                    {
                        want_undo = Some(((*root).clone(), run.run.clone()));
                    }
                    ui.end_row();
                }
            });
        }
        let (count, bytes) = pending_quarantine(&data.runs);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(quarantine_text(count, bytes));
            if count > 0 && ui.button("Quarantäne leeren …").clicked() {
                shell.goto = Some(Goto {
                    page: Page::History,
                    ..Goto::default()
                });
            }
        });
        if let Some((root, run)) = want_undo {
            self.undo.begin(shell, Route::Overview, run, Some(root));
        }
    }

    pub fn on_answer(&mut self, id: &str, answer: Answer, shell: &mut Shell) {
        if id == ID_UNDO {
            self.undo.on_answer(shell, Route::Overview, answer);
        }
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        match name {
            UNDO_CHECK => return self.undo.on_checked(result, shell, ID_UNDO),
            TASK_UNDO => return self.undo.on_done(result, shell),
            _ => {}
        }
        if name == TASK_LOAD {
            match result.and_then(|b| {
                b.downcast::<OverviewData>()
                    .map_err(|_| crate::gui::tasks::TaskError::Failed("Typfehler".into()))
            }) {
                Ok(data) => self.data = Some(*data),
                Err(e) => shell.show_error(name, &e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::{ActionCounts, RunId};

    fn run(n: u32, status: RunStatus, bytes: u64) -> RunSummary {
        RunSummary {
            run: RunId::parse(&format!("20261003-12000{n}-ab12")).unwrap(),
            started: None,
            moved: 1,
            counts: ActionCounts::default(),
            bytes,
            status,
            expires: None,
        }
    }

    #[test]
    fn ausstehende_quarantaene_zaehlt_nur_offene_laeufe_mit_inhalt() {
        let runs = vec![(
            PathBuf::from(r"D:\Daten"),
            vec![
                run(1, RunStatus::Complete, 100),
                run(2, RunStatus::Purged, 50),
                run(3, RunStatus::Undone, 70),
                run(4, RunStatus::Partial, 0),
                run(5, RunStatus::PartiallyUndone, 5),
            ],
        )];
        assert_eq!(pending_quarantine(&runs), (2, 105));
    }
}
