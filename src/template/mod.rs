//! Vorlagen: eine Zielstruktur für eine Wurzel, als TOML-Datei. Eingebaut sind `para` und
//! `johnny-decimal`; eigene Vorlagen haben dasselbe Format.
//!
//! ```toml
//! name = "PARA"
//! max_depth = 8              # optional: tiefere Ordner sind Abweichungen
//! [[levels]]                 # Ebene 1 = direkt unter der Wurzel
//! required = ["1 Projekte"]  # muss in jedem Ordner der Ebene davor existieren
//! allowed = ["Sonstiges"]    # zusätzlich erlaubte Namen
//! patterns = ['^\d\d ']      # erlaubte Namensmuster (Regex, Groß-/Kleinschreibung egal)
//! allow_other = false        # weitere Ordner sind Abweichungen (Default: nur wenn die Ebene
//!                            # weder required noch allowed noch patterns nennt)
//! files = false              # Dateien auf dieser Ebene sind Abweichungen (Default: erlaubt)
//! ```
//!
//! Ebenen ohne Eintrag sind frei. `_Archiv` und `.ordner-cleanup` sind überall erlaubt und werden
//! nie geprüft.

pub mod check;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::Deserialize;
use toml::Spanned;
use xxhash_rust::xxh3::xxh3_64;

const BUILTIN: [(&str, &str); 2] = [
    ("para", include_str!("para.toml")),
    ("johnny-decimal", include_str!("johnny-decimal.toml")),
];

/// Namen der eingebauten Vorlagen.
pub fn builtin_names() -> Vec<&'static str> {
    BUILTIN.iter().map(|(n, _)| *n).collect()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTemplate {
    name: Option<String>,
    max_depth: Option<usize>,
    #[serde(default)]
    levels: Vec<RawLevel>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawLevel {
    required: Vec<Spanned<String>>,
    allowed: Vec<Spanned<String>>,
    patterns: Vec<Spanned<String>>,
    allow_other: Option<bool>,
    files: bool,
}

impl Default for RawLevel {
    fn default() -> Self {
        Self {
            required: Vec::new(),
            allowed: Vec::new(),
            patterns: Vec::new(),
            allow_other: None,
            files: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Level {
    pub required: Vec<String>,
    pub allowed: Vec<String>,
    pub patterns: Vec<Regex>,
    pub allow_other: bool,
    pub files: bool,
}

#[derive(Debug, Clone)]
pub struct Template {
    pub name: String,
    pub max_depth: Option<usize>,
    pub levels: Vec<Level>,
}

/// Eine geladene Vorlage mit der Kennung, die in den Fingerabdruck und den Verlauf eingeht.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub template: Template,
    /// Wie die Vorlage angegeben wurde (Name oder Pfad)
    pub spec: String,
    /// `para` bzw. Pfad der Datei, mit Inhalts-Hash der Vorlage (`para:1a2b…`)
    pub id: String,
}

/// 1-basierte Zeile eines Byte-Offsets.
fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

fn names(items: &[Spanned<String>], what: &str, text: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let name = item.get_ref().trim().to_string();
        let line = line_of(text, item.span().start);
        if name.is_empty() {
            bail!("Zeile {line}: {what} darf keinen leeren Namen enthalten");
        }
        if name.contains(['\\', '/']) {
            bail!("Zeile {line}: {what} '{name}' darf kein Pfadtrenner enthalten");
        }
        if out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            bail!("Zeile {line}: {what} '{name}' kommt doppelt vor");
        }
        out.push(name);
    }
    Ok(out)
}

impl Template {
    /// Parst eine Vorlage. Fehler nennen die Zeile (`Zeile N: …`).
    pub fn parse(text: &str) -> Result<Template> {
        let raw: RawTemplate = toml::from_str(text).map_err(|e| {
            let line = e.span().map_or(0, |s| line_of(text, s.start));
            anyhow::anyhow!(
                "{}{}",
                if line > 0 {
                    format!("Zeile {line}: ")
                } else {
                    String::new()
                },
                e.message()
            )
        })?;
        let mut levels = Vec::new();
        for raw in &raw.levels {
            let required = names(&raw.required, "required", text)?;
            let allowed = names(&raw.allowed, "allowed", text)?;
            let mut patterns = Vec::new();
            for p in &raw.patterns {
                let line = line_of(text, p.span().start);
                // Groß-/Kleinschreibung ist unter Windows egal; `regex` läuft in linearer Zeit.
                let regex = Regex::new(&format!("(?i){}", p.get_ref())).map_err(|e| {
                    anyhow::anyhow!("Zeile {line}: ungültige Regex '{}': {e}", p.get_ref())
                })?;
                patterns.push(regex);
            }
            let constrained = !(required.is_empty() && allowed.is_empty() && patterns.is_empty());
            levels.push(Level {
                allow_other: raw.allow_other.unwrap_or(!constrained),
                required,
                allowed,
                patterns,
                files: raw.files,
            });
        }
        if let Some(depth) = raw.max_depth {
            if depth == 0 || depth < levels.len() {
                bail!(
                    "max_depth {depth} ist kleiner als die Zahl der Ebenen ({}) bzw. 0",
                    levels.len()
                );
            }
        }
        if levels.is_empty() && raw.max_depth.is_none() {
            bail!("Die Vorlage beschreibt nichts: mindestens eine [[levels]]-Ebene oder max_depth angeben");
        }
        Ok(Template {
            name: raw.name.unwrap_or_else(|| "Vorlage".to_string()),
            max_depth: raw.max_depth,
            levels,
        })
    }

    /// Eingebaute Vorlage nach Name (Groß-/Kleinschreibung egal).
    pub fn builtin(name: &str) -> Option<Template> {
        BUILTIN
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, text)| Template::parse(text).expect("eingebaute Vorlagen sind gültig"))
    }
}

/// Löst „Name oder Pfad“ auf. Eine eingebaute Vorlage geht vor; sonst ist `spec` ein Pfad, der
/// relativ zu `base` gilt (Config-Ordner für Profile, sonst der aktuelle Ordner).
pub fn resolve(spec: &str, base: Option<&Path>) -> Result<Loaded> {
    if let Some(template) = Template::builtin(spec) {
        let text = BUILTIN
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(spec))
            .map_or("", |(_, t)| *t);
        return Ok(Loaded {
            template,
            spec: spec.to_lowercase(),
            id: format!("{}:{:016x}", spec.to_lowercase(), xxh3_64(text.as_bytes())),
        });
    }
    let path = Path::new(spec);
    let path: PathBuf = match base {
        Some(base) if path.is_relative() => base.join(path),
        _ => path.to_path_buf(),
    };
    let text = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Vorlage '{spec}' ist weder eingebaut ({}) noch als Datei vorhanden ({})",
                builtin_names().join(", "),
                crate::paths::display(&path)
            )
        } else {
            anyhow::anyhow!("Vorlage {} nicht lesbar: {e}", crate::paths::display(&path))
        }
    })?;
    let template = Template::parse(&text)
        .with_context(|| format!("Vorlage {} ist ungültig", crate::paths::display(&path)))?;
    Ok(Loaded {
        template,
        spec: crate::paths::display(&path),
        id: format!(
            "{}:{:016x}",
            crate::paths::display(&path),
            xxh3_64(text.as_bytes())
        ),
    })
}

#[cfg(test)]
mod tests;
