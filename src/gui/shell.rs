//! Gemeinsamer Zustand aller Ansichten: Task-Runner mit Zuordnung zur Ansicht, Dialoge und die
//! Statusmeldung. Die Ansichten starten über [`Shell::spawn`] und bekommen das Ergebnis über
//! `on_finished` zurück.

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;

use super::tasks::{Finished, TaskError, TaskId, TaskKind, TaskRunner};
use super::texts;
use crate::ops::OpCtx;

/// Welche Ansicht ein Task-Ergebnis erhält.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Route {
    /// Kopfleiste: Ziel prüfen, Scan, Klassifizieren
    Header,
    Overview,
    Analysis,
    Cleanup,
    History,
    Quarantine,
    Settings,
}

/// Seiten der Seitenleiste.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum Page {
    #[default]
    Overview,
    Analysis,
    Cleanup,
    History,
    Quarantine,
    Settings,
}

impl Page {
    pub const ALL: [Page; 6] = [
        Page::Overview,
        Page::Analysis,
        Page::Cleanup,
        Page::History,
        Page::Quarantine,
        Page::Settings,
    ];

    /// Symbol der Seitenleiste (Segoe UI Symbol); ohne die Schrift entfällt es.
    pub fn icon(self) -> &'static str {
        match self {
            Page::Overview => "\u{2302}",
            Page::Analysis => "\u{25D4}",
            Page::Cleanup => "\u{2702}",
            Page::History => "\u{21BA}",
            Page::Quarantine => "\u{25A3}",
            Page::Settings => "\u{2699}",
        }
    }

    /// Beschriftung in der Seitenleiste, mit Symbol, wenn die Symbolschrift geladen ist.
    pub fn nav_label(self, icons: bool) -> String {
        if icons {
            format!("{}  {}", self.icon(), self.label())
        } else {
            self.label().to_string()
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Page::Overview => texts::NAV_OVERVIEW,
            Page::Analysis => texts::NAV_ANALYSIS,
            Page::Cleanup => texts::NAV_CLEANUP,
            Page::History => texts::NAV_HISTORY,
            Page::Quarantine => texts::NAV_QUARANTINE,
            Page::Settings => texts::NAV_SETTINGS,
        }
    }
}

/// Sprung in eine andere Ansicht (z. B. aus der Analyse zum passenden Plan).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Goto {
    pub page: Page,
    /// Vorgewählte Plan-Art (nur `Page::Cleanup`): `dedupe`, `junk`, `empty-dirs`, …
    pub plan_kind: Option<&'static str>,
    /// Vorgewählter Tab (nur `Page::Analysis`): `problems` oder `content`
    pub analysis_tab: Option<&'static str>,
    /// Nur `Page::Quarantine`: Wurzel und Lauf, auf die die Liste gefiltert wird
    pub quarantine_run: Option<(std::path::PathBuf, crate::change::RunId)>,
}

/// Was eine Ansicht von der Kopfleiste verlangt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderRequest {
    /// Ordner wählen und danach sofort scannen (Leerzustand der Übersicht)
    PickAndScan,
}

/// Antwort auf einen Bestätigungsdialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub ok: bool,
    pub checked: bool,
}

#[derive(Debug, Clone)]
pub enum Dialog {
    Message {
        title: String,
        text: String,
    },
    Confirm {
        id: &'static str,
        title: String,
        text: String,
        ok_label: String,
        /// Haken, der für „OK“ gesetzt sein muss (z. B. Obergrenze aufheben)
        must_check: Option<String>,
        checked: bool,
        danger: bool,
    },
}

pub struct Shell {
    pub runner: TaskRunner,
    routes: HashMap<TaskId, Route>,
    pub dialogs: Vec<Dialog>,
    answers: HashMap<&'static str, Answer>,
    pub status: String,
    /// Gewählter Ordner bzw. gewähltes Profil (gilt für alle Ansichten)
    pub target: Option<Choice>,
    pub goto: Option<Goto>,
    /// Zählt Änderungen an Index oder Dateien (Scan, Classify, Apply, Undo); Ansichten laden danach neu
    pub generation: u64,
    /// Zuletzt gewählte Ziele, das neueste zuerst (höchstens [`RECENT_MAX`])
    pub recent_targets: Vec<Choice>,
    pub header_request: Option<HeaderRequest>,
    /// Ein „?“ wurde geklickt; das Hauptfenster öffnet die Hilfe zu diesem Thema
    pub help_request: Option<super::help::Topic>,
}

