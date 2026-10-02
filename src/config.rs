use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::cli::{ReportArgs, ScanArgs};

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
}

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
        }
    }
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
        Ok(toml::from_str(text)?)
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
