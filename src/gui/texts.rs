//! Alle sichtbaren Texte der Oberfläche an einer Stelle (nur Deutsch, kein i18n-Framework).

pub const TITLE: &str = "ordner-cleanup";

pub const NAV_OVERVIEW: &str = "Übersicht";
pub const NAV_ANALYSIS: &str = "Analyse";
pub const NAV_CLEANUP: &str = "Aufräumen";
pub const NAV_HISTORY: &str = "Verlauf";
pub const NAV_QUARANTINE: &str = "Quarantäne";
pub const NAV_SETTINGS: &str = "Einstellungen";

pub const BUSY_SCAN: &str = "Ein anderer Scan läuft. Bitte später erneut versuchen.";
pub const RETRY: &str = "Erneut versuchen";
pub const CANCEL: &str = "Abbrechen";
pub const CLOSE: &str = "Schließen";
pub const OK: &str = "OK";
pub const NO_ROOT_YET: &str = "Noch nichts gescannt. Ordner wählen und scannen.";
pub const EMPTY_TITLE: &str = "Noch nichts gescannt";
pub const EMPTY_TEXT: &str =
    "Wähle einen Ordner und scanne ihn. Danach erscheinen hier Zustand und Läufe.";
pub const EMPTY_BUTTON: &str = "Ordner wählen und scannen";

pub const LOADING: &str = "Wird geladen …";
pub const ALL: &str = "Alle";

pub const QUARANTINE_ROOT: &str = "Wurzel:";
pub const QUARANTINE_SEARCH: &str = "Suchen in Name und Pfad";
pub const QUARANTINE_EMPTY: &str = "Die Quarantäne ist leer.";
pub const QUARANTINE_NO_MATCH: &str = "Keine Einträge für diese Auswahl.";
pub const QUARANTINE_REMOVE_RUN_FILTER: &str = "Filter auf diesen Lauf entfernen";
pub const COL_NAME: &str = "Name";
pub const COL_ORIGIN: &str = "Ursprünglicher Ort";
pub const COL_SIZE: &str = "Größe";
pub const COL_DATE: &str = "Lauf";
pub const COL_EXPIRES: &str = "Läuft ab";
pub const COL_STATUS: &str = "Status";

pub fn quarantine_only_run(run: &str) -> String {
    format!("Nur Lauf {run}  ✕")
}

pub fn quarantine_folder_name(name: &str) -> String {
    format!("{name} (Ordner)")
}

pub fn quarantine_file_count(count: u64) -> String {
    match count {
        1 => "1 Datei".to_string(),
        n => format!("{} Dateien", grouped(n)),
    }
}

pub fn quarantine_missing(count: usize) -> String {
    match count {
        1 => "1 Eintrag fehlt in der Quarantäne (zum Beispiel von Hand gelöscht).".to_string(),
        n => format!("{n} Einträge fehlen in der Quarantäne (zum Beispiel von Hand gelöscht)."),
    }
}

pub fn quarantine_unreadable(count: usize) -> String {
    match count {
        1 => "Ein Journal ließ sich nicht lesen; seine Einträge fehlen in der Liste.".to_string(),
        n => format!("{n} Journale ließen sich nicht lesen; ihre Einträge fehlen in der Liste."),
    }
}

pub fn quarantine_unreachable(root: &str) -> String {
    format!("Nicht erreichbar, daher nicht aufgelistet: {root}")
}

pub const CLOSE_TITLE: &str = "Aufgabe läuft noch";
pub const CLOSE_TEXT: &str = "Es läuft noch eine Aufgabe. Beim Abbrechen endet die laufende \
     Einzeloperation sauber; bereits Erledigtes bleibt (bei Apply lässt sich der Teil-Lauf \
     rückgängig machen).";
pub const CLOSE_CANCEL_AND_QUIT: &str = "Abbrechen und schließen";
pub const CLOSE_KEEP_RUNNING: &str = "Weiter laufen lassen";
pub const CLOSING: &str = "Wird beendet …";

pub fn running(name: &str) -> String {
    format!("Läuft: {name}")
}

pub fn error_title(name: &str) -> String {
    format!("{name}: Fehler")
}

pub fn bytes(n: u64) -> String {
    bytesize::ByteSize::b(n).to_string()
}

/// `1234567` → `1.234.567`
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zahlen_werden_mit_punkten_gruppiert() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_234_567), "1.234.567");
    }
}
