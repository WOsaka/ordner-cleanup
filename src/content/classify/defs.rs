//! Kategorie-Definitionen: eingebauter Startsatz (`assets/categories.toml`) plus optionale
//! eigene Datei `categories.toml`. Gleicher Name ersetzt die eingebaute Kategorie vollständig.
//! Fehler nennen Datei, Zeile und Kategorie.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use regex::Regex;
use serde::Deserialize;
use toml::Spanned;
use xxhash_rust::xxh3::xxh3_128;

use crate::content::CLASSIFIER_VERSION;

const BUILTIN: &str = include_str!("../../../assets/categories.toml");

/// Endungen, für die eine Kategorie ohne eigene `ext`-Liste gilt (Dokumente und Bilder).
pub const DEFAULT_EXTS: &[&str] = &[
    "pdf", "docx", "xlsx", "pptx", "jpg", "jpeg", "png", "heic", "heif", "tif", "tiff", "webp",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefsError {
    pub file: String,
    pub line: Option<usize>,
    pub category: Option<String>,
    pub message: String,
}

impl fmt::Display for DefsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.file)?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        f.write_str(": ")?;
        if let Some(c) = &self.category {
            write!(f, "Kategorie „{c}“: ")?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for DefsError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Meta,
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub source: String,
    pub regex: Regex,
    pub weight: u32,
}

#[derive(Debug, Clone)]
pub struct CategoryDef {
    pub name: String,
    pub kind: Kind,
    pub description: String,
    /// Schlüsselwort (klein geschrieben) → Gewicht
    pub keywords: Vec<(String, u32)>,
    pub patterns: Vec<Pattern>,
    pub exclude_keywords: Vec<String>,
    /// Leer = alle Endungen aus [`DEFAULT_EXTS`].
    pub ext: Vec<String>,
}

impl CategoryDef {
    pub fn applies_to(&self, ext: &str) -> bool {
        if self.ext.is_empty() {
            DEFAULT_EXTS.contains(&ext)
        } else {
            self.ext.iter().any(|e| e == ext)
        }
    }
}

