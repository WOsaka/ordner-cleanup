//! Prüfungen vor dem Speichern: immer der vorhandene Parser, damit die GUI nichts schreibt, was
//! die CLI später ablehnt.

use std::path::Path;

use super::{line_in_message, ValidationError};
use crate::config::Config;
use crate::content::classify::defs::CategoryDefs;
use crate::rules::{KnownCategories, RuleSet};
use crate::template::Template;

/// `config.toml` (inklusive Profile).
pub fn config_text(text: &str) -> Result<(), ValidationError> {
    Config::parse(text).map(|_| ()).map_err(|e| {
        let message = format!("{e:#}");
        let mut error = ValidationError::new(message.clone());
        error.line = line_in_message(&message);
        // „profiles.foto: …“ nennt das Profil
        if let Some(rest) = message.strip_prefix("profiles.") {
            if let Some((name, _)) = rest.split_once(':') {
                error.rule = Some(name.to_string());
            }
        }
        error
    })
}

/// Regeldatei; die Kategorien kommen aus der Config (bzw. den eingebauten).
pub fn rules_text(text: &str, config: &Config) -> Result<(), ValidationError> {
    let defs = crate::ops::classify::load_defs(config);
    let known = match &defs {
        Ok(d) => KnownCategories::Names(d.names()),
        Err(e) => KnownCategories::Unavailable(format!("{e:#}")),
    };
    RuleSet::parse(text, Path::new("rules.toml"), &known)
        .map(|_| ())
        .map_err(|e| ValidationError {
            message: e.message,
            line: e.line,
            rule: e.rule,
            field: None,
        })
}

/// Eigene Kategorien (`categories.toml`).
pub fn categories_text(text: &str) -> Result<(), ValidationError> {
    CategoryDefs::load(text, "categories.toml")
        .map(|_| ())
        .map_err(|e| ValidationError {
            message: e.message,
            line: e.line,
            rule: e.category,
            field: None,
        })
}

/// Zielstruktur-Vorlage.
pub fn template_text(text: &str) -> Result<(), ValidationError> {
    Template::parse(text).map(|_| ()).map_err(|e| {
        let message = e.to_string();
        let line = line_in_message(&message);
        let message = match message.split_once(": ") {
            Some((head, rest)) if head.starts_with("Zeile ") => rest.to_string(),
            _ => message,
        };
        ValidationError {
            message,
            line,
            rule: None,
            field: None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_gueltig_und_ungueltig() {
        assert!(config_text("top = 10\n").is_ok());
        let err = config_text("top = \"viele\"\n").unwrap_err();
        assert!(err.line == Some(1), "{err}");
        let err = config_text("quarantine_days = 0\n").unwrap_err();
        assert!(err.message.contains("quarantine_days"), "{err}");
        let err = config_text("[profiles.Foto]\nroot = 'D:\\x'\n").unwrap_err();
        assert_eq!(err.rule.as_deref(), Some("Foto"), "{err}");
        assert!(config_text("unbekannt = 1\n").is_err());
    }

    #[test]
    fn regeln_nennen_regel_und_zeile() {
        let config = Config::default();
        let ok = "[[rules]]\nname = \"pdf\"\next = [\"pdf\"]\ntarget = \"Dokumente/\"\n";
        assert!(rules_text(ok, &config).is_ok());
        let bad = "[[rules]]\nname = \"pdf\"\next = [\"pdf\"]\ntarget = \"Dokumente/\"\n\n[[rules]]\nname = \"x\"\nmin_age = \"bald\"\ntarget = \"X/\"\n";
        let err = rules_text(bad, &config).unwrap_err();
        assert_eq!(err.rule.as_deref(), Some("x"), "{err}");
        assert_eq!(err.line, Some(6), "{err}");
        let err = rules_text("", &config).unwrap_err();
        assert!(err.message.contains("keine Regeln"), "{err}");
    }

    #[test]
    fn kategorien_und_vorlagen() {
        assert!(categories_text("").is_ok());
        assert!(categories_text("[[category]]\nname = \n").is_err());
        assert!(template_text(crate::template::builtin_text("para").unwrap()).is_ok());
        let err = template_text("[[level]]\nname = \n").unwrap_err();
        assert!(err.line.is_some() || !err.message.is_empty());
    }
}
