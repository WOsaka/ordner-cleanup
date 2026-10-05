//! Rückgängig machen: Lauf suchen, bestätigen lassen, im Hintergrund zurückdrehen, Ergebnis
//! zeigen. Gemeinsam für Ergebnisansicht und Verlauf.

use std::path::PathBuf;

use crate::change::undo::{RestoreStatus, UndoOutcome};
use crate::change::RunId;
use crate::gui::shell::{Answer, Dialog, Route, Shell, TaskResult};
use crate::gui::tasks::TaskKind;
use crate::ops::runs::{undo_check, undo_execute, UndoCheck};
use crate::ops::{local_time, status_label};

pub const TASK_CHECK: &str = "Lauf prüfen";
pub const TASK_UNDO: &str = "Rückgängig";

/// Text für die Ergebnismeldung eines Undo.
pub fn outcome_text(outcome: &UndoOutcome) -> String {
    let mut text = format!(
        "Lauf {}: {} wiederhergestellt, {} Kollisionen, {} nicht mehr vorhanden, {} Fehler",
        outcome.run,
        outcome.restored(),
        outcome.conflicts(),
        outcome.missing(),
        outcome.failed()
    );
    if outcome.aborted {
        text.push_str("\nAbgebrochen; ein weiteres „Rückgängig“ setzt fort.");
    }
    for r in outcome.results.iter().take(30) {
        match &r.status {
            RestoreStatus::Conflict(reason) => {
                text.push_str(&format!("\nKollision ({reason}): {}", r.path));
            }
            RestoreStatus::Missing => {
                text.push_str(&format!("\nNicht mehr wiederherstellbar: {}", r.path));
            }
            RestoreStatus::Failed(error) => {
                text.push_str(&format!("\nFehler ({error}): {}", r.path))
            }
            RestoreStatus::Restored | RestoreStatus::NothingToDo => {}
        }
    }
    if outcome.conflicts() > 0 {
        text.push_str(
            "\nKollidierende Einträge bleiben, wo sie sind; nach dem Auflösen erneut \
             „Rückgängig“ ausführen.",
        );
    }
    text
}

#[derive(Default)]
pub struct UndoFlow {
    pending: Option<UndoCheck>,
}

impl UndoFlow {
    pub fn begin(&mut self, shell: &mut Shell, route: Route, run: RunId, root: Option<PathBuf>) {
        shell.spawn(route, TASK_CHECK, TaskKind::Read, move |_| {
            undo_check(&run, root.as_deref())
        });
    }

    /// Ergebnis von „Lauf prüfen“: Rückfrage oder Hinweis.
    pub fn on_checked(&mut self, result: TaskResult, shell: &mut Shell, id: &'static str) {
        let check = match result.map(|b| b.downcast::<UndoCheck>()) {
            Ok(Ok(check)) => *check,
            Ok(Err(_)) => return,
            Err(e) => return shell.show_error(TASK_CHECK, &e),
        };
        let run = check.summary.run.clone();
        if check.already_undone() {
            return shell.message(
                "Rückgängig",
                &format!("Lauf {run} wurde bereits zurückgedreht."),
            );
        }
        if check.purged() {
            return shell.message(
                "Rückgängig",
                &format!("Lauf {run}: Quarantäne wurde gelöscht, nicht mehr wiederherstellbar."),
            );
        }
        let s = &check.summary;
        let in_quarantine = if s.bytes > 0 {
            format!(", {} in der Quarantäne", crate::gui::texts::bytes(s.bytes))
        } else {
            String::new()
        };
        let text = format!(
            "Lauf {run} ({}{in_quarantine}, Status: {}, {}) zurückdrehen?",
            s.counts.done_text(),
            status_label(s.status),
            s.started.as_deref().map(local_time).unwrap_or_default()
        );
        self.pending = Some(check);
        shell.confirm(Dialog::Confirm {
            id,
            title: "Rückgängig machen".into(),
            text,
            ok_label: "Zurückdrehen".into(),
            must_check: None,
            checked: false,
            danger: false,
        });
    }

    pub fn on_answer(&mut self, shell: &mut Shell, route: Route, answer: Answer) {
        let Some(check) = self.pending.take() else {
            return;
        };
        if !answer.ok {
            return;
        }
        let (run, root) = (check.summary.run.clone(), check.root.clone());
        shell.spawn(route, TASK_UNDO, TaskKind::Write, move |ctx| {
            undo_execute(&run, &root, ctx)
        });
    }

    pub fn on_done(&mut self, result: TaskResult, shell: &mut Shell) {
        match result {
            Ok(b) => {
                if let Ok(outcome) = b.downcast::<UndoOutcome>() {
                    shell.message("Rückgängig", &outcome_text(&outcome));
                    shell.generation += 1;
                }
            }
            Err(e) => shell.show_error(TASK_UNDO, &e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::undo::RestoreResult;

    #[test]
    fn ergebnistext_nennt_zaehler_und_konflikte() {
        let outcome = UndoOutcome {
            run: RunId::parse("20261003-120000-ab12").unwrap(),
            results: vec![
                RestoreResult {
                    action: 1,
                    path: r"D:\a.txt".into(),
                    status: RestoreStatus::Restored,
                },
                RestoreResult {
                    action: 2,
                    path: r"D:\b.txt".into(),
                    status: RestoreStatus::Conflict("Ursprungspfad ist belegt".into()),
                },
            ],
            already_undone: false,
            purged: false,
            aborted: false,
        };
        let text = outcome_text(&outcome);
        assert!(
            text.contains("1 wiederhergestellt, 1 Kollisionen"),
            "{text}"
        );
        assert!(
            text.contains(r"Kollision (Ursprungspfad ist belegt): D:\b.txt"),
            "{text}"
        );
        assert!(text.contains("erneut"), "{text}");
    }

    #[test]
    fn abgebrochener_undo_weist_auf_fortsetzen_hin() {
        let outcome = UndoOutcome {
            run: RunId::parse("20261003-120000-ab12").unwrap(),
            results: vec![],
            already_undone: false,
            purged: false,
            aborted: true,
        };
        assert!(outcome_text(&outcome).contains("setzt fort"));
    }
}
