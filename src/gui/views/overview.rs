//! Ansicht „Übersicht“: gescannte Wurzeln und Profile mit Score, letzte Läufe, ausstehende
//! Quarantäne, darüber das Scannen und Klassifizieren.

use std::path::PathBuf;

use anyhow::Result;
use eframe::egui;

use super::scan::ScanPanel;
use crate::change::undo::{RunStatus, RunSummary};
use crate::gui::shell::{Choice, Goto, Page, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::texts;
use crate::history::History;
use crate::index::RootInfo;
use crate::ops::admin::{index_roots, profiles, ProfileInfo};
use crate::ops::runs::runs;
use crate::paths;

const TASK_LOAD: &str = "Übersicht laden";

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

fn load() -> Result<OverviewData> {
    let roots = match paths::index_path() {
        Ok(file) if file.exists() => index_roots()?,
        _ => Vec::new(),
    };
    let history = paths::history_path()
        .ok()
        .filter(|p| p.exists())
        .and_then(|p| History::open(&p).ok());
    let roots = roots
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
        .collect();
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
    scan: ScanPanel,
}

fn trend(score: Option<u8>, previous: Option<u8>) -> String {
    match (score, previous) {
        (Some(s), Some(p)) if s > p => format!("{s} (▲ {})", s - p),
        (Some(s), Some(p)) if s < p => format!("{s} (▼ {})", p - s),
        (Some(s), Some(_)) => format!("{s} (=)"),
        (Some(s), None) => s.to_string(),
        _ => "–".into(),
    }
}

impl OverviewView {
    pub fn refresh(&mut self, shell: &mut Shell) {
        shell.spawn(Route::Overview, TASK_LOAD, TaskKind::Read, |_| load());
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading(texts::NAV_OVERVIEW);
            let before = shell.generation;
            let names: Vec<String> = self
                .data
                .as_ref()
                .map(|d| d.profiles.iter().map(|p| p.name.clone()).collect())
                .unwrap_or_default();
            self.scan.ui(ui, shell, &names);
            if shell.generation != before {
                self.refresh(shell);
            }
            ui.add_space(12.0);
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
            ui.label(texts::NO_ROOT_YET);
        }
        let mut chosen: Option<(PathBuf, Page)> = None;
        egui::Grid::new("roots").striped(true).show(ui, |ui| {
            for h in ["Ordner", "Letzter Scan", "Score", "Status", ""] {
                ui.strong(h);
            }
            ui.end_row();
            for row in &data.roots {
                let r = &row.info;
                ui.label(&r.path);
                ui.label(r.finished_at.as_deref().or(r.started_at.as_deref()).unwrap_or("–"));
                ui.label(trend(row.score, row.previous_score));
                ui.label(format!("{:?}, {} Fehler", r.status, r.error_count));
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
                plan_kind: None,
            });
        }

        ui.add_space(12.0);
        ui.heading("Profile");
        if let Some(e) = &data.profile_error {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Config nicht lesbar: {e}"));
        } else if data.profiles.is_empty() {
            ui.label("Keine Profile in der config.toml.");
        }
        egui::Grid::new("profiles").striped(true).show(ui, |ui| {
            for p in &data.profiles {
                ui.label(&p.name);
                ui.label(&p.profile.root);
                ui.label(match &p.last_run {
                    Some(r) => format!("{} – {}", crate::ops::local_time(&r.started), r.status.label()),
                    None => "noch kein Lauf".into(),
                });
                ui.label(
                    p.last_point
                        .as_ref()
                        .map_or("–".to_string(), |pt| format!("Score {}", pt.score)),
                );
                if ui.button("Wählen").clicked() {
                    shell.target = Some(Choice::Profile(p.name.clone()));
                }
                ui.end_row();
            }
        });

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
        for (root, run) in recent.into_iter().take(5) {
            ui.label(format!(
                "{}  {}  {}  {}  ({})",
                run.run,
                run.started.as_deref().map(crate::ops::local_time).unwrap_or_default(),
                run.counts.short_text(run.bytes),
                crate::ops::status_label(run.status),
                paths::display(root)
            ));
        }
        let (count, bytes) = pending_quarantine(&data.runs);
        ui.add_space(8.0);
        ui.label(format!(
            "Quarantäne: {count} Läufe warten auf „Quarantäne leeren“ ({})",
            texts::bytes(bytes)
        ));
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        if name == TASK_LOAD {
            match result.and_then(|b| {
                b.downcast::<OverviewData>()
                    .map_err(|_| crate::gui::tasks::TaskError::Failed("Typfehler".into()))
            }) {
                Ok(data) => self.data = Some(*data),
                Err(e) => shell.show_error(name, &e),
            }
            return;
        }
        self.scan.on_finished(name, result, shell);
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

    #[test]
    fn trend_zeigt_richtung_und_differenz() {
        assert_eq!(trend(Some(80), Some(70)), "80 (▲ 10)");
        assert_eq!(trend(Some(60), Some(70)), "60 (▼ 10)");
        assert_eq!(trend(Some(60), Some(60)), "60 (=)");
        assert_eq!(trend(Some(60), None), "60");
        assert_eq!(trend(None, None), "–");
    }
}
