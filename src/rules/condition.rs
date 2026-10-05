//! Bedingungen einer Regel (alle optional, UND-verknüpft).
//!
//! Auswertungsreihenfolge: erst die billigen Bedingungen (Pfad, Endung, Name, Alter, Größe),
//! dann die Bedingungen aus dem Inhalts-Cache (Kategorie, Felder), zuletzt `text_regex`
//! und `min_dwell`. Scheitert eine Regel *nur* an einer weichen Bedingung (zu geringe
//! Konfidenz, noch nicht klassifiziert, Wartezeit nicht erreicht), meldet [`Conditions::evaluate`]
//! einen [`MatchResult::NearMiss`], damit die Datei im Plan als übersprungen erscheint statt
//! als „ohne Regel“.

use std::collections::BTreeMap;

use globset::{Glob, GlobBuilder, GlobMatcher};
use regex::Regex;

use crate::analysis::age::age_days;
use crate::change::SkipReason;
use crate::content::ContentRecord;

/// Eine Datei aus Sicht der Regeln.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    /// Pfad relativ zur Wurzel, mit `/` als Trenner.
    pub rel: &'a str,
    /// Dateiname mit Endung.
    pub name: &'a str,
    /// Name des Elternordners (bei Dateien direkt in der Wurzel: Name der Wurzel).
    pub parent: &'a str,
    pub size: u64,
    pub mtime_ticks: i64,
    /// Ergebnis der Inhaltsklassifikation (aus dem Cache), falls vorhanden.
    pub content: Option<&'a ContentRecord>,
    /// Ankunft im Ordner: das spätere von Erstellzeit und `first_seen` (Ticks).
    pub arrived_ticks: i64,
    /// Ergebnis der `text_regex` je Regel (Index = Regelnummer); `None` = Text nicht gelesen.
    pub text_hits: Option<&'a [bool]>,
}

impl<'a> Candidate<'a> {
    pub fn new(rel: &'a str, name: &'a str, parent: &'a str, size: u64, mtime_ticks: i64) -> Self {
        Self {
            rel,
            name,
            parent,
            size,
            mtime_ticks,
            content: None,
            arrived_ticks: mtime_ticks,
            text_hits: None,
        }
    }

    pub fn with_content(mut self, content: Option<&'a ContentRecord>) -> Self {
        self.content = content;
        self
    }

    pub fn with_arrival(mut self, arrived_ticks: i64) -> Self {
        self.arrived_ticks = arrived_ticks;
        self
    }

    pub fn with_text_hits(mut self, hits: Option<&'a [bool]>) -> Self {
        self.text_hits = hits;
        self
    }
}

/// Woher eine Kategorie stammen darf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CategorySource {
    /// Regeln und LLM
    #[default]
    Any,
    /// Nur Schlüsselwort-Treffer
    Rules,
}

#[derive(Debug, Default)]
pub struct Conditions {
    pub glob: Option<GlobMatcher>,
    /// Kleingeschrieben, ohne Punkt.
    pub ext: Vec<String>,
    pub name_regex: Option<Regex>,
    pub min_age_days: Option<i64>,
    pub max_age_days: Option<i64>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Kleingeschriebene Kategorienamen; leer = keine Kategorie-Bedingung
    pub category: Vec<String>,
    pub min_confidence: Option<f32>,
    pub category_source: CategorySource,
    /// Feld (z. B. `exif.has_gps`) → erwarteter Wert, klein geschrieben
    pub fields: BTreeMap<String, String>,
    pub fields_regex: Vec<(String, Regex)>,
    pub text_regex: Option<Regex>,
    pub min_dwell_days: Option<i64>,
}

/// Rahmen der Auswertung.
#[derive(Debug, Clone, Copy)]
pub struct MatchCtx {
    pub now_ticks: i64,
    /// `[classify] min_confidence`, wenn die Regel keine eigene Schwelle hat
    pub min_confidence: f32,
    /// Index der Regel (für `text_hits`)
    pub rule_index: usize,
}

impl MatchCtx {
    pub fn new(now_ticks: i64) -> Self {
        Self {
            now_ticks,
            min_confidence: 0.8,
            rule_index: 0,
        }
    }
}

