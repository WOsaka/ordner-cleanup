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
