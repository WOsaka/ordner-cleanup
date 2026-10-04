//! Befehle `schedule add|list|remove`.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDateTime};

use super::{load_config, local_time};
use crate::cli::{ScheduleAddArgs, ScheduleCommand, ScheduleRemoveArgs};
use crate::paths;
use crate::platform::toast;
use crate::runlog::{self, RunRecord};
use crate::schedule::schtasks::Schtasks;
use crate::schedule::{self, task_name, Row, TaskState, Trigger};

/// Name des Hintergrundprogramms; es liegt neben der Konsolen-exe.
pub const BG_EXE: &str = "ordner-cleanup-bg.exe";

pub(super) fn schedule_command(cmd: &ScheduleCommand) -> Result<i32> {
    match cmd {
        ScheduleCommand::Add(args) => add_command(args),
        ScheduleCommand::List => list_command(),
        ScheduleCommand::Remove(args) => remove_command(args),
    }
}

fn trigger_of(args: &ScheduleAddArgs) -> Result<Trigger> {
    let trigger = match (&args.weekly, &args.daily) {
        (Some(parts), None) => match parts.as_slice() {
            [day, time] => Trigger::weekly(day, time),
            _ => bail!("--weekly erwartet Wochentag und Uhrzeit, z. B. --weekly MO 09:00"),
        },
        (None, Some(time)) => Trigger::daily(time),
        _ => bail!("Entweder --weekly <Wochentag> <Uhrzeit> oder --daily <Uhrzeit> angeben"),
    };
    trigger.map_err(anyhow::Error::msg)
}

/// Das Hintergrundprogramm neben der laufenden exe.
fn bg_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("Pfad des Programms nicht ermittelbar")?;
    let bg = exe.with_file_name(BG_EXE);
    if !bg.is_file() {
        bail!(
            "{} fehlt neben {}. Die Aufgabe startet dieses Programm ohne Konsolenfenster.",
            BG_EXE,
            paths::display(&exe)
        );
    }
    Ok(PathBuf::from(paths::display(&bg)))
}

fn current_user() -> Option<String> {
    let domain = std::env::var("USERDOMAIN").ok()?;
    let user = std::env::var("USERNAME").ok()?;
    Some(format!("{domain}\\{user}"))
}

fn add_command(args: &ScheduleAddArgs) -> Result<i32> {
    let config = load_config()?;
    config.profile(&args.profile)?;
    let trigger = trigger_of(args)?;
    let exe = bg_exe()?;
    if let Err(e) = toast::register_aumid() {
        eprintln!("Hinweis: Absender für Benachrichtigungen nicht registriert: {e}");
    }
    let now = Local::now().naive_local();
    let added = schedule::add(
        &Schtasks,
        &paths::schedules_path()?,
        &args.profile,
        &trigger,
        &exe,
        current_user().as_deref(),
        now,
    )
    .context("Aufgabe konnte nicht angelegt werden")?;
    println!(
        "Aufgabe {} {}: {}, nächster Lauf {}.",
        task_name(&args.profile),
        if added.replaced {
            "ersetzt"
        } else {
            "angelegt"
        },
        trigger.describe(),
        added.next_run.format("%d.%m.%Y %H:%M")
    );
    println!(
        "Sie läuft als aktueller Benutzer ohne Adminrechte, nur am Netzteil; verpasste Läufe \
         werden nachgeholt. Sie erzeugt Bericht und Pläne, führt aber nie `apply` aus."
    );
    Ok(0)
}

fn remove_command(args: &ScheduleRemoveArgs) -> Result<i32> {
    let removed = schedule::remove(&Schtasks, &paths::schedules_path()?, &args.profile)
        .context("Aufgabe konnte nicht entfernt werden")?;
    if removed {
        println!("Aufgabe {} entfernt.", task_name(&args.profile));
        Ok(0)
    } else {
        println!("Für Profil '{}' gibt es keine Aufgabe.", args.profile);
        Ok(1)
    }
}

fn last_run(profile: &str) -> Option<RunRecord> {
    runlog::read_all(&paths::runs_log(profile).ok()?).pop()
}

