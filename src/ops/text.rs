//! Texte, die CLI und GUI teilen.

use bytesize::ByteSize;

use crate::change::apply::{ActionResult, ActionStatus, ApplyOutcome};

/// Eine Zeile zu einer nicht ausgeführten oder fehlgeschlagenen Aktion; erledigte haben keine.
pub fn status_line(result: &ActionResult) -> Option<String> {
    let text = match &result.status {
        ActionStatus::Done | ActionStatus::AlreadyDone => return None,
        ActionStatus::Skipped(reason) => reason.to_string(),
        ActionStatus::Failed(error) => format!("Fehler: {error}"),
    };
    Some(format!("  {text}: {}", result.path))
}

/// „Lauf <ID>: 3 in die Quarantäne verschoben (1 KB), 0 bereits erledigt, …“
pub fn apply_headline(outcome: &ApplyOutcome) -> String {
    let moved = if outcome.moved_bytes > 0 {
        format!(" ({})", ByteSize::b(outcome.moved_bytes))
    } else {
        String::new()
    };
    format!(
        "Lauf {}: {}{moved}, {} bereits erledigt, {} stale, {} übersprungen, {} Fehler",
        outcome.run,
        outcome.counts().done_text(),
        outcome.already_done(),
        outcome.stale(),
        outcome.skipped(),
        outcome.failed()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::plan::ActionType;
    use crate::change::{RunId, SkipReason};

    fn result(status: ActionStatus) -> ActionResult {
        ActionResult {
            id: 1,
            path: r"D:\x.txt".into(),
            kind: ActionType::Quarantine,
            is_dir: false,
            sorted: false,
            status,
        }
    }

    #[test]
    fn statuszeilen_nennen_nur_nicht_erledigtes() {
        assert!(status_line(&result(ActionStatus::Done)).is_none());
        assert!(status_line(&result(ActionStatus::AlreadyDone)).is_none());
        assert!(
            status_line(&result(ActionStatus::Skipped(SkipReason::Stale)))
                .unwrap()
                .contains("stale")
        );
        assert!(
            status_line(&result(ActionStatus::Failed("gesperrt".into())))
                .unwrap()
                .contains("Fehler: gesperrt")
        );
    }

    #[test]
    fn kopfzeile_zaehlt_alle_ergebnisse() {
        let outcome = ApplyOutcome {
            run: RunId::parse("20261003-120000-ab12").unwrap(),
            results: vec![
                result(ActionStatus::Done),
                result(ActionStatus::AlreadyDone),
                result(ActionStatus::Skipped(SkipReason::Stale)),
                result(ActionStatus::Failed("x".into())),
            ],
            moved_bytes: 2048,
            aborted: false,
        };
        let line = apply_headline(&outcome);
        assert!(
            line.starts_with("Lauf 20261003-120000-ab12: 1 in die Quarantäne"),
            "{line}"
        );
        assert!(
            line.contains("(2.0 KiB)") && line.contains("1 bereits erledigt"),
            "{line}"
        );
        assert!(
            line.contains("1 stale") && line.contains("1 Fehler"),
            "{line}"
        );
    }
}
