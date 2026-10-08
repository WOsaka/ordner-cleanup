//! Kopfleiste über dem Seiteninhalt: Ziel wählen, scannen, klassifizieren. Sie gilt für alle
//! Seiten außer den Einstellungen und zeigt immer die tatsächliche Auswahl.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use chrono::Local;
use eframe::egui;

use super::format::{self, scan_info};
use super::result::{self, ResultAction, ResultView};
use super::shell::{dropdown_targets, Choice, Goto, HeaderRequest, Page, Route, Shell, TaskResult};
use super::tasks::{TaskError, TaskKind};
use super::texts;
use super::views::overview::{root_rows, RootRow};
use crate::ops::admin::{profiles, ProfileInfo};
use crate::ops::classify::{classify, ClassifyOutcome, ClassifyRequest};
use crate::ops::scan::{scan, ScanReport, ScanRequest};
use crate::ops::target::TargetSpec;
use crate::paths;

const TASK_TARGETS: &str = "Ziele laden";
const TASK_PICK: &str = "Ordner wählen";
const TASK_CHECK: &str = "Ziel prüfen";
pub const TASK_SCAN: &str = "Scannen";
pub const TASK_CLASSIFY: &str = "Klassifizieren";

/// Kurze Hintergrundaufgaben der Kopfleiste, die die Statusleiste nicht anzeigen soll.
pub fn is_silent(task_name: &str) -> bool {
    task_name == TASK_TARGETS || task_name == TASK_CHECK
}

/// So breit darf der Pfad im Dropdown werden, bevor er in der Mitte gekürzt wird.
const LABEL_CHARS: usize = 44;

/// Eingaben des Dialogs „Scan-Optionen“, wie sie als Text im Formular stehen.
#[derive(Debug, Default, Clone)]
pub struct ScanOptions {
    // Scannen
    pub net_force: bool,
    pub no_default_excludes: bool,
    pub reset_index: bool,
    pub threads: String,
    pub template: String,
    pub exclude: String,
    pub summary_only: String,
    // Klassifizieren
    pub no_llm: bool,
    pub force: bool,
    pub ext: String,
    pub only: String,
}

