//! Ansicht „Aufräumen“: Plan-Art und Optionen, Plan erzeugen oder öffnen, Review, Anwenden.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui;

use super::apply::{ApplyFlow, ID_APPLY};
use super::review::{ReviewState, ID_DEPENDENTS};
use super::undo_flow::{UndoFlow, TASK_CHECK as UNDO_CHECK, TASK_UNDO};
use crate::change::dedupe::KeepStrategy;
use crate::change::plan::Plan;
use crate::gui::review::ReviewModel;
use crate::gui::shell::{Answer, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::texts;
use crate::ops::plan::{plan, PlanKindRequest, PlanOut, PlanOutcome, PlanRequest};
use crate::paths;

const TASK_PLAN: &str = "Plan erzeugen";
const TASK_OPEN: &str = "Plan öffnen";
const TASK_RECENT: &str = "Pläne suchen";
const TASK_PICK_PLAN: &str = "Plan wählen";
const TASK_PICK_RULES: &str = "Regeldatei wählen";
const ID_UNDO: &str = "cleanup.undo";

const KINDS: [(&str, &str); 6] = [
    ("dedupe", "Duplikate"),
    ("junk", "Müll"),
    ("empty-dirs", "Leere Ordner"),
    ("archive", "Archivieren"),
    ("versions", "Versionen"),
    ("rules", "Nach Regeln"),
];

/// Ein vorhandener Plan auf der Platte.
#[derive(Debug, Clone)]
pub struct PlanFile {
    pub path: PathBuf,
    pub modified: SystemTime,
}

/// Die neuesten Plan-Dateien aus den Ordnern und deren Unterordnern (eine Ebene), neueste zuerst.
pub fn recent_plans(dirs: &[PathBuf], limit: usize) -> Vec<PlanFile> {
    let mut found: Vec<PlanFile> = Vec::new();
    let mut scan = |dir: &Path| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                    found.push(PlanFile { path, modified });
                }
            }
        }
    };
    for dir in dirs {
        scan(dir);
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    scan(&entry.path());
                }
            }
        }
    }
    found.sort_by_key(|a| std::cmp::Reverse(a.modified));
    found.truncate(limit);
    found
}

fn plan_dirs() -> Vec<PathBuf> {
    let config = crate::ops::load_config().unwrap_or_default();
    let mut dirs = Vec::new();
    if let Ok(gui) = paths::gui_plans_dir(config.gui.plans_dir.as_deref()) {
        dirs.push(gui);
    }
    if let Some(data) = paths::data_dir() {
        dirs.push(data.join("plans"));
    }
    dirs
}

pub struct CleanupView {
    kind: &'static str,
    keep: &'static str,
    keep_path: String,
    categories: String,
    older_than: String,
    min_age: String,
    rules_file: String,
    only: String,
    no_classify: bool,
    review: Option<ReviewState>,
    recent: Vec<PlanFile>,
    recent_loaded: bool,
    apply: ApplyFlow,
    undo: UndoFlow,
}

