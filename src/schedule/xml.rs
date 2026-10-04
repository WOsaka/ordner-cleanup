//! Task-XML für die Windows-Aufgabenplanung (`schtasks /Create /XML`) und das Auslesen des
//! Programmpfads aus `schtasks /Query /XML`.

use std::path::{Path, PathBuf};

use chrono::{NaiveDateTime, Weekday};

use super::Trigger;

fn escape(text: &str) -> String {
    quick_xml::escape::escape(text).into_owned()
}

fn day_element(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

/// Die Aufgabe für `profile`: Hintergrundprogramm mit `run --profile <name> --notify`.
///
/// Einstellungen laut Spec: aktueller Benutzer ohne Adminrechte und ohne Passwort
/// (`InteractiveToken`, `LeastPrivilege`), nur am Netzteil und Abbruch beim Wechsel auf Akku,
/// verpasste Läufe werden nachgeholt, Priorität „unter normal“, nie zwei Läufe gleichzeitig.
pub fn task_xml(
    profile: &str,
    exe: &Path,
    trigger: &Trigger,
    start: NaiveDateTime,
    user: Option<&str>,
    task_name: &str,
) -> String {
    let start = start.format("%Y-%m-%dT%H:%M:%S");
    let schedule = match trigger {
        Trigger::Weekly { day, .. } => format!(
            "<ScheduleByWeek><DaysOfWeek><{0}/></DaysOfWeek><WeeksInterval>1</WeeksInterval></ScheduleByWeek>",
            day_element(*day)
        ),
        Trigger::Daily { .. } => {
            "<ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay>".to_string()
        }
    };
    let user_id = user
        .map(|u| format!("<UserId>{}</UserId>", escape(u)))
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>{description}</Description>
    <URI>{uri}</URI>
  </RegistrationInfo>
  <Triggers>
    <CalendarTrigger>
      <StartBoundary>{start}</StartBoundary>
      <Enabled>true</Enabled>
      {schedule}
    </CalendarTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      {user_id}
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>true</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>true</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT4H</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>run --profile {profile} --notify</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        description = escape(&format!(
            "ordner-cleanup: geplanter Lauf für Profil {profile} (Scan, Bericht, Pläne; nie apply)"
        )),
        uri = escape(task_name),
        command = escape(&exe.display().to_string()),
        profile = escape(profile),
    )
}

/// Die Datei, die `schtasks` erwartet: UTF-16 LE mit Byte-Order-Mark.
pub fn to_utf16_le(xml: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for unit in xml.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

/// Dekodiert die Ausgabe von `schtasks /Query /XML` (UTF-16 mit BOM, sonst UTF-8/Lossy).
pub fn decode_output(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// `<Exec><Command>` aus dem Aufgaben-XML. Das XML stammt von der Aufgabenplanung bzw. aus
/// [`task_xml`]; der Befehl steht in einem einfachen `<Command>…</Command>`.
pub fn command_from_xml(xml: &str) -> Option<PathBuf> {
    let start = xml.find("<Command>")? + "<Command>".len();
    let end = start + xml[start..].find("</Command>")?;
    let text = quick_xml::escape::unescape(&xml[start..end]).ok()?;
    let text = text.trim().trim_matches('"');
    (!text.is_empty()).then(|| PathBuf::from(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveTime};

    fn start() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 5)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap()
    }

    fn weekly() -> Trigger {
        Trigger::Weekly {
            day: Weekday::Mon,
            time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
        }
    }

    #[test]
    fn xml_enthaelt_alle_zugesagten_einstellungen() {
        let xml = task_xml(
            "downloads",
            Path::new(r"C:\Programme\ordner-cleanup\ordner-cleanup-bg.exe"),
            &weekly(),
            start(),
            Some(r"PC\oskar"),
            r"\ordner-cleanup\downloads",
        );
        for expected in [
            "<LogonType>InteractiveToken</LogonType>",
            "<RunLevel>LeastPrivilege</RunLevel>",
            "<DisallowStartIfOnBatteries>true</DisallowStartIfOnBatteries>",
            "<StopIfGoingOnBatteries>true</StopIfGoingOnBatteries>",
            "<StartWhenAvailable>true</StartWhenAvailable>",
            "<Priority>7</Priority>",
            "<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
            "<ExecutionTimeLimit>PT4H</ExecutionTimeLimit>",
            "<StartBoundary>2026-10-05T09:00:00</StartBoundary>",
            "<ScheduleByWeek><DaysOfWeek><Monday/></DaysOfWeek><WeeksInterval>1</WeeksInterval></ScheduleByWeek>",
            r"<Command>C:\Programme\ordner-cleanup\ordner-cleanup-bg.exe</Command>",
            "<Arguments>run --profile downloads --notify</Arguments>",
            r"<UserId>PC\oskar</UserId>",
            r"<URI>\ordner-cleanup\downloads</URI>",
        ] {
            assert!(xml.contains(expected), "fehlt: {expected}\n{xml}");
        }
        // nie ein gespeichertes Passwort, nie erhöhte Rechte
        assert!(!xml.contains("Password") && !xml.contains("HighestAvailable"));
        let args_start = xml.find("<Arguments>").unwrap();
        let args = &xml[args_start..xml[args_start..].find("</Arguments>").unwrap() + args_start];
        assert!(
            !args.contains("apply"),
            "die Aufgabe ruft nie apply auf: {args}"
        );
    }

    #[test]
    fn taeglich_nutzt_schedule_by_day() {
        let xml = task_xml(
            "p",
            Path::new(r"C:\x.exe"),
            &Trigger::Daily {
                time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            },
            start(),
            None,
            r"\ordner-cleanup\p",
        );
        assert!(xml.contains("<ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay>"));
        assert!(!xml.contains("<UserId>"));
    }

    #[test]
    fn pfad_mit_leerzeichen_und_kaufmaennischem_und_wird_escaped() {
        let exe = Path::new(r"C:\Program Files\A&B <x>\ordner-cleanup-bg.exe");
        let xml = task_xml("p", exe, &weekly(), start(), None, r"\ordner-cleanup\p");
        assert!(
            xml.contains(
                r"<Command>C:\Program Files\A&amp;B &lt;x&gt;\ordner-cleanup-bg.exe</Command>"
            ),
            "{xml}"
        );
        // wohlgeformt, und der Pfad kommt unverändert zurück
        assert_eq!(command_from_xml(&xml).as_deref(), Some(exe));
        let mut reader = quick_xml::Reader::from_str(&xml);
        while reader.read_event().unwrap() != quick_xml::events::Event::Eof {}
    }

    #[test]
    fn utf16_datei_hat_bom_und_laesst_sich_zuruecklesen() {
        let bytes = to_utf16_le("<a>Ü</a>");
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        assert_eq!(decode_output(&bytes), "<a>Ü</a>");
        assert_eq!(decode_output(b"<a>x</a>"), "<a>x</a>");
    }

    #[test]
    fn befehl_aus_abfrage_ohne_command_ist_none() {
        assert_eq!(command_from_xml("<Task><Actions/></Task>"), None);
        assert_eq!(command_from_xml("kein xml <<<"), None);
        assert_eq!(
            command_from_xml(
                r#"<Task xmlns="x"><Actions><Exec><Command>"C:\a b\c.exe"</Command></Exec></Actions></Task>"#
            ),
            Some(PathBuf::from(r"C:\a b\c.exe"))
        );
    }
}
