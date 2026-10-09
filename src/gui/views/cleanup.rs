//! Ansicht „Aufräumen“: Plan-Art und Optionen, Plan erzeugen oder öffnen, Review, Anwenden.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui;

use super::analysis::request_for;
use super::apply::{ApplyFlow, ID_APPLY};
use super::cleanup_cards::{
    card_layout, duration_placeholder, only_from_checks, rule_names, CardFacts, DurationField,
    CARDS,
};
use super::review::{ReviewState, ID_DEPENDENTS};
use super::undo_flow::{UndoFlow, TASK_CHECK as UNDO_CHECK, TASK_UNDO};
use crate::change::dedupe::KeepStrategy;
use crate::change::plan::{Plan, PlanKind};
use crate::gui::format::{self, DurationUnit};
use crate::gui::help::Topic;
use crate::gui::keys::KeyAction;
use crate::gui::review::ReviewModel;
use crate::gui::shell::{Answer, Choice, Page, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::gui::texts;
use crate::gui::theme;
use crate::gui::widgets::help_button::{heading_with_help, help_button_at};
use crate::ops::plan::{
    plan, resolve_rules_path, PlanKindRequest, PlanOut, PlanOutcome, PlanRequest,
};
use crate::paths;

const TASK_PLAN: &str = "Plan erzeugen";
const TASK_OPEN: &str = "Plan öffnen";
const TASK_RECENT: &str = "Pläne suchen";
const TASK_PICK_PLAN: &str = "Plan wählen";
const TASK_PICK_RULES: &str = "Regeldatei wählen";
const TASK_PICK_KEEP: &str = "Ordner wählen";
const TASK_FACTS: &str = "Zahlen laden";
const TASK_RULE_NAMES: &str = "Regeln lesen";
const ID_UNDO: &str = "cleanup.undo";

const CARD_HEIGHT: f32 = 110.0;

/// Zahlen für die Karten und die Standardwerte aus der Config (Platzhalter der Dauer-Felder).
struct CardsData {
    facts: Option<CardFacts>,
    archive_default: String,
    versions_default: String,
}

fn load_cards(choice: &Choice, ctx: &crate::ops::OpCtx) -> anyhow::Result<CardsData> {
    let config = match choice {
        Choice::Profile(name) => crate::ops::target::target(None, Some(name))
            .map(|t| t.config)
            .or_else(|_| crate::ops::load_config()),
        Choice::Folder(_) => crate::ops::load_config(),
    }
    .unwrap_or_default();
    // Ohne Scan gibt es keinen Report; die Karten zeigen dann nur ihre Beschreibung.
    let facts = crate::ops::report::report_model(&request_for(choice, "", ""), ctx)
        .ok()
        .map(|view| CardFacts::from_report(&view.model));
    Ok(CardsData {
        facts,
        archive_default: config.archive_older_than.clone(),
        versions_default: config.versions_min_age.clone(),
    })
}

/// Ein vorhandener Plan auf der Platte.
#[derive(Debug, Clone)]
pub struct PlanFile {
    pub path: PathBuf,
    pub modified: SystemTime,
    /// Kopfdaten; `None`, wenn sich die Datei nicht als Plan lesen ließ
    pub info: Option<PlanInfo>,
}

/// Art, Wurzel und Umfang eines Plans für die Liste „Zuletzt erzeugte Pläne“.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanInfo {
    pub kind: &'static str,
    pub root: String,
    pub actions: usize,
    pub bytes: u64,
}

/// Anzeigename einer Plan-Art.
pub fn kind_title(kind: PlanKind) -> &'static str {
    let key = match kind {
        PlanKind::Dedupe => "dedupe",
        PlanKind::DedupeDirs => "dedupe-dirs",
        PlanKind::Junk => "junk",
        PlanKind::EmptyDirs => "empty-dirs",
        PlanKind::Archive => "archive",
        PlanKind::Versions => "versions",
        PlanKind::Rules => "rules",
    };
    CARDS.iter().find(|c| c.key == key).map_or(key, |c| c.title)
}

