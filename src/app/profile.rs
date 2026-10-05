//! Profile: Wurzel und Config-Abweichungen aus der `config.toml` statt Pfad und Optionen.

use std::fmt::Write;

use anyhow::Result;

use crate::ops::admin::ProfileInfo;
use crate::ops::local_time;

pub(super) use crate::ops::target::{load_template, target, Target};

/// `profiles`: alle Profile mit Wurzel, letztem Verlaufseintrag und Score.
pub(super) fn profiles_command() -> Result<i32> {
    let infos = crate::ops::admin::profiles()?;
    if infos.is_empty() {
        println!(
            "Keine Profile. Lege sie als [profiles.<name>] mit `root = '…'` in der config.toml an."
        );
        return Ok(0);
    }
    print!("{}", render(&infos));
    Ok(0)
}

fn render(infos: &[ProfileInfo]) -> String {
    let mut s = String::new();
    for info in infos {
        let p = &info.profile;
        let _ = writeln!(s, "{}", info.name);
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
        match &info.last_run {
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
        match &info.last_point {
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
    use crate::config::Config;

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
        let infos: Vec<ProfileInfo> = cfg(CONFIG)
            .profiles
            .iter()
            .map(|(name, profile)| ProfileInfo {
                name: name.clone(),
                profile: profile.clone(),
                last_run: None,
                last_point: None,
            })
            .collect();
        let text = render(&infos);
        assert!(text.starts_with("downloads\n"), "{text}");
        assert!(text.contains(r"C:\Users\x\Downloads") && text.contains("rules, junk"));
        assert!(text.contains("noch kein Verlauf"));
        assert!(text.contains("Letzter Lauf:  noch keiner"));
    }
}