/// Weiche Bedingung, an der eine Regel gescheitert ist (alle anderen waren erfüllt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearMiss {
    pub reason: SkipReason,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchResult {
    /// Gruppen der `name_regex`
    Match(Vec<String>),
    NoMatch,
    NearMiss(NearMiss),
}

impl MatchResult {
    pub fn captures(self) -> Option<Vec<String>> {
        match self {
            Self::Match(c) => Some(c),
            _ => None,
        }
    }
}

/// Glob auf den relativen Pfad: `/` und `\` gleichwertig, Groß-/Kleinschreibung egal,
/// `*` bleibt innerhalb eines Ordners, `**` geht über Ebenen.
pub fn compile_glob(pattern: &str) -> Result<GlobMatcher, String> {
    let pattern = pattern.replace('\\', "/");
    let glob: Glob = GlobBuilder::new(&pattern)
        .case_insensitive(true)
        .literal_separator(true)
        .build()
        .map_err(|e| format!("ungültiger Glob „{pattern}“ ({})", e.kind()))?;
    Ok(glob.compile_matcher())
}

fn format_confidence(c: f32) -> String {
    format!("{c:.2}")
}

impl Conditions {
    pub fn is_empty(&self) -> bool {
        self.glob.is_none()
            && self.ext.is_empty()
            && self.name_regex.is_none()
            && self.min_age_days.is_none()
            && self.max_age_days.is_none()
            && self.min_size.is_none()
            && self.max_size.is_none()
            && self.category.is_empty()
            && self.fields.is_empty()
            && self.fields_regex.is_empty()
            && self.text_regex.is_none()
            && self.min_dwell_days.is_none()
    }

    /// Anzahl der Gruppen der `name_regex` (ohne Gruppe 0); `None` ohne Regex.
    pub fn group_count(&self) -> Option<usize> {
        self.name_regex.as_ref().map(|r| r.captures_len() - 1)
    }

    /// Braucht die Regel den Inhalts-Cache (Kategorie oder Felder)?
    pub fn needs_content(&self) -> bool {
        !self.category.is_empty() || !self.fields.is_empty() || !self.fields_regex.is_empty()
    }

    /// Braucht die Regel den Volltext (`text_regex`)?
    pub fn needs_text(&self) -> bool {
        self.text_regex.is_some()
    }

    /// Nur die billigen Bedingungen: Treffer = Gruppen der `name_regex`.
    pub fn matches_cheap(&self, c: &Candidate, now_ticks: i64) -> Option<Vec<String>> {
        if let Some(glob) = &self.glob {
            if !glob.is_match(c.rel) {
                return None;
            }
        }
        if !self.ext.is_empty() {
            let ext = match c.name.rfind('.') {
                Some(i) if i > 0 => c.name[i + 1..].to_lowercase(),
                _ => return None,
            };
            if !self.ext.contains(&ext) {
                return None;
            }
        }
        let age = age_days(c.mtime_ticks, now_ticks);
        if self.min_age_days.is_some_and(|min| age < min)
            || self.max_age_days.is_some_and(|max| age > max)
            || self.min_size.is_some_and(|min| c.size < min)
            || self.max_size.is_some_and(|max| c.size > max)
        {
            return None;
        }
        let Some(regex) = &self.name_regex else {
            return Some(Vec::new());
        };
        let caps = regex.captures(c.name)?;
        Some(
            (1..caps.len())
                .map(|i| caps.get(i).map_or("", |m| m.as_str()).to_string())
                .collect(),
        )
    }

    /// Wie [`evaluate`](Self::evaluate), aber ohne `text_regex` und ohne `min_dwell`: Grundlage
    /// für die Entscheidung, für welche Dateien der Text gelesen werden muss.
    pub fn matches_before_text(&self, c: &Candidate, ctx: &MatchCtx) -> MatchResult {
        self.run(c, ctx, false)
    }

    /// Volle Auswertung mit allen Bedingungen.
    pub fn evaluate(&self, c: &Candidate, ctx: &MatchCtx) -> MatchResult {
        self.run(c, ctx, true)
    }

