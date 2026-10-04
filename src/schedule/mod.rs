//! Geplante Läufe über die Windows-Aufgabenplanung.
//!
//! Die Aufgaben legt `schtasks.exe /XML` an (siehe [`schtasks`]); die Wahrheit für die Anzeige ist
//! ein eigenes Register (`schedules.json`). Die Aufgabenplanung wird nur auf Existenz und
//! Programmpfad geprüft, weil ihre Textausgaben lokalisiert sind.

pub mod schtasks;
pub mod xml;

use std::io;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Duration, NaiveDateTime, NaiveTime, Weekday};
use serde::{Deserialize, Serialize};

/// Ordner der Aufgaben in der Aufgabenplanung.
pub const TASK_FOLDER: &str = r"\ordner-cleanup";

pub fn task_name(profile: &str) -> String {
    format!("{TASK_FOLDER}\\{profile}")
}

/// Wann ein Lauf stattfindet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Weekly { day: Weekday, time: NaiveTime },
    Daily { time: NaiveTime },
}

const DAYS: [(&str, Weekday, &str); 7] = [
    ("MO", Weekday::Mon, "Mo"),
    ("DI", Weekday::Tue, "Di"),
    ("MI", Weekday::Wed, "Mi"),
    ("DO", Weekday::Thu, "Do"),
    ("FR", Weekday::Fri, "Fr"),
    ("SA", Weekday::Sat, "Sa"),
    ("SO", Weekday::Sun, "So"),
];

/// Englische Kürzel gelten ebenfalls.
const ENGLISH: [(&str, Weekday); 7] = [
    ("MON", Weekday::Mon),
    ("TUE", Weekday::Tue),
    ("WED", Weekday::Wed),
    ("THU", Weekday::Thu),
    ("FRI", Weekday::Fri),
    ("SAT", Weekday::Sat),
    ("SUN", Weekday::Sun),
];

pub fn parse_time(text: &str) -> Result<NaiveTime, String> {
    let bad = || format!("ungültige Uhrzeit '{text}' (erwartet HH:MM, z. B. 09:00)");
    let (h, m) = text.trim().split_once(':').ok_or_else(bad)?;
    if h.len() > 2 || m.len() != 2 {
        return Err(bad());
    }
    let (h, m): (u32, u32) = (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
    NaiveTime::from_hms_opt(h, m, 0).ok_or_else(bad)
}

pub fn parse_day(text: &str) -> Result<Weekday, String> {
    let key = text.trim().to_uppercase();
    DAYS.iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, d, _)| *d)
        .or_else(|| ENGLISH.iter().find(|(k, _)| *k == key).map(|(_, d)| *d))
        .ok_or_else(|| {
            format!("ungültiger Wochentag '{text}' (erlaubt: MO, DI, MI, DO, FR, SA, SO)")
        })
}

fn day_key(day: Weekday) -> &'static str {
    DAYS.iter()
        .find(|(_, d, _)| *d == day)
        .map_or("MO", |(k, _, _)| k)
}

fn day_short(day: Weekday) -> &'static str {
    DAYS.iter()
        .find(|(_, d, _)| *d == day)
        .map_or("Mo", |(_, _, s)| s)
}

impl Trigger {
    pub fn weekly(day: &str, time: &str) -> Result<Self, String> {
        Ok(Self::Weekly {
            day: parse_day(day)?,
            time: parse_time(time)?,
        })
    }

    pub fn daily(time: &str) -> Result<Self, String> {
        Ok(Self::Daily {
            time: parse_time(time)?,
        })
    }

    pub fn time(&self) -> NaiveTime {
        match self {
            Self::Weekly { time, .. } | Self::Daily { time } => *time,
        }
    }

    /// „wöchentlich Mo 09:00“ bzw. „täglich 09:00“
    pub fn describe(&self) -> String {
        match self {
            Self::Weekly { day, time } => {
                format!("wöchentlich {} {}", day_short(*day), time.format("%H:%M"))
            }
            Self::Daily { time } => format!("täglich {}", time.format("%H:%M")),
        }
    }

    /// Der nächste Lauf ab `now` (ein Lauf genau zu `now` zählt als bevorstehend).
    pub fn next_run(&self, now: NaiveDateTime) -> NaiveDateTime {
        let time = self.time();
        let today = now.date().and_time(time);
        match self {
            Self::Daily { .. } => {
                if today >= now {
                    today
                } else {
                    today + Duration::days(1)
                }
            }
            Self::Weekly { day, .. } => {
                let ahead = (i64::from(day.num_days_from_monday())
                    - i64::from(now.date().weekday().num_days_from_monday()))
                .rem_euclid(7);
                let candidate = today + Duration::days(ahead);
                if candidate >= now {
                    candidate
                } else {
                    candidate + Duration::days(7)
                }
            }
        }
    }
}

/// Gespeicherte Form des Auslösers im Register.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredTrigger {
    Weekly { day: String, time: String },
    Daily { time: String },
}

