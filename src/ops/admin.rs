//! Verwaltung und Verlauf: `profiles`, `history` und `index`.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use super::target::target;
use super::{load_config, normalize};
use crate::config::Profile;
use crate::history::{History, Point};
use crate::index::{Index, RootInfo};
use crate::paths;
use crate::runlog::{self, RunRecord};

/// Ein Profil der Config mit seinem letzten Lauf und Stand.
#[derive(Debug, Clone)]
pub struct ProfileInfo {
    pub name: String,
    pub profile: Profile,
    /// Letzter Lauf aus dem Lauf-Protokoll
    pub last_run: Option<RunRecord>,
    /// Letzte Momentaufnahme (Score) der Wurzel des Profils
    pub last_point: Option<Point>,
}

/// Alle Profile der Config (nach Namen sortiert) mit letztem Lauf und letztem Stand.
pub fn profiles() -> Result<Vec<ProfileInfo>> {
    let config = load_config()?;
    let history = paths::history_path()
        .ok()
        .filter(|p| p.exists())
        .and_then(|p| History::open(&p).ok());
    Ok(config
        .profiles
        .iter()
        .map(|(name, profile)| {
            let last_run = paths::runs_log(name)
                .ok()
                .and_then(|log| runlog::read_all(&log).pop());
            let key = paths::dir_key(&normalize(Path::new(&profile.root)));
            let last_point = history
                .as_ref()
                .and_then(|h| h.series(&key, "", 1).ok())
                .and_then(|mut v| v.pop());
            ProfileInfo {
                name: name.clone(),
                profile: profile.clone(),
                last_run,
                last_point,
            }
        })
        .collect())
}

/// Verlauf einer Wurzel (oder eines Ordners darin).
#[derive(Debug, Clone)]
pub struct HistoryView {
    pub root_key: String,
    pub root_path: String,
    /// Leer = ganze Wurzel
    pub folder: String,
    /// Chronologisch, älteste zuerst
    pub points: Vec<Point>,
}

/// Wurzel des Verlaufs: der angegebene Pfad oder die einzige im Verlauf bekannte Wurzel.
fn pick_root(history: &History, path: Option<&Path>) -> Result<(String, String)> {
    let roots = history.roots()?;
    match path {
        Some(path) => {
            let key = paths::dir_key(&normalize(path));
            match roots.into_iter().find(|(k, _)| *k == key) {
                Some(found) => Ok(found),
                None => bail!(
                    "Kein Verlauf für {} – zuerst `scan` ausführen",
                    paths::display(path)
                ),
            }
        }
        None => match roots.as_slice() {
            [] => bail!("Noch kein Verlauf – zuerst `scan` ausführen"),
            [one] => Ok(one.clone()),
            many => {
                let list: Vec<String> = many.iter().map(|(_, p)| format!("  {p}")).collect();
                bail!(
                    "Mehrere Wurzeln im Verlauf, bitte eine auswählen:\n{}",
                    list.join("\n")
                )
            }
        },
    }
}

/// Verlauf aus der Datei `history.db`; `profile` löst die Wurzel aus der Config auf.
pub fn history(
    path: Option<&Path>,
    profile: Option<&str>,
    folder: Option<&str>,
    last: usize,
) -> Result<HistoryView> {
    let path: Option<PathBuf> = match profile {
        Some(name) => Some(target(None, Some(name))?.root),
        None => path.map(Path::to_path_buf),
    };
    history_in(
        &History::open(&paths::history_path()?)?,
        path.as_deref(),
        folder,
        last,
    )
}

fn history_in(
    history: &History,
    path: Option<&Path>,
    folder: Option<&str>,
    last: usize,
) -> Result<HistoryView> {
    let (root_key, root_path) = pick_root(history, path)?;
    let folder = folder.unwrap_or_default().to_string();
    let points = history.series(&root_key, &folder, last)?;
    if points.is_empty() {
        let known = history.folders(&root_key)?;
        bail!(
            "Kein Verlauf für den Ordner '{folder}' (bekannt: {})",
            if known.is_empty() {
                "keine".to_string()
            } else {
                known.join(", ")
            }
        );
    }
    Ok(HistoryView {
        root_key,
        root_path,
        folder,
        points,
    })
}

/// Prüft das lokale LLM: Endpunkt (nur Loopback), Erreichbarkeit und Modell.
pub fn llm_check(config: &crate::config::LlmConfig) -> Result<(), String> {
    let client = crate::content::llm::LlmClient::new(config)?;
    client.check().map_err(|e| e.to_string())
}

/// Alle gescannten Wurzeln im Index.
pub fn index_roots() -> Result<Vec<RootInfo>> {
    Ok(Index::open(&paths::index_path()?)?.roots()?)
}

/// Entfernt eine Wurzel aus dem Index; `false`, wenn sie nicht darin war.
pub fn index_remove(path: &Path) -> Result<bool> {
    let mut index = Index::open(&paths::index_path()?)?;
    Ok(index.remove_root(&paths::dir_key(&normalize(path)))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_pruefung_lehnt_fremde_endpunkte_ab_ohne_netzwerkzugriff() {
        let config = crate::config::LlmConfig {
            endpoint: "http://example.com:11434".into(),
            ..Default::default()
        };
        let err = llm_check(&config).unwrap_err();
        assert!(err.contains("Loopback"), "{err}");
    }

    #[test]
    fn leerer_verlauf_nennt_den_naechsten_schritt() {
        let history = History::open_in_memory().unwrap();
        let err = history_in(&history, None, None, 10).unwrap_err();
        assert!(err.to_string().contains("Noch kein Verlauf"), "{err}");
    }

    #[test]
    fn unbekannter_pfad_im_verlauf_wird_benannt() {
        let history = History::open_in_memory().unwrap();
        let err = history_in(&history, Some(Path::new(r"Z:\gibt-es-nicht")), None, 10).unwrap_err();
        assert!(err.to_string().contains("Kein Verlauf für"), "{err}");
    }
}
