//! Feldmuster auf bereinigtem Text (Groß-/Kleinschreibung erhalten): Datum, Betrag, Nummer,
//! Absender, Titel. Reihenfolge der Quellen: Muster, dann Dokument-Metadaten (LLM kommt
//! später dazu). Jeder Wert wird auf Plausibilität geprüft und bereinigt.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use regex::Regex;

use super::defs::Sender;
use crate::content::Fields;

/// Feld → `pattern` | `meta` | `llm`
pub type FieldSources = BTreeMap<String, String>;

/// Frühestes plausibles Jahr für Dokumentdaten.
pub const MIN_YEAR: i32 = 1990;

const MONTHS: &[(&str, u32)] = &[
    ("januar", 1),
    ("january", 1),
    ("jan", 1),
    ("februar", 2),
    ("february", 2),
    ("feb", 2),
    ("märz", 3),
    ("maerz", 3),
    ("march", 3),
    ("mär", 3),
    ("mar", 3),
    ("april", 4),
    ("apr", 4),
    ("mai", 5),
    ("may", 5),
    ("juni", 6),
    ("june", 6),
    ("jun", 6),
    ("juli", 7),
    ("july", 7),
    ("jul", 7),
    ("august", 8),
    ("aug", 8),
    ("september", 9),
    ("sept", 9),
    ("sep", 9),
    ("oktober", 10),
    ("october", 10),
    ("okt", 10),
    ("oct", 10),
    ("november", 11),
    ("nov", 11),
    ("dezember", 12),
    ("december", 12),
    ("dez", 12),
    ("dec", 12),
];

const MONTH_ALT: &str = "januar|january|jan|februar|february|feb|märz|maerz|march|mär|mar|april|apr|mai|may|juni|june|jun|juli|july|jul|august|aug|september|sept|sep|oktober|october|okt|oct|november|nov|dezember|december|dez|dec";

/// Datumsformen; Gruppen 1 bis 3 sind je nach Form anders belegt (siehe `parse_date`).
static DATE_NUMERIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,2})\.\s?(\d{1,2})\.\s?(\d{4}|\d{2})\b").unwrap());
static DATE_ISO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{4})-(\d{2})-(\d{2})\b").unwrap());
static DATE_DE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b(\d{{1,2}})\.?\s+({MONTH_ALT})\.?\s+(\d{{4}})\b"
    ))
    .unwrap()
});
static DATE_EN_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)\b({MONTH_ALT})\.?\s+(\d{{1,2}})(?:st|nd|rd|th)?,?\s+(\d{{4}})\b"
    ))
    .unwrap()
});
static DATE_LABEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[^\p{L}])(rechnungsdatum|rechnungs-datum|belegdatum|buchungsdatum|invoice date|ausstellungsdatum|ausgestellt am|datum|date|vom)\s*[:.]?\s*",
    )
    .unwrap()
});

fn month_number(name: &str) -> Option<u32> {
    let name = name.to_lowercase();
    MONTHS.iter().find(|(m, _)| *m == name).map(|(_, n)| *n)
}