/// Bekannter Absender für `{doc.sender}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sender {
    pub name: String,
    /// Kleingeschriebene Suchtexte
    pub matches: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CategoryDefs {
    pub categories: Vec<CategoryDef>,
    pub senders: Vec<Sender>,
    fingerprint: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    disable_builtin: Vec<String>,
    #[serde(default)]
    categories: Vec<Spanned<RawCategory>>,
    #[serde(default)]
    senders: Vec<Spanned<RawSender>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCategory {
    name: String,
    kind: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    keywords: BTreeMap<String, i64>,
    #[serde(default)]
    patterns: BTreeMap<String, i64>,
    #[serde(default)]
    exclude_keywords: Vec<String>,
    #[serde(default)]
    ext: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSender {
    name: String,
    #[serde(rename = "match")]
    matches: Vec<String>,
}

fn line_of(text: &str, offset: usize) -> usize {
    let mut end = offset.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].matches('\n').count() + 1
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

struct Parsed {
    categories: Vec<CategoryDef>,
    senders: Vec<Sender>,
    disable_builtin: Vec<String>,
}

fn parse_file(text: &str, file: &str, allow_meta: bool) -> Result<Parsed, DefsError> {
    let raw: RawFile = toml::from_str(text).map_err(|e| DefsError {
        file: file.into(),
        line: e.span().map(|s| line_of(text, s.start)),
        category: None,
        message: e.message().to_string(),
    })?;
    let mut seen = BTreeSet::new();
    let mut categories = Vec::new();
    for spanned in raw.categories {
        let line = line_of(text, spanned.span().start);
        let c = spanned.into_inner();
        let fail = |message: String| DefsError {
            file: file.into(),
            line: Some(line),
            category: Some(c.name.clone()),
            message,
        };
        if !valid_name(&c.name) {
            return Err(fail(
                "name muss aus [a-z0-9_-] bestehen (Kleinbuchstaben, keine Leerzeichen)".into(),
            ));
        }
        if !seen.insert(c.name.clone()) {
            return Err(fail("Name kommt doppelt vor".into()));
        }
        let kind = match c.kind.as_deref() {
            None | Some("text") => Kind::Text,
            Some("meta") if allow_meta => Kind::Meta,
            Some("meta") => {
                return Err(fail(
                    "kind = \"meta\" ist nur für eingebaute Kategorien erlaubt".into(),
                ))
            }
            Some(other) => {
                return Err(fail(format!(
                    "kind: „{other}“ ist ungültig (erlaubt: text, meta)"
                )))
            }
        };
        let weight = |what: &str, w: i64| -> Result<u32, DefsError> {
            u32::try_from(w)
                .ok()
                .filter(|w| (1..=10).contains(w))
                .ok_or_else(|| fail(format!("Gewicht {w} für {what} liegt nicht in 1 bis 10")))
        };
        let mut keywords = Vec::new();
        for (k, w) in &c.keywords {
            let key = k.trim().to_lowercase();
            if key.is_empty() {
                return Err(fail("keywords: leeres Schlüsselwort".into()));
            }
            keywords.push((key, weight(&format!("„{k}“"), *w)?));
        }
        let mut patterns = Vec::new();
        for (p, w) in &c.patterns {
            let regex = Regex::new(p)
                .map_err(|e| fail(format!("patterns: ungültige Regex „{p}“ ({e})")))?;
            patterns.push(Pattern {
                source: p.clone(),
                regex,
                weight: weight(&format!("Muster „{p}“"), *w)?,
            });
        }
        let mut exts = Vec::new();
        for e in &c.ext {
            let e = e.trim().trim_start_matches('.').to_lowercase();
            if e.is_empty() {
                return Err(fail("ext: leere Endung".into()));
            }
            exts.push(e);
        }
        if kind == Kind::Text && keywords.is_empty() && patterns.is_empty() {
            return Err(fail(
                "weder keywords noch patterns (die Kategorie könnte nie passen)".into(),
            ));
        }
        categories.push(CategoryDef {
            name: c.name.clone(),
            kind,
            description: c.description.clone(),
            keywords,
            patterns,
            exclude_keywords: c
                .exclude_keywords
                .iter()
                .map(|k| k.trim().to_lowercase())
                .filter(|k| !k.is_empty())
                .collect(),
            ext: exts,
        });
    }
    let mut senders = Vec::new();
    for spanned in raw.senders {
        let line = line_of(text, spanned.span().start);
        let s = spanned.into_inner();
        if s.name.trim().is_empty() || s.matches.iter().all(|m| m.trim().is_empty()) {
            return Err(DefsError {
                file: file.into(),
                line: Some(line),
                category: None,
                message: format!(
                    "Absender „{}“: name und match dürfen nicht leer sein",
                    s.name
                ),
            });
        }
        senders.push(Sender {
            name: s.name.trim().to_string(),
            matches: s
                .matches
                .iter()
                .map(|m| m.trim().to_lowercase())
                .filter(|m| !m.is_empty())
                .collect(),
        });
    }
    Ok(Parsed {
        categories,
        senders,
        disable_builtin: raw.disable_builtin,
    })
}

impl CategoryDefs {
    /// Nur der eingebaute Startsatz.
    pub fn builtin() -> Self {
        let parsed = parse_file(BUILTIN, "assets/categories.toml", true)
            .expect("eingebaute categories.toml muss gültig sein");
        Self::assemble(parsed.categories, parsed.senders)
    }

    /// Eingebauter Startsatz plus eigene Datei (`text`, `source` für Fehlermeldungen).
    pub fn load(text: &str, source: &str) -> Result<Self, DefsError> {
        let user = parse_file(text, source, false)?;
        let builtin = parse_file(BUILTIN, "assets/categories.toml", true)
            .expect("eingebaute categories.toml muss gültig sein");
        let builtin_names: BTreeSet<&str> =
            builtin.categories.iter().map(|c| c.name.as_str()).collect();
        for name in &user.disable_builtin {
            if !builtin_names.contains(name.as_str()) {
                return Err(DefsError {
                    file: source.into(),
                    line: None,
                    category: None,
                    message: format!(
                        "disable_builtin: „{name}“ ist keine eingebaute Kategorie (bekannt: {})",
                        builtin_names.iter().copied().collect::<Vec<_>>().join(", ")
                    ),
                });
            }
        }
        let replaced: BTreeSet<&str> = user.categories.iter().map(|c| c.name.as_str()).collect();
        let mut categories: Vec<CategoryDef> = builtin
            .categories
            .into_iter()
            .filter(|c| {
                !user.disable_builtin.contains(&c.name) && !replaced.contains(c.name.as_str())
            })
            .collect();
        categories.extend(user.categories);
        let mut senders = builtin.senders;
        senders.extend(user.senders);
        Ok(Self::assemble(categories, senders))
    }

    fn assemble(mut categories: Vec<CategoryDef>, senders: Vec<Sender>) -> Self {
        categories.sort_by(|a, b| a.name.cmp(&b.name));
        let fingerprint = fingerprint_of(&categories, &senders);
        Self {
            categories,
            senders,
            fingerprint,
        }
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn names(&self) -> BTreeSet<String> {
        self.categories.iter().map(|c| c.name.clone()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&CategoryDef> {
        self.categories.iter().find(|c| c.name == name)
    }
}

/// xxh3 über eine kanonische Darstellung (sortiert, ohne Beschreibung), samt Klassifikator-Version.
fn fingerprint_of(categories: &[CategoryDef], senders: &[Sender]) -> String {
    let mut canon = serde_json::json!({ "v": CLASSIFIER_VERSION });
    let cats: Vec<_> = categories
        .iter()
        .map(|c| {
            let mut patterns: Vec<_> = c.patterns.iter().map(|p| (&p.source, p.weight)).collect();
            patterns.sort();
            let mut excl = c.exclude_keywords.clone();
            excl.sort();
            let mut ext = c.ext.clone();
            ext.sort();
            serde_json::json!({
                "n": c.name,
                "k": matches!(c.kind, Kind::Meta),
                "kw": c.keywords,
                "p": patterns,
                "x": excl,
                "e": ext,
            })
        })
        .collect();
    let mut sender_json: Vec<_> = senders
        .iter()
        .map(|s| serde_json::json!({ "n": s.name, "m": s.matches }))
        .collect();
    sender_json.sort_by_key(|v| v.to_string());
    canon["c"] = cats.into();
    canon["s"] = sender_json.into();
    format!("{:032x}", xxh3_128(canon.to_string().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn eingebaute_datei_ist_gueltig_und_hat_14_kategorien() {
        let defs = CategoryDefs::builtin();
        assert_eq!(defs.categories.len(), 14);
        for name in [
            "rechnung",
            "mahnung",
            "angebot",
            "auftragsbestaetigung",
            "quittung",
            "vertrag",
            "kontoauszug",
            "gehaltsabrechnung",
            "steuer",
            "versicherung",
            "medizin",
            "bescheinigung",
            "foto",
            "screenshot",
        ] {
            assert!(defs.get(name).is_some(), "{name} fehlt");
        }
        assert_eq!(defs.get("foto").unwrap().kind, Kind::Meta);
    }

    #[test]
    fn fingerabdruck_ist_stabil_und_aendert_sich_mit_dem_inhalt() {
        let a = CategoryDefs::builtin();
        assert_eq!(a.fingerprint(), CategoryDefs::builtin().fingerprint());
        let leer = CategoryDefs::load("", "c.toml").unwrap();
        assert_eq!(
            leer.fingerprint(),
            a.fingerprint(),
            "leere Datei ändert nichts"
        );
        let b = CategoryDefs::load(
            "[[categories]]\nname = \"kita\"\nkeywords = { kita = 3 }\n",
            "c.toml",
        )
        .unwrap();
        assert_ne!(a.fingerprint(), b.fingerprint());
        let c = CategoryDefs::load(
            "[[categories]]\nname = \"kita\"\nkeywords = { kita = 4 }\n",
            "c.toml",
        )
        .unwrap();
        assert_ne!(b.fingerprint(), c.fingerprint());
    }

    #[test]
    fn eigene_kategorie_ersetzt_eingebaute_vollstaendig() {
        let defs = CategoryDefs::load(
            "[[categories]]\nname = \"rechnung\"\nkeywords = { faktura = 5 }\n",
            "c.toml",
        )
        .unwrap();
        let r = defs.get("rechnung").unwrap();
        assert_eq!(r.keywords, vec![("faktura".to_string(), 5)]);
        assert!(r.patterns.is_empty());
        assert_eq!(defs.categories.len(), 14);
    }

    #[test]
    fn disable_builtin_und_senders() {
        let defs = CategoryDefs::load(
            "disable_builtin = [\"medizin\"]\n[[senders]]\nname = \"Telekom\"\nmatch = [\"Telekom Deutschland GmbH\", \"telekom.de\"]\n",
            "c.toml",
        )
        .unwrap();
        assert!(defs.get("medizin").is_none());
        assert_eq!(defs.senders[0].name, "Telekom");
        assert_eq!(defs.senders[0].matches[0], "telekom deutschland gmbh");
    }

    #[rstest]
    #[case(
        "[[categories]]\nname = \"a\"\nbogus = 1\n",
        Some(3),
        None,
        "unknown field"
    )]
    #[case(
        "[[categories]]\nname = \"Gross\"\nkeywords = { x = 1 }\n",
        Some(1),
        Some("Gross"),
        "[a-z0-9_-]"
    )]
    #[case(
        "[[categories]]\nname = \"a\"\nkeywords = { x = 11 }\n",
        Some(1),
        Some("a"),
        "1 bis 10"
    )]
    #[case(
        "[[categories]]\nname = \"a\"\nkeywords = { x = 0 }\n",
        Some(1),
        Some("a"),
        "1 bis 10"
    )]
    #[case(
        "\n\n[[categories]]\nname = \"a\"\npatterns = { '(' = 3 }\n",
        Some(3),
        Some("a"),
        "ungültige Regex"
    )]
    #[case("[[categories]]\nname = \"a\"\nkeywords = { x = 1 }\n[[categories]]\nname = \"a\"\nkeywords = { y = 1 }\n", Some(4), Some("a"), "doppelt")]
    #[case(
        "[[categories]]\nname = \"a\"\n",
        Some(1),
        Some("a"),
        "weder keywords noch patterns"
    )]
    #[case(
        "[[categories]]\nname = \"a\"\nkind = \"meta\"\n",
        Some(1),
        Some("a"),
        "nur für eingebaute"
    )]
    #[case(
        "disable_builtin = [\"gibtsnicht\"]\n",
        None,
        None,
        "keine eingebaute Kategorie"
    )]
    #[case(
        "[[categories]]\nname = \"a\"\nkeywords = { x = 1 }\next = [\"\"]\n",
        Some(1),
        Some("a"),
        "leere Endung"
    )]
    #[case("not toml [", Some(1), None, "")]
    fn fehler_nennen_datei_zeile_und_kategorie(
        #[case] text: &str,
        #[case] line: Option<usize>,
        #[case] category: Option<&str>,
        #[case] contains: &str,
    ) {
        let err = CategoryDefs::load(text, "categories.toml").unwrap_err();
        assert_eq!(err.file, "categories.toml");
        assert_eq!(err.line, line, "{err}");
        assert_eq!(err.category.as_deref(), category, "{err}");
        assert!(err.message.contains(contains), "{err}");
        assert!(err.to_string().starts_with("categories.toml"));
    }

    #[test]
    fn gueltigkeit_nach_endung() {
        let defs = CategoryDefs::builtin();
        assert!(defs.get("rechnung").unwrap().applies_to("pdf"));
        assert!(!defs.get("rechnung").unwrap().applies_to("mp4"));
        assert!(defs.get("screenshot").unwrap().applies_to("png"));
        assert!(!defs.get("screenshot").unwrap().applies_to("pdf"));
    }
}
