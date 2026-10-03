//! Namensheuristik für „ähnliche Dateien / Versionen“.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

const DATE: &str = r"(?:\d{4}-\d{2}-\d{2}|\d{8}|\d{2}\.\d{2}\.\d{4})";

static DATE_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^{DATE}(?:[\s-]+|$)")).unwrap());
static DATE_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?:^|[\s-]+){DATE}$")).unwrap());
static COPY_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\s*-\s*|\s+)(?:kopie|copy)(?:\s*\(\d+\))?$").unwrap());
static COPY_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:copy of|kopie von)\s+").unwrap());
static NUMBER_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(\d+\)$").unwrap());
static VERSION_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[\s-])(?:v\d+|version\s*\d+|rev\s*\d+|final|endgültig|neu|new|alt|old)(?:$|[\s-])",
    )
    .unwrap()
});

fn strip_dates(s: &str) -> String {
    let s = DATE_START.replace(s, "");
    DATE_END.replace(&s, "").trim().to_string()
}

/// Normalisiert einen Dateinamen ohne Endung. Ist danach nichts übrig (z. B. „final“),
/// bleibt der kleingeschriebene Originalname erhalten.
pub fn normalize_name(stem: &str) -> String {
    let original = stem.trim().to_lowercase();
    let mut s = original.replace('_', " ");
    s = strip_dates(&s);
    s = COPY_PREFIX.replace(&s, "").into_owned();
    // Mehrfach anwenden, damit z. B. „x - Kopie (2) (3)“ vollständig abgebaut wird.
    loop {
        let next = COPY_SUFFIX.replace(&s, "");
        let next = NUMBER_SUFFIX.replace(&next, "").trim().to_string();
        if next == s {
            break;
        }
        s = next;
    }
    // Leerzeichen als Trenner für Versionsmarker: „-“ bleibt vorerst erhalten.
    loop {
        let next = VERSION_TOKEN.replace(&s, " ").trim().to_string();
        if next == s {
            break;
        }
        s = next;
    }
    s = strip_dates(&s);
    let collapsed: Vec<&str> = s
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter(|p| !p.is_empty())
        .collect();
    let result = collapsed.join(" ");
    if result.is_empty() {
        original
    } else {
        result
    }
}

fn split_ext(name: &str) -> (&str, String) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], name[i + 1..].to_lowercase()),
        _ => (name, String::new()),
    }
}

/// Gruppiert Dateinamen **eines Ordners**: gleicher normalisierter Name und gleiche Endung,
/// mindestens zwei Dateien. Liefert Indizes in `names`, Gruppen und Mitglieder aufsteigend.
pub fn group_similar<S: AsRef<str>>(names: &[S]) -> Vec<Vec<usize>> {
    let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (i, name) in names.iter().enumerate() {
        let (stem, ext) = split_ext(name.as_ref());
        groups
            .entry((normalize_name(stem), ext))
            .or_default()
            .push(i);
    }
    groups.into_values().filter(|g| g.len() >= 2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("Angebot", "angebot")]
    #[case("Angebot_v2", "angebot")]
    #[case("Angebot - Kopie (3)", "angebot")]
    #[case("Angebot - Kopie", "angebot")]
    #[case("Angebot - Copy", "angebot")]
    #[case("Angebot - Copy (2)", "angebot")]
    #[case("Copy of Angebot", "angebot")]
    #[case("Kopie von Angebot", "angebot")]
    #[case("Angebot (2)", "angebot")]
    #[case("2024-03-01 Angebot final", "angebot")]
    #[case("Angebot 2024-03-01", "angebot")]
    #[case("20240301_Angebot", "angebot")]
    #[case("Angebot_01.03.2024", "angebot")]
    #[case("Angebot version 3", "angebot")]
    #[case("Angebot rev4", "angebot")]
    #[case("Angebot endgültig", "angebot")]
    #[case("Angebot NEU", "angebot")]
    #[case("Angebot_old", "angebot")]
    #[case("Angebot-alt", "angebot")]
    #[case("  Angebot  ", "angebot")]
    #[case("Mein_Angebot-Entwurf", "mein angebot entwurf")]
    #[case("Mein Angebot", "mein angebot")]
    fn normalisierung(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(normalize_name(input), expected, "{input}");
    }

    #[rstest]
    #[case("final")]
    #[case("Neu")]
    #[case("v2")]
    #[case("20240301")]
    fn leerer_rest_behaelt_den_originalnamen(#[case] input: &str) {
        assert_eq!(normalize_name(input), input.to_lowercase());
    }

    #[rstest]
    #[case("Renewal")]
    #[case("Oldtimer")]
    #[case("Newton")]
    #[case("Finale")]
    #[case("Rover2")]
    fn marker_nur_als_ganze_woerter(#[case] input: &str) {
        assert_eq!(normalize_name(input), input.to_lowercase());
    }

    #[test]
    fn beispiel_aus_der_spec_bildet_eine_gruppe() {
        let names = [
            "Angebot.docx",
            "Angebot_v2.docx",
            "Angebot - Kopie (3).docx",
            "2024-03-01 Angebot final.docx",
            "Rechnung.docx",
        ];
        assert_eq!(group_similar(&names), vec![vec![0, 1, 2, 3]]);
    }

    #[test]
    fn verschiedene_endungen_bilden_keine_gruppe() {
        assert!(group_similar(&["Angebot.docx", "Angebot.pdf"]).is_empty());
    }

    #[test]
    fn endung_ist_case_insensitiv() {
        assert_eq!(group_similar(&["a.DOCX", "a_v2.docx"]), vec![vec![0, 1]]);
    }

    #[test]
    fn einzelne_dateien_sind_keine_gruppe() {
        assert!(group_similar(&["a.txt", "b.txt", "c.txt"]).is_empty());
        assert!(group_similar::<&str>(&[]).is_empty());
    }

    #[test]
    fn dateien_ohne_endung_werden_gruppiert() {
        assert_eq!(
            group_similar(&["README", "README - Kopie"]),
            vec![vec![0, 1]]
        );
    }

    #[test]
    fn mehrere_gruppen_getrennt() {
        let names = ["a.txt", "b.txt", "a (2).txt", "b (2).txt", "c.txt"];
        assert_eq!(group_similar(&names), vec![vec![0, 2], vec![1, 3]]);
    }
}