/// Prüft Plausibilität: gültiges Datum, Jahr zwischen 1990 und heute + 1 Jahr.
pub fn plausible(y: i32, m: u32, d: u32, today: NaiveDate) -> Option<NaiveDate> {
    if y < MIN_YEAR || y > today.year() + 1 {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    (date <= today + chrono::Days::new(366)).then_some(date)
}

/// Erstes Datum, das direkt am Anfang von `s` steht.
fn date_at_start(s: &str, today: NaiveDate) -> Option<NaiveDate> {
    let anchored = |re: &Regex| {
        re.captures(s)
            .filter(|c| c.get(0).is_some_and(|m| m.start() == 0))
    };
    if let Some(c) = anchored(&DATE_NUMERIC) {
        let y: i32 = c[3].parse().ok()?;
        let y = if c[3].len() == 2 { 2000 + y } else { y };
        return plausible(y, c[2].parse().ok()?, c[1].parse().ok()?, today);
    }
    if let Some(c) = anchored(&DATE_ISO) {
        return plausible(
            c[1].parse().ok()?,
            c[2].parse().ok()?,
            c[3].parse().ok()?,
            today,
        );
    }
    if let Some(c) = anchored(&DATE_DE_NAME) {
        return plausible(
            c[3].parse().ok()?,
            month_number(&c[2])?,
            c[1].parse().ok()?,
            today,
        );
    }
    if let Some(c) = anchored(&DATE_EN_NAME) {
        return plausible(
            c[3].parse().ok()?,
            month_number(&c[1])?,
            c[2].parse().ok()?,
            today,
        );
    }
    None
}

/// Alle plausiblen Daten im Text mit Startposition, nach Position sortiert.
fn all_dates(text: &str, today: NaiveDate) -> Vec<(usize, NaiveDate)> {
    let mut found = Vec::new();
    for re in [&*DATE_NUMERIC, &*DATE_ISO, &*DATE_DE_NAME, &*DATE_EN_NAME] {
        for m in re.find_iter(text) {
            if let Some(d) = date_at_start(&text[m.start()..], today) {
                found.push((m.start(), d));
            }
        }
    }
    found.sort();
    found.dedup_by_key(|(pos, _)| *pos);
    found
}

/// Dokumentdatum: beschriftet („Rechnungsdatum“, „Datum“, „Date“, „vom“) vor unbeschriftet.
pub fn find_date(text: &str, today: NaiveDate) -> Option<NaiveDate> {
    for label in DATE_LABEL.captures_iter(text) {
        let end = label.get(0)?.end();
        if let Some(d) = date_at_start(&text[end..], today) {
            return Some(d);
        }
    }
    all_dates(text, today).first().map(|(_, d)| *d)
}

static AMOUNT_NUM: &str = r"(-?\d{1,3}(?:[., ]\d{3})*[,.]\d{2}|-?\d+[,.]\d{2})";

/// Betragsbeschriftungen, stärkste Stufe zuerst.
static AMOUNT_TIERS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        "gesamtbetrag|endbetrag|rechnungsbetrag|zahlbetrag|gesamtsumme|amount due|total amount|zu zahlen",
        "summe|total|gesamt|betrag",
    ]
    .iter()
    .map(|labels| {
        Regex::new(&format!(
            r"(?i)(?:^|[^\p{{L}}])(?:{labels})\s*(?:\((?:brutto|inkl\.?[^)]*)\))?\s*:?\s*(?:eur|€|\$)?\s*{AMOUNT_NUM}"
        ))
        .unwrap()
    })
    .collect()
});

/// `1.234,56` / `1234.56` / `12,5`… → `1234,56`; `None` bei unsinnigen Werten.
pub fn normalize_amount(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let negative = raw.starts_with('-');
    let digits_part = raw.trim_start_matches('-');
    let sep_pos = digits_part.rfind([',', '.'])?;
    let (int_part, dec_part) = (&digits_part[..sep_pos], &digits_part[sep_pos + 1..]);
    if dec_part.len() != 2 || !dec_part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let int_digits: String = int_part.chars().filter(char::is_ascii_digit).collect();
    if int_digits.is_empty() || int_digits.len() > 9 {
        return None;
    }
    let int_clean = int_digits.trim_start_matches('0');
    let int_clean = if int_clean.is_empty() { "0" } else { int_clean };
    Some(format!(
        "{}{int_clean},{dec_part}",
        if negative { "-" } else { "" }
    ))
}

/// Betrag: je Stufe der letzte beschriftete Treffer (Endsummen stehen unten).
pub fn find_amount(text: &str) -> Option<String> {
    for tier in AMOUNT_TIERS.iter() {
        let last = tier
            .captures_iter(text)
            .filter_map(|c| normalize_amount(&c[1]))
            .last();
        if last.is_some() {
            return last;
        }
    }
    None
}

