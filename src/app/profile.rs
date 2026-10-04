//! Profile: Wurzel und Config-Abweichungen aus der `config.toml` statt Pfad und Optionen.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use super::{load_config, local_time, normalize};
use crate::config::{Config, Profile};
use crate::history::History;
use crate::paths;
use crate::template::{self, Loaded};

/// Das Profil, über das ein Befehl läuft.
#[derive(Debug, Clone)]
pub(super) struct ProfileRun {
    pub name: String,
    pub profile: Profile,
}

/// Wurzel und zusammengeführte Config eines Befehls, mit oder ohne Profil.
#[derive(Debug)]
pub(super) struct Target {
    pub root: PathBuf,
    pub config: Config,
    pub profile: Option<ProfileRun>,
}

impl Target {
    /// Netzlaufwerke nur mit `--force` bzw. `force = true` im Profil.
    pub fn force(&self) -> bool {
        self.profile.as_ref().is_some_and(|p| p.profile.force)
    }

    pub fn profile_name(&self) -> Option<&str> {
        self.profile.as_ref().map(|p| p.name.as_str())
    }
}

/// Wurzel und Config aus `--profile` bzw. dem Pfad. Explizite CLI-Angaben legen die Aufrufer
/// danach über die zusammengeführte Config (CLI vor Profil vor globaler Config).
pub(super) fn target(path: Option<&Path>, profile: Option<&str>) -> Result<Target> {
    target_from(
        load_config()?,
        paths::config_dir().as_deref(),
        path,
        profile,
    )
}

fn target_from(
    base: Config,
    config_dir: Option<&Path>,
    path: Option<&Path>,
    profile: Option<&str>,
) -> Result<Target> {
    match (profile, path) {
        (Some(name), None) => {
            let p = base.profile(name)?.clone();
            Ok(Target {
                root: PathBuf::from(&p.root),
                config: base.with_profile(&p, config_dir),
                profile: Some(ProfileRun {
                    name: name.to_string(),
                    profile: p,
                }),
            })
        }
        (Some(_), Some(_)) => bail!("Pfad und --profile schließen sich aus"),
        (None, Some(path)) => Ok(Target {
            root: path.to_path_buf(),
            config: base,
            profile: None,
        }),
        (None, None) => bail!("Pfad oder --profile angeben"),
    }
}

/// Die Vorlage des Befehls: `--template` vor dem Profil. Relative Pfade gelten für `--template`
/// ab dem aktuellen Ordner, für das Profil ab dem Config-Ordner.
pub(super) fn load_template(cli: Option<&str>, target: &Target) -> Result<Option<Loaded>> {
    match (
        cli,
        target
            .profile
            .as_ref()
            .and_then(|p| p.profile.template.as_deref()),
    ) {
        (Some(spec), _) => Ok(Some(template::resolve(spec, None)?)),
        (None, Some(spec)) => Ok(Some(template::resolve(
            spec,
            paths::config_dir().as_deref(),
        )?)),
        (None, None) => Ok(None),
    }
}

/// `profiles`: alle Profile mit Wurzel, letztem Verlaufseintrag und Score.
pub(super) fn profiles_command() -> Result<i32> {
    let config = load_config()?;
    if config.profiles.is_empty() {
        println!(
            "Keine Profile. Lege sie als [profiles.<name>] mit `root = '…'` in der config.toml an."
        );
        return Ok(0);
    }
    let history = paths::history_path()
        .ok()
        .filter(|p| p.exists())
        .and_then(|p| History::open(&p).ok());
    let last_run = |name: &str| {
        paths::runs_log(name)
            .ok()
            .and_then(|log| crate::runlog::read_all(&log).pop())
    };
    print!("{}", render(&config, history.as_ref(), &last_run));
    Ok(0)
}

