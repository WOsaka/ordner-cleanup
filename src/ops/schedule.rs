//! Geplante Läufe (Windows-Aufgabenplanung): anlegen, entfernen, auflisten.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Local;

use super::load_config;
use crate::paths;
use crate::platform::toast;
use crate::schedule::schtasks::Schtasks;
use crate::schedule::{self, Added, Row, Trigger};

/// Name des Hintergrundprogramms; es liegt neben der laufenden exe.
pub const BG_EXE: &str = "ordner-cleanup-bg.exe";

/// Das Hintergrundprogramm neben der laufenden exe.
pub fn bg_exe() -> Result<PathBuf> {
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

/// Ergebnis von [`schedule_add`].
#[derive(Debug)]
pub struct ScheduleAdded {
    pub added: Added,
    /// Der Absender für Benachrichtigungen ließ sich nicht registrieren (die Aufgabe steht trotzdem)
    pub aumid_warning: Option<String>,
}

/// Legt die Aufgabe für ein Profil an (ersetzt eine vorhandene).
pub fn schedule_add(profile: &str, trigger: &Trigger) -> Result<ScheduleAdded> {
    let config = load_config()?;
    config.profile(profile)?;
    let exe = bg_exe()?;
    let aumid_warning = toast::register_aumid().err().map(|e| e.to_string());
    let added = schedule::add(
        &Schtasks,
        &paths::schedules_path()?,
        profile,
        trigger,
        &exe,
        current_user().as_deref(),
        Local::now().naive_local(),
    )
    .context("Aufgabe konnte nicht angelegt werden")?;
    Ok(ScheduleAdded {
        added,
        aumid_warning,
    })
}

/// Entfernt die Aufgabe eines Profils; `false`, wenn es keine gab.
pub fn schedule_remove(profile: &str) -> Result<bool> {
    schedule::remove(&Schtasks, &paths::schedules_path()?, profile)
        .context("Aufgabe konnte nicht entfernt werden")
}

/// Alle angelegten Aufgaben mit ihrem Zustand.
pub fn schedule_list() -> Result<Vec<Row>> {
    schedule::list(&Schtasks, &paths::schedules_path()?, &|p: &Path| {
        p.is_file()
    })
    .context("Aufgaben konnten nicht gelesen werden")
}
