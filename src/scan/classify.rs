use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::config::Config;
use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirMode {
    /// Wird nicht betreten.
    Exclude,
    /// Nur Größe und Dateianzahl werden aufsummiert.
    SummaryOnly,
    Full,
}

/// Absolute Pfade der eingebauten Default-Regeln (aus Umgebungsvariablen).
#[derive(Debug, Clone, Default)]
pub struct DefaultPaths {
    pub excluded: Vec<PathBuf>,
    pub summary: Vec<PathBuf>,
}

impl DefaultPaths {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
        let mut excluded: Vec<PathBuf> = [
            "SystemRoot",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramData",
        ]
        .iter()
        .filter_map(|n| var(n))
        .collect();
        if let Some(local) = var("LOCALAPPDATA") {
            excluded.push(local.join("ordner-cleanup"));
        }
        let summary = var("USERPROFILE")
            .map(|p| vec![p.join("AppData")])
            .unwrap_or_default();
        Self { excluded, summary }
    }
}

const EXCLUDED_NAMES: [&str; 2] = ["$recycle.bin", "system volume information"];
const EXCLUDED_FILES: [&str; 3] = ["pagefile.sys", "hiberfil.sys", "swapfile.sys"];
const REPORT_DIR_PREFIX: &str = "ordner-cleanup-report-";
const SUMMARY_NAMES: [&str; 7] = [
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".gradle",
];

pub struct Classifier {
    root_key: String,
    user_exclude: GlobSet,
    user_summary: GlobSet,
    /// `None` bei `--no-default-excludes`.
    defaults: Option<ActiveDefaults>,
}

struct ActiveDefaults {
    excluded: Vec<String>,
    summary: Vec<String>,
}

fn build_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        let glob = GlobBuilder::new(&p.replace('\\', "/"))
            .case_insensitive(true)
            .literal_separator(false)
            .build()
            .with_context(|| format!("Ungültiges Glob-Muster: {p}"))?;
        builder.add(glob);
    }
    Ok(builder.build()?)
}

fn under(key: &str, prefix_key: &str) -> bool {
    key == prefix_key || key.starts_with(&format!("{prefix_key}\\"))
}

impl Classifier {
    pub fn new(root: &Path, config: &Config, default_paths: &DefaultPaths) -> Result<Self> {
        let root_key = paths::path_key(root);
        // Liegt die explizite Wurzel in einem Default-Bereich, ist diese Regel ausgesetzt.
        let keep = |list: &[PathBuf]| -> Vec<String> {
            list.iter()
                .map(|p| paths::path_key(p))
                .filter(|k| !under(&root_key, k))
                .collect()
        };
        let defaults = (!config.no_default_excludes).then(|| ActiveDefaults {
            excluded: keep(&default_paths.excluded),
            summary: keep(&default_paths.summary),
        });
        Ok(Self {
            user_exclude: build_globs(&config.exclude)?,
            user_summary: build_globs(&config.summary_only)?,
            root_key,
            defaults,
        })
    }

    /// Default-Bereich, in dem die explizite Wurzel liegt (für den Hinweis beim Scan).
    pub fn root_in_default_area(root: &Path, default_paths: &DefaultPaths) -> Option<PathBuf> {
        let root_key = paths::path_key(root);
        default_paths
            .excluded
            .iter()
            .chain(&default_paths.summary)
            .find(|p| under(&root_key, &paths::path_key(p)))
            .cloned()
    }

    fn glob_hit(set: &GlobSet, path: &Path, name: &str) -> bool {
        let full = paths::display(path).replace('\\', "/");
        set.is_match(name) || set.is_match(&full)
    }

