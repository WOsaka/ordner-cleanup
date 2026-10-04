//! Wann ein geplanter Lauf eine Benachrichtigung zeigt, und mit welchem Text. Rein: der echte
//! Toast steht in `platform::toast`.

use crate::config::NotifyConfig;
use crate::report::history::format_delta;
use crate::runlog::{RunRecord, RunStatus};

/// Was ein Klick auf die Benachrichtigung öffnet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTarget {
    Report,
    /// Das Lauf-Protokoll (bei Fehlern gibt es keinen neuen Bericht)
    Log,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub open: OpenTarget,
}

/// Entscheidet über die Benachrichtigung eines Laufs. `previous` ist der Eintrag davor im
/// Lauf-Protokoll (für „wiederholter Ausfall“).
pub fn decide(
    profile: &str,
    record: &RunRecord,
    previous: Option<&RunRecord>,
    config: &NotifyConfig,
) -> Option<Notification> {
    let first_error = || record.errors.first().cloned().unwrap_or_default();
    match record.status {
        RunStatus::RootMissing => Some(Notification {
            title: format!("{profile}: Ordner nicht erreichbar"),
            body: first_error(),
            open: OpenTarget::Log,
        }),
        RunStatus::Failed => Some(Notification {
            title: format!("{profile}: Lauf gescheitert"),
            body: first_error(),
            open: OpenTarget::Log,
        }),
        // Ein einzelner übersprungener Lauf ist normal (ein manueller Scan lief gerade).
        RunStatus::SkippedLocked => previous
            .is_some_and(|p| p.status == RunStatus::SkippedLocked)
            .then(|| Notification {
                title: format!("{profile}: Lauf wiederholt übersprungen"),
                body: "Ein anderer Scan blockiert die geplanten Läufe.".to_string(),
                open: OpenTarget::Log,
            }),
        RunStatus::Ok | RunStatus::Partial => {
            let dropped = record.score_delta.is_some_and(|d| {
                config.score_drop > 0 && d <= -i32::try_from(config.score_drop).unwrap_or(i32::MAX)
            });
            let low = record
                .score
                .is_some_and(|s| u32::from(s) < config.score_below);
            let plans = record.non_empty_plans();
            let failed_plans: Vec<&str> = record
                .plans
                .iter()
                .filter(|p| p.error.is_some())
                .map(|p| p.kind.as_str())
                .collect();
            if !(dropped || low || plans > 0 || !failed_plans.is_empty()) {
                return None;
            }
            let mut parts = Vec::new();
            if let Some(score) = record.score {
                parts.push(match record.score_delta {
                    Some(d) => format!("Health-Score {score} ({})", format_delta(d)),
                    None => format!("Health-Score {score}"),
                });
            }
            match plans {
                0 => {}
                1 => parts.push("1 Plan bereit".to_string()),
                n => parts.push(format!("{n} Pläne bereit")),
            }
            if !failed_plans.is_empty() {
                parts.push(format!("Plan fehlgeschlagen: {}", failed_plans.join(", ")));
            }
            Some(Notification {
                title: profile.to_string(),
                body: parts.join(", "),
                open: if record.report.is_some() {
                    OpenTarget::Report
                } else {
                    OpenTarget::Log
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runlog::PlanRecord;

    fn record(status: RunStatus) -> RunRecord {
        RunRecord {
            started: "2026-10-04T08:00:00Z".into(),
            ended: "2026-10-04T08:01:00Z".into(),
            status,
            score: Some(80),
            score_delta: Some(0),
            report: Some(r"C:\r\latest.html".into()),
            plans: vec![],
            errors: vec![],
            notified: false,
        }
    }

    fn plan(kind: &str, actions: u64, error: Option<&str>) -> PlanRecord {
        PlanRecord {
            kind: kind.into(),
            actions,
            path: None,
            error: error.map(String::from),
        }
    }

    fn decide_default(r: &RunRecord, previous: Option<&RunRecord>) -> Option<Notification> {
        decide("downloads", r, previous, &NotifyConfig::default())
    }

    #[test]
    fn ruhiger_lauf_zeigt_nichts() {
        assert_eq!(decide_default(&record(RunStatus::Ok), None), None);
        let mut r = record(RunStatus::Ok);
        r.score_delta = Some(-4);
        r.plans = vec![plan("junk", 0, None)];
        assert_eq!(
            decide_default(&r, None),
            None,
            "−4 und leerer Plan reichen nicht"
        );
    }

    #[test]
    fn score_abfall_ab_der_schwelle() {
        let mut r = record(RunStatus::Ok);
        r.score = Some(75);
        r.score_delta = Some(-5);
        let n = decide_default(&r, None).unwrap();
        assert_eq!(n.title, "downloads");
        assert_eq!(n.body, "Health-Score 75 (−5)");
        assert_eq!(n.open, OpenTarget::Report);
    }

    #[test]
    fn score_unter_der_schwelle_auch_ohne_veraenderung() {
        let mut r = record(RunStatus::Ok);
        r.score = Some(59);
        r.score_delta = None;
        assert_eq!(decide_default(&r, None).unwrap().body, "Health-Score 59");
        r.score = Some(60);
        assert_eq!(
            decide_default(&r, None),
            None,
            "genau 60 ist nicht darunter"
        );
    }

    #[test]
    fn nicht_leere_plaene_werden_genannt() {
        let mut r = record(RunStatus::Ok);
        r.plans = vec![
            plan("junk", 3, None),
            plan("rules", 1, None),
            plan("empty-dirs", 0, None),
        ];
        assert_eq!(
            decide_default(&r, None).unwrap().body,
            "Health-Score 80 (±0), 2 Pläne bereit"
        );
        r.plans = vec![plan("junk", 3, None)];
        assert!(decide_default(&r, None)
            .unwrap()
            .body
            .ends_with("1 Plan bereit"));
    }

    #[test]
    fn gescheiterter_plan_wird_gemeldet() {
        let mut r = record(RunStatus::Partial);
        r.plans = vec![plan("rules", 0, Some("Regeldatei fehlt"))];
        let n = decide_default(&r, None).unwrap();
        assert!(n.body.contains("Plan fehlgeschlagen: rules"), "{}", n.body);
    }

    #[test]
    fn fehler_zeigen_das_protokoll() {
        let mut r = record(RunStatus::RootMissing);
        r.errors = vec!["Wurzel D:\\x nicht erreichbar".into()];
        r.report = None;
        let n = decide_default(&r, None).unwrap();
        assert_eq!(n.title, "downloads: Ordner nicht erreichbar");
        assert_eq!(n.body, "Wurzel D:\\x nicht erreichbar");
        assert_eq!(n.open, OpenTarget::Log);
        let mut f = record(RunStatus::Failed);
        f.errors = vec!["Scan abgebrochen".into()];
        assert_eq!(
            decide_default(&f, None).unwrap().title,
            "downloads: Lauf gescheitert"
        );
    }

    #[test]
    fn uebersprungen_nur_bei_wiederholung() {
        let skipped = record(RunStatus::SkippedLocked);
        assert_eq!(decide_default(&skipped, None), None);
        assert_eq!(decide_default(&skipped, Some(&record(RunStatus::Ok))), None);
        let n = decide_default(&skipped, Some(&record(RunStatus::SkippedLocked))).unwrap();
        assert!(n.title.contains("wiederholt übersprungen"));
        assert_eq!(n.open, OpenTarget::Log);
    }

    #[test]
    fn schwellen_kommen_aus_der_config() {
        let mut r = record(RunStatus::Ok);
        r.score = Some(90);
        r.score_delta = Some(-2);
        let strict = NotifyConfig {
            score_drop: 2,
            score_below: 95,
        };
        assert!(decide("p", &r, None, &strict).is_some());
        let off = NotifyConfig {
            score_drop: 0,
            score_below: 0,
        };
        assert_eq!(decide("p", &r, None, &off), None);
    }
}
