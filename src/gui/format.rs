//! Darstellung von Werten für die Oberfläche: Status, Zeiten, Pfade, Score, Dauer-Eingaben.
//! Alles rein und ohne egui, damit es ohne Fenster testbar ist. Farben kennt nur `theme`.

use chrono::{DateTime, Local};

use crate::change::undo::RunStatus;
use crate::index::RootStatus;

/// Wie ein Wert gewichtet dargestellt wird; `theme` ordnet jedem Ton eine Farbe zu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Error,
    Muted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusStyle {
    pub symbol: &'static str,
    pub text: String,
    pub tone: Tone,
}

impl StatusStyle {
    /// „✔ Vollständig“ bzw. nur der Text, wenn es kein Symbol gibt.
    pub fn label(&self) -> String {
        if self.symbol.is_empty() {
            self.text.clone()
        } else {
            format!("{} {}", self.symbol, self.text)
        }
    }
}

/// Status eines Laufs im Verlauf.
pub fn run_status(status: RunStatus) -> StatusStyle {
    let (symbol, text, tone) = match status {
        RunStatus::Complete => ("✔", "Vollständig", Tone::Ok),
        RunStatus::Partial => ("⚠", "Teilweise", Tone::Warn),
        RunStatus::Incomplete => ("⚠", "Unvollständig", Tone::Warn),
        RunStatus::Undone => ("↩", "Rückgängig gemacht", Tone::Muted),
        RunStatus::PartiallyUndone => ("↩", "Teilweise rückgängig", Tone::Warn),
        RunStatus::Purged => ("–", "Quarantäne gelöscht", Tone::Muted),
        RunStatus::Unreadable => ("✖", "Journal unlesbar", Tone::Error),
    };
    StatusStyle {
        symbol,
        text: text.into(),
        tone,
    }
}

/// Status einer gescannten Wurzel samt Fehleranzahl.
pub fn root_status(status: RootStatus, errors: i64) -> StatusStyle {
    let with_errors = |base: &str| match errors {
        0 => base.to_string(),
        n => format!("{base}, {n} Fehler"),
    };
    match status {
        RootStatus::Running => StatusStyle {
            symbol: "",
            text: "Läuft".into(),
            tone: Tone::Muted,
        },
        RootStatus::Complete => StatusStyle {
            symbol: if errors == 0 { "✔" } else { "⚠" },
            text: with_errors("Vollständig"),
            tone: if errors == 0 { Tone::Ok } else { Tone::Warn },
        },
        RootStatus::Aborted => StatusStyle {
            symbol: "⚠",
            text: with_errors("Scan unvollständig"),
            tone: Tone::Warn,
        },
    }
}

/// Kurztext und Tooltip eines Zeitpunkts („vor 2 Tagen“ / „08.10.2026 14:03“).
pub fn relative_time(now: DateTime<Local>, t: DateTime<Local>) -> (String, String) {
    let exact = t.format("%d.%m.%Y %H:%M").to_string();
    let date = t.format("%d.%m.%Y").to_string();
    let diff = now - t;
    // Mehr als eine Minute in der Zukunft (falsch gehende Uhr): das Datum statt „vor …“.
    if diff < chrono::Duration::seconds(-60) {
        return (date, exact);
    }
    let short = if diff < chrono::Duration::minutes(1) {
        "gerade eben".to_string()
    } else if diff < chrono::Duration::hours(1) {
        format!("vor {} Min.", diff.num_minutes())
    } else {
        let days = (now.date_naive() - t.date_naive()).num_days();
        match days {
            0 => format!("heute {}", t.format("%H:%M")),
            1 => format!("gestern {}", t.format("%H:%M")),
            2..=6 => format!("vor {days} Tagen"),
            _ => date,
        }
    };
    (short, exact)
}

/// Hinweis „Stand: vor 12 Tagen, neu scannen?“, wenn der Scan älter als `STALE_DAYS` ist.
pub const STALE_DAYS: i64 = 7;

pub fn stale_note(scanned: DateTime<Local>, now: DateTime<Local>) -> Option<String> {
    let days = (now - scanned).num_days();
    (days > STALE_DAYS).then(|| format!("Stand: vor {days} Tagen, neu scannen?"))
}

