use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::analysis::age::parse_old_after;
use crate::cli::{ReportArgs, ScanArgs};

/// Eingebaute Kategorien von `plan junk`; eigene Regeln dürfen diese Namen nicht verwenden.
pub const BUILTIN_JUNK_CATEGORIES: [&str; 4] = ["system", "temp", "downloads", "installer"];

/// Eigene Müll-Kategorie (`[[junk_rules]]` in der Config).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JunkRule {
    pub name: String,
    /// Globs auf den Dateinamen (ohne Beachtung der Groß-/Kleinschreibung).
    pub patterns: Vec<String>,
    /// Mindestalter, z. B. `30d`; ohne Angabe gilt kein Mindestalter.
    #[serde(default)]
    pub min_age: Option<String>,
    /// Nur Dateien direkt in diesen Ordnern; leer = überall.
    #[serde(default)]
    pub dirs: Vec<String>,
}

/// Inhalt der `config.toml`; alle Felder sind optional.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub exclude: Vec<String>,
    pub summary_only: Vec<String>,
    pub no_default_excludes: bool,
    pub old_after: String,
    pub top: usize,
    pub max_depth_warning: usize,
    pub huge_dir_entries: usize,
    pub onedrive_conflict_hostnames: Vec<String>,
    pub threads: usize,
    pub protected_paths: Vec<String>,
    pub quarantine_days: u32,
    pub junk_categories: Vec<String>,
    pub installer_min_age: String,
    /// Ersetzt den Windows-Ordner „Downloads“, wenn nicht leer.
    pub downloads_dirs: Vec<String>,
    pub junk_rules: Vec<JunkRule>,
    pub archive_older_than: String,
    pub versions_min_age: String,
    pub onedrive_max_move_files: u64,
    pub onedrive_max_move_bytes: String,
    /// Regeldatei für `plan rules`; ohne Angabe `rules.toml` neben der Config.
    pub rules_file: Option<String>,
}

const DEFAULT_INSTALLER_MIN_AGE_DAYS: i64 = 90;
const DEFAULT_ARCHIVE_OLDER_THAN_DAYS: i64 = 730;
const DEFAULT_VERSIONS_MIN_AGE_DAYS: i64 = 30;
const DEFAULT_ONEDRIVE_MAX_MOVE_BYTES: u64 = 5_000_000_000;

impl Default for Config {
    fn default() -> Self {
        Self {
            exclude: Vec::new(),
            summary_only: Vec::new(),
            no_default_excludes: false,
            old_after: "1y".to_string(),
            top: 50,
            max_depth_warning: 8,
            huge_dir_entries: 1000,
            onedrive_conflict_hostnames: Vec::new(),
            threads: 0,
            protected_paths: Vec::new(),
            quarantine_days: 30,
            junk_categories: BUILTIN_JUNK_CATEGORIES.map(String::from).to_vec(),
            installer_min_age: "90d".to_string(),
            downloads_dirs: Vec::new(),
            junk_rules: Vec::new(),
            archive_older_than: "2y".to_string(),
            versions_min_age: "30d".to_string(),
            onedrive_max_move_files: 1000,
            onedrive_max_move_bytes: "5GB".to_string(),
            rules_file: None,
        }
    }
}

pub(crate) fn parse_bytes(text: &str) -> Option<u64> {
    text.parse::<bytesize::ByteSize>()
        .ok()
        .map(|b| b.as_u64())
        .filter(|b| *b > 0)
}