/// Liest den Kopf eines Plans; eine Datei, die kein Plan ist, ergibt `None`.
pub fn plan_info(path: &Path) -> Option<PlanInfo> {
    let plan = Plan::load(path).ok()?;
    Some(PlanInfo {
        kind: kind_title(plan.kind),
        root: plan.root.clone(),
        actions: plan.actions.len(),
        bytes: plan.total_bytes(),
    })
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
                    found.push(PlanFile {
                        path,
                        modified,
                        info: None,
                    });
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
    // Die Suchordner können sich überlappen (`plans\_gui` liegt unter `plans`).
    let mut seen = HashSet::new();
    found.retain(|p| seen.insert(paths::path_key(&p.path)));
    found.sort_by_key(|a| std::cmp::Reverse(a.modified));
    found.truncate(limit);
    // Der Kopf wird erst für die angezeigten Pläne gelesen.
    for p in &mut found {
        p.info = plan_info(&p.path);
    }
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
    older_than: DurationField,
    min_age: DurationField,
    rules_file: String,
    /// Regeln der geladenen Datei mit Häkchen; leer, solange nichts geladen ist
    rules: Vec<(String, bool)>,
    rules_error: Option<String>,
    /// Für welchen Dateinamen `rules` zuletzt angefordert wurde
    rules_requested: Option<String>,
    no_classify: bool,
    cards: Option<CardsData>,
    /// Ziel und Scan-Stand, für die `cards` angefordert wurden
    cards_requested: Option<(Choice, u64)>,
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
            older_than: DurationField::new(DurationUnit::Years),
            min_age: DurationField::new(DurationUnit::Days),
            rules_file: String::new(),
            rules: Vec::new(),
            rules_error: None,
            rules_requested: None,
            no_classify: false,
            cards: None,
            cards_requested: None,
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
    /// Ist gerade ein Plan zur Prüfung offen (nicht die Ergebnisansicht)?
    pub fn review_open(&self) -> bool {
        self.review.is_some() && self.apply.result.is_none()
    }

    pub fn review_key(&mut self, action: KeyAction, ctx: &egui::Context, shell: &mut Shell) {
        if let Some(review) = self.review.as_mut().filter(|_| self.apply.result.is_none()) {
            review.handle_key(action, ctx, shell);
        }
    }

    /// „Plan öffnen …“ (Knopf und Strg+O).
    pub fn request_open_plan(&self, shell: &mut Shell) {
        if self.review.is_some() || self.apply.result.is_some() {
            return;
        }
        shell.spawn(Route::Cleanup, TASK_PICK_PLAN, TaskKind::Read, |_| {
            Ok(crate::platform::shell::pick_file(
                None,
                Some(("Plan", &["json"])),
            ))
        });
    }

    pub fn preselect(&mut self, kind: &'static str) {
        self.kind = CARDS
            .iter()
            .map(|c| c.key)
            .find(|k| *k == kind)
            .unwrap_or("dedupe");
        self.review = None;
        self.apply.result = None;
    }

    fn kind_request(&self) -> Result<PlanKindRequest, String> {
        Ok(match self.kind {
            "junk" => PlanKindRequest::Junk {
                categories: words(&self.categories),
            },
            "empty-dirs" => PlanKindRequest::EmptyDirs,
            "archive" => PlanKindRequest::Archive {
                older_than: self.older_than.arg()?,
            },
            "versions" => PlanKindRequest::Versions {
                min_age: self.min_age.arg()?,
            },
            "rules" => PlanKindRequest::Rules {
                rules_file: opt(&self.rules_file).map(PathBuf::from),
                rules_text: None,
                only: only_from_checks(&self.rules)?,
                no_classify: self.no_classify,
                lock_held: false,
            },
            "dedupe-dirs" => PlanKindRequest::DedupeDirs {
                keep: self.keep_strategy()?,
                min_size: None,
            },
            _ => PlanKindRequest::Dedupe {
                keep: self.keep_strategy()?,
            },
        })
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
        heading_with_help(ui, texts::NAV_CLEANUP, Topic::Page(Page::Cleanup), shell);
        if !self.recent_loaded {
            self.refresh_recent(shell);
        }
        if self.apply.result.is_some() {
            let (undo, close) = egui::ScrollArea::vertical()
                .id_salt("cleanup.result")
                .auto_shrink([false, false])
                .show(ui, |ui| self.apply.result_ui(ui))
                .inner;
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
            egui::ScrollArea::vertical()
                .id_salt("cleanup.form")
                .auto_shrink([false, false])
                .show(ui, |ui| self.form_ui(ui, shell));
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

    /// Zahlen neu anfordern, wenn Ziel oder Scan-Stand wechseln.
    fn ensure_cards(&mut self, shell: &mut Shell) {
        let Some(choice) = shell.target.clone() else {
            self.cards = None;
            self.cards_requested = None;
            return;
        };
        let key = (choice.clone(), shell.generation);
        if self.cards_requested.as_ref() == Some(&key) {
            return;
        }
        self.cards_requested = Some(key);
        shell.spawn(Route::Cleanup, TASK_FACTS, TaskKind::Read, move |ctx| {
            load_cards(&choice, ctx)
        });
    }

    /// Liest die Regeln der gewählten Datei, sobald die Karte „Nach Regeln“ offen ist.
    fn ensure_rules(&mut self, shell: &mut Shell) {
        let wanted = self.rules_file.trim().to_string();
        if self.kind != "rules" || self.rules_requested.as_deref() == Some(wanted.as_str()) {
            return;
        }
        self.rules_requested = Some(wanted);
        let file = opt(&self.rules_file).map(PathBuf::from);
        shell.spawn(Route::Cleanup, TASK_RULE_NAMES, TaskKind::Read, move |_| {
            let config = crate::ops::load_config().unwrap_or_default();
            let names: Result<Vec<String>, String> =
                resolve_rules_path(file.as_deref(), &config, paths::config_dir().as_deref())
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|path| rule_names(&path, &config));
            Ok(names)
        });
    }

    fn cards_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let now = chrono::Local::now();
        let facts = self.cards.as_ref().and_then(|c| c.facts.as_ref());
        match facts {
            None => {
                ui.weak("Keine Zahlen: Dieser Ordner ist noch nicht gescannt.");
            }
            Some(f) => {
                if let Some(note) = f.scanned().and_then(|t| format::stale_note(t, now)) {
                    let warn = theme::tone_color(ui, format::Tone::Warn);
                    ui.colored_label(warn, note);
                }
            }
        }
        let palette = theme::palette(ui.visuals().dark_mode);
        let mut chosen: Option<&'static str> = None;
        // Frames brechen in einem umbrechenden Layout nicht um; das Grid ordnet nach Breite.
        let spacing = 8.0;
        let chrome = egui::Frame::group(ui.style()).total_margin().sum().x;
        let (columns, card_width) = card_layout(ui.available_width(), chrome, spacing);
        egui::Grid::new("cards")
            .spacing([spacing, spacing])
            .show(ui, |ui| {
                for (index, card) in CARDS.iter().enumerate() {
                    let selected = self.kind == card.key;
                    let color = if selected {
                        palette.accent
                    } else {
                        ui.visuals().widgets.noninteractive.bg_stroke.color
                    };
                    let frame = egui::Frame::group(ui.style())
                        .stroke(egui::Stroke::new(if selected { 2.0 } else { 1.0 }, color));
                    let response = frame
                        .show(ui, |ui| {
                            // Der Frame erbt das Links-nach-rechts-Layout der Grid-Zelle; ohne
                            // `vertical` stünde die Zahlzeile neben dem Text und bekäme keine Breite.
                            ui.vertical(|ui| {
                                // Auswählbarer Text finge den Klick ab, der die Karte wählen soll.
                                ui.style_mut().interaction.selectable_labels = false;
                                ui.set_width(card_width);
                                ui.set_min_height(CARD_HEIGHT);
                                ui.label(egui::RichText::new(card.title).strong());
                                ui.add(egui::Label::new(card.text).wrap());
                                if let Some(n) = facts.and_then(|f| f.number_text(card.key)) {
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(n).strong()).wrap(),
                                    );
                                }
                            });
                        })
                        .response;
                    let click = response
                        .interact(egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    click.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::Button,
                            true,
                            selected,
                            card.title,
                        )
                    });
                    if click.clicked() {
                        chosen = Some(card.key);
                    }
                    // Das „?“ wird nach der Kartenfläche gezeichnet und liegt damit darüber:
                    // sein Klick wählt die Karte nicht.
                    let spot = egui::Rect::from_min_size(
                        egui::pos2(response.rect.right() - 30.0, response.rect.top() + 6.0),
                        egui::vec2(24.0, 20.0),
                    );
                    if help_button_at(ui, spot, Topic::Card(card.key)) {
                        shell.open_help(Topic::Card(card.key));
                    }
                    if (index + 1) % columns == 0 {
                        ui.end_row();
                    }
                }
            });
        if let Some(key) = chosen {
            self.kind = key;
        }
    }

    fn duration_row(
        ui: &mut egui::Ui,
        label: &str,
        field: &mut DurationField,
        config_default: &str,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.label(label);
            ui.add(
                egui::TextEdit::singleline(&mut field.amount)
                    .desired_width(60.0)
                    .hint_text(duration_placeholder(config_default)),
            );
            egui::ComboBox::from_id_salt(label)
                .selected_text(match field.unit {
                    DurationUnit::Days => "Tage",
                    DurationUnit::Months => "Monate",
                    DurationUnit::Years => "Jahre",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut field.unit, DurationUnit::Days, "Tage");
                    ui.selectable_value(&mut field.unit, DurationUnit::Months, "Monate");
                    ui.selectable_value(&mut field.unit, DurationUnit::Years, "Jahre");
                });
        });
    }

    fn options_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let (archive_default, versions_default) = self
            .cards
            .as_ref()
            .map_or(("2y".to_string(), "30d".to_string()), |c| {
                (c.archive_default.clone(), c.versions_default.clone())
            });
        match self.kind {
            "dedupe" | "dedupe-dirs" => {
                let noun = if self.kind == "dedupe-dirs" {
                    "Ordner"
                } else {
                    "Datei"
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(if self.kind == "dedupe-dirs" {
                        "Welcher Ordner bleibt (Namen wie „Kopie“ oder „Backup“ zählen zuerst):"
                    } else {
                        "Welche Kopie bleibt:"
                    });
                    let oldest = format!("Der älteste {noun} bleibt");
                    let newest = format!("Der neueste {noun} bleibt");
                    let in_folder = "Die Kopie in einem bestimmten Ordner bleibt";
                    let text = match self.keep {
                        "newest" => newest.as_str(),
                        "path" => in_folder,
                        _ => oldest.as_str(),
                    };
                    egui::ComboBox::from_id_salt("keep")
                        .selected_text(text)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.keep, "oldest", oldest.as_str());
                            ui.selectable_value(&mut self.keep, "newest", newest.as_str());
                            ui.selectable_value(&mut self.keep, "path", in_folder);
                        });
                });
                if self.keep == "path" {
                    ui.horizontal_wrapped(|ui| {
                        if self.keep_path.trim().is_empty() {
                            ui.weak("Noch kein Ordner gewählt");
                        } else {
                            ui.label(format::short_path(&self.keep_path, 56))
                                .on_hover_text(&self.keep_path);
                        }
                        if ui.button("Ordner wählen …").clicked() {
                            shell.spawn(Route::Cleanup, TASK_PICK_KEEP, TaskKind::Read, |_| {
                                Ok(crate::platform::shell::pick_folder(None))
                            });
                        }
                    });
                }
            }
            "junk" => {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Kategorien (leer = aus der Config):");
                    ui.add(egui::TextEdit::singleline(&mut self.categories).desired_width(260.0));
                });
                ui.label(
                    egui::RichText::new("system, temp, downloads, installer oder eigene Regeln")
                        .weak(),
                );
            }
            "archive" => {
                Self::duration_row(ui, "Älter als:", &mut self.older_than, &archive_default)
            }
            "versions" => {
                Self::duration_row(ui, "Mindestalter:", &mut self.min_age, &versions_default)
            }
            "rules" => {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Regeldatei:");
                    if self.rules_file.trim().is_empty() {
                        ui.weak("Standard");
                    } else {
                        ui.label(format::short_path(&self.rules_file, 56))
                            .on_hover_text(&self.rules_file);
                    }
                    if ui.button("Wählen …").clicked() {
                        shell.spawn(Route::Cleanup, TASK_PICK_RULES, TaskKind::Read, |_| {
                            Ok(crate::platform::shell::pick_file(
                                None,
                                Some(("Regeldatei", &["toml"])),
                            ))
                        });
                    }
                    if !self.rules_file.trim().is_empty() && ui.button("Standard").clicked() {
                        self.rules_file.clear();
                    }
                });
                match &self.rules_error {
                    Some(e) => {
                        let error = theme::tone_color(ui, format::Tone::Error);
                        ui.colored_label(error, e);
                    }
                    None if !self.rules.is_empty() => {
                        ui.label("Diese Regeln anwenden:");
                        egui::ScrollArea::vertical()
                            .max_height(160.0)
                            .show(ui, |ui| {
                                for (name, on) in &mut self.rules {
                                    ui.checkbox(on, name.as_str());
                                }
                            });
                    }
                    None => {}
                }
                ui.checkbox(
                    &mut self.no_classify,
                    "Fehlende Inhalte nicht nachklassifizieren",
                );
            }
            _ => {}
        }
    }

    fn form_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let busy = shell
            .runner
            .running()
            .iter()
            .any(|r| r.kind == TaskKind::Write);
        self.ensure_cards(shell);
        self.ensure_rules(shell);
        self.cards_ui(ui, shell);
        ui.add_space(crate::gui::theme::SPACE_M);
        self.options_ui(ui, shell);
        ui.add_space(crate::gui::theme::SPACE_M);
        ui.horizontal(|ui| {
            let ready = shell.target.is_some() && !busy;
            let button = egui::Button::new(egui::RichText::new("Plan erzeugen").strong());
            if ui.add_enabled(ready, button).clicked() {
                self.generate(shell);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Plan öffnen …"))
                .clicked()
            {
                self.request_open_plan(shell);
            }
            if shell.target.is_none() {
                ui.weak("Wähle oben einen Ordner.");
            }
        });
        ui.add_space(crate::gui::theme::SPACE_L);
        ui.heading("Zuletzt erzeugte Pläne");
        if self.recent.is_empty() {
            ui.label("Noch keine Pläne.");
        }
        let now = chrono::Local::now();
        let mut open: Option<PathBuf> = None;
        for p in &self.recent {
            let when: chrono::DateTime<chrono::Local> = p.modified.into();
            let (short, exact) = format::relative_time(now, when);
            let text = match &p.info {
                Some(i) => format!(
                    "{short}  ·  {}  ·  {} Aktionen, {}  ·  {}",
                    i.kind,
                    texts::grouped(i.actions as u64),
                    texts::bytes(i.bytes),
                    format::short_path(&i.root, 40)
                ),
                None => format!(
                    "{short}  ·  {}",
                    p.path
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                ),
            };
            if ui
                .button(text)
                .on_hover_text(format!("{exact}\n{}", paths::display(&p.path)))
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
        let kind = match self.kind_request() {
            Ok(kind) => kind,
            Err(e) => return shell.message("Plan erzeugen", &e),
        };
        let req = PlanRequest {
            target,
            kind,
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
            TASK_PICK_KEEP => {
                if let Ok(Ok(path)) = result.map(|b| b.downcast::<Option<PathBuf>>()) {
                    if let Some(path) = *path {
                        self.keep_path = paths::display(&path);
                    }
                }
            }
            TASK_FACTS => {
                if let Ok(Ok(data)) = result.map(|b| b.downcast::<CardsData>()) {
                    self.cards = Some(*data);
                }
            }
            TASK_RULE_NAMES => {
                if let Ok(Ok(names)) = result.map(|b| b.downcast::<Result<Vec<String>, String>>()) {
                    match *names {
                        Ok(names) => {
                            // Bereits getroffene Auswahl bleibt, neue Regeln sind angehakt.
                            let old = std::mem::take(&mut self.rules);
                            self.rules = names
                                .into_iter()
                                .map(|n| {
                                    let on =
                                        old.iter().find(|(o, _)| *o == n).is_none_or(|(_, on)| *on);
                                    (n, on)
                                })
                                .collect();
                            self.rules_error = None;
                        }
                        Err(e) => {
                            self.rules.clear();
                            self.rules_error = Some(e);
                        }
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
    fn ueberlappende_suchordner_zeigen_jeden_plan_nur_einmal() {
        let plans = tempfile::tempdir().unwrap();
        let gui = plans.path().join("_gui");
        std::fs::create_dir(&gui).unwrap();
        let in_gui = gui.join("gui.json");
        let direct = plans.path().join("direkt.json");
        for f in [&in_gui, &direct] {
            std::fs::write(f, "{}").unwrap();
        }
        // So liefert `plan_dirs()` sie: der GUI-Ordner und sein Elternordner.
        let dirs = [gui.clone(), plans.path().to_path_buf()];
        let found = recent_plans(&dirs, 10);
        let mut names: Vec<_> = found
            .iter()
            .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["direkt.json", "gui.json"]);
        // `limit` zählt nach dem Entdoppeln.
        assert_eq!(recent_plans(&dirs, 2).len(), 2);
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

    #[test]
    fn plankopf_nennt_art_wurzel_anzahl_und_groesse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.json");
        let plan = Plan {
            version: crate::change::plan::PLAN_VERSION,
            created: "2026-10-08T10:00:00+02:00".into(),
            kind: PlanKind::EmptyDirs,
            root: r"D:\Daten".into(),
            keep_strategy: None,
            params: Default::default(),
            protected_paths: Vec::new(),
            actions: Vec::new(),
            skipped: Vec::new(),
        };
        plan.save(&path).unwrap();
        let info = plan_info(&path).unwrap();
        assert_eq!(info.kind, "Leere Ordner");
        assert_eq!(
            (info.root.as_str(), info.actions, info.bytes),
            (r"D:\Daten", 0, 0)
        );
        let junk = dir.path().join("kein-plan.json");
        std::fs::write(&junk, "{}").unwrap();
        assert_eq!(plan_info(&junk), None);
        // `recent_plans` hängt den Kopf an.
        let found = recent_plans(&[dir.path().to_path_buf()], 5);
        assert_eq!(found.iter().filter(|p| p.info.is_some()).count(), 1);
    }
}