/// So viele zuletzt gewählte Ziele merkt sich die Oberfläche.
pub const RECENT_MAX: usize = 8;

/// Dasselbe Ziel? Ordner werden wie im Index verglichen (Groß-/Kleinschreibung, Schrägstriche).
fn same_target(a: &Choice, b: &Choice) -> bool {
    match (a, b) {
        (Choice::Folder(x), Choice::Folder(y)) => {
            crate::paths::path_key(x) == crate::paths::path_key(y)
        }
        (Choice::Profile(x), Choice::Profile(y)) => x == y,
        _ => false,
    }
}

/// Einträge des Ordner-Dropdowns: zuerst die zuletzt gewählten Ziele, danach alle gescannten
/// Wurzeln aus dem Index, jeweils ohne Dubletten.
pub fn dropdown_targets(recent: &[Choice], scanned: &[std::path::PathBuf]) -> Vec<Choice> {
    let mut out: Vec<Choice> = Vec::new();
    let all = recent
        .iter()
        .cloned()
        .chain(scanned.iter().cloned().map(Choice::Folder));
    for choice in all {
        if !out.iter().any(|c| same_target(c, &choice)) {
            out.push(choice);
        }
    }
    out
}

/// Ordner oder Profil, mit dem gearbeitet wird.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Choice {
    Folder(std::path::PathBuf),
    Profile(String),
}

impl Choice {
    pub fn spec(&self) -> crate::ops::target::TargetSpec {
        match self {
            Self::Folder(path) => crate::ops::target::TargetSpec::Path {
                path: path.clone(),
                force: false,
            },
            Self::Profile(name) => crate::ops::target::TargetSpec::Profile(name.clone()),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Folder(path) => crate::paths::display(path),
            Self::Profile(name) => format!("Profil {name}"),
        }
    }
}

impl Shell {
    pub fn new(ctx: egui::Context) -> Self {
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        Self {
            runner: TaskRunner::new(wake),
            routes: HashMap::new(),
            dialogs: Vec::new(),
            answers: HashMap::new(),
            status: String::new(),
            target: None,
            goto: None,
            generation: 0,
            recent_targets: Vec::new(),
            header_request: None,
            help_request: None,
        }
    }

    /// Bittet das Hauptfenster, die Hilfe zu einem Thema zu öffnen.
    pub fn open_help(&mut self, topic: super::help::Topic) {
        self.help_request = Some(topic);
    }

    /// Merkt sich ein gewähltes Ziel als neuestes. Leere Pfade und Namen werden ignoriert.
    pub fn remember(&mut self, choice: Choice) {
        let empty = match &choice {
            Choice::Folder(path) => path.as_os_str().is_empty(),
            Choice::Profile(name) => name.trim().is_empty(),
        };
        if empty {
            return;
        }
        self.recent_targets.retain(|c| !same_target(c, &choice));
        self.recent_targets.insert(0, choice);
        self.recent_targets.truncate(RECENT_MAX);
    }

    /// Startet einen Task für eine Ansicht. Ein zweiter schreibender Task wird mit einem Hinweis
    /// abgelehnt (`false`).
    pub fn spawn<T, F>(&mut self, route: Route, name: &str, kind: TaskKind, f: F) -> bool
    where
        T: Send + 'static,
        F: FnOnce(&OpCtx) -> anyhow::Result<T> + Send + 'static,
    {
        match self.runner.spawn(name, kind, f) {
            Ok(id) => {
                self.routes.insert(id, route);
                true
            }
            Err(_) => {
                self.message(
                    name,
                    "Es läuft bereits eine Aufgabe, die Daten verändert. Bitte warten oder \
                     abbrechen.",
                );
                false
            }
        }
    }

    pub fn message(&mut self, title: &str, text: &str) {
        self.dialogs.push(Dialog::Message {
            title: title.to_string(),
            text: text.to_string(),
        });
    }

    /// Fehler eines Tasks als Dialog (Busy mit eigenem Text).
    pub fn show_error(&mut self, name: &str, error: &TaskError) {
        let text = match error {
            TaskError::Busy => texts::BUSY_SCAN.to_string(),
            other => other.to_string(),
        };
        self.message(&texts::error_title(name), &text);
    }

    pub fn confirm(&mut self, dialog: Dialog) {
        self.dialogs.push(dialog);
    }

    pub fn set_answer(&mut self, id: &'static str, answer: Answer) {
        self.answers.insert(id, answer);
    }

