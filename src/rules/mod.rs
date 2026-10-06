//! Phase 4: Regeldatei laden, prüfen und auf Dateien anwenden. Reine Logik ohne Dateizugriff
//! (außer dem Lesen der Regeldatei).

pub mod condition;
mod file;
pub mod normalize;
pub mod template;

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime};

pub use condition::{Candidate, CategorySource, Conditions, MatchCtx, MatchResult, NearMiss};
pub use normalize::Normalize;
pub use template::{TargetError, TargetKind, Template};

use crate::scan::source::TICKS_PER_SEC;
use template::Values;

/// Fehler in der Regeldatei mit Datei, Zeile und Regelname (soweit bekannt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesError {
    pub file: String,
    pub line: Option<usize>,
    pub rule: Option<String>,
    pub message: String,
}

impl fmt::Display for RulesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.file)?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        f.write_str(": ")?;
        if let Some(rule) = &self.rule {
            write!(f, "Regel „{rule}“: ")?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for RulesError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExifFallback {
    Mtime,
    Skip,
}

/// Kategorienamen, die `category` in Regeln annehmen darf.
#[derive(Debug, Clone)]
pub enum KnownCategories {
    Names(BTreeSet<String>),
    /// Die Kategorie-Datei ließ sich nicht laden; Regeln mit `category` scheitern damit.
    /// (Regeln ohne Inhaltsbedingung brauchen keine gültige `categories.toml`.)
    Unavailable(String),
}

impl KnownCategories {
    /// Nur die eingebauten Kategorien.
    pub fn builtin() -> Self {
        Self::Names(crate::content::classify::defs::CategoryDefs::builtin().names())
    }

    pub fn check(&self, name: &str) -> Result<(), String> {
        match self {
            Self::Names(names) if names.contains(name) => Ok(()),
            Self::Names(names) => Err(format!(
                "unbekannte Kategorie „{name}“ (bekannt: {})",
                names.iter().cloned().collect::<Vec<_>>().join(", ")
            )),
            Self::Unavailable(why) => Err(format!(
                "Kategorie „{name}“ nicht prüfbar, die Kategorie-Datei ist fehlerhaft: {why}"
            )),
        }
    }
}

#[derive(Debug)]
pub struct Rule {
    pub name: String,
    /// Position in der Regelmenge (0-basiert; `select` vergibt sie neu), Schlüssel für die
    /// `text_regex`-Ergebnisse.
    pub index: usize,
    /// Zeile des `[[rules]]`-Blocks in der Regeldatei.
    pub line: usize,
    pub conditions: Conditions,
    pub target: Template,
    pub exif_fallback: ExifFallback,
    pub normalize: Normalize,
}

/// Ziel relativ zur Wurzel (bzw. zum Ordner der Datei bei `same_dir`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelTarget {
    pub dirs: Vec<String>,
    pub file: String,
    pub same_dir: bool,
}

#[derive(Debug)]
pub struct RuleSet {
    pub source: PathBuf,
    pub rules: Vec<Rule>,
}

/// Zeitstempel (100-ns-Ticks seit Unix-Epoche) als lokale Zeit.
pub fn local_naive(ticks: i64) -> NaiveDateTime {
    DateTime::from_timestamp(ticks.div_euclid(TICKS_PER_SEC), 0)
        .unwrap_or_default()
        .with_timezone(&chrono::Local)
        .naive_local()
}

impl RuleSet {
    pub fn load(path: &Path, known: &KnownCategories) -> Result<Self, RulesError> {
        let text = std::fs::read_to_string(path).map_err(|e| RulesError {
            file: path.display().to_string(),
            line: None,
            rule: None,
            message: format!("nicht lesbar ({e})"),
        })?;
        Self::parse(&text, path, known)
    }

    pub fn parse(text: &str, source: &Path, known: &KnownCategories) -> Result<Self, RulesError> {
        file::parse(text, source, known)
    }

