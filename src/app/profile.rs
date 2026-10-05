//! Profile: Wurzel und Config-Abweichungen aus der `config.toml` statt Pfad und Optionen.

use std::fmt::Write;
use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::history::History;
use crate::ops::{load_config, local_time, normalize};
use crate::paths;

pub(super) use crate::ops::target::{load_template, target, Target};

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
[profiles.downloads]
root = 'C:\Users\x\Downloads'
rules_file = "downloads.rules.toml"
plans = ["rules", "junk"]
"#;

    fn cfg(text: &str) -> Config {
        Config::parse(text).unwrap()
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