static NUMBER_TIERS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"rechnungs-?\s?nr\.?|rechnungsnummer|invoice\s*(?:no\.?|number|#)",
        r"vertrags-?\s?nr\.?|vertragsnummer|contract\s*(?:no\.?|number)",
        r"kunden-?\s?nr\.?|kundennummer|customer\s*(?:no\.?|number)",
    ]
    .iter()
    .map(|labels| {
        Regex::new(&format!(
            r"(?i)(?:^|[^\p{{L}}])(?:{labels})\s*[:#]?\s*([A-Za-z0-9][A-Za-z0-9/_\-.]{{2,30}})"
        ))
        .unwrap()
    })
    .collect()
});

pub fn find_number(text: &str) -> Option<String> {
    for tier in NUMBER_TIERS.iter() {
        for c in tier.captures_iter(text) {
            let value = c[1].trim_end_matches(['.', '-', '/', '_']).to_string();
            if value.chars().any(|ch| ch.is_ascii_digit()) && value.len() >= 3 {
                return Some(value);
            }
        }
    }
    None
}

/// Erster bekannter Absender (frühester Treffer im Text).
pub fn find_sender(text: &str, senders: &[Sender]) -> Option<String> {
    let folded = text.to_lowercase();
    senders
        .iter()
        .filter_map(|s| {
            s.matches
                .iter()
                .filter_map(|m| folded.find(m.as_str()))
                .min()
                .map(|pos| (pos, &s.name))
        })
        .min_by_key(|(pos, _)| *pos)
        .map(|(_, name)| name.clone())
}

/// Metadaten-Titel, die nichts aussagen (Programmvorgabe, Dateiname, Platzhalter).
pub fn is_junk_title(title: &str, file_name: &str) -> bool {
    let t = title.trim().to_lowercase();
    if t.is_empty() || t.chars().all(|c| c.is_ascii_digit() || c.is_whitespace()) {
        return true;
    }
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(s, _)| s)
        .to_lowercase();
    if t == stem || t == file_name.to_lowercase() {
        return true;
    }
    const PREFIXES: &[&str] = &[
        "microsoft word",
        "microsoft excel",
        "microsoft powerpoint",
        "untitled",
        "unbenannt",
        "dokument",
        "document",
        "präsentation",
        "presentation",
        "mappe",
        "arbeitsmappe",
        "folie",
        "slide",
        "scan",
        "img_",
        "image",
        "pdf",
    ];
    if PREFIXES.iter().any(|p| {
        t.strip_prefix(p).is_some_and(|rest| {
            let rest = rest.trim_start_matches([' ', '-', '_', ':']);
            rest.is_empty()
                || rest.chars().all(|c| c.is_ascii_digit() || c == ' ')
                || *p == "microsoft word"
                || *p == "microsoft excel"
                || *p == "microsoft powerpoint"
        })
    }) {
        return true;
    }
    const SUFFIXES: &[&str] = &[
        ".doc", ".docx", ".pdf", ".xls", ".xlsx", ".ppt", ".pptx", ".tmp", ".indd",
    ];
    SUFFIXES.iter().any(|s| t.ends_with(s))
}

/// Entfernt Zeichen, die in Windows-Namen verboten sind (`<>:"/\|?*`, Steuerzeichen),
/// fasst Leerraum zusammen und schneidet Punkte und Leerzeichen am Ende ab.
pub fn clean_value(raw: &str) -> String {
    let replaced: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                ' '
            } else {
                c
            }
        })
        .collect();
    replaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['.', ' '])
        .to_string()
}

/// Kürzt auf höchstens `max` Zeichen, nach Möglichkeit am Wortende.
pub fn shorten(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let cut: String = value.chars().take(max).collect();
    let boundary_after = value.chars().nth(max).is_some_and(char::is_whitespace);
    let result = if boundary_after {
        cut.as_str()
    } else {
        match cut.rfind(' ') {
            Some(pos) if pos * 10 >= max * 4 => &cut[..pos],
            _ => cut.as_str(),
        }
    };
    result.trim_end_matches(['.', ' ']).to_string()
}