/// Kürzt einen Pfad in der Mitte auf höchstens `max_chars` Zeichen: `D:\…\Archiv\Datei.txt`.
pub fn short_path(path: &str, max_chars: usize) -> String {
    let len = path.chars().count();
    if len <= max_chars {
        return path.to_string();
    }
    // Laufwerk bzw. Server und Freigabe bleiben stehen.
    let separators = if path.starts_with(r"\\") { 4 } else { 1 };
    let head_end = path
        .match_indices('\\')
        .nth(separators - 1)
        .map_or(path.len(), |(i, _)| i + 1);
    let (head, rest) = path.split_at(head_end);
    let head_len = head.chars().count();
    let budget = max_chars.saturating_sub(head_len + 2);
    let mut tail = String::new();
    for segment in rest.split('\\').rev() {
        let candidate = if tail.is_empty() {
            segment.to_string()
        } else {
            format!("{segment}\\{tail}")
        };
        if candidate.chars().count() > budget {
            break;
        }
        tail = candidate;
    }
    if tail.is_empty() {
        // Schon der Dateiname passt nicht: vorne abschneiden.
        let name = rest.rsplit('\\').next().unwrap_or(rest);
        let keep = max_chars.saturating_sub(head_len + 1);
        let skip = name.chars().count().saturating_sub(keep);
        let suffix: String = name.chars().skip(skip).collect();
        return format!("{head}…{suffix}");
    }
    format!("{head}…\\{tail}")
}

