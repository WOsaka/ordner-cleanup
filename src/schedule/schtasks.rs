//! Die echte Aufgabenplanung über `schtasks.exe`. Ausgewertet werden nur Exit-Code und die
//! (nicht lokalisierte) XML-Ausgabe von `/Query /XML`.

use std::io;
use std::path::PathBuf;
use std::process::Command;

use super::{xml, TaskService};

pub struct Schtasks;

fn schtasks() -> Command {
    let mut cmd = Command::new("schtasks.exe");
    // Kein Konsolenfenster, auch wenn wir aus dem Hintergrundprogramm starten.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn failure(action: &str, output: &std::process::Output) -> io::Error {
    let text = String::from_utf8_lossy(&output.stderr);
    let text = text.trim();
    io::Error::other(format!(
        "schtasks {action} scheiterte (Exit-Code {}){}{}",
        output.status.code().unwrap_or(-1),
        if text.is_empty() { "" } else { ": " },
        text
    ))
}

impl TaskService for Schtasks {
    fn create(&self, name: &str, task_xml: &str) -> io::Result<()> {
        let file =
            std::env::temp_dir().join(format!("ordner-cleanup-task-{}.xml", std::process::id()));
        std::fs::write(&file, xml::to_utf16_le(task_xml))?;
        let output = schtasks()
            .args(["/Create", "/TN", name, "/XML"])
            .arg(&file)
            .arg("/F")
            .output();
        let _ = std::fs::remove_file(&file);
        let output = output?;
        if output.status.success() {
            Ok(())
        } else {
            Err(failure("/Create", &output))
        }
    }

    fn delete(&self, name: &str) -> io::Result<()> {
        if self.query_command(name)?.is_none() {
            return Ok(());
        }
        let output = schtasks().args(["/Delete", "/TN", name, "/F"]).output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(failure("/Delete", &output))
        }
    }

    fn query_command(&self, name: &str) -> io::Result<Option<PathBuf>> {
        let output = schtasks().args(["/Query", "/TN", name, "/XML"]).output()?;
        if !output.status.success() {
            // Die Meldung ist lokalisiert; „nicht gefunden“ ist der einzige erwartete Fehler.
            return Ok(None);
        }
        let text = xml::decode_output(&output.stdout);
        // Die Aufgabe gibt es, auch wenn sich ihr Programm nicht auslesen lässt.
        Ok(Some(xml::command_from_xml(&text).unwrap_or_default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::{task_name, Trigger};
    use chrono::NaiveDate;

    /// Echte Aufgabenplanung unter `\ordner-cleanup-test\`: anlegen, abfragen, ersetzen, löschen.
    #[test]
    #[ignore = "legt kurz eine echte Aufgabe in der Windows-Aufgabenplanung an"]
    fn echte_aufgabe_anlegen_abfragen_loeschen() {
        let name = r"\ordner-cleanup-test\unit";
        let service = Schtasks;
        let exe = std::env::current_exe().unwrap();
        let now = NaiveDate::from_ymd_opt(2026, 10, 5)
            .unwrap()
            .and_hms_opt(8, 0, 0)
            .unwrap();
        let trigger = Trigger::daily("09:00").unwrap();
        let user = format!(
            "{}\\{}",
            std::env::var("USERDOMAIN").unwrap(),
            std::env::var("USERNAME").unwrap()
        );
        let xml = crate::schedule::xml::task_xml(
            "unit",
            &exe,
            &trigger,
            trigger.next_run(now),
            Some(&user),
            name,
        );
        service.delete(name).unwrap();
        assert_eq!(service.query_command(name).unwrap(), None);
        service.create(name, &xml).unwrap();
        let queried = service.query_command(name).unwrap().unwrap();
        assert_eq!(
            queried.to_string_lossy().to_lowercase(),
            exe.to_string_lossy().to_lowercase()
        );
        // Ersetzen statt doppelt anlegen
        service.create(name, &xml).unwrap();
        service.delete(name).unwrap();
        assert_eq!(service.query_command(name).unwrap(), None);
        assert_eq!(task_name("x"), r"\ordner-cleanup\x");
    }
}