/// Felder aus Text und Metadaten. `text` ist bereinigter Text (Groß-/Kleinschreibung erhalten).
fn put(fields: &mut Fields, sources: &mut FieldSources, name: &str, value: String, source: &str) {
    if !value.is_empty() {
        fields.insert(name.to_string(), value);
        sources.insert(name.to_string(), source.to_string());
    }
}

pub fn extract_fields(
    text: Option<&str>,
    meta: &Fields,
    senders: &[Sender],
    file_name: &str,
    today: NaiveDate,
) -> (Fields, FieldSources) {
    let mut fields = Fields::new();
    let mut sources = FieldSources::new();
    if let Some(text) = text {
        if let Some(d) = find_date(text, today) {
            put(
                &mut fields,
                &mut sources,
                "doc.date",
                d.format("%Y-%m-%d").to_string(),
                "pattern",
            );
        }
        if let Some(a) = find_amount(text) {
            put(&mut fields, &mut sources, "doc.amount", a, "pattern");
        }
        if let Some(n) = find_number(text) {
            put(
                &mut fields,
                &mut sources,
                "doc.number",
                clean_value(&n),
                "pattern",
            );
        }
        if let Some(s) = find_sender(text, senders) {
            put(
                &mut fields,
                &mut sources,
                "doc.sender",
                clean_value(&s),
                "pattern",
            );
        }
    }
    if !fields.contains_key("doc.date") {
        if let Some(created) = meta.get("doc.created") {
            if let Ok(d) = NaiveDate::parse_from_str(created, "%Y-%m-%d") {
                if plausible(d.year(), d.month(), d.day(), today).is_some() {
                    put(
                        &mut fields,
                        &mut sources,
                        "doc.date",
                        created.clone(),
                        "meta",
                    );
                }
            }
        }
    }
    if let Some(title) = meta.get("doc.title") {
        let title = clean_value(title);
        if !is_junk_title(&title, file_name) {
            put(&mut fields, &mut sources, "doc.title", title, "meta");
        }
    }
    if let Some(author) = meta.get("doc.author") {
        put(
            &mut fields,
            &mut sources,
            "doc.author",
            clean_value(author),
            "meta",
        );
    }
    if let Some(pages) = meta.get("doc.pages") {
        put(
            &mut fields,
            &mut sources,
            "doc.pages",
            pages.clone(),
            "meta",
        );
    }
    (fields, sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 4).unwrap()
    }

    fn d(y: i32, m: u32, day: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, day)
    }

    #[rstest]
    #[case("Rechnungsdatum: 30.09.2026", d(2026, 9, 30))]
    #[case("Rechnungsdatum 30. 09. 2026 Seite 1", d(2026, 9, 30))]
    #[case("Datum: 05.01.2025", d(2025, 1, 5))]
    #[case("Date: 2025-12-31", d(2025, 12, 31))]
    #[case("Invoice Date: 12 March 2024", d(2024, 3, 12))]
    #[case("Date: March 3rd, 2024", d(2024, 3, 3))]
    #[case("Berlin, den 1. Juli 2023", d(2023, 7, 1))]
    #[case("Hamburg, 15. Dezember 2022", d(2022, 12, 15))]
    #[case("Rechnung vom 03.02.25", d(2025, 2, 3))]
    #[case(
        "Leistungszeitraum 01.01.2026 bis 31.03.2026 Datum: 15.04.2026",
        d(2026, 4, 15)
    )]
    #[case("Geboren 12.05.1950, Rechnungsdatum 01.02.2026", d(2026, 2, 1))]
    #[case("nur 31.02.2026 und 99.99.2026", None)]
    #[case("zu alt 01.01.1980", None)]
    #[case("zu neu 01.01.2030", None)]
    #[case("kein datum hier", None)]
    #[case("Fälligkeitsdatum: 30.10.2026", d(2026, 10, 30))]
    #[case("Datum: 02.10.2027 und danach", d(2027, 10, 2))]
    #[case("2026-09-30 im Kopf", d(2026, 9, 30))]
    #[case("Stand: 7. Okt. 2025", d(2025, 10, 7))]
    #[case("Abrechnung Mai 2024 vom 31.05.2024", d(2024, 5, 31))]
    fn daten(#[case] text: &str, #[case] expected: Option<NaiveDate>) {
        assert_eq!(find_date(text, today()), expected, "{text}");
    }

    #[test]
    fn beschriftetes_datum_gewinnt_vor_frueherem_unbeschrifteten() {
        let text = "Kundenstamm seit 01.01.2020 ... Rechnungsdatum: 15.08.2026";
        assert_eq!(find_date(text, today()), d(2026, 8, 15));
    }

    #[rstest]
    #[case("Gesamtbetrag: 123,45 EUR", Some("123,45"))]
    #[case("Gesamtbetrag 1.234,56 €", Some("1234,56"))]
    #[case("Endbetrag EUR 99,00", Some("99,00"))]
    #[case("Total: $1,234.56", Some("1234,56"))]
    #[case("Total: 1234.56", Some("1234,56"))]
    #[case("Rechnungsbetrag (brutto): 119,00 EUR", Some("119,00"))]
    #[case("Summe 10,00 Summe 25,50 Gesamtbetrag 35,50", Some("35,50"))]
    #[case("Zwischensumme 10,00 Summe 20,00", Some("20,00"))]
    #[case("Amount due: 7.50", Some("7,50"))]
    #[case("Gesamtbetrag: -12,30 EUR", Some("-12,30"))]
    #[case("Gesamtbetrag: 12 EUR", None)]
    #[case("kein Betrag", None)]
    #[case("Gesamtbetrag: 1 234,56 €", Some("1234,56"))]
    fn betraege(#[case] text: &str, #[case] expected: Option<&str>) {
        assert_eq!(find_amount(text).as_deref(), expected, "{text}");
    }

    #[rstest]
    #[case("Rechnungsnummer: RE-2026-0042", Some("RE-2026-0042"))]
    #[case("Rechnungs-Nr. 4711", Some("4711"))]
    #[case("Rechnungs-Nr.: 2026/0042.", Some("2026/0042"))]
    #[case("Invoice No: INV-1001", Some("INV-1001"))]
    #[case("Invoice #12345", Some("12345"))]
    #[case("Vertragsnummer 88-1234-5", Some("88-1234-5"))]
    #[case("Kundennummer: 4400123", Some("4400123"))]
    #[case("Rechnungsnummer: abc", None)]
    #[case("keine nummer", None)]
    fn nummern(#[case] text: &str, #[case] expected: Option<&str>) {
        assert_eq!(find_number(text).as_deref(), expected, "{text}");
    }

    #[test]
    fn rechnungsnummer_geht_vor_kundennummer() {
        assert_eq!(
            find_number("Kundennummer: 4400123 Rechnungsnummer: R-99-1").as_deref(),
            Some("R-99-1")
        );
    }

    #[test]
    fn absender_der_fruehere_treffer_gewinnt() {
        let senders = vec![
            Sender {
                name: "Telekom".into(),
                matches: vec!["telekom deutschland gmbh".into(), "telekom.de".into()],
            },
            Sender {
                name: "Amazon".into(),
                matches: vec!["amazon".into()],
            },
        ];
        assert_eq!(
            find_sender("Amazon EU ... Telekom Deutschland GmbH", &senders).as_deref(),
            Some("Amazon")
        );
        assert_eq!(
            find_sender("Besuchen Sie TELEKOM.DE", &senders).as_deref(),
            Some("Telekom")
        );
        assert_eq!(find_sender("niemand", &senders), None);
        assert_eq!(find_sender("Amazon", &[]), None);
    }

    #[rstest]
    #[case("Microsoft Word - Dokument1", "a.docx", true)]
    #[case("Microsoft Word - Rechnung Müller.docx", "a.docx", true)]
    #[case("Untitled", "a.pdf", true)]
    #[case("Unbenannt 3", "a.pdf", true)]
    #[case("Dokument1", "a.docx", true)]
    #[case("Dokument", "a.docx", true)]
    #[case("scan0012", "scan0012.pdf", true)]
    #[case("Scan 2024", "x.pdf", true)]
    #[case("Mappe1", "x.xlsx", true)]
    #[case("12345", "x.pdf", true)]
    #[case("", "x.pdf", true)]
    #[case("rechnung.pdf", "x.pdf", true)]
    #[case("Rechnung Telekom Mai", "x.pdf", false)]
    #[case("Dokumentation zum Projekt", "x.pdf", false)]
    #[case("Mietvertrag Hauptstraße 5", "x.pdf", false)]
    fn titel_muell(#[case] title: &str, #[case] file: &str, #[case] junk: bool) {
        assert_eq!(is_junk_title(title, file), junk, "{title}");
    }

    #[test]
    fn werte_bereinigen() {
        assert_eq!(clean_value("A<B>:C/D\\E|F?G*H\"I"), "A B C D E F G H I");
        assert_eq!(
            clean_value("  zu   viel \t Leerraum.. "),
            "zu viel Leerraum"
        );
        assert_eq!(clean_value("Tab\u{0007}Bell"), "Tab Bell");
        assert_eq!(clean_value("..."), "");
    }

    #[test]
    fn kuerzen_am_wortende() {
        assert_eq!(shorten("kurz", 10), "kurz");
        assert_eq!(
            shorten("Rechnung Telekom Deutschland", 20),
            "Rechnung Telekom"
        );
        assert_eq!(
            shorten("Rechnung Telekom Deutschland", 16),
            "Rechnung Telekom"
        );
        assert_eq!(shorten("Telekomdeutschlandgmbh", 10), "Telekomdeu");
        assert_eq!(
            shorten("ein langes Wort ohne", 8),
            "ein lang",
            "kein Wortende im hinteren Teil"
        );
    }

    #[test]
    fn felder_aus_text_und_metadaten() {
        let mut meta = Fields::new();
        meta.insert("doc.title".into(), "Rechnung Mai".into());
        meta.insert("doc.author".into(), "Max Muster".into());
        meta.insert("doc.pages".into(), "2".into());
        meta.insert("doc.created".into(), "2026-05-01".into());
        let senders = vec![Sender {
            name: "Telekom".into(),
            matches: vec!["telekom".into()],
        }];
        let (fields, sources) = extract_fields(
            Some("Telekom Rechnungsnummer: R-5 Rechnungsdatum: 30.04.2026 Gesamtbetrag: 12,50 EUR"),
            &meta,
            &senders,
            "scan.pdf",
            today(),
        );
        assert_eq!(fields["doc.date"], "2026-04-30");
        assert_eq!(sources["doc.date"], "pattern", "Text vor Metadaten");
        assert_eq!(fields["doc.amount"], "12,50");
        assert_eq!(fields["doc.number"], "R-5");
        assert_eq!(fields["doc.sender"], "Telekom");
        assert_eq!(fields["doc.title"], "Rechnung Mai");
        assert_eq!(sources["doc.title"], "meta");
        assert_eq!(fields["doc.author"], "Max Muster");
        assert_eq!(fields["doc.pages"], "2");
    }

    #[test]
    fn metadaten_datum_ist_der_zweite_wunsch() {
        let mut meta = Fields::new();
        meta.insert("doc.created".into(), "2026-05-01".into());
        let (fields, sources) = extract_fields(Some("ohne Datum"), &meta, &[], "a.pdf", today());
        assert_eq!(fields["doc.date"], "2026-05-01");
        assert_eq!(sources["doc.date"], "meta");
        meta.insert("doc.created".into(), "1970-01-01".into());
        let (fields, _) = extract_fields(None, &meta, &[], "a.pdf", today());
        assert!(!fields.contains_key("doc.date"), "unplausibel");
    }

    #[test]
    fn muell_titel_wird_verworfen() {
        let mut meta = Fields::new();
        meta.insert("doc.title".into(), "Microsoft Word - Dokument1".into());
        let (fields, _) = extract_fields(None, &meta, &[], "a.docx", today());
        assert!(!fields.contains_key("doc.title"));
    }
}