    fn run(&self, c: &Candidate, ctx: &MatchCtx, with_text_and_dwell: bool) -> MatchResult {
        let Some(captures) = self.matches_cheap(c, ctx.now_ticks) else {
            return MatchResult::NoMatch;
        };
        let mut near: Option<NearMiss> = None;

        if self.needs_content() {
            let Some(content) = c.content else {
                // Noch nicht klassifiziert: kein hartes Nein, die Datei bleibt „zu prüfen“.
                return MatchResult::NearMiss(NearMiss {
                    reason: SkipReason::NotClassified,
                    detail: None,
                });
            };
            if !self.category.is_empty() {
                let Some(category) = content.category.as_deref() else {
                    return MatchResult::NoMatch;
                };
                if !self
                    .category
                    .iter()
                    .any(|n| n.eq_ignore_ascii_case(category))
                {
                    return MatchResult::NoMatch;
                }
                if self.category_source == CategorySource::Rules
                    && content.source == crate::content::Source::Llm
                {
                    return MatchResult::NoMatch;
                }
                let min = self.min_confidence.unwrap_or(ctx.min_confidence);
                if content.confidence < min {
                    let mut detail =
                        format!("{category} {}", format_confidence(content.confidence));
                    if let Some((second, conf)) = &content.category2 {
                        detail.push_str(&format!(" / {second} {}", format_confidence(*conf)));
                    }
                    near = Some(NearMiss {
                        reason: SkipReason::LowConfidence,
                        detail: Some(detail),
                    });
                }
            }
            for (name, expected) in &self.fields {
                match content.fields.get(name) {
                    Some(v) if v.to_lowercase() == *expected => {}
                    _ => return MatchResult::NoMatch,
                }
            }
            for (name, regex) in &self.fields_regex {
                match content.fields.get(name) {
                    Some(v) if regex.is_match(v) => {}
                    _ => return MatchResult::NoMatch,
                }
            }
        }

        if with_text_and_dwell {
            if self.text_regex.is_some() {
                // Text nicht gelesen oder kein Text: die Regel kann nicht greifen.
                match c.text_hits.and_then(|hits| hits.get(ctx.rule_index)) {
                    Some(true) => {}
                    _ => return MatchResult::NoMatch,
                }
            }
            if let Some(dwell) = self.min_dwell_days {
                let waited = age_days(c.arrived_ticks, ctx.now_ticks);
                if waited < dwell && near.is_none() {
                    near = Some(NearMiss {
                        reason: SkipReason::TooRecentArrival,
                        detail: Some(format!("seit {waited} von {dwell} Tagen im Ordner")),
                    });
                }
            }
        }
        match near {
            Some(n) => MatchResult::NearMiss(n),
            None => MatchResult::Match(captures),
        }
    }