impl Default for CleanupView {
    fn default() -> Self {
        Self {
            kind: "dedupe",
            keep: "oldest",
            keep_path: String::new(),
            categories: String::new(),
            older_than: String::new(),
            min_age: String::new(),
            rules_file: String::new(),
            only: String::new(),
            no_classify: false,
            review: None,
            recent: Vec::new(),
            recent_loaded: false,
            apply: ApplyFlow::default(),
            undo: UndoFlow::default(),
        }
    }
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

fn opt(text: &str) -> Option<String> {
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

impl CleanupView {
    pub fn preselect(&mut self, kind: &'static str) {
        self.kind = KINDS
            .iter()
            .map(|(k, _)| *k)
            .find(|k| *k == kind)
            .unwrap_or("dedupe");
        self.review = None;
        self.apply.result = None;
    }

    fn kind_request(&self) -> PlanKindRequest {
        match self.kind {
            "junk" => PlanKindRequest::Junk {
                categories: words(&self.categories),
            },
            "empty-dirs" => PlanKindRequest::EmptyDirs,
            "archive" => PlanKindRequest::Archive {
                older_than: opt(&self.older_than),
            },
            "versions" => PlanKindRequest::Versions {
                min_age: opt(&self.min_age),
            },
            "rules" => PlanKindRequest::Rules {
                rules_file: opt(&self.rules_file).map(PathBuf::from),
                rules_text: None,
                only: words(&self.only),
                no_classify: self.no_classify,
                lock_held: false,
            },
            _ => PlanKindRequest::Dedupe {
                keep: self.keep_strategy().unwrap_or(KeepStrategy::Oldest),
            },
        }
    }

    /// Die gewählte Strategie; „path“ braucht einen absoluten Ordner.
    fn keep_strategy(&self) -> Result<KeepStrategy, String> {
        match self.keep {
            "newest" => Ok(KeepStrategy::Newest),
            "path" => format!("path:{}", self.keep_path.trim()).parse(),
            _ => Ok(KeepStrategy::Oldest),
        }
    }

    fn refresh_recent(&mut self, shell: &mut Shell) {
        self.recent_loaded = true;
        shell.spawn(Route::Cleanup, TASK_RECENT, TaskKind::Read, |_| {
            Ok(recent_plans(&plan_dirs(), 12))
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        ui.heading(texts::NAV_CLEANUP);
        if !self.recent_loaded {
            self.refresh_recent(shell);
        }
        if self.apply.result.is_some() {
            let (undo, close) = self.apply.result_ui(ui);
            if let Some((run, root)) = undo {
                self.undo.begin(shell, Route::Cleanup, run, Some(root));
            }
            if close {
                self.apply.result = None;
                self.review = None;
                self.recent_loaded = false;
            }
            return;
        }
        let Some(mut review) = self.review.take() else {
            self.form_ui(ui, shell);
            return;
        };
        let mut start_apply = false;
        let mut leave = false;
        ui.horizontal(|ui| {
            if ui.button("← Anderen Plan wählen").clicked() {
                leave = true;
            }
            if let Some(p) = &review.plan_path {
                ui.label(egui::RichText::new(paths::display(p)).monospace().weak());
            }
        });
        if !leave {
            start_apply = review.ui(ui, shell);
            if start_apply {
                let plan = review.model.plan().clone();
                let ids = review.model.selected_ids();
                self.apply
                    .begin(shell, &plan, review.plan_path.clone(), ids);
            }
            self.review = Some(review);
        } else {
            self.recent_loaded = false;
        }
        let _ = start_apply;
    }

    fn form_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let busy = shell.is_running(Route::Cleanup);
        ui.horizontal_wrapped(|ui| {
            for (key, label) in KINDS {
                ui.selectable_value(&mut self.kind, key, label);
            }
        });
        ui.add_space(6.0);
        match self.kind {
            "dedupe" => {
                ui.horizontal(|ui| {
                    ui.label("Welche Kopie bleibt:");
                    ui.selectable_value(&mut self.keep, "oldest", "älteste");
                    ui.selectable_value(&mut self.keep, "newest", "neueste");
                    ui.selectable_value(&mut self.keep, "path", "unterhalb von Ordner …");
                });
                if self.keep == "path" {
                    ui.horizontal(|ui| {
                        ui.label("Ordner (absolut):");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.keep_path).desired_width(320.0),
                        );
                    });
                }
            }
            "junk" => {
                ui.horizontal(|ui| {
                    ui.label("Kategorien (leer = aus der Config):");
                    ui.add(egui::TextEdit::singleline(&mut self.categories).desired_width(260.0));
                });
                ui.label(
                    egui::RichText::new("system, temp, downloads, installer oder eigene Regeln")
                        .weak(),
                );
            }
            "archive" => {
                ui.horizontal(|ui| {
                    ui.label("Älter als (leer = aus der Config, z. B. 2y):");
                    ui.add(egui::TextEdit::singleline(&mut self.older_than).desired_width(80.0));
                });
            }
            "versions" => {
                ui.horizontal(|ui| {
                    ui.label("Mindestalter (leer = aus der Config, z. B. 30d):");
                    ui.add(egui::TextEdit::singleline(&mut self.min_age).desired_width(80.0));
                });
            }
            "rules" => {
                ui.horizontal(|ui| {
                    ui.label("Regeldatei (leer = Standard):");
                    ui.add(egui::TextEdit::singleline(&mut self.rules_file).desired_width(320.0));
                    if ui.button("Wählen …").clicked() {
                        shell.spawn(Route::Cleanup, TASK_PICK_RULES, TaskKind::Read, |_| {
                            Ok(crate::platform::shell::pick_file(
                                None,
                                Some(("Regeldatei", &["toml"])),
                            ))
                        });
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Nur diese Regeln (Namen, kommagetrennt):");
                    ui.add(egui::TextEdit::singleline(&mut self.only).desired_width(200.0));
                });
                ui.checkbox(
                    &mut self.no_classify,
                    "Fehlende Inhalte nicht nachklassifizieren",
                );
            }
            _ => {}
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ready = shell.target.is_some() && !busy;
            if ui
                .add_enabled(ready, egui::Button::new("Plan erzeugen"))
                .clicked()
            {
                self.generate(shell);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Plan öffnen …"))
                .clicked()
            {
                shell.spawn(Route::Cleanup, TASK_PICK_PLAN, TaskKind::Read, |_| {
                    Ok(crate::platform::shell::pick_file(
                        None,
                        Some(("Plan", &["json"])),
                    ))
                });
            }
            match &shell.target {
                Some(t) => ui.label(format!("Ziel: {}", t.label())),
                None => ui.label(texts::NO_ROOT_YET),
            };
        });
        ui.add_space(10.0);
        ui.heading("Zuletzt erzeugte Pläne");
        if self.recent.is_empty() {
            ui.label("Noch keine Pläne.");
        }
        let mut open: Option<PathBuf> = None;
        for p in &self.recent {
            let when: chrono::DateTime<chrono::Local> = p.modified.into();
            if ui
                .button(format!(
                    "{}  {}",
                    when.format("%d.%m.%Y %H:%M"),
                    paths::display(&p.path)
                ))
                .clicked()
            {
                open = Some(p.path.clone());
            }
        }
        if let Some(path) = open {
            self.open_plan(shell, path);
        }
    }

    fn generate(&mut self, shell: &mut Shell) {
        let Some(target) = shell.target.as_ref().map(|t| t.spec()) else {
            return;
        };
        if self.kind == "dedupe" {
            if let Err(e) = self.keep_strategy() {
                return shell.message("Plan erzeugen", &e);
            }
        }
        let req = PlanRequest {
            target,
            kind: self.kind_request(),
            out: PlanOut::GuiDir,
        };
        shell.spawn(Route::Cleanup, TASK_PLAN, TaskKind::Write, move |ctx| {
            plan(&req, ctx)
        });
    }

    fn open_plan(&mut self, shell: &mut Shell, path: PathBuf) {
        shell.spawn(Route::Cleanup, TASK_OPEN, TaskKind::Read, move |_| {
            let plan = Plan::load(&path)?;
            Ok((plan, path))
        });
    }

    pub fn on_answer(&mut self, id: &str, answer: Answer, shell: &mut Shell) {
        match id {
            ID_APPLY => self.apply.on_answer(shell, answer),
            ID_UNDO => self.undo.on_answer(shell, Route::Cleanup, answer),
            ID_DEPENDENTS => {
                if let Some(r) = &mut self.review {
                    r.on_dependents_answer(answer.ok);
                }
            }
            _ => {}
        }
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        match name {
            super::apply::TASK_CHECK => self.apply.on_checked(result, shell),
            super::apply::TASK_APPLY => self.apply.on_applied(result, shell),
            UNDO_CHECK => self.undo.on_checked(result, shell, ID_UNDO),
            TASK_UNDO => {
                self.undo.on_done(result, shell);
            }
            TASK_PLAN => match result.map(|b| b.downcast::<PlanOutcome>()) {
                Ok(Ok(outcome)) => self.show_outcome(*outcome, shell),
                Ok(Err(_)) => {}
                Err(e) => shell.show_error(name, &e),
            },
            TASK_OPEN => match result.map(|b| b.downcast::<(Plan, PathBuf)>()) {
                Ok(Ok(loaded)) => {
                    let (plan, path) = *loaded;
                    let headline = format!(
                        "Plan vom {}: {} Aktionen, {}, {} übersprungen",
                        plan.created,
                        plan.actions.len(),
                        texts::bytes(plan.total_bytes()),
                        plan.skipped.len()
                    );
                    self.review = Some(ReviewState::new(
                        ReviewModel::new(plan),
                        Some(path),
                        headline,
                        Vec::new(),
                    ));
                }
                Ok(Err(_)) => {}
                Err(e) => shell.show_error(name, &e),
            },
            TASK_RECENT => {
                if let Ok(Ok(list)) = result.map(|b| b.downcast::<Vec<PlanFile>>()) {
                    self.recent = *list;
                }
            }
            TASK_PICK_PLAN => {
                if let Ok(Ok(path)) = result.map(|b| b.downcast::<Option<PathBuf>>()) {
                    if let Some(path) = *path {
                        self.open_plan(shell, path);
                    }
                }
            }
            TASK_PICK_RULES => {
                if let Ok(Ok(path)) = result.map(|b| b.downcast::<Option<PathBuf>>()) {
                    if let Some(path) = *path {
                        self.rules_file = paths::display(&path);
                    }
                }
            }
            _ => {}
        }
    }

    fn show_outcome(&mut self, outcome: PlanOutcome, shell: &mut Shell) {
        let mut notes: Vec<String> = Vec::new();
        for (reason, count) in &outcome.skipped_by_reason {
            notes.push(format!("übersprungen: {count} × {reason}"));
        }
        notes.extend(outcome.notes.iter().cloned());
        notes.extend(outcome.prep.hints.iter().cloned());
        notes.extend(outcome.prep.warnings.iter().cloned());
        if outcome.limit.is_some() {
            notes.push("Der Plan überschreitet die OneDrive-Obergrenze; beim Anwenden ist ein Haken nötig.".into());
        }
        let path = outcome.saved.clone();
        shell.generation += 1;
        self.recent_loaded = false;
        self.review = Some(ReviewState::new(
            ReviewModel::new(outcome.plan),
            path,
            outcome.headline,
            notes,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn neueste_plaene_zuerst_auch_aus_unterordnern() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("profil");
        std::fs::create_dir(&sub).unwrap();
        let old = dir.path().join("alt.json");
        let new = sub.join("neu.json");
        let other = dir.path().join("notiz.txt");
        for f in [&old, &new, &other] {
            std::fs::write(f, "{}").unwrap();
        }
        let past = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(past)
            .unwrap();
        let found = recent_plans(&[dir.path().to_path_buf()], 10);
        let names: Vec<_> = found
            .iter()
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["neu.json", "alt.json"]);
        assert_eq!(recent_plans(&[dir.path().to_path_buf()], 1).len(), 1);
    }

    #[test]
    fn fehlender_ordner_ergibt_keine_plaene() {
        assert!(recent_plans(&[PathBuf::from(r"Z:\gibt-es-nicht")], 5).is_empty());
    }

    #[test]
    fn woerter_trennen_nach_komma_und_leerraum() {
        assert_eq!(
            words("temp, logs  installer"),
            ["temp", "logs", "installer"]
        );
        assert!(words("  ").is_empty());
    }
}