impl From<&Trigger> for StoredTrigger {
    fn from(t: &Trigger) -> Self {
        match t {
            Trigger::Weekly { day, time } => Self::Weekly {
                day: day_key(*day).to_string(),
                time: time.format("%H:%M").to_string(),
            },
            Trigger::Daily { time } => Self::Daily {
                time: time.format("%H:%M").to_string(),
            },
        }
    }
}

impl StoredTrigger {
    pub fn to_trigger(&self) -> Result<Trigger, String> {
        match self {
            Self::Weekly { day, time } => Trigger::weekly(day, time),
            Self::Daily { time } => Trigger::daily(time),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub profile: String,
    pub task: String,
    pub trigger: StoredTrigger,
    /// Programm, auf das die Aufgabe beim Anlegen zeigte
    pub exe: String,
    pub created: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegisterFile {
    version: u32,
    tasks: Vec<Entry>,
}

pub fn load_register(path: &Path) -> io::Result<Vec<Entry>> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<RegisterFile>(&text)
            .map(|f| f.tasks)
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Register {} ist beschädigt: {e}", path.display()),
                )
            }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn save_register(path: &Path, tasks: Vec<Entry>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&RegisterFile { version: 1, tasks })
        .map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Die Aufgabenplanung hinter einer Schnittstelle, damit Tests keine echten Aufgaben anlegen.
pub trait TaskService {
    /// Legt die Aufgabe aus dem XML an oder ersetzt sie.
    fn create(&self, name: &str, xml: &str) -> io::Result<()>;
    /// Löscht die Aufgabe; eine nicht vorhandene ist kein Fehler.
    fn delete(&self, name: &str) -> io::Result<()>;
    /// Programm der Aufgabe; `None`, wenn es die Aufgabe nicht gibt.
    fn query_command(&self, name: &str) -> io::Result<Option<PathBuf>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    /// Eine vorhandene Aufgabe wurde ersetzt
    pub replaced: bool,
    pub next_run: NaiveDateTime,
}

/// Legt die Aufgabe für `profile` an; eine vorhandene wird ersetzt, nie doppelt angelegt.
pub fn add(
    service: &dyn TaskService,
    register: &Path,
    profile: &str,
    trigger: &Trigger,
    exe: &Path,
    user: Option<&str>,
    now: NaiveDateTime,
) -> io::Result<Added> {
    let mut tasks = load_register(register)?;
    let name = task_name(profile);
    let next_run = trigger.next_run(now);
    let xml = xml::task_xml(profile, exe, trigger, next_run, user, &name);
    // Ersetzen heißt: die Aufgabe existiert schon, im Register oder in der Aufgabenplanung.
    let existed = service.query_command(&name)?.is_some();
    service.create(&name, &xml)?;
    let in_register = tasks.iter().any(|e| e.profile == profile);
    tasks.retain(|e| e.profile != profile);
    tasks.push(Entry {
        profile: profile.to_string(),
        task: name,
        trigger: trigger.into(),
        exe: exe.display().to_string(),
        created: now.format("%Y-%m-%dT%H:%M:%S").to_string(),
    });
    tasks.sort_by(|a, b| a.profile.cmp(&b.profile));
    save_register(register, tasks)?;
    Ok(Added {
        replaced: existed || in_register,
        next_run,
    })
}

/// Entfernt die Aufgabe; `false`, wenn es weder Aufgabe noch Registereintrag gab.
pub fn remove(service: &dyn TaskService, register: &Path, profile: &str) -> io::Result<bool> {
    let mut tasks = load_register(register)?;
    let name = task_name(profile);
    let existed = service.query_command(&name)?.is_some();
    service.delete(&name)?;
    let before = tasks.len();
    tasks.retain(|e| e.profile != profile);
    let had_entry = tasks.len() != before;
    if had_entry {
        save_register(register, tasks)?;
    }
    Ok(existed || had_entry)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Ok,
    /// Im Register, aber nicht (mehr) in der Aufgabenplanung
    TaskMissing,
    /// Die Aufgabe zeigt auf ein Programm, das es nicht mehr gibt
    ProgramMissing(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub entry: Entry,
    pub state: TaskState,
}

/// Alle registrierten Aufgaben mit ihrem Zustand. `exists` prüft, ob eine Datei existiert.
pub fn list(
    service: &dyn TaskService,
    register: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> io::Result<Vec<Row>> {
    load_register(register)?
        .into_iter()
        .map(|entry| {
            let state = match service.query_command(&entry.task)? {
                None => TaskState::TaskMissing,
                Some(queried) => {
                    // Die Abfrage kann Sonderzeichen verfälschen; das Programm gilt als da, wenn
                    // entweder der abgefragte oder der registrierte Pfad existiert.
                    let registered = PathBuf::from(&entry.exe);
                    if exists(&queried) || exists(&registered) {
                        TaskState::Ok
                    } else {
                        TaskState::ProgramMissing(registered)
                    }
                }
            };
            Ok(Row { entry, state })
        })
        .collect()
}

#[cfg(test)]
mod tests;
