//! Regeldatei (TOML): Rohstrukturen mit Zeilenangaben und Umwandlung in geprüfte Regeln.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use toml::Spanned;

use super::condition::{compile_glob, Conditions};
use super::normalize::Normalize;
use super::template::Template;
use super::{ExifFallback, Rule, RuleSet, RulesError};
use crate::analysis::age::parse_old_after;
use crate::config::parse_bytes;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    rules: Vec<Spanned<RawRule>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    name: String,
    glob: Option<String>,
    ext: Option<Vec<String>>,
    name_regex: Option<String>,
    min_age: Option<String>,
    max_age: Option<String>,
    min_size: Option<String>,
    max_size: Option<String>,
    target: String,
    exif_fallback: Option<String>,
    iso_date_prefix: Option<bool>,
    spaces: Option<String>,
    umlauts: Option<bool>,
    strip_copy_suffix: Option<bool>,
    lowercase_ext: Option<bool>,
}

static NAME_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*name\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap()
});

/// 1-basierte Zeile eines Byte-Offsets.
fn line_of(text: &str, offset: usize) -> usize {
    let mut end = offset.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].matches('\n').count() + 1
}

/// Name der Regel, in deren `[[rules]]`-Block der Offset liegt (Best-Effort für Fehler,
/// bei denen TOML keine fertige Regel geliefert hat).
fn rule_name_near(text: &str, offset: usize) -> Option<String> {
    let mut end = offset.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let start = text[..(end + 9).min(text.len())].rfind("[[rules]]")?;
    let block_end = text[start + 9..]
        .find("[[rules]]")
        .map_or(text.len(), |i| start + 9 + i);
    let caps = NAME_KEY.captures(&text[start..block_end])?;
    caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str().to_string())
}

pub(super) fn parse(text: &str, source: &Path) -> Result<RuleSet, RulesError> {
    let file = source.display().to_string();
    let raw: RawFile = toml::from_str(text).map_err(|e| {
        let offset = e.span().map(|s| s.start);
        RulesError {
            file: file.clone(),
            line: offset.map(|o| line_of(text, o)),
            rule: offset.and_then(|o| rule_name_near(text, o)),
            message: e.message().to_string(),
        }
    })?;
    if raw.rules.is_empty() {
        return Err(RulesError {
            file,
            line: None,
            rule: None,
            message: "enthält keine Regeln (erwartet: mindestens ein [[rules]])".into(),
        });
    }
    let mut names = HashSet::new();
    let mut rules = Vec::with_capacity(raw.rules.len());
    for spanned in raw.rules {
        let line = line_of(text, spanned.span().start);
        let raw_rule = spanned.into_inner();
        let fail = |message: String| RulesError {
            file: file.clone(),
            line: Some(line),
            rule: Some(raw_rule.name.clone()),
            message,
        };
        if raw_rule.name.trim().is_empty() {
            return Err(fail("name darf nicht leer sein".into()));
        }
        if !names.insert(raw_rule.name.to_lowercase()) {
            return Err(fail(
                "Name kommt doppelt vor (Groß-/Kleinschreibung zählt nicht)".into(),
            ));
        }
        rules.push(convert(&raw_rule, line).map_err(fail)?);
    }
    Ok(RuleSet {
        source: source.to_path_buf(),
        rules,
    })
}

fn age(key: &str, value: &Option<String>) -> Result<Option<i64>, String> {
    value
        .as_deref()
        .map(|v| parse_old_after(v).map_err(|e| format!("{key}: {e}")))
        .transpose()
}

fn size(key: &str, value: &Option<String>) -> Result<Option<u64>, String> {
    value
        .as_deref()
        .map(|v| {
            parse_bytes(v)
                .ok_or_else(|| format!("{key}: ungültige Größe „{v}“ (erwartet z. B. 10MB, 1GB)"))
        })
        .transpose()
}

fn convert(raw: &RawRule, line: usize) -> Result<Rule, String> {
    let name_regex = raw
        .name_regex
        .as_deref()
        .map(|r| Regex::new(r).map_err(|e| format!("name_regex: ungültige Regex ({e})")))
        .transpose()?;
    let mut ext = Vec::new();
    for e in raw.ext.iter().flatten() {
        let e = e.trim().trim_start_matches('.').to_lowercase();
        if e.is_empty() {
            return Err("ext: leere Endung".into());
        }
        ext.push(e);
    }
    let conditions = Conditions {
        glob: raw.glob.as_deref().map(compile_glob).transpose()?,
        ext,
        name_regex,
        min_age_days: age("min_age", &raw.min_age)?,
        max_age_days: age("max_age", &raw.max_age)?,
        min_size: size("min_size", &raw.min_size)?,
        max_size: size("max_size", &raw.max_size)?,
    };
    if conditions.is_empty() {
        return Err(
            "keine Bedingung (mindestens eine von glob, ext, name_regex, min_age, max_age, min_size, max_size)"
                .into(),
        );
    }
    if let (Some(min), Some(max)) = (conditions.min_age_days, conditions.max_age_days) {
        if min > max {
            return Err("min_age ist größer als max_age".into());
        }
    }
    if let (Some(min), Some(max)) = (conditions.min_size, conditions.max_size) {
        if min > max {
            return Err("min_size ist größer als max_size".into());
        }
    }
    let target = Template::parse(&raw.target, conditions.group_count())?;
    let exif_fallback = match raw.exif_fallback.as_deref() {
        None | Some("mtime") => ExifFallback::Mtime,
        Some("skip") => ExifFallback::Skip,
        Some(other) => {
            return Err(format!(
                "exif_fallback: „{other}“ ist ungültig (erlaubt: mtime, skip)"
            ))
        }
    };
    let spaces = match raw.spaces.as_deref() {
        None => None,
        Some("_") => Some('_'),
        Some("-") => Some('-'),
        Some(other) => {
            return Err(format!("spaces: „{other}“ ist ungültig (erlaubt: \"_\", \"-\")"))
        }
    };
    Ok(Rule {
        name: raw.name.clone(),
        line,
        conditions,
        target,
        exif_fallback,
        normalize: Normalize {
            iso_date_prefix: raw.iso_date_prefix.unwrap_or(false),
            spaces,
            umlauts: raw.umlauts.unwrap_or(false),
            strip_copy_suffix: raw.strip_copy_suffix.unwrap_or(false),
            lowercase_ext: raw.lowercase_ext.unwrap_or(false),
        },
    })
}