impl Config {
    /// Lädt die Config; eine fehlende Datei ergibt die Defaults.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text)
                .with_context(|| format!("Config {} ist ungültig", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("Config {} nicht lesbar", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Self = toml::from_str(text)?;
        anyhow::ensure!(
            config.quarantine_days >= 1,
            "quarantine_days muss mindestens 1 sein"
        );
        config.validate_cleanup()?;
        Ok(config)
    }

    /// Prüft die Einstellungen der Aufräumaktionen schon beim Laden statt erst beim Planen.
    fn validate_cleanup(&self) -> Result<()> {
        for (key, value) in [
            ("installer_min_age", &self.installer_min_age),
            ("archive_older_than", &self.archive_older_than),
            ("versions_min_age", &self.versions_min_age),
        ] {
            parse_old_after(value).map_err(|e| anyhow::anyhow!("{key}: {e}"))?;
        }
        anyhow::ensure!(
            parse_bytes(&self.onedrive_max_move_bytes).is_some(),
            "onedrive_max_move_bytes: ungültige Größe '{}' (erwartet z. B. 5GB, 500MB)",
            self.onedrive_max_move_bytes
        );
        anyhow::ensure!(
            self.onedrive_max_move_files >= 1,
            "onedrive_max_move_files muss mindestens 1 sein"
        );

        let mut names = std::collections::HashSet::new();
        for rule in &self.junk_rules {
            let name = rule.name.trim();
            anyhow::ensure!(!name.is_empty(), "junk_rules: name darf nicht leer sein");
            anyhow::ensure!(
                !BUILTIN_JUNK_CATEGORIES.contains(&name),
                "junk_rules: '{name}' ist der Name einer eingebauten Kategorie"
            );
            anyhow::ensure!(
                names.insert(name.to_string()),
                "junk_rules: Name '{name}' kommt doppelt vor"
            );
            anyhow::ensure!(
                !rule.patterns.is_empty(),
                "junk_rules '{name}': patterns darf nicht leer sein"
            );
            for pattern in &rule.patterns {
                globset::Glob::new(pattern).map_err(|e| {
                    anyhow::anyhow!("junk_rules '{name}': ungültiges Muster '{pattern}' ({e})")
                })?;
            }
            if let Some(age) = &rule.min_age {
                parse_old_after(age)
                    .map_err(|e| anyhow::anyhow!("junk_rules '{name}': min_age: {e}"))?;
            }
        }
        for category in &self.junk_categories {
            anyhow::ensure!(
                BUILTIN_JUNK_CATEGORIES.contains(&category.as_str()) || names.contains(category),
                "junk_categories: unbekannte Kategorie '{category}' (eingebaut: {}; eigene über [[junk_rules]])",
                BUILTIN_JUNK_CATEGORIES.join(", ")
            );
        }
        Ok(())
    }

    pub fn installer_min_age_days(&self) -> i64 {
        parse_old_after(&self.installer_min_age).unwrap_or(DEFAULT_INSTALLER_MIN_AGE_DAYS)
    }

    pub fn archive_older_than_days(&self) -> i64 {
        parse_old_after(&self.archive_older_than).unwrap_or(DEFAULT_ARCHIVE_OLDER_THAN_DAYS)
    }

    pub fn versions_min_age_days(&self) -> i64 {
        parse_old_after(&self.versions_min_age).unwrap_or(DEFAULT_VERSIONS_MIN_AGE_DAYS)
    }

    pub fn onedrive_max_move_bytes_value(&self) -> u64 {
        parse_bytes(&self.onedrive_max_move_bytes).unwrap_or(DEFAULT_ONEDRIVE_MAX_MOVE_BYTES)
    }

    /// CLI-Flags haben Vorrang vor der Config; Listen werden ergänzt.
    pub fn apply_scan_args(&mut self, args: &ScanArgs) {
        self.exclude.extend(args.exclude.iter().cloned());
        self.summary_only.extend(args.summary_only.iter().cloned());
        self.no_default_excludes |= args.no_default_excludes;
        if let Some(threads) = args.threads {
            self.threads = threads;
        }
    }

    pub fn apply_report_args(&mut self, args: &ReportArgs) {
        if let Some(old_after) = &args.old_after {
            self.old_after = old_after.clone();
        }
        if let Some(top) = args.top {
            self.top = top;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scan_args() -> ScanArgs {
        ScanArgs {
            path: PathBuf::from("C:\\x"),
            exclude: vec![],
            summary_only: vec![],
            no_default_excludes: false,
            force: false,
            reset_index: false,
            threads: None,
        }
    }

    #[test]
    fn leere_config_ergibt_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        assert_eq!(Config::default().top, 50);
        assert_eq!(Config::default().old_after, "1y");
    }

    #[test]
    fn felder_werden_gelesen() {
        let c = Config::parse(
            "exclude = [\"*.iso\"]\ntop = 10\nthreads = 4\nno_default_excludes = true",
        )
        .unwrap();
        assert_eq!(c.exclude, vec!["*.iso"]);
        assert_eq!(c.top, 10);
        assert_eq!(c.threads, 4);
        assert!(c.no_default_excludes);
        assert_eq!(c.huge_dir_entries, 1000);
    }

    #[test]
    fn schutz_und_quarantaene_defaults() {
        let c = Config::default();
        assert!(c.protected_paths.is_empty());
        assert_eq!(c.quarantine_days, 30);
    }

    #[test]
    fn schutz_und_quarantaene_werden_gelesen() {
        let c =
            Config::parse("protected_paths = [\"D:\\\\Wichtig\"]\nquarantine_days = 7").unwrap();
        assert_eq!(c.protected_paths, vec!["D:\\Wichtig"]);
        assert_eq!(c.quarantine_days, 7);
    }

    #[test]
    fn quarantaene_null_tage_ist_ungueltig() {
        assert!(Config::parse("quarantine_days = 0").is_err());
    }

    #[test]
    fn aufraeumaktionen_defaults() {
        let c = Config::default();
        assert_eq!(
            c.junk_categories,
            ["system", "temp", "downloads", "installer"]
        );
        assert_eq!(c.installer_min_age, "90d");
        assert!(c.downloads_dirs.is_empty());
        assert!(c.junk_rules.is_empty());
        assert_eq!(c.archive_older_than, "2y");
        assert_eq!(c.versions_min_age, "30d");
        assert_eq!(c.onedrive_max_move_files, 1000);
        assert_eq!(c.onedrive_max_move_bytes, "5GB");
        assert_eq!(c.installer_min_age_days(), 90);
        assert_eq!(c.archive_older_than_days(), 730);
        assert_eq!(c.versions_min_age_days(), 30);
        assert_eq!(c.onedrive_max_move_bytes_value(), 5_000_000_000);
    }

    #[test]
    fn aufraeumaktionen_felder_und_eigene_regeln_werden_gelesen() {
        let c = Config::parse(
            r#"
junk_categories = ["temp", "logs"]
installer_min_age = "6m"
downloads_dirs = ["D:\\Eingang"]
archive_older_than = "3y"
versions_min_age = "14d"
onedrive_max_move_files = 50
onedrive_max_move_bytes = "2GiB"

[[junk_rules]]
name = "logs"
patterns = ["*.log", "*.LOG.1"]
min_age = "30d"
dirs = ["D:\\Logs"]

[[junk_rules]]
name = "dumps"
patterns = ["*.dmp"]
"#,
        )
        .unwrap();
        assert_eq!(c.junk_categories, ["temp", "logs"]);
        assert_eq!(c.installer_min_age_days(), 180);
        assert_eq!(c.downloads_dirs, ["D:\\Eingang"]);
        assert_eq!(c.archive_older_than_days(), 1095);
        assert_eq!(c.versions_min_age_days(), 14);
        assert_eq!(c.onedrive_max_move_files, 50);
        assert_eq!(c.onedrive_max_move_bytes_value(), 2 * 1024 * 1024 * 1024);
        assert_eq!(c.junk_rules.len(), 2);
        let logs = &c.junk_rules[0];
        assert_eq!(logs.name, "logs");
        assert_eq!(logs.patterns, ["*.log", "*.LOG.1"]);
        assert_eq!(logs.min_age.as_deref(), Some("30d"));
        assert_eq!(logs.dirs, ["D:\\Logs"]);
        let dumps = &c.junk_rules[1];
        assert!(dumps.min_age.is_none() && dumps.dirs.is_empty());
    }

    #[test]
    fn ungueltige_dauern_und_groessen_scheitern_beim_laden() {
        for (text, needle) in [
            ("installer_min_age = \"bald\"", "installer_min_age"),
            ("installer_min_age = \"0d\"", "installer_min_age"),
            ("archive_older_than = \"2x\"", "archive_older_than"),
            ("versions_min_age = \"\"", "versions_min_age"),
            (
                "onedrive_max_move_bytes = \"viel\"",
                "onedrive_max_move_bytes",
            ),
            (
                "onedrive_max_move_bytes = \"0B\"",
                "onedrive_max_move_bytes",
            ),
            ("onedrive_max_move_files = 0", "onedrive_max_move_files"),
        ] {
            let err = Config::parse(text).unwrap_err();
            assert!(
                format!("{err:#}").contains(needle),
                "{text}: Fehlertext nennt {needle} nicht: {err:#}"
            );
        }
    }

    #[test]
    fn junk_kategorien_muessen_eingebaut_oder_eigene_regeln_sein() {
        let err = Config::parse("junk_categories = [\"temp\", \"logs\"]").unwrap_err();
        assert!(format!("{err:#}").contains("logs"));
        assert!(Config::parse("junk_categories = []").is_ok());
        let ok = Config::parse(
            "junk_categories = [\"logs\"]\n[[junk_rules]]\nname = \"logs\"\npatterns = [\"*.log\"]",
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn junk_regeln_werden_streng_geprueft() {
        let rule = |body: &str| format!("[[junk_rules]]\n{body}");
        for (text, needle) in [
            (rule("name = \"\"\npatterns = [\"*.x\"]"), "name"),
            (rule("name = \"temp\"\npatterns = [\"*.x\"]"), "temp"),
            (rule("name = \"a\"\npatterns = []"), "patterns"),
            (rule("name = \"a\"\npatterns = [\"[kaputt\"]"), "kaputt"),
            (
                rule("name = \"a\"\npatterns = [\"*.x\"]\nmin_age = \"x\""),
                "min_age",
            ),
            (
                format!(
                    "{}\n{}",
                    rule("name = \"a\"\npatterns = [\"*.x\"]"),
                    rule("name = \"a\"\npatterns = [\"*.y\"]")
                ),
                "doppelt",
            ),
            (
                rule("name = \"a\"\npatterns = [\"*.x\"]\nextra = 1"),
                "extra",
            ),
        ] {
            let err = Config::parse(&text).unwrap_err();
            assert!(
                format!("{err:#}").contains(needle),
                "{text}: Fehlertext nennt {needle} nicht: {err:#}"
            );
        }
    }

    #[test]
    fn unbekannte_felder_sind_fehler() {
        assert!(Config::parse("tpo = 3").is_err());
    }

    #[test]
    fn fehlende_datei_ergibt_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn kaputte_datei_nennt_den_pfad() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "top = \"viele\"").unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(format!("{err:#}").contains("config.toml"));
    }

    #[test]
    fn cli_ergaenzt_listen_und_ueberschreibt_skalare() {
        let mut c = Config::parse("exclude = [\"a\"]\nthreads = 2").unwrap();
        let mut args = scan_args();
        args.exclude = vec!["b".into()];
        args.summary_only = vec!["c".into()];
        args.no_default_excludes = true;
        args.threads = Some(8);
        c.apply_scan_args(&args);
        assert_eq!(c.exclude, vec!["a", "b"]);
        assert_eq!(c.summary_only, vec!["c"]);
        assert!(c.no_default_excludes);
        assert_eq!(c.threads, 8);
    }

    #[test]
    fn cli_ohne_flags_laesst_config_unberuehrt() {
        let mut c = Config::parse("threads = 2\nno_default_excludes = true").unwrap();
        c.apply_scan_args(&scan_args());
        assert_eq!(c.threads, 2);
        assert!(c.no_default_excludes);
    }

    #[test]
    fn report_flags_haben_vorrang() {
        let mut c = Config::parse("top = 10\nold_after = \"2y\"").unwrap();
        c.apply_report_args(&ReportArgs {
            path: None,
            out: None,
            format: vec![],
            old_after: Some("18m".into()),
            top: None,
        });
        assert_eq!(c.old_after, "18m");
        assert_eq!(c.top, 10);
    }
}