    /// Alle Bedingungen; nur ein voller Treffer zählt (für Regeln ohne Inhaltsbedingungen
    /// identisch zu früher).
    pub fn matches(&self, c: &Candidate, now_ticks: i64) -> Option<Vec<String>> {
        self.evaluate(c, &MatchCtx::new(now_ticks)).captures()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::classify::fields::FieldSources;
    use crate::content::{Fields, Source, Status, TextSource, EXTRACTOR_VERSION};
    use crate::scan::source::TICKS_PER_SEC;

    const NOW: i64 = 1_790_985_600 * TICKS_PER_SEC;
    const DAY: i64 = 86_400 * TICKS_PER_SEC;

    fn cand<'a>(rel: &'a str, size: u64, age_days: i64) -> Candidate<'a> {
        Candidate::new(
            rel,
            rel.rsplit('/').next().unwrap(),
            "x",
            size,
            NOW - age_days * DAY,
        )
    }

    fn glob(p: &str) -> Conditions {
        Conditions {
            glob: Some(compile_glob(p).unwrap()),
            ..Conditions::default()
        }
    }

    fn content(category: Option<&str>, confidence: f32, source: Source) -> ContentRecord {
        ContentRecord {
            path_key: "k".into(),
            size: 1,
            mtime: 1,
            status: Status::Ok,
            category: category.map(String::from),
            confidence,
            category2: Some(("mahnung".into(), 0.41)),
            source,
            hits: vec![],
            fields: Fields::new(),
            field_sources: FieldSources::new(),
            text_source: TextSource::Layer,
            llm_model: None,
            defs_fingerprint: "f".into(),
            extractor_version: EXTRACTOR_VERSION,
            classified_at: "t".into(),
        }
    }

    fn category_rule(names: &[&str]) -> Conditions {
        Conditions {
            category: names.iter().map(|n| n.to_string()).collect(),
            ..Conditions::default()
        }
    }

    #[test]
    fn glob_ist_case_insensitiv_und_akzeptiert_beide_trenner() {
        for pattern in ["Downloads/**", r"downloads\**", "DOWNLOADS/**/*.pdf"] {
            let c = glob(pattern);
            assert!(
                c.matches(&cand("downloads/a/b/x.PDF", 1, 0), NOW).is_some(),
                "{pattern}"
            );
        }
        assert!(glob("Downloads/**")
            .matches(&cand("Dokumente/x.pdf", 1, 0), NOW)
            .is_none());
    }

    #[test]
    fn einzelner_stern_bleibt_in_einer_ebene() {
        let c = glob("*.pdf");
        assert!(c.matches(&cand("x.pdf", 1, 0), NOW).is_some());
        assert!(c.matches(&cand("a/x.pdf", 1, 0), NOW).is_none());
        assert!(glob("**/*.pdf")
            .matches(&cand("a/b/c/x.pdf", 1, 0), NOW)
            .is_some());
    }

    #[test]
    fn ungueltiger_glob() {
        assert!(compile_glob("a[").is_err());
    }

    #[test]
    fn endung_ignoriert_gross_und_kleinschreibung() {
        let c = Conditions {
            ext: vec!["jpg".into(), "heic".into()],
            ..Conditions::default()
        };
        assert!(c.matches(&cand("a/IMG.JPG", 1, 0), NOW).is_some());
        assert!(c.matches(&cand("a/IMG.png", 1, 0), NOW).is_none());
        assert!(c.matches(&cand("a/jpg", 1, 0), NOW).is_none());
        assert!(c.matches(&cand("a/.jpg", 1, 0), NOW).is_none());
    }

    #[test]
    fn alter_und_groesse_haben_inklusive_grenzen() {
        let c = Conditions {
            min_age_days: Some(7),
            max_age_days: Some(30),
            min_size: Some(10),
            max_size: Some(20),
            ..Conditions::default()
        };
        assert!(c.matches(&cand("x", 10, 7), NOW).is_some());
        assert!(c.matches(&cand("x", 20, 30), NOW).is_some());
        assert!(c.matches(&cand("x", 10, 6), NOW).is_none(), "zu jung");
        assert!(c.matches(&cand("x", 10, 31), NOW).is_none(), "zu alt");
        assert!(c.matches(&cand("x", 9, 7), NOW).is_none(), "zu klein");
        assert!(c.matches(&cand("x", 21, 7), NOW).is_none(), "zu groß");
    }

    #[test]
    fn regex_liefert_gruppen_und_leere_fuer_unbeteiligte() {
        let c = Conditions {
            name_regex: Some(Regex::new(r"(?i)rechnung[ _-]*(\d+)(x)?").unwrap()),
            ..Conditions::default()
        };
        assert_eq!(c.group_count(), Some(2));
        let caps = c.matches(&cand("a/Rechnung_4711.pdf", 1, 0), NOW).unwrap();
        assert_eq!(caps, ["4711", ""]);
        assert!(c.matches(&cand("a/Brief.pdf", 1, 0), NOW).is_none());
    }

    #[test]
    fn alle_bedingungen_sind_und_verknuepft() {
        let c = Conditions {
            ext: vec!["pdf".into()],
            min_size: Some(5),
            ..Conditions::default()
        };
        assert!(c.matches(&cand("x.pdf", 5, 0), NOW).is_some());
        assert!(c.matches(&cand("x.pdf", 4, 0), NOW).is_none());
        assert!(c.matches(&cand("x.txt", 5, 0), NOW).is_none());
        assert!(Conditions::default().is_empty());
        assert!(!c.is_empty());
    }

    #[test]
    fn kategorie_mit_konfidenz_und_nahe_treffer() {
        let rule = category_rule(&["rechnung", "mahnung"]);
        let ctx = MatchCtx::new(NOW);
        let sure = content(Some("Rechnung"), 0.9, Source::Rules);
        let c = cand("a.pdf", 1, 0).with_content(Some(&sure));
        assert_eq!(rule.evaluate(&c, &ctx), MatchResult::Match(vec![]));

        let weak = content(Some("rechnung"), 0.62, Source::Rules);
        let c = cand("a.pdf", 1, 0).with_content(Some(&weak));
        assert_eq!(
            rule.evaluate(&c, &ctx),
            MatchResult::NearMiss(NearMiss {
                reason: SkipReason::LowConfidence,
                detail: Some("rechnung 0.62 / mahnung 0.41".into())
            })
        );

        let other = content(Some("vertrag"), 0.99, Source::Rules);
        let c = cand("a.pdf", 1, 0).with_content(Some(&other));
        assert_eq!(rule.evaluate(&c, &ctx), MatchResult::NoMatch);
        let none = content(None, 0.0, Source::Rules);
        let c = cand("a.pdf", 1, 0).with_content(Some(&none));
        assert_eq!(rule.evaluate(&c, &ctx), MatchResult::NoMatch);
    }

    #[test]
    fn eigene_schwelle_und_default_schwelle() {
        let mut rule = category_rule(&["rechnung"]);
        let medium = content(Some("rechnung"), 0.7, Source::Rules);
        let c = cand("a.pdf", 1, 0).with_content(Some(&medium));
        let ctx = MatchCtx::new(NOW);
        assert!(matches!(rule.evaluate(&c, &ctx), MatchResult::NearMiss(_)));
        rule.min_confidence = Some(0.6);
        assert!(matches!(rule.evaluate(&c, &ctx), MatchResult::Match(_)));
        rule.min_confidence = None;
        let lenient = MatchCtx {
            min_confidence: 0.5,
            ..ctx
        };
        assert!(matches!(rule.evaluate(&c, &lenient), MatchResult::Match(_)));
        // genau an der Schwelle zählt
        let exact = content(Some("rechnung"), 0.8, Source::Rules);
        let c = cand("a.pdf", 1, 0).with_content(Some(&exact));
        assert!(matches!(rule.evaluate(&c, &ctx), MatchResult::Match(_)));
    }

    #[test]
    fn nicht_klassifiziert_ist_ein_naher_treffer() {
        let rule = category_rule(&["rechnung"]);
        let c = cand("a.pdf", 1, 0);
        assert_eq!(
            rule.evaluate(&c, &MatchCtx::new(NOW)),
            MatchResult::NearMiss(NearMiss {
                reason: SkipReason::NotClassified,
                detail: None
            })
        );
        // scheitert vorher an einer billigen Bedingung: kein naher Treffer
        let mut strict = category_rule(&["rechnung"]);
        strict.ext = vec!["docx".into()];
        assert_eq!(
            strict.evaluate(&c, &MatchCtx::new(NOW)),
            MatchResult::NoMatch
        );
    }

    #[test]
    fn category_source_rules_lehnt_llm_kategorien_ab() {
        let mut rule = category_rule(&["rechnung"]);
        rule.category_source = CategorySource::Rules;
        let llm = content(Some("rechnung"), 0.85, Source::Llm);
        let c = cand("a.pdf", 1, 0).with_content(Some(&llm));
        assert_eq!(rule.evaluate(&c, &MatchCtx::new(NOW)), MatchResult::NoMatch);
        rule.category_source = CategorySource::Any;
        assert!(matches!(
            rule.evaluate(&c, &MatchCtx::new(NOW)),
            MatchResult::Match(_)
        ));
    }

    #[test]
    fn felder_exakt_ohne_gross_klein_und_regex() {
        let mut record = content(None, 0.0, Source::Rules);
        record.fields.insert("exif.has_gps".into(), "true".into());
        record.fields.insert("exif.country".into(), "PT".into());
        let mut rule = Conditions::default();
        rule.fields.insert("exif.has_gps".into(), "true".into());
        rule.fields_regex.push((
            "exif.country".into(),
            Regex::new("^(?:[^D].|D[^E])$").unwrap(),
        ));
        let ctx = MatchCtx::new(NOW);
        let c = cand("a.jpg", 1, 0).with_content(Some(&record));
        assert!(matches!(rule.evaluate(&c, &ctx), MatchResult::Match(_)));
        record.fields.insert("exif.has_gps".into(), "TRUE".into());
        let c = cand("a.jpg", 1, 0).with_content(Some(&record));
        assert!(
            matches!(rule.evaluate(&c, &ctx), MatchResult::Match(_)),
            "Groß-/Kleinschreibung egal"
        );
        record.fields.insert("exif.country".into(), "DE".into());
        let c = cand("a.jpg", 1, 0).with_content(Some(&record));
        assert_eq!(rule.evaluate(&c, &ctx), MatchResult::NoMatch);
        record.fields.remove("exif.country");
        let c = cand("a.jpg", 1, 0).with_content(Some(&record));
        assert_eq!(
            rule.evaluate(&c, &ctx),
            MatchResult::NoMatch,
            "fehlendes Feld"
        );
    }

    #[test]
    fn text_regex_braucht_das_ergebnis_je_regel() {
        let rule = Conditions {
            text_regex: Some(Regex::new("(?i)telekom").unwrap()),
            ..Conditions::default()
        };
        let ctx = MatchCtx {
            rule_index: 1,
            ..MatchCtx::new(NOW)
        };
        assert_eq!(
            rule.evaluate(&cand("a.pdf", 1, 0), &ctx),
            MatchResult::NoMatch
        );
        let hits = [false, true];
        let c = cand("a.pdf", 1, 0).with_text_hits(Some(&hits));
        assert!(matches!(rule.evaluate(&c, &ctx), MatchResult::Match(_)));
        let hits = [true, false];
        let c = cand("a.pdf", 1, 0).with_text_hits(Some(&hits));
        assert_eq!(rule.evaluate(&c, &ctx), MatchResult::NoMatch);
        // ohne text_regex-Auswertung (Vorauswahl) zählt die Bedingung noch nicht
        assert!(matches!(
            rule.matches_before_text(&cand("a.pdf", 1, 0), &ctx),
            MatchResult::Match(_)
        ));
    }

    #[test]
    fn wartezeit_seit_ankunft() {
        let rule = Conditions {
            min_dwell_days: Some(14),
            ..Conditions::default()
        };
        let ctx = MatchCtx::new(NOW);
        let recent = cand("a.pdf", 1, 400).with_arrival(NOW - 3 * DAY);
        match rule.evaluate(&recent, &ctx) {
            MatchResult::NearMiss(n) => {
                assert_eq!(n.reason, SkipReason::TooRecentArrival);
                assert_eq!(n.detail.as_deref(), Some("seit 3 von 14 Tagen im Ordner"));
            }
            other => panic!("{other:?}"),
        }
        let old = cand("a.pdf", 1, 400).with_arrival(NOW - 14 * DAY);
        assert!(matches!(rule.evaluate(&old, &ctx), MatchResult::Match(_)));
        // ohne Angabe zählt der mtime als Ankunft
        assert!(matches!(
            rule.evaluate(&cand("a.pdf", 1, 20), &ctx),
            MatchResult::Match(_)
        ));
    }

    #[test]
    fn zu_wenig_konfidenz_geht_vor_wartezeit() {
        let mut rule = category_rule(&["rechnung"]);
        rule.min_dwell_days = Some(14);
        let weak = content(Some("rechnung"), 0.5, Source::Rules);
        let c = cand("a.pdf", 1, 0)
            .with_content(Some(&weak))
            .with_arrival(NOW - DAY);
        match rule.evaluate(&c, &MatchCtx::new(NOW)) {
            MatchResult::NearMiss(n) => assert_eq!(n.reason, SkipReason::LowConfidence),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bedingungen_melden_ihren_bedarf() {
        let mut c = Conditions::default();
        assert!(!c.needs_content() && !c.needs_text());
        c.category = vec!["x".into()];
        assert!(c.needs_content() && !c.is_empty());
        let t = Conditions {
            text_regex: Some(Regex::new("a").unwrap()),
            ..Conditions::default()
        };
        assert!(t.needs_text() && !t.needs_content() && !t.is_empty());
        let d = Conditions {
            min_dwell_days: Some(1),
            ..Conditions::default()
        };
        assert!(!d.is_empty());
    }
}