/// Eine Zeile je Aufgabe plus eine Zeile Zustand; rein, damit sie sich testen lässt.
fn render_list(
    rows: &[Row],
    last: &dyn Fn(&str) -> Option<RunRecord>,
    now: NaiveDateTime,
) -> String {
    let mut s = String::new();
    for row in rows {
        let e = &row.entry;
        let when = e
            .trigger
            .to_trigger()
            .map(|t| {
                (
                    t.describe(),
                    t.next_run(now).format("%d.%m.%Y %H:%M").to_string(),
                )
            })
            .unwrap_or_else(|_| ("unbekannter Auslöser".to_string(), "–".to_string()));
        let _ = writeln!(s, "{}", e.profile);
        let _ = writeln!(s, "  Auslöser:      {}", when.0);
        let _ = writeln!(s, "  Nächster Lauf: {}", when.1);
        let _ = match last(&e.profile) {
            Some(r) => writeln!(
                s,
                "  Letzter Lauf:  {} – {}{}",
                local_time(&r.started),
                r.status.label(),
                r.score.map(|v| format!(", Score {v}")).unwrap_or_default()
            ),
            None => writeln!(s, "  Letzter Lauf:  noch keiner"),
        };
        match &row.state {
            TaskState::Ok => {}
            TaskState::TaskMissing => {
                let _ = writeln!(
                    s,
                    "  ACHTUNG: Die Aufgabe fehlt in der Aufgabenplanung. Mit `schedule add --profile {}` neu anlegen.",
                    e.profile
                );
            }
            TaskState::ProgramMissing(exe) => {
                let _ = writeln!(
                    s,
                    "  ACHTUNG: Aufgabe zeigt auf fehlendes Programm {}. Mit `schedule add --profile {}` neu anlegen.",
                    paths::display(exe),
                    e.profile
                );
            }
        }
    }
    s
}

fn list_command() -> Result<i32> {
    let rows = schedule::list(&Schtasks, &paths::schedules_path()?, &|p: &Path| {
        p.is_file()
    })
    .context("Aufgaben konnten nicht gelesen werden")?;
    if rows.is_empty() {
        println!(
            "Keine geplanten Läufe. Anlegen mit `schedule add --profile <name> --weekly MO 09:00`."
        );
        return Ok(0);
    }
    print!(
        "{}",
        render_list(&rows, &last_run, Local::now().naive_local())
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runlog::RunStatus;
    use crate::schedule::{Entry, StoredTrigger};
    use chrono::NaiveDate;

    fn row(profile: &str, state: TaskState) -> Row {
        Row {
            entry: Entry {
                profile: profile.into(),
                task: task_name(profile),
                trigger: StoredTrigger::Weekly {
                    day: "MO".into(),
                    time: "09:00".into(),
                },
                exe: r"C:\oc\ordner-cleanup-bg.exe".into(),
                created: "2026-10-04T12:00:00".into(),
            },
            state,
        }
    }

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 4)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn record() -> RunRecord {
        RunRecord {
            started: "2026-10-03T08:00:00Z".into(),
            ended: "2026-10-03T08:01:00Z".into(),
            status: RunStatus::Ok,
            score: Some(72),
            score_delta: None,
            report: None,
            plans: vec![],
            errors: vec![],
            notified: false,
        }
    }

    #[test]
    fn liste_zeigt_ausloeser_naechsten_und_letzten_lauf() {
        let text = render_list(
            &[row("downloads", TaskState::Ok)],
            &|_| Some(record()),
            now(),
        );
        assert!(text.starts_with("downloads\n"), "{text}");
        assert!(text.contains("wöchentlich Mo 09:00"), "{text}");
        assert!(text.contains("05.10.2026 09:00"), "{text}");
        assert!(text.contains("ok, Score 72"), "{text}");
        assert!(!text.contains("ACHTUNG"));
    }

    #[test]
    fn liste_ohne_lauf_sagt_noch_keiner() {
        let text = render_list(&[row("p", TaskState::Ok)], &|_| None, now());
        assert!(text.contains("Letzter Lauf:  noch keiner"), "{text}");
    }

    #[test]
    fn liste_warnt_vor_fehlendem_programm_und_fehlender_aufgabe() {
        let text = render_list(
            &[
                row(
                    "a",
                    TaskState::ProgramMissing(PathBuf::from(r"C:\alt\bg.exe")),
                ),
                row("b", TaskState::TaskMissing),
            ],
            &|_| None,
            now(),
        );
        assert!(
            text.contains("Aufgabe zeigt auf fehlendes Programm C:\\alt\\bg.exe"),
            "{text}"
        );
        assert!(text.contains("schedule add --profile a"), "{text}");
        assert!(
            text.contains("Die Aufgabe fehlt in der Aufgabenplanung"),
            "{text}"
        );
    }
}