fn render(
    config: &Config,
    history: Option<&History>,
    last_run: &dyn Fn(&str) -> Option<crate::runlog::RunRecord>,
) -> String {
    let mut s = String::new();
    for (name, p) in &config.profiles {
        let _ = writeln!(s, "{name}");
        let _ = writeln!(s, "  Wurzel:   {}", p.root);
        let plans = if p.plans.is_empty() {
            "keine".to_string()
        } else {
            p.plans.join(", ")
        };
        let _ = writeln!(s, "  Pläne:    {plans}");
        if let Some(t) = &p.template {
            let _ = writeln!(s, "  Vorlage:  {t}");
        }
        match last_run(name) {
            Some(r) => {
                let _ = writeln!(
                    s,
                    "  Letzter Lauf:  {} – {}",
                    local_time(&r.started),
                    r.status.label()
                );
            }
            None => {
                let _ = writeln!(s, "  Letzter Lauf:  noch keiner");
            }
        }
        let key = paths::dir_key(&normalize(Path::new(&p.root)));
        let last = history
            .and_then(|h| h.series(&key, "", 1).ok())
            .and_then(|mut v| v.pop());
        match last {
            Some(point) => {
                let _ = writeln!(
                    s,
                    "  Letzter Stand: {}, Score {}",
                    local_time(&point.taken_at),
                    point.score
                );
            }
            None => {
                let _ = writeln!(s, "  Letzter Stand: noch kein Verlauf");
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
exclude = ["*.tmp"]
old_after = "1y"
top = 50

[profiles.downloads]
root = 'C:\Users\x\Downloads'
rules_file = "downloads.rules.toml"
plans = ["rules", "junk"]
exclude = ["*.iso"]
old_after = "2y"
force = true
"#;

    fn cfg(text: &str) -> Config {
        Config::parse(text).unwrap()
    }

    #[test]
    fn profil_ergaenzt_listen_und_ersetzt_einzelwerte() {
        let t = target_from(
            cfg(CONFIG),
            Some(Path::new(r"C:\Cfg")),
            None,
            Some("downloads"),
        )
        .unwrap();
        assert_eq!(t.root, PathBuf::from(r"C:\Users\x\Downloads"));
        assert_eq!(t.config.exclude, ["*.tmp", "*.iso"]);
        assert_eq!(t.config.old_after, "2y");
        assert_eq!(t.config.top, 50, "nicht genannte Werte bleiben global");
        assert!(t.force());
        assert_eq!(t.profile_name(), Some("downloads"));
        assert_eq!(
            t.config.rules_file.as_deref(),
            Some(r"C:\Cfg\downloads.rules.toml"),
            "relativ zum Config-Ordner"
        );
    }

    #[test]
    fn ohne_profil_bleibt_die_config_unveraendert() {
        let t = target_from(cfg(CONFIG), None, Some(Path::new(r"D:\Daten")), None).unwrap();
        assert_eq!(t.root, PathBuf::from(r"D:\Daten"));
        assert_eq!(t.config, cfg(CONFIG));
        assert!(!t.force());
        assert!(t.profile.is_none());
    }

    #[test]
    fn absolute_regeldatei_bleibt_absolut() {
        let c = cfg("[profiles.a]\nroot = 'D:\\x'\nrules_file = 'E:\\r.toml'");
        let t = target_from(c, Some(Path::new(r"C:\Cfg")), None, Some("a")).unwrap();
        assert_eq!(t.config.rules_file.as_deref(), Some(r"E:\r.toml"));
    }

    #[test]
    fn unbekanntes_profil_nennt_die_vorhandenen() {
        let err = target_from(cfg(CONFIG), None, None, Some("nope")).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("nope") && text.contains("downloads"),
            "{text}"
        );
        let err = target_from(Config::default(), None, None, Some("x")).unwrap_err();
        assert!(err.to_string().contains("keine"), "{err}");
    }

    #[test]
    fn pfad_und_profil_zusammen_sind_ein_fehler() {
        assert!(target_from(
            cfg(CONFIG),
            None,
            Some(Path::new(r"D:\x")),
            Some("downloads")
        )
        .is_err());
        assert!(target_from(cfg(CONFIG), None, None, None).is_err());
    }

    #[test]
    fn liste_zeigt_wurzel_plaene_und_fehlenden_verlauf() {
        let text = render(&cfg(CONFIG), None, &|_| None);
        assert!(text.starts_with("downloads\n"), "{text}");
        assert!(text.contains(r"C:\Users\x\Downloads") && text.contains("rules, junk"));
        assert!(text.contains("noch kein Verlauf"));
        assert!(text.contains("Letzter Lauf:  noch keiner"));
    }
}
