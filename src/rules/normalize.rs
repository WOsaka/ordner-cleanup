//! Namens-Normalisierung einer Regel. Wirkt nur auf den Dateinamen, erhält die
//! Groß-/Kleinschreibung und ist idempotent (`n(n(x)) == n(x)`).

use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;

/// `- Kopie`, `- Kopie (n)`, ` (n)`, `- Copy`, `_final`, `_final_vN` am Namensende.
static COPY_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\s*-\s*(?:kopie|copy)(?:\s*\(\d+\))?|\s+\(\d+\)|_final(?:_v\d+)?)$")
        .unwrap()
});
static ISO_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// Mehrfache Durchläufe, bis sich nichts mehr ändert: Ein Schritt kann den Auslöser eines
/// anderen erzeugen (`x final` → `x_final`). Mehr als ein paar Durchläufe sind nie nötig.
const MAX_PASSES: usize = 8;

fn split_ext(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

fn replace_umlauts(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'ä' => out.push_str("ae"),
            'ö' => out.push_str("oe"),
            'ü' => out.push_str("ue"),
            'Ä' => out.push_str("Ae"),
            'Ö' => out.push_str("Oe"),
            'Ü' => out.push_str("Ue"),
            'ß' => out.push_str("ss"),
            'ẞ' => out.push_str("SS"),
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Normalize {
    pub iso_date_prefix: bool,
    /// Ersatzzeichen für Leerzeichen (`_` oder `-`).
    pub spaces: Option<char>,
    pub umlauts: bool,
    pub strip_copy_suffix: bool,
    pub lowercase_ext: bool,
}

impl Normalize {
    pub fn is_noop(&self) -> bool {
        *self == Self::default()
    }

    /// Normalisiert `name` (mit Endung). `date` ist das Datum für `iso_date_prefix`.
    pub fn apply(&self, name: &str, date: NaiveDate) -> String {
        let mut current = name.to_string();
        for _ in 0..MAX_PASSES {
            let next = self.pass(&current, date);
            if next == current {
                break;
            }
            current = next;
        }
        current
    }

    fn pass(&self, name: &str, date: NaiveDate) -> String {
        let (stem, ext) = split_ext(name);
        let mut stem = stem.to_string();
        if self.strip_copy_suffix {
            let mut stripped = stem.clone();
            while let Some(m) = COPY_SUFFIX.find(&stripped) {
                stripped.truncate(m.start());
                stripped.truncate(stripped.trim_end().len());
            }
            if !stripped.is_empty() {
                stem = stripped;
            }
        }
        if self.umlauts {
            stem = replace_umlauts(&stem);
        }
        if let Some(with) = self.spaces {
            stem = SPACES.replace_all(&stem, with.to_string()).into_owned();
        }
        if self.iso_date_prefix && !ISO_START.is_match(&stem) {
            stem = format!("{}_{stem}", date.format("%Y-%m-%d"));
        }
        match ext {
            Some(ext) if self.lowercase_ext => format!("{stem}.{}", ext.to_lowercase()),
            Some(ext) => format!("{stem}.{ext}"),
            None => stem,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 30).unwrap()
    }

    fn all() -> Normalize {
        Normalize {
            iso_date_prefix: true,
            spaces: Some('_'),
            umlauts: true,
            strip_copy_suffix: true,
            lowercase_ext: true,
        }
    }

    #[test]
    fn spec_beispiel_rechnung() {
        assert_eq!(
            all().apply("Rechnung März - Kopie (2).PDF", date()),
            "2026-09-30_Rechnung_Maerz.pdf"
        );
    }

    #[rstest]
    #[case("Bericht - Kopie.docx", "Bericht.docx")]
    #[case("Bericht - Kopie (3).docx", "Bericht.docx")]
    #[case("Bericht (2).docx", "Bericht.docx")]
    #[case("Bericht - Copy.docx", "Bericht.docx")]
    #[case("Bericht_final.docx", "Bericht.docx")]
    #[case("Bericht_final_v3.docx", "Bericht.docx")]
    #[case("Bericht - Kopie (2) (3).docx", "Bericht.docx")]
    #[case("Bericht(2).docx", "Bericht(2).docx")]
    #[case("- Kopie.docx", "- Kopie.docx")]
    fn kopie_suffix(#[case] input: &str, #[case] expected: &str) {
        let n = Normalize {
            strip_copy_suffix: true,
            ..Normalize::default()
        };
        assert_eq!(n.apply(input, date()), expected);
    }

    #[rstest]
    #[case("Müller Öl Äpfel Maß.txt", "Mueller Oel Aepfel Mass.txt")]
    #[case("ÜBER.txt", "UeBER.txt")]
    fn umlaute(#[case] input: &str, #[case] expected: &str) {
        let n = Normalize {
            umlauts: true,
            ..Normalize::default()
        };
        assert_eq!(n.apply(input, date()), expected);
    }

    #[rstest]
    #[case('_', "a  b   c.txt", "a_b_c.txt")]
    #[case('-', "a b.txt", "a-b.txt")]
    fn leerzeichen(#[case] with: char, #[case] input: &str, #[case] expected: &str) {
        let n = Normalize {
            spaces: Some(with),
            ..Normalize::default()
        };
        assert_eq!(n.apply(input, date()), expected);
    }

    #[test]
    fn endung_klein_aber_name_bleibt() {
        let n = Normalize {
            lowercase_ext: true,
            ..Normalize::default()
        };
        assert_eq!(n.apply("Foto.JPG", date()), "Foto.jpg");
        assert_eq!(n.apply("Archiv.Tar.GZ", date()), "Archiv.Tar.gz");
        assert_eq!(n.apply(".GITIGNORE", date()), ".GITIGNORE");
        assert_eq!(n.apply("README", date()), "README");
    }

    #[rstest]
    #[case("Rechnung.pdf", "2026-09-30_Rechnung.pdf")]
    #[case("2025-01-02_Rechnung.pdf", "2025-01-02_Rechnung.pdf")]
    #[case("2025-01-02 Rechnung.pdf", "2025-01-02 Rechnung.pdf")]
    fn iso_praefix(#[case] input: &str, #[case] expected: &str) {
        let n = Normalize {
            iso_date_prefix: true,
            ..Normalize::default()
        };
        assert_eq!(n.apply(input, date()), expected);
    }

    #[test]
    fn ohne_optionen_bleibt_alles_wie_es_ist() {
        assert!(Normalize::default().is_noop());
        assert_eq!(
            Normalize::default().apply("Ä  b - Kopie.PDF", date()),
            "Ä  b - Kopie.PDF"
        );
    }

    #[rstest]
    #[case("Rechnung März - Kopie (2).PDF")]
    #[case("x final.pdf")]
    #[case("x  _final .pdf")]
    #[case("a - Kopie - Kopie.txt")]
    #[case("2026-09-30_x (2).txt")]
    #[case("ß ß (2) (3).TXT")]
    #[case("(2).txt")]
    #[case(" - Copy.txt")]
    #[case("Bericht_final_v2_final.docx")]
    fn idempotent(#[case] input: &str) {
        for n in [
            all(),
            Normalize {
                spaces: Some('-'),
                strip_copy_suffix: true,
                ..Normalize::default()
            },
            Normalize {
                strip_copy_suffix: true,
                iso_date_prefix: true,
                ..Normalize::default()
            },
        ] {
            let once = n.apply(input, date());
            assert_eq!(n.apply(&once, date()), once, "{input} -> {once}");
        }
    }
}