    /// Entnimmt die Antwort eines Bestätigungsdialogs, falls schon gegeben.
    pub fn take_answer(&mut self, id: &'static str) -> Option<Answer> {
        self.answers.remove(id)
    }

    /// Alle noch nicht abgeholten Antworten (die App verteilt sie an die Ansichten).
    pub fn drain_answers(&mut self) -> Vec<(&'static str, Answer)> {
        self.answers.drain().collect()
    }

    /// Beendete Tasks samt Ansicht.
    pub fn poll(&mut self) -> Vec<(Route, Finished)> {
        self.runner
            .poll()
            .into_iter()
            .filter_map(|f| self.routes.remove(&f.id).map(|r| (r, f)))
            .collect()
    }

    pub fn is_running(&self, route: Route) -> bool {
        self.runner
            .running()
            .iter()
            .any(|r| self.routes.get(&r.id) == Some(&route))
    }

    /// Fortschritt des (ersten) laufenden Tasks einer Ansicht.
    pub fn running_ctx(&self, route: Route) -> Option<(String, OpCtx)> {
        self.runner
            .running()
            .into_iter()
            .find(|r| self.routes.get(&r.id) == Some(&route))
            .map(|r| (r.name.clone(), r.ctx.clone()))
    }
}

/// Ergebnis eines Tasks, vom Typ gelöst.
pub type TaskResult = Result<Box<dyn Any + Send>, TaskError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn folder(p: &str) -> Choice {
        Choice::Folder(PathBuf::from(p))
    }

    fn shell() -> Shell {
        Shell::new(egui::Context::default())
    }

    #[test]
    fn neuestes_ziel_steht_vorn() {
        let mut s = shell();
        s.remember(folder(r"D:\A"));
        s.remember(Choice::Profile("downloads".into()));
        assert_eq!(
            s.recent_targets,
            [Choice::Profile("downloads".into()), folder(r"D:\A")]
        );
    }

    #[test]
    fn dasselbe_ziel_rueckt_nach_vorn_statt_doppelt_zu_stehen() {
        let mut s = shell();
        s.remember(folder(r"D:\A"));
        s.remember(folder(r"D:\B"));
        s.remember(folder(r"d:\a\"));
        assert_eq!(s.recent_targets, [folder(r"d:\a\"), folder(r"D:\B")]);
    }

    #[test]
    fn es_bleiben_hoechstens_acht_ziele() {
        let mut s = shell();
        for i in 0..12 {
            s.remember(folder(&format!(r"D:\Ordner{i}")));
        }
        assert_eq!(s.recent_targets.len(), RECENT_MAX);
        assert_eq!(s.recent_targets[0], folder(r"D:\Ordner11"));
        assert_eq!(s.recent_targets[7], folder(r"D:\Ordner4"));
    }

    #[test]
    fn leere_ziele_werden_nicht_gemerkt() {
        let mut s = shell();
        s.remember(folder(""));
        s.remember(Choice::Profile("  ".into()));
        assert!(s.recent_targets.is_empty());
    }

    #[test]
    fn dropdown_zeigt_gewaehlte_zuerst_dann_gescannte_ohne_dubletten() {
        let recent = [folder(r"D:\B"), Choice::Profile("p".into())];
        let scanned = [PathBuf::from(r"d:\b"), PathBuf::from(r"D:\C")];
        assert_eq!(
            dropdown_targets(&recent, &scanned),
            [
                folder(r"D:\B"),
                Choice::Profile("p".into()),
                folder(r"D:\C")
            ]
        );
    }

    #[test]
    fn dropdown_ohne_gemerkte_ziele_zeigt_die_gescannten() {
        let scanned = [PathBuf::from(r"D:\C")];
        assert_eq!(dropdown_targets(&[], &scanned), [folder(r"D:\C")]);
        assert!(dropdown_targets(&[], &[]).is_empty());
    }

    #[test]
    fn seitenleiste_zeigt_symbole_nur_mit_symbolschrift() {
        for page in Page::ALL {
            assert!(!page.icon().is_empty());
            assert_eq!(page.nav_label(false), page.label());
            let with = page.nav_label(true);
            assert!(
                with.starts_with(page.icon()) && with.ends_with(page.label()),
                "{with}"
            );
        }
        let icons: std::collections::HashSet<_> = Page::ALL.iter().map(|p| p.icon()).collect();
        assert_eq!(icons.len(), Page::ALL.len(), "jedes Symbol nur einmal");
    }
}