    /// `parent_has_project`: im übergeordneten Ordner liegt eine `*.csproj`/`*.sln`.
    pub fn classify_dir(&self, path: &Path, parent_has_project: bool) -> DirMode {
        let key = paths::path_key(path);
        if key == self.root_key {
            return DirMode::Full;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let name_str = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        if Self::glob_hit(&self.user_exclude, path, &name_str) {
            return DirMode::Exclude;
        }
        if let Some(d) = &self.defaults {
            if EXCLUDED_NAMES.contains(&name.as_str())
                || name.starts_with(REPORT_DIR_PREFIX)
                || d.excluded.iter().any(|p| under(&key, p))
            {
                return DirMode::Exclude;
            }
        }
        if Self::glob_hit(&self.user_summary, path, &name_str) {
            return DirMode::SummaryOnly;
        }
        if let Some(d) = &self.defaults {
            let build_dir = (name == "bin" || name == "obj") && parent_has_project;
            if SUMMARY_NAMES.contains(&name.as_str())
                || build_dir
                || d.summary.iter().any(|p| under(&key, p))
            {
                return DirMode::SummaryOnly;
            }
        }
        DirMode::Full
    }

    /// Dateien, die nie erfasst werden (Auslagerungsdateien u. Ä.).
    pub fn is_excluded_file(&self, path: &Path) -> bool {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if Self::glob_hit(&self.user_exclude, path, &name) {
            return true;
        }
        self.defaults.is_some() && EXCLUDED_FILES.contains(&name.to_lowercase().as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn defaults() -> DefaultPaths {
        DefaultPaths {
            excluded: vec![
                PathBuf::from(r"C:\Windows"),
                PathBuf::from(r"C:\Program Files"),
                PathBuf::from(r"C:\Program Files (x86)"),
                PathBuf::from(r"C:\ProgramData"),
                PathBuf::from(r"C:\Users\me\AppData\Local\ordner-cleanup"),
            ],
            summary: vec![PathBuf::from(r"C:\Users\me\AppData")],
        }
    }

    fn classifier(root: &str, config: &Config) -> Classifier {
        Classifier::new(Path::new(root), config, &defaults()).unwrap()
    }

    #[rstest]
    #[case(r"C:\Windows", DirMode::Exclude)]
    #[case(r"c:\windows\system32", DirMode::Exclude)]
    #[case(r"C:\Program Files\Foo", DirMode::Exclude)]
    #[case(r"C:\Program Files (x86)", DirMode::Exclude)]
    #[case(r"C:\ProgramData", DirMode::Exclude)]
    #[case(r"C:\$Recycle.Bin", DirMode::Exclude)]
    #[case(r"D:\System Volume Information", DirMode::Exclude)]
    #[case(r"C:\Users\me\AppData\Local\ordner-cleanup", DirMode::Exclude)]
    #[case(
        r"C:\Users\me\Dokumente\ordner-cleanup-report-20261002",
        DirMode::Exclude
    )]
    #[case(r"C:\Users\me\AppData", DirMode::SummaryOnly)]
    #[case(r"C:\Users\me\AppData\Roaming\Foo", DirMode::SummaryOnly)]
    #[case(r"C:\Users\me\Dokumente\proj\.git", DirMode::SummaryOnly)]
    #[case(r"C:\Users\me\Dokumente\proj\node_modules", DirMode::SummaryOnly)]
    #[case(r"C:\Users\me\Dokumente\proj\target", DirMode::SummaryOnly)]
    #[case(r"C:\Users\me\Dokumente\proj\.venv", DirMode::SummaryOnly)]
    #[case(r"C:\Users\me\Dokumente\proj\venv", DirMode::SummaryOnly)]
    #[case(r"C:\Users\me\Dokumente\proj\__pycache__", DirMode::SummaryOnly)]
    #[case(r"C:\Users\me\Dokumente\proj\.gradle", DirMode::SummaryOnly)]
    #[case(r"C:\Users\me\Dokumente\Rechnungen", DirMode::Full)]
    #[case(r"C:\Users\me\Dokumente\Windows-Notizen", DirMode::Full)]
    fn default_regeln(#[case] path: &str, #[case] expected: DirMode) {
        let c = classifier(r"C:\Users\me\Dokumente", &Config::default());
        assert_eq!(c.classify_dir(Path::new(path), false), expected);
    }

    #[rstest]
    #[case("bin")]
    #[case("obj")]
    fn bin_obj_nur_mit_projektdatei(#[case] name: &str) {
        let c = classifier(r"C:\Users\me\Dokumente", &Config::default());
        let p = PathBuf::from(r"C:\Users\me\Dokumente\app").join(name);
        assert_eq!(c.classify_dir(&p, true), DirMode::SummaryOnly);
        assert_eq!(c.classify_dir(&p, false), DirMode::Full);
    }

    #[test]
    fn explizite_wurzel_hat_vorrang() {
        let c = classifier(r"C:\Windows\Temp\x", &Config::default());
        assert_eq!(
            c.classify_dir(Path::new(r"C:\Windows\Temp\x"), false),
            DirMode::Full
        );
        assert_eq!(
            c.classify_dir(Path::new(r"C:\Windows\Temp\x\sub"), false),
            DirMode::Full
        );
    }

    #[test]
    fn wurzel_im_summary_bereich_wird_voll_gescannt() {
        let c = classifier(r"C:\Users\me\AppData\Roaming\Foo", &Config::default());
        let sub = Path::new(r"C:\Users\me\AppData\Roaming\Foo\data");
        assert_eq!(c.classify_dir(sub, false), DirMode::Full);
    }

    #[test]
    fn wurzel_im_default_bereich_wird_gemeldet() {
        let hit = Classifier::root_in_default_area(Path::new(r"C:\Windows\Temp"), &defaults());
        assert_eq!(hit, Some(PathBuf::from(r"C:\Windows")));
        assert_eq!(
            Classifier::root_in_default_area(Path::new(r"C:\Users\me\Dokumente"), &defaults()),
            None
        );
    }

    #[test]
    fn no_default_excludes_scannt_alles() {
        let config = Config {
            no_default_excludes: true,
            ..Config::default()
        };
        let c = classifier(r"C:\Users\me\Dokumente", &config);
        for p in [
            r"C:\Windows",
            r"C:\Users\me\AppData",
            r"C:\Users\me\Dokumente\proj\node_modules",
            r"C:\$Recycle.Bin",
        ] {
            assert_eq!(c.classify_dir(Path::new(p), false), DirMode::Full, "{p}");
        }
        assert!(!c.is_excluded_file(Path::new(r"C:\pagefile.sys")));
    }

    #[test]
    fn user_globs_greifen_auch_ohne_defaults() {
        let config = Config {
            exclude: vec!["Backup*".into(), r"**\Temp\Cache".into()],
            summary_only: vec!["dist".into()],
            no_default_excludes: true,
            ..Config::default()
        };
        let c = classifier(r"C:\Users\me\Dokumente", &config);
        let d = |p: &str| c.classify_dir(Path::new(p), false);
        assert_eq!(d(r"C:\Users\me\Dokumente\backup-2020"), DirMode::Exclude);
        assert_eq!(d(r"C:\Users\me\Dokumente\a\Temp\Cache"), DirMode::Exclude);
        assert_eq!(d(r"C:\Users\me\Dokumente\proj\dist"), DirMode::SummaryOnly);
        assert_eq!(d(r"C:\Users\me\Dokumente\proj\src"), DirMode::Full);
    }

    #[test]
    fn exclude_schlaegt_summary() {
        let config = Config {
            exclude: vec!["node_modules".into()],
            ..Config::default()
        };
        let c = classifier(r"C:\Users\me\Dokumente", &config);
        let p = Path::new(r"C:\Users\me\Dokumente\proj\node_modules");
        assert_eq!(c.classify_dir(p, false), DirMode::Exclude);
    }

    #[test]
    fn systemdateien_und_user_globs_fuer_dateien() {
        let config = Config {
            exclude: vec!["*.iso".into()],
            ..Config::default()
        };
        let c = classifier(r"C:\Users\me\Dokumente", &config);
        assert!(c.is_excluded_file(Path::new(r"C:\pagefile.sys")));
        assert!(c.is_excluded_file(Path::new(r"C:\HIBERFIL.SYS")));
        assert!(c.is_excluded_file(Path::new(r"C:\swapfile.sys")));
        assert!(c.is_excluded_file(Path::new(r"C:\x\Windows.ISO")));
        assert!(!c.is_excluded_file(Path::new(r"C:\x\bericht.docx")));
    }

    #[test]
    fn ungueltiges_glob_ist_fehler() {
        let config = Config {
            exclude: vec!["[".into()],
            ..Config::default()
        };
        assert!(Classifier::new(Path::new(r"C:\x"), &config, &defaults()).is_err());
    }
}