    /// `--rule`: behält nur die genannten Regeln (Reihenfolge der Datei bleibt).
    pub fn select(self, names: &[String]) -> Result<Self, RulesError> {
        let known = |n: &str| self.rules.iter().any(|r| r.name.eq_ignore_ascii_case(n));
        if let Some(unknown) = names.iter().find(|n| !known(n)) {
            return Err(RulesError {
                file: self.source.display().to_string(),
                line: None,
                rule: None,
                message: format!(
                    "unbekannte Regel „{unknown}“ (vorhanden: {})",
                    self.rules
                        .iter()
                        .map(|r| r.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
        let mut rules: Vec<Rule> = self
            .rules
            .into_iter()
            .filter(|r| names.iter().any(|n| r.name.eq_ignore_ascii_case(n)))
            .collect();
        for (i, rule) in rules.iter_mut().enumerate() {
            rule.index = i;
        }
        Ok(Self {
            source: self.source,
            rules,
        })
    }
}

impl Rule {
    /// Gruppen der `name_regex`, wenn alle Bedingungen zutreffen.
    pub fn matches(&self, c: &Candidate, now_ticks: i64) -> Option<Vec<String>> {
        self.conditions.matches(c, now_ticks)
    }

    /// Volle Auswertung mit Treffer, Nicht-Treffer oder nahem Treffer.
    pub fn evaluate(&self, c: &Candidate, ctx: &MatchCtx) -> MatchResult {
        self.conditions.evaluate(
            c,
            &MatchCtx {
                rule_index: self.index,
                ..*ctx
            },
        )
    }

    /// Wie [`evaluate`](Self::evaluate), aber ohne `text_regex` und `min_dwell`.
    pub fn matches_before_text(&self, c: &Candidate, ctx: &MatchCtx) -> MatchResult {
        self.conditions.matches_before_text(
            c,
            &MatchCtx {
                rule_index: self.index,
                ..*ctx
            },
        )
    }

    /// Braucht der Inhalts-Cache: Bedingung oder Ziel nutzt Kategorie bzw. Felder.
    pub fn needs_content(&self) -> bool {
        self.conditions.needs_content() || self.target.uses_content()
    }

    /// Braucht den Volltext (`text_regex`).
    pub fn needs_text(&self) -> bool {
        self.conditions.needs_text()
    }

    /// Das Ziel braucht das Aufnahmedatum.
    pub fn needs_exif(&self) -> bool {
        self.target.uses_exif()
    }

    pub fn target(
        &self,
        c: &Candidate,
        captures: &[String],
        exif: Option<NaiveDateTime>,
    ) -> Result<RelTarget, TargetError> {
        let mtime = local_naive(c.mtime_ticks);
        let exif = if self.needs_exif() {
            match (exif, self.exif_fallback) {
                (Some(date), _) => Some(date),
                (None, ExifFallback::Mtime) => Some(mtime),
                (None, ExifFallback::Skip) => return Err(TargetError::NoExifDate),
            }
        } else {
            None
        };
        let (stem, ext) = match c.name.rfind('.') {
            Some(i) if i > 0 => (&c.name[..i], &c.name[i + 1..]),
            _ => (c.name, ""),
        };
        let ext = if self.normalize.lowercase_ext {
            ext.to_lowercase()
        } else {
            ext.to_string()
        };
        let expanded = self.target.expand(&Values {
            name: stem,
            ext: &ext,
            parent: c.parent,
            mtime,
            exif,
            groups: captures,
            content: c.content,
        })?;
        let raw = expanded.file.unwrap_or_else(|| c.name.to_string());
        let date = exif.unwrap_or(mtime).date();
        Ok(RelTarget {
            dirs: expanded.dirs,
            file: self.normalize.apply(&raw, date),
            same_dir: expanded.same_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone};
    use rstest::rstest;

    fn ticks_of(y: i32, m: u32, d: u32) -> i64 {
        let local = NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        chrono::Local
            .from_local_datetime(&local)
            .unwrap()
            .timestamp()
            * TICKS_PER_SEC
    }

    const NOW: i64 = 1_790_985_600 * TICKS_PER_SEC;

    fn parse(text: &str) -> Result<RuleSet, RulesError> {
        RuleSet::parse(text, Path::new("rules.toml"), &KnownCategories::builtin())
    }

    fn cand<'a>(rel: &'a str, size: u64, mtime_ticks: i64) -> Candidate<'a> {
        Candidate::new(
            rel,
            rel.rsplit('/').next().unwrap(),
            rel.rsplit('/').nth(1).unwrap_or("Wurzel"),
            size,
            mtime_ticks,
        )
    }

    const SPEC_FILE: &str = r#"
[[rules]]
name   = "fotos"
ext    = ["jpg", "jpeg", "heic"]
target = "Fotos/{exif.date:%Y}/{exif.date:%Y-%m-%d}_{name}.{ext}"
exif_fallback = "mtime"
spaces = "_"

[[rules]]
name       = "rechnungen"
glob       = "Downloads/**"
name_regex = '(?i)rechnung[ _-]*(\d+)'
min_age    = "7d"
target     = "Finanzen/Rechnungen/{year}/"
iso_date_prefix   = true
strip_copy_suffix = true
"#;

    #[test]
    fn beispiel_aus_der_spec_wird_geladen() {
        let set = parse(SPEC_FILE).unwrap();
        let names: Vec<_> = set.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["fotos", "rechnungen"]);
        assert_eq!(set.rules[0].line, 2);
        assert_eq!(set.rules[1].line, 9);
        assert!(set.rules[0].needs_exif() && !set.rules[1].needs_exif());
    }

    #[rstest]
    #[case("[[rules]\nname =", Some(1), None, "")]
    #[case(
        "[[rules]]\nname = \"a\"\next = [\"x\"]\ntarget = \"t/\"\nfoo = 1\n",
        Some(5),
        Some("a"),
        "foo"
    )]
    #[case("", None, None, "keine Regeln")]
    #[case(
        "[[rules]]\nname = \"a\"\ntarget = \"t/\"\n",
        Some(1),
        Some("a"),
        "keine Bedingung"
    )]
    #[case(
        "[[rules]]\nname = \"a\"\next = []\ntarget = \"t/\"\n",
        Some(1),
        Some("a"),
        "keine Bedingung"
    )]
    #[case("[[rules]]\next = [\"x\"]\ntarget = \"t/\"\n", Some(1), None, "name")]
    #[case(
        "[[rules]]\nname = \"a\"\next = [\"x\"]\n",
        Some(1),
        Some("a"),
        "target"
    )]
    #[case(
        "[[rules]]\nname = \"\"\next = [\"x\"]\ntarget = \"t/\"\n",
        Some(1),
        Some(""),
        "leer"
    )]
    fn grundlegende_fehler(
        #[case] text: &str,
        #[case] line: Option<usize>,
        #[case] rule: Option<&str>,
        #[case] hint: &str,
    ) {
        let err = parse(text).unwrap_err();
        assert!(err.message.contains(hint), "{err}");
        assert_eq!(err.line, line, "{err}");
        assert_eq!(err.rule.as_deref(), rule, "{err}");
        assert!(err.to_string().starts_with("rules.toml"), "{err}");
    }

    fn one(extra: &str) -> String {
        format!("# Kopf\n\n[[rules]]\nname = \"r\"\n{extra}\n")
    }

    #[rstest]
    #[case("ext = [\"a\"]\ntarget = \"{foo}/\"", "unbekannter Platzhalter")]
    #[case("ext = [\"a\"]\ntarget = \"{1}/\"", "name_regex")]
    #[case("name_regex = '(a)'\ntarget = \"{2}/\"", "1 Gruppe")]
    #[case("ext = [\"a\"]\ntarget = \"{exif.date:%Q}/\"", "Datumsformat")]
    #[case("ext = [\"a\"]\ntarget = \"C:\\\\x\\\\\"", "relativ")]
    #[case("ext = [\"a\"]\ntarget = \"\\\\\\\\srv\\\\share\\\\\"", "relativ")]
    #[case("ext = [\"a\"]\ntarget = \"../x/\"", "„..“")]
    #[case("ext = [\"a\"]\ntarget = \"\"", "leer")]
    #[case("name_regex = '('\ntarget = \"x/\"", "name_regex")]
    #[case("glob = 'a['\ntarget = \"x/\"", "Glob")]
    #[case("min_age = \"7x\"\ntarget = \"x/\"", "min_age")]
    #[case("max_age = \"abc\"\ntarget = \"x/\"", "max_age")]
    #[case("min_size = \"viel\"\ntarget = \"x/\"", "min_size")]
    #[case("max_size = \"0\"\ntarget = \"x/\"", "max_size")]
    #[case(
        "min_age = \"30d\"\nmax_age = \"7d\"\ntarget = \"x/\"",
        "min_age ist größer"
    )]
    #[case(
        "min_size = \"2MB\"\nmax_size = \"1MB\"\ntarget = \"x/\"",
        "min_size ist größer"
    )]
    #[case(
        "ext = [\"a\"]\ntarget = \"x/\"\nexif_fallback = \"nie\"",
        "exif_fallback"
    )]
    #[case("ext = [\"a\"]\ntarget = \"x/\"\nspaces = \"+\"", "spaces")]
    #[case("ext = [\"\"]\ntarget = \"x/\"", "Endung")]
    fn semantische_fehler_haben_zeile_und_regelname(#[case] body: &str, #[case] hint: &str) {
        let err = parse(&one(body)).unwrap_err();
        assert!(err.message.contains(hint), "{err}");
        assert_eq!(err.line, Some(3), "{err}");
        assert_eq!(err.rule.as_deref(), Some("r"), "{err}");
    }

    #[test]
    fn doppelter_name_auch_bei_anderer_schreibweise() {
        let text = "[[rules]]\nname = \"Foto\"\next = [\"a\"]\ntarget = \"x/\"\n\n[[rules]]\nname = \"foto\"\next = [\"b\"]\ntarget = \"y/\"\n";
        let err = parse(text).unwrap_err();
        assert!(err.message.contains("doppelt"), "{err}");
        assert_eq!((err.line, err.rule.as_deref()), (Some(6), Some("foto")));
    }

    #[test]
    fn select_filtert_und_meldet_unbekannte_regeln() {
        let set = parse(SPEC_FILE).unwrap();
        let only = set.select(&["Rechnungen".to_string()]).unwrap();
        assert_eq!(only.rules.len(), 1);
        assert_eq!(only.rules[0].name, "rechnungen");

        let set = parse(SPEC_FILE).unwrap();
        let err = set.select(&["fotoz".to_string()]).unwrap_err();
        assert!(err.message.contains("fotoz") && err.message.contains("fotos, rechnungen"));
    }

    #[test]
    fn select_vergibt_den_index_neu() {
        let set = parse(SPEC_CONTENT_FILE).unwrap();
        let only = set
            .select(&["telekom".to_string(), "vertraege".to_string()])
            .unwrap();
        let names: Vec<_> = only.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["vertraege", "telekom"]);
        // Der Index ist die Position in der gefilterten Liste (Schlüssel für `text_hits`).
        assert_eq!(
            only.rules.iter().map(|r| r.index).collect::<Vec<_>>(),
            [0, 1]
        );
    }

    #[test]
    fn fehlende_datei_ist_ein_fehler_mit_pfad() {
        let err = RuleSet::load(
            Path::new(r"C:\gibt\es\nicht\rules.toml"),
            &KnownCategories::builtin(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("rules.toml"), "{err}");
    }

    #[test]
    fn rechnung_mit_nummer_und_alle_optionen() {
        let text = r#"
[[rules]]
name = "r"
glob = "Downloads/**"
name_regex = '(?i)rechnung'
target = "Finanzen/{year}/"
iso_date_prefix = true
strip_copy_suffix = true
spaces = "_"
umlauts = true
lowercase_ext = true
"#;
        let set = parse(text).unwrap();
        let c = cand(
            "Downloads/Rechnung März - Kopie (2).PDF",
            10,
            ticks_of(2026, 9, 30),
        );
        let caps = set.rules[0].matches(&c, NOW).unwrap();
        let t = set.rules[0].target(&c, &caps, None).unwrap();
        assert_eq!(t.dirs, ["Finanzen", "2026"]);
        assert_eq!(t.file, "2026-09-30_Rechnung_Maerz.pdf");
        assert!(!t.same_dir);
    }

    #[test]
    fn exif_datum_hat_vorrang_vor_mtime_und_fallback_greift() {
        let set = parse(SPEC_FILE).unwrap();
        let rule = &set.rules[0];
        let c = cand("Bilder/Urlaub 1.JPG", 10, ticks_of(2026, 3, 7));
        let exif = NaiveDate::from_ymd_opt(2024, 8, 15)
            .unwrap()
            .and_hms_opt(10, 0, 0);
        let t = rule.target(&c, &[], exif).unwrap();
        assert_eq!(t.dirs, ["Fotos", "2024"]);
        assert_eq!(t.file, "2024-08-15_Urlaub_1.JPG");
        let t = rule.target(&c, &[], None).unwrap();
        assert_eq!(t.dirs, ["Fotos", "2026"]);
        assert_eq!(t.file, "2026-03-07_Urlaub_1.JPG");
    }

    #[test]
    fn exif_fallback_skip() {
        let text = "[[rules]]\nname = \"r\"\next = [\"jpg\"]\ntarget = \"{exif.date}/\"\nexif_fallback = \"skip\"\n";
        let set = parse(text).unwrap();
        let c = cand("a/x.jpg", 1, ticks_of(2026, 3, 7));
        assert_eq!(
            set.rules[0].target(&c, &[], None),
            Err(TargetError::NoExifDate)
        );
    }

    // ---- Inhaltsbedingungen (Phase 6a) ----

    const SPEC_CONTENT_FILE: &str = r#"
[[rules]]
name           = "rechnungen"
glob           = "Downloads/**"
category       = ["rechnung", "mahnung"]
min_confidence = 0.85
min_dwell      = "14d"
target         = "Finanzen/Rechnungen/{doc.date:%Y}/{doc.date:%Y-%m-%d}_{doc.sender|Unbekannt}_{doc.number|{name}}.{ext}"

[[rules]]
name            = "vertraege"
glob            = "Scans/**"
category        = "vertrag"
category_source = "rules"
target          = "Verträge/{doc.sender|Unbekannt}/{doc.date:%Y-%m-%d}_{doc.title|{name}}.{ext}"

[[rules]]
name   = "urlaubsfotos"
glob   = "Eingang/**"
ext    = ["jpg", "jpeg", "heic"]
fields = { "exif.has_gps" = "true" }
fields_regex = { "exif.country" = '^(?:[^D].|D[^E])$' }
target = "Fotos/{exif.date:%Y}/{exif.country}-{exif.city}/{exif.date:%Y-%m-%d}_{name}.{ext}"

[[rules]]
name       = "telekom"
glob       = "Downloads/**"
ext        = ["pdf"]
text_regex = '(?i)telekom deutschland gmbh'
target     = "Finanzen/Telekom/"
"#;

    #[test]
    fn beispiel_mit_inhaltsbedingungen_aus_der_spec_wird_geladen() {
        let set = parse(SPEC_CONTENT_FILE).unwrap();
        let names: Vec<_> = set.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            ["rechnungen", "vertraege", "urlaubsfotos", "telekom"]
        );
        assert_eq!(
            set.rules.iter().map(|r| r.index).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        let [rechnungen, vertraege, fotos, telekom] = &set.rules[..] else {
            panic!()
        };
        assert_eq!(rechnungen.conditions.category, ["rechnung", "mahnung"]);
        assert_eq!(rechnungen.conditions.min_confidence, Some(0.85));
        assert_eq!(rechnungen.conditions.min_dwell_days, Some(14));
        assert!(rechnungen.needs_content() && !rechnungen.needs_text());
        assert_eq!(vertraege.conditions.category_source, CategorySource::Rules);
        assert_eq!(fotos.conditions.fields["exif.has_gps"], "true");
        assert_eq!(fotos.conditions.fields_regex.len(), 1);
        assert!(telekom.needs_text() && !telekom.needs_content());
    }

    #[test]
    fn ziel_mit_feldern_macht_eine_regel_inhaltsabhaengig() {
        let text = "[[rules]]\nname = \"r\"\next = [\"pdf\"]\ntarget = \"{doc.sender|X}/\"\n";
        let set = parse(text).unwrap();
        assert!(set.rules[0].needs_content());
        let plain = parse("[[rules]]\nname = \"r\"\next = [\"pdf\"]\ntarget = \"x/\"\n").unwrap();
        assert!(!plain.rules[0].needs_content());
    }

    #[test]
    fn kategorie_gross_klein_und_liste() {
        let set = parse(
            "[[rules]]\nname = \"r\"\ncategory = [\" Rechnung \", \"MAHNUNG\"]\ntarget = \"x/\"\n",
        )
        .unwrap();
        assert_eq!(set.rules[0].conditions.category, ["rechnung", "mahnung"]);
    }

    #[rstest]
    #[case("category = \"gibtsnicht\"", "unbekannte Kategorie")]
    #[case("category = [\"rechnung\", \"nope\"]", "„nope“")]
    #[case("category = []", "leere Liste")]
    #[case("min_confidence = 0.9\next = [\"pdf\"]", "braucht eine category")]
    #[case("category = \"rechnung\"\nmin_confidence = 1.5", "zwischen 0 und 1")]
    #[case("category = \"rechnung\"\nmin_confidence = -0.1", "zwischen 0 und 1")]
    #[case(
        "category_source = \"rules\"\next = [\"pdf\"]",
        "braucht eine category"
    )]
    #[case(
        "category = \"rechnung\"\ncategory_source = \"llm\"",
        "category_source"
    )]
    #[case("fields = { \"doc.bogus\" = \"x\" }", "unbekanntes Feld")]
    #[case("fields_regex = { \"exif.bogus\" = \"x\" }", "unbekanntes Feld")]
    #[case("fields_regex = { \"doc.sender\" = \"(\" }", "ungültige Regex")]
    #[case("ext = [\"pdf\"]\ntext_regex = \"(\"", "text_regex")]
    #[case("ext = [\"pdf\"]\nmin_dwell = \"bald\"", "min_dwell")]
    #[case("ext = [\"pdf\"]\nmin_dwell = \"0d\"", "min_dwell")]
    #[case("ext = [\"pdf\"]\ncategory = 5", "category")]
    fn fehler_in_inhaltsbedingungen_haben_zeile_und_regelname(
        #[case] body: &str,
        #[case] hint: &str,
    ) {
        let target = if body.contains("target") {
            ""
        } else {
            "\ntarget = \"x/\""
        };
        let err = parse(&one(&format!("{body}{target}"))).unwrap_err();
        assert!(err.message.contains(hint), "{err}");
        assert_eq!(err.rule.as_deref(), Some("r"), "{err}");
        assert!(err.line.is_some(), "{err}");
    }

    #[test]
    fn unbekannte_kategorie_nennt_bekannte_namen() {
        let err = parse(&one("category = \"x\"\ntarget = \"y/\"")).unwrap_err();
        assert!(
            err.message.contains("rechnung") && err.message.contains("vertrag"),
            "{err}"
        );
        assert_eq!(err.line, Some(3));
    }

    #[test]
    fn kategorie_datei_kaputt_betrifft_nur_regeln_mit_category() {
        let known = KnownCategories::Unavailable("categories.toml:3: kaputt".into());
        let with_category = RuleSet::parse(
            &one("category = \"rechnung\"\ntarget = \"y/\""),
            Path::new("rules.toml"),
            &known,
        )
        .unwrap_err();
        assert!(
            with_category.message.contains("categories.toml:3"),
            "{with_category}"
        );
        let without = RuleSet::parse(
            &one("ext = [\"pdf\"]\ntarget = \"y/\""),
            Path::new("rules.toml"),
            &known,
        );
        assert!(
            without.is_ok(),
            "Regeln ohne Inhaltsbedingung brauchen keine gültige Datei"
        );
    }

    #[test]
    fn eigene_kategorien_sind_bekannt() {
        let defs = crate::content::classify::defs::CategoryDefs::load(
            "[[categories]]\nname = \"kita\"\nkeywords = { kita = 3 }\n",
            "c.toml",
        )
        .unwrap();
        let known = KnownCategories::Names(defs.names());
        assert!(RuleSet::parse(
            &one("category = \"kita\"\ntarget = \"y/\""),
            Path::new("rules.toml"),
            &known
        )
        .is_ok());
    }
}
