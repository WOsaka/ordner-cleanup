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
    Overview,
    Analysis,
    Cleanup,
    History,
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
    Settings,
}

impl Page {
    pub const ALL: [Page; 5] = [
        Page::Overview,
        Page::Analysis,
        Page::Cleanup,
        Page::History,
        Page::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Page::Overview => texts::NAV_OVERVIEW,
            Page::Analysis => texts::NAV_ANALYSIS,
            Page::Cleanup => texts::NAV_CLEANUP,
            Page::History => texts::NAV_HISTORY,
            Page::Settings => texts::NAV_SETTINGS,
        }
    }
}

/// Sprung in eine andere Ansicht (z. B. aus der Analyse zum passenden Plan).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goto {
    pub page: Page,
    /// Vorgewählte Plan-Art (nur `Page::Cleanup`): `dedupe`, `junk`, `empty-dirs`, …
    pub plan_kind: Option<&'static str>,
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
        }
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