/// Nichtleere, getrimmte Zeilen.
pub fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Getrimmter Text, leer = `None`.
pub fn opt(text: &str) -> Option<String> {
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

impl ScanOptions {
    /// Ordner oder Profil als Ziel; „Netzlaufwerk trotzdem“ gilt nur für Ordner.
    pub fn target_spec(&self, choice: &Choice) -> TargetSpec {
        match choice {
            Choice::Folder(path) => TargetSpec::Path {
                path: path.clone(),
                force: self.net_force,
            },
            Choice::Profile(name) => TargetSpec::Profile(name.clone()),
        }
    }

    pub fn scan_request(&self, choice: &Choice) -> ScanRequest {
        ScanRequest {
            target: self.target_spec(choice),
            exclude: lines(&self.exclude),
            summary_only: lines(&self.summary_only),
            no_default_excludes: self.no_default_excludes,
            reset_index: self.reset_index,
            threads: self.threads.trim().parse().ok(),
            template: opt(&self.template),
        }
    }

    pub fn classify_request(&self, choice: &Choice, clear: bool) -> ClassifyRequest {
        ClassifyRequest {
            target: self.target_spec(choice),
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
        }
    }
}

/// Der Dialog „Scan-Optionen“ mit den getrennten Abschnitten Scannen und Klassifizieren.
/// `true`, wenn „Text-Cache löschen“ gedrückt wurde.
pub fn options_ui(ui: &mut egui::Ui, options: &mut ScanOptions, can_clear: bool) -> bool {
    ui.strong("Scannen");
    ui.checkbox(&mut options.net_force, "Netzlaufwerk trotzdem scannen");
    ui.checkbox(&mut options.no_default_excludes, "Standard-Ausschlüsse aus");
    ui.checkbox(&mut options.reset_index, "Index neu aufbauen");
    ui.horizontal(|ui| {
        ui.label("Threads (leer = automatisch):");
        ui.add(egui::TextEdit::singleline(&mut options.threads).desired_width(50.0));
    });
    ui.horizontal(|ui| {
        ui.label("Vorlage für Soll/Ist (para, johnny-decimal oder Pfad):");
        ui.add(egui::TextEdit::singleline(&mut options.template).desired_width(220.0));
    });
    ui.label("Zusätzliche Ausschlüsse (ein Muster je Zeile):");
    ui.add(egui::TextEdit::multiline(&mut options.exclude).desired_rows(2));
    ui.label("Nur Summe (ein Muster je Zeile):");
    ui.add(egui::TextEdit::multiline(&mut options.summary_only).desired_rows(2));
    ui.separator();
    ui.strong("Klassifizieren");
    ui.checkbox(&mut options.no_llm, "Ohne LLM");
    ui.checkbox(&mut options.force, "Alles neu analysieren");
    ui.horizontal(|ui| {
        ui.label("Nur Endungen (z. B. pdf,jpg):");
        ui.add(egui::TextEdit::singleline(&mut options.ext).desired_width(140.0));
    });
    ui.horizontal(|ui| {
        ui.label("Nur Glob (relativ zur Wurzel):");
        ui.add(egui::TextEdit::singleline(&mut options.only).desired_width(180.0));
    });
    ui.add_enabled(can_clear, egui::Button::new("Text-Cache löschen"))
        .clicked()
}

/// Wurzeln und Profile für das Dropdown und die Zeile „Letzter Scan“.
struct TargetsData {
    roots: Vec<RootRow>,
    profiles: Vec<ProfileInfo>,
    /// Fehler beim Lesen der Config (Ordner lassen sich trotzdem wählen)
    profile_error: Option<String>,
}

fn load_targets() -> anyhow::Result<TargetsData> {
    let roots = root_rows()?;
    let (profiles, profile_error) = match profiles() {
        Ok(p) => (p, None),
        Err(e) => (Vec::new(), Some(format!("{e:#}"))),
    };
    Ok(TargetsData {
        roots,
        profiles,
        profile_error,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Retry {
    Scan,
    Classify,
}

#[derive(Default)]
pub struct Header {
    options: ScanOptions,
    options_open: bool,
    data: Option<TargetsData>,
    /// Stand von `Shell::generation`, für den `data` geladen wird bzw. wurde
    loaded_generation: Option<u64>,
    /// Ob der gewählte Ordner existiert (aus einem Hintergrund-Task, damit ein getrenntes
    /// Netzlaufwerk die Oberfläche nicht anhält)
    exists: Option<(Choice, bool)>,
    checking: Option<Choice>,
    /// Nach dem Ordnerdialog sofort scannen (Leerzustand der Übersicht)
    scan_after_pick: bool,
    result: Option<ResultView>,
    retry: Option<Retry>,
}

impl Header {
    fn busy(shell: &Shell) -> bool {
        shell
            .runner
            .running()
            .iter()
            .any(|r| r.kind == TaskKind::Write || r.name == TASK_PICK)
    }

    /// Lädt Daten nach und beantwortet Wünsche anderer Ansichten.
    fn maintain(&mut self, shell: &mut Shell) {
        if let Some(HeaderRequest::PickAndScan) = shell.header_request.take() {
            if !Self::busy(shell) {
                self.scan_after_pick = true;
                self.pick_folder(shell);
            }
        }
        if self.loaded_generation != Some(shell.generation) {
            self.loaded_generation = Some(shell.generation);
            self.exists = None;
            self.checking = None;
            shell.spawn(Route::Header, TASK_TARGETS, TaskKind::Read, |_| {
                load_targets()
            });
        }
        if let Some(choice @ Choice::Folder(_)) = shell.target.clone() {
            if self.checking.as_ref() != Some(&choice) {
                self.checking = Some(choice.clone());
                shell.spawn(Route::Header, TASK_CHECK, TaskKind::Read, move |_| {
                    let exists = match &choice {
                        Choice::Folder(path) => path.exists(),
                        Choice::Profile(_) => true,
                    };
                    Ok((choice, exists))
                });
            }
        }
    }

    fn pick_folder(&self, shell: &mut Shell) {
        shell.spawn(Route::Header, TASK_PICK, TaskKind::Read, |_| {
            Ok(crate::platform::shell::pick_folder(None))
        });
    }

    /// Existiert das gewählte Ziel? Solange unbekannt, gilt es als vorhanden.
    fn target_exists(&self, shell: &Shell) -> bool {
        match (&shell.target, &self.exists) {
            (Some(chosen), Some((checked, exists))) if chosen == checked => *exists,
            _ => true,
        }
    }

    /// Ein Eintrag der Dropdown-Liste: Profile, die es nicht mehr gibt, fallen heraus.
    fn is_known(&self, choice: &Choice) -> bool {
        match (choice, &self.data) {
            (Choice::Profile(name), Some(data)) if data.profile_error.is_none() => {
                data.profiles.iter().any(|p| &p.name == name)
            }
            _ => true,
        }
    }

    fn combo_label(&self, shell: &Shell) -> String {
        match &shell.target {
            None => "Ordner wählen …".into(),
            Some(choice) => {
                let text = format::short_path(&choice.label(), LABEL_CHARS);
                if self.target_exists(shell) {
                    text
                } else {
                    format!("⚠ {text}")
                }
            }
        }
    }

    /// „Letzter Scan: vor 2 Tagen · Score 72 (▲ 3)“ für das gewählte Ziel.
    fn info(&self, choice: &Choice) -> String {
        let Some(data) = &self.data else {
            return String::new();
        };
        let now = Local::now();
        match choice {
            Choice::Folder(path) => {
                let key = paths::path_key(path);
                data.roots
                    .iter()
                    .find(|r| paths::path_key(Path::new(&r.info.path)) == key)
                    .map_or_else(
                        || scan_info(now, None, None, None),
                        |r| {
                            let finished = r
                                .info
                                .finished_at
                                .as_deref()
                                .or(r.info.started_at.as_deref());
                            scan_info(now, finished, r.score, r.previous_score)
                        },
                    )
            }
            Choice::Profile(name) => data
                .profiles
                .iter()
                .find(|p| &p.name == name)
                .map(|p| {
                    let finished = p.last_run.as_ref().map(|r| r.ended.as_str());
                    scan_info(
                        now,
                        finished,
                        p.last_point.as_ref().map(|pt| pt.score),
                        None,
                    )
                })
                .unwrap_or_default(),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        self.maintain(shell);
        let busy = Self::busy(shell);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            self.target_combo(ui, shell);
            let ready = shell.target.is_some() && !busy && self.target_exists(shell);
            if ui
                .add_enabled(ready, egui::Button::new("Scannen"))
                .clicked()
            {
                self.start_scan(shell);
            }
            if ui
                .add_enabled(ready, egui::Button::new("Klassifizieren"))
                .clicked()
            {
                self.start_classify(shell, false);
            }
            if ui
                .add_enabled(!busy, egui::Button::new("Optionen …"))
                .clicked()
            {
                self.options_open = true;
            }
            if self.retry.is_some() && !busy && ui.button(texts::RETRY).clicked() {
                match self.retry.take() {
                    Some(Retry::Scan) => self.start_scan(shell),
                    Some(Retry::Classify) => self.start_classify(shell, false),
                    None => {}
                }
            }
            if let Some(choice) = &shell.target {
                if !self.target_exists(shell) {
                    ui.colored_label(
                        super::theme::palette(ui.visuals().dark_mode).warn,
                        "Ordner nicht gefunden",
                    );
                } else {
                    ui.weak(self.info(choice));
                }
            }
        });
        self.progress_ui(ui, shell);
        self.result_ui(ui, shell);
        ui.add_space(4.0);
        self.options_window(ui, shell, busy);
    }

    fn target_combo(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let mut picked: Option<Choice> = None;
        let mut pick_other = false;
        let label = self.combo_label(shell);
        let scanned: Vec<PathBuf> = self.data.as_ref().map_or_else(Vec::new, |d| {
            d.roots
                .iter()
                .map(|r| PathBuf::from(&r.info.path))
                .collect()
        });
        egui::ComboBox::from_id_salt("header-target")
            .selected_text(label)
            .width(320.0)
            .show_ui(ui, |ui| {
                let entries = dropdown_targets(&shell.recent_targets, &scanned);
                for choice in entries.into_iter().filter(|c| self.is_known(c)) {
                    let text = format::short_path(&choice.label(), LABEL_CHARS);
                    let selected = shell.target.as_ref() == Some(&choice);
                    if ui.selectable_label(selected, text).clicked() {
                        picked = Some(choice);
                    }
                }
                if let Some(data) = &self.data {
                    if let Some(error) = &data.profile_error {
                        ui.separator();
                        ui.weak(format!("Config nicht lesbar: {error}"));
                    } else if !data.profiles.is_empty() {
                        ui.separator();
                        ui.weak("Profile");
                        egui::ScrollArea::vertical()
                            .max_height(160.0)
                            .show(ui, |ui| {
                                for p in &data.profiles {
                                    let choice = Choice::Profile(p.name.clone());
                                    let selected = shell.target.as_ref() == Some(&choice);
                                    if ui.selectable_label(selected, &p.name).clicked() {
                                        picked = Some(choice);
                                    }
                                }
                            });
                    }
                }
                ui.separator();
                if ui.button("Anderen Ordner wählen …").clicked() {
                    pick_other = true;
                }
            });
        if let Some(choice) = picked {
            shell.target = Some(choice);
        }
        if pick_other && !Self::busy(shell) {
            self.pick_folder(shell);
        }
    }

    fn progress_ui(&self, ui: &mut egui::Ui, shell: &Shell) {
        let running = shell
            .runner
            .running()
            .into_iter()
            .find(|r| r.name == TASK_SCAN || r.name == TASK_CLASSIFY);
        let Some(task) = running else { return };
        let p = &task.ctx.progress;
        let (done, total) = (
            p.done.load(Ordering::Relaxed),
            p.total.load(Ordering::Relaxed),
        );
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!("{}: {}", task.name, p.phase()));
            if total > 0 {
                ui.add(
                    egui::ProgressBar::new(done as f32 / total as f32)
                        .desired_width(220.0)
                        .text(format!(
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
            if ui.button(texts::CANCEL).clicked() {
                task.ctx.cancel.store(true, Ordering::Relaxed);
            }
        });
        ui.label(egui::RichText::new(p.current()).monospace().weak());
    }

    fn result_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let Some(view) = &self.result else { return };
        match result::show(ui, view) {
            ResultAction::Close => self.result = None,
            ResultAction::ShowErrors => {
                shell.goto = Some(Goto {
                    page: Page::Analysis,
                    analysis_tab: Some("problems"),
                    ..Goto::default()
                });
            }
            ResultAction::None => {}
        }
    }

    fn options_window(&mut self, ui: &egui::Ui, shell: &mut Shell, busy: bool) {
        if !self.options_open {
            return;
        }
        let mut open = true;
        let mut clear = false;
        let can_clear = shell.target.is_some() && !busy;
        egui::Window::new("Scan-Optionen")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                clear = options_ui(ui, &mut self.options, can_clear);
            });
        self.options_open = open && !clear;
        if clear {
            self.start_classify(shell, true);
        }
    }

    fn start_scan(&mut self, shell: &mut Shell) {
        let Some(choice) = shell.target.clone() else {
            return;
        };
        let request = self.options.scan_request(&choice);
        self.result = None;
        self.retry = Some(Retry::Scan);
        shell.spawn(Route::Header, TASK_SCAN, TaskKind::Write, move |ctx| {
            scan(&request, ctx)
        });
    }

    fn start_classify(&mut self, shell: &mut Shell, clear: bool) {
        let Some(choice) = shell.target.clone() else {
            return;
        };
        let request = self.options.classify_request(&choice, clear);
        self.result = None;
        self.retry = Some(Retry::Classify);
        shell.spawn(Route::Header, TASK_CLASSIFY, TaskKind::Write, move |ctx| {
            classify(&request, ctx)
        });
    }

    pub fn on_finished(&mut self, name: &str, result: TaskResult, shell: &mut Shell) {
        match name {
            TASK_TARGETS => {
                if let Ok(Ok(data)) = result.map(|b| b.downcast::<TargetsData>()) {
                    self.data = Some(*data);
                }
            }
            TASK_CHECK => {
                if let Ok(Ok(checked)) = result.map(|b| b.downcast::<(Choice, bool)>()) {
                    self.exists = Some(*checked);
                }
            }
            TASK_PICK => {
                let picked = result
                    .ok()
                    .and_then(|b| b.downcast::<Option<PathBuf>>().ok())
                    .and_then(|p| *p);
                let scan_now = std::mem::take(&mut self.scan_after_pick);
                if let Some(path) = picked {
                    shell.target = Some(Choice::Folder(path));
                    if scan_now {
                        self.start_scan(shell);
                    }
                }
            }
            TASK_SCAN => match result {
                Ok(boxed) => {
                    if let Ok(report) = boxed.downcast::<ScanReport>() {
                        self.result = Some(result::scan_view(&report));
                        self.retry = None;
                    }
                    shell.generation += 1;
                }
                Err(e) => self.fail(name, &e, shell),
            },
            TASK_CLASSIFY => match result {
                Ok(boxed) => {
                    if let Ok(outcome) = boxed.downcast::<ClassifyOutcome>() {
                        self.result = Some(result::classify_view(&outcome));
                        self.retry = None;
                    }
                    shell.generation += 1;
                }
                Err(e) => self.fail(name, &e, shell),
            },
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

    fn folder() -> Choice {
        Choice::Folder(PathBuf::from(r"D:\Daten"))
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

    #[test]
    fn scan_anfrage_uebernimmt_alle_scan_optionen() {
        let options = ScanOptions {
            net_force: true,
            no_default_excludes: true,
            reset_index: true,
            threads: " 4 ".into(),
            template: " para ".into(),
            exclude: "*.tmp\n  node_modules \n".into(),
            summary_only: "Backup\n".into(),
            ..ScanOptions::default()
        };
        let req = options.scan_request(&folder());
        assert!(req.no_default_excludes && req.reset_index);
        assert_eq!(req.threads, Some(4));
        assert_eq!(req.template.as_deref(), Some("para"));
        assert_eq!(req.exclude, ["*.tmp", "node_modules"]);
        assert_eq!(req.summary_only, ["Backup"]);
        match req.target {
            TargetSpec::Path { path, force } => {
                assert_eq!(path, PathBuf::from(r"D:\Daten"));
                assert!(force, "Netzlaufwerk trotzdem");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn leere_felder_bleiben_leer_und_ungueltige_threads_werden_automatisch() {
        let options = ScanOptions {
            threads: "abc".into(),
            ..ScanOptions::default()
        };
        let req = options.scan_request(&folder());
        assert_eq!(req.threads, None);
        assert_eq!(req.template, None);
        assert!(req.exclude.is_empty() && req.summary_only.is_empty());
        assert!(!req.no_default_excludes && !req.reset_index);
    }

    #[test]
    fn profil_als_ziel_ignoriert_netzlaufwerk_haken() {
        let options = ScanOptions {
            net_force: true,
            ..ScanOptions::default()
        };
        let req = options.scan_request(&Choice::Profile("downloads".into()));
        assert!(matches!(req.target, TargetSpec::Profile(n) if n == "downloads"));
    }

    #[test]
    fn klassifizier_anfrage_uebernimmt_ihre_optionen() {
        let options = ScanOptions {
            no_llm: true,
            force: true,
            ext: " pdf, jpg ,,docx".into(),
            only: " Rechnungen/** ".into(),
            net_force: true,
            ..ScanOptions::default()
        };
        let req = options.classify_request(&folder(), false);
        assert!(req.no_llm && req.force && !req.clear);
        assert_eq!(req.ext, ["pdf", "jpg", "docx"]);
        assert_eq!(req.only.as_deref(), Some("Rechnungen/**"));
        assert!(matches!(req.target, TargetSpec::Path { force: true, .. }));
    }

    #[test]
    fn text_cache_loeschen_setzt_clear() {
        let req = ScanOptions::default().classify_request(&folder(), true);
        assert!(req.clear);
    }

    #[test]
    fn unbekanntes_ziel_gilt_als_vorhanden_bis_die_pruefung_zurueck_ist() {
        let shell = Shell::new(egui::Context::default());
        let mut header = Header::default();
        let mut shell = shell;
        shell.target = Some(folder());
        assert!(header.target_exists(&shell));
        header.exists = Some((folder(), false));
        assert!(!header.target_exists(&shell));
        // Ergebnis eines früheren Ziels zählt nicht für das neue
        shell.target = Some(Choice::Folder(PathBuf::from(r"D:\Anderer")));
        assert!(header.target_exists(&shell));
    }

    #[test]
    fn profile_ohne_config_eintrag_fallen_aus_der_liste() {
        let header = Header {
            data: Some(TargetsData {
                roots: Vec::new(),
                profiles: Vec::new(),
                profile_error: None,
            }),
            ..Header::default()
        };
        assert!(!header.is_known(&Choice::Profile("weg".into())));
        assert!(header.is_known(&folder()));
        // Ist die Config nicht lesbar, bleiben gemerkte Profile stehen
        let broken = Header {
            data: Some(TargetsData {
                roots: Vec::new(),
                profiles: Vec::new(),
                profile_error: Some("kaputt".into()),
            }),
            ..Header::default()
        };
        assert!(broken.is_known(&Choice::Profile("weg".into())));
    }
}
