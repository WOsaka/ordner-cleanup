//! Bedingungen einer Regel (alle optional, UND-verknüpft).

use globset::{Glob, GlobBuilder, GlobMatcher};
use regex::Regex;

use crate::analysis::age::age_days;

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

impl Conditions {
    pub fn is_empty(&self) -> bool {
        self.glob.is_none()
            && self.ext.is_empty()
            && self.name_regex.is_none()
            && self.min_age_days.is_none()
            && self.max_age_days.is_none()
            && self.min_size.is_none()
            && self.max_size.is_none()
    }

    /// Anzahl der Gruppen der `name_regex` (ohne Gruppe 0); `None` ohne Regex.
    pub fn group_count(&self) -> Option<usize> {
        self.name_regex.as_ref().map(|r| r.captures_len() - 1)
    }

    /// Treffer: die Gruppen der Regex (`{1}` = Index 0; nicht beteiligte Gruppen sind leer).
    pub fn matches(&self, c: &Candidate, now_ticks: i64) -> Option<Vec<String>> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::source::TICKS_PER_SEC;

    const NOW: i64 = 1_790_985_600 * TICKS_PER_SEC;
    const DAY: i64 = 86_400 * TICKS_PER_SEC;

    fn cand<'a>(rel: &'a str, size: u64, age_days: i64) -> Candidate<'a> {
        Candidate {
            rel,
            name: rel.rsplit('/').next().unwrap(),
            parent: "x",
            size,
            mtime_ticks: NOW - age_days * DAY,
        }
    }

    fn glob(p: &str) -> Conditions {
        Conditions {
            glob: Some(compile_glob(p).unwrap()),
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
}