/// Einordnung des Health-Scores: ≥ 80 gut, 60–79 mittel, darunter schwach. Reine Darstellung;
/// die Berechnung steht in `health::score`.
pub fn score_style(score: u8) -> StatusStyle {
    let (symbol, text, tone) = match score {
        80.. => ("✔", "gut", Tone::Ok),
        60..=79 => ("⚠", "mittel", Tone::Warn),
        _ => ("✖", "schwach", Tone::Error),
    };
    StatusStyle {
        symbol,
        text: text.into(),
        tone,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationUnit {
    Days,
    Months,
    Years,
}

/// Zahl und Einheit als Argument der CLI (`2` + Jahre → `2y`). Leer = `None` (Wert aus der
/// Config).
pub fn duration_to_arg(amount: &str, unit: DurationUnit) -> Result<Option<String>, String> {
    let amount = amount.trim();
    if amount.is_empty() {
        return Ok(None);
    }
    let n: u64 =
        amount.parse().ok().filter(|n| *n > 0).ok_or_else(|| {
            format!("Ungültige Zahl '{amount}' (erwartet eine ganze Zahl über 0)")
        })?;
    let suffix = match unit {
        DurationUnit::Days => 'd',
        DurationUnit::Months => 'm',
        DurationUnit::Years => 'y',
    };
    let arg = format!("{n}{suffix}");
    // Derselbe Parser wie in der CLI, damit auch ein Überlauf auffällt.
    crate::analysis::age::parse_old_after(&arg)?;
    Ok(Some(arg))
}

/// Zeile der Kopfleiste: „Letzter Scan: vor 2 Tagen · Score 72 (▲ 3)“.
pub fn scan_info(
    now: DateTime<Local>,
    finished: Option<&str>,
    score: Option<u8>,
    previous: Option<u8>,
) -> String {
    let Some(finished) = finished else {
        return "Noch nicht gescannt".into();
    };
    let when = match DateTime::parse_from_rfc3339(finished) {
        Ok(t) => relative_time(now, t.with_timezone(&Local)).0,
        Err(_) => finished.to_string(),
    };
    match score {
        Some(_) => format!("Letzter Scan: {when} · Score {}", trend(score, previous)),
        None => format!("Letzter Scan: {when}"),
    }
}

/// `80 (▲ 10)`, `60 (▼ 10)`, `60 (=)`, `60` oder `–`.
pub fn trend(score: Option<u8>, previous: Option<u8>) -> String {
    match (score, previous) {
        (Some(s), Some(p)) if s > p => format!("{s} (▲ {})", s - p),
        (Some(s), Some(p)) if s < p => format!("{s} (▼ {})", p - s),
        (Some(s), Some(_)) => format!("{s} (=)"),
        (Some(s), None) => s.to_string(),
        _ => "–".into(),
    }
}

/// „n Dateien zum Prüfen“ aus dem letzten Lauf (nur Profile mit `classify = true`).
pub fn review_text(last_run: Option<&crate::runlog::RunRecord>) -> Option<String> {
    match last_run?.review? {
        1 => Some("1 Datei zum Prüfen".into()),
        n => Some(format!("{n} Dateien zum Prüfen")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn laufstatus_hat_deutschen_text_und_ton() {
        let all = [
            (RunStatus::Complete, "Vollständig", Tone::Ok),
            (RunStatus::Partial, "Teilweise", Tone::Warn),
            (RunStatus::Incomplete, "Unvollständig", Tone::Warn),
            (RunStatus::Undone, "Rückgängig gemacht", Tone::Muted),
            (
                RunStatus::PartiallyUndone,
                "Teilweise rückgängig",
                Tone::Warn,
            ),
            (RunStatus::Purged, "Quarantäne gelöscht", Tone::Muted),
            (RunStatus::Unreadable, "Journal unlesbar", Tone::Error),
        ];
        for (status, text, tone) in all {
            let s = run_status(status);
            assert_eq!((s.text.as_str(), s.tone), (text, tone), "{status:?}");
            assert!(!s.text.contains("Complete"), "kein Debug-Text: {s:?}");
        }
        assert_eq!(run_status(RunStatus::Complete).label(), "✔ Vollständig");
    }

    #[test]
    fn wurzelstatus_nennt_fehler_und_unterscheidet_abbruch() {
        assert_eq!(root_status(RootStatus::Complete, 0).text, "Vollständig");
        assert_eq!(root_status(RootStatus::Complete, 0).tone, Tone::Ok);
        let with_errors = root_status(RootStatus::Complete, 3);
        assert_eq!(with_errors.text, "Vollständig, 3 Fehler");
        assert_eq!(with_errors.tone, Tone::Warn);
        let aborted = root_status(RootStatus::Aborted, 1);
        assert_eq!(aborted.text, "Scan unvollständig, 1 Fehler");
        assert_eq!(aborted.tone, Tone::Warn);
        assert_eq!(
            root_status(RootStatus::Aborted, 0).text,
            "Scan unvollständig"
        );
        assert_eq!(root_status(RootStatus::Running, 0).tone, Tone::Muted);
    }

    #[test]
    fn relative_zeit_staffelt_von_minuten_bis_datum() {
        let now = at(2026, 10, 8, 14, 30);
        let rel = |t| relative_time(now, t).0;
        assert_eq!(rel(at(2026, 10, 8, 14, 30)), "gerade eben");
        assert_eq!(rel(at(2026, 10, 8, 14, 5)), "vor 25 Min.");
        assert_eq!(rel(at(2026, 10, 8, 9, 3)), "heute 09:03");
        assert_eq!(rel(at(2026, 10, 7, 23, 50)), "gestern 23:50");
        assert_eq!(rel(at(2026, 10, 6, 12, 0)), "vor 2 Tagen");
        assert_eq!(rel(at(2026, 10, 1, 12, 0)), "01.10.2026");
        assert_eq!(rel(at(2026, 9, 12, 8, 0)), "12.09.2026");
    }

    #[test]
    fn relative_zeit_hat_den_genauen_wert_als_tooltip() {
        let now = at(2026, 10, 8, 14, 30);
        assert_eq!(
            relative_time(now, at(2026, 10, 6, 12, 7)).1,
            "06.10.2026 12:07"
        );
    }

    #[test]
    fn zeit_in_der_zukunft_faellt_auf_das_datum_zurueck() {
        let now = at(2026, 10, 8, 14, 30);
        let (short, _) = relative_time(now, at(2026, 10, 12, 9, 0));
        assert_eq!(short, "12.10.2026");
        assert!(!short.contains("vor"), "{short}");
    }

    #[test]
    fn stand_hinweis_erst_nach_sieben_tagen() {
        let now = at(2026, 10, 20, 12, 0);
        assert_eq!(stale_note(at(2026, 10, 13, 12, 0), now), None);
        assert_eq!(
            stale_note(at(2026, 10, 8, 12, 0), now).as_deref(),
            Some("Stand: vor 12 Tagen, neu scannen?")
        );
        assert_eq!(stale_note(at(2026, 10, 21, 12, 0), now), None);
    }

    #[test]
    fn kurzer_pfad_bleibt_unveraendert() {
        assert_eq!(short_path(r"D:\Daten\a.txt", 40), r"D:\Daten\a.txt");
    }

    #[test]
    fn langer_pfad_wird_in_der_mitte_gekuerzt() {
        let p = r"D:\Daten\Projekte\Alt\Archiv\Datei.txt";
        assert_eq!(short_path(p, 25), r"D:\…\Alt\Archiv\Datei.txt");
        assert_eq!(short_path(p, 24), r"D:\…\Archiv\Datei.txt");
        for max in 18..p.chars().count() {
            assert!(short_path(p, max).chars().count() <= max, "max {max}");
        }
    }

    #[test]
    fn zu_langer_dateiname_wird_vorne_abgeschnitten() {
        let p = r"D:\Ordner\eine-sehr-lange-datei-mit-umlauten-äöü.docx";
        let s = short_path(p, 20);
        assert_eq!(s.chars().count(), 20, "{s}");
        assert!(s.starts_with(r"D:\…"), "{s}");
        assert!(s.ends_with(".docx"), "{s}");
    }

    #[test]
    fn netzpfad_behaelt_server_und_freigabe() {
        let p = r"\\server\freigabe\a\b\c\d\datei.txt";
        assert_eq!(short_path(p, 31), r"\\server\freigabe\…\d\datei.txt");
        assert_eq!(short_path(p, 30), r"\\server\freigabe\…\datei.txt");
    }

    #[test]
    fn score_wird_in_drei_baender_eingeordnet() {
        for (score, text, tone) in [
            (100, "gut", Tone::Ok),
            (80, "gut", Tone::Ok),
            (79, "mittel", Tone::Warn),
            (60, "mittel", Tone::Warn),
            (59, "schwach", Tone::Error),
            (0, "schwach", Tone::Error),
        ] {
            let s = score_style(score);
            assert_eq!((s.text.as_str(), s.tone), (text, tone), "Score {score}");
        }
    }

    #[test]
    fn dauer_wird_zum_cli_argument() {
        use DurationUnit::*;
        assert_eq!(duration_to_arg("2", Years), Ok(Some("2y".into())));
        assert_eq!(duration_to_arg(" 30 ", Days), Ok(Some("30d".into())));
        assert_eq!(duration_to_arg("18", Months), Ok(Some("18m".into())));
    }

    #[test]
    fn leere_dauer_heisst_wert_aus_der_config() {
        assert_eq!(duration_to_arg("  ", DurationUnit::Days), Ok(None));
    }

    #[test]
    fn ungueltige_dauer_ist_ein_fehler_kein_ueberlauf() {
        use DurationUnit::*;
        for bad in ["0", "-3", "abc", "1,5", "2y"] {
            assert!(duration_to_arg(bad, Days).is_err(), "{bad}");
        }
        assert!(duration_to_arg("99999999999999999", Years).is_err());
    }

    #[test]
    fn kopfzeile_nennt_scan_zeit_und_score() {
        let now = at(2026, 10, 8, 14, 30);
        let finished = at(2026, 10, 6, 12, 0).to_rfc3339();
        assert_eq!(
            scan_info(now, Some(&finished), Some(72), Some(69)),
            "Letzter Scan: vor 2 Tagen · Score 72 (▲ 3)"
        );
        assert_eq!(
            scan_info(now, Some(&finished), None, None),
            "Letzter Scan: vor 2 Tagen"
        );
        assert_eq!(
            scan_info(now, Some(&finished), Some(72), None),
            "Letzter Scan: vor 2 Tagen · Score 72"
        );
    }

    #[test]
    fn kopfzeile_ohne_scan_sagt_das() {
        let now = at(2026, 10, 8, 14, 30);
        assert_eq!(scan_info(now, None, None, None), "Noch nicht gescannt");
    }

    #[test]
    fn kopfzeile_zeigt_unlesbare_zeit_unveraendert() {
        let now = at(2026, 10, 8, 14, 30);
        assert_eq!(
            scan_info(now, Some("gestern"), None, None),
            "Letzter Scan: gestern"
        );
    }

    #[test]
    fn trend_zeigt_richtung_und_differenz() {
        assert_eq!(trend(Some(80), Some(70)), "80 (▲ 10)");
        assert_eq!(trend(Some(60), Some(70)), "60 (▼ 10)");
        assert_eq!(trend(Some(60), Some(60)), "60 (=)");
        assert_eq!(trend(Some(60), None), "60");
        assert_eq!(trend(None, None), "–");
    }

    #[test]
    fn zum_pruefen_kommt_aus_dem_letzten_lauf() {
        assert_eq!(review_text(None), None);
        let mut r = crate::runlog::RunRecord {
            started: "2026-10-03T12:00:00+02:00".into(),
            ended: "2026-10-03T12:01:00+02:00".into(),
            status: crate::runlog::RunStatus::Ok,
            score: None,
            score_delta: None,
            report: None,
            plans: Vec::new(),
            errors: Vec::new(),
            notified: false,
            review: None,
        };
        assert_eq!(review_text(Some(&r)), None);
        r.review = Some(1);
        assert_eq!(review_text(Some(&r)).as_deref(), Some("1 Datei zum Prüfen"));
        r.review = Some(19);
        assert_eq!(
            review_text(Some(&r)).as_deref(),
            Some("19 Dateien zum Prüfen")
        );
    }
}
