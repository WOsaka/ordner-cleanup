//! Fingerabdruck der Einstellungen, die die Kennzahlen beeinflussen. Zwei Läufe mit gleichem
//! Fingerabdruck (und gleicher `METRICS_VERSION`) sind direkt vergleichbar.

use serde_json::{json, Value};
use xxhash_rust::xxh3::xxh3_64;

use crate::config::Config;

fn sorted(items: &[String]) -> Vec<&str> {
    let mut v: Vec<&str> = items.iter().map(String::as_str).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Kanonische Form der relevanten Einstellungen. `template` ist Name und Inhalts-Hash der Vorlage.
fn canonical(config: &Config, template: Option<&str>) -> Value {
    let rules: Vec<Value> = config
        .junk_rules
        .iter()
        .map(|r| {
            json!({
                "name": r.name,
                "patterns": sorted(&r.patterns),
                "min_age": r.min_age,
                "dirs": sorted(&r.dirs),
            })
        })
        .collect();
    let w = &config.health.weights;
    json!({
        "exclude": sorted(&config.exclude),
        "summary_only": sorted(&config.summary_only),
        "no_default_excludes": config.no_default_excludes,
        // Die Reihenfolge entscheidet, welche Kategorie zuerst greift.
        "junk_categories": config.junk_categories,
        "junk_rules": rules,
        "installer_min_age": config.installer_min_age_days(),
        "downloads_dirs": sorted(&config.downloads_dirs),
        "old_after": config.old_after,
        "max_depth_warning": config.max_depth_warning,
        "huge_dir_entries": config.huge_dir_entries,
        "onedrive_conflict_hostnames": sorted(&config.onedrive_conflict_hostnames),
        "weights": [w.junk, w.duplicates, w.problems, w.structure, w.versions, w.template],
        "template": template,
    })
}

pub fn fingerprint(config: &Config, template: Option<&str>) -> String {
    let text = canonical(config, template).to_string();
    format!("{:016x}", xxh3_64(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(text: &str) -> String {
        fingerprint(&Config::parse(text).unwrap(), None)
    }

    #[test]
    fn gleiche_config_gleicher_abdruck() {
        assert_eq!(fp(""), fp(""));
        assert_eq!(
            fp("exclude = [\"a\", \"b\"]"),
            fp("exclude = [\"b\", \"a\", \"a\"]")
        );
    }

    #[test]
    fn jede_relevante_aenderung_aendert_den_abdruck() {
        let base = fp("");
        for text in [
            "exclude = [\"*.iso\"]",
            "summary_only = [\"x\"]",
            "no_default_excludes = true",
            "junk_categories = [\"temp\"]",
            "installer_min_age = \"30d\"",
            "downloads_dirs = [\"D:\\\\Eingang\"]",
            "old_after = \"2y\"",
            "max_depth_warning = 5",
            "huge_dir_entries = 10",
            "onedrive_conflict_hostnames = [\"pc\"]",
            "[health.weights]\njunk = 1",
            "[[junk_rules]]\nname = \"logs\"\npatterns = [\"*.log\"]",
        ] {
            assert_ne!(fp(text), base, "{text}");
        }
        assert_ne!(
            fingerprint(&Config::default(), Some("para:abc")),
            fingerprint(&Config::default(), None)
        );
        assert_ne!(
            fingerprint(&Config::default(), Some("para:abc")),
            fingerprint(&Config::default(), Some("para:abd"))
        );
    }

    #[test]
    fn irrelevante_einstellungen_aendern_ihn_nicht() {
        let base = fp("");
        for text in [
            "top = 5",
            "threads = 3",
            "quarantine_days = 3",
            "archive_older_than = \"5y\"",
            "versions_min_age = \"1d\"",
            "rules_file = \"x.toml\"",
            "onedrive_max_move_files = 3",
        ] {
            assert_eq!(fp(text), base, "{text}");
        }
    }

    #[test]
    fn reihenfolge_der_kategorien_zaehlt() {
        assert_ne!(
            fp("junk_categories = [\"temp\", \"system\"]"),
            fp("junk_categories = [\"system\", \"temp\"]")
        );
    }
}
