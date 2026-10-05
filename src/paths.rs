use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Umgebungsvariable, die Index- und Config-Verzeichnis überschreibt (Tests).
pub const HOME_OVERRIDE_ENV: &str = "ORDNER_CLEANUP_HOME";

const EXTENDED: &str = r"\\?\";
const EXTENDED_UNC: &str = r"\\?\UNC\";

/// Pfad mit `\` als Trenner und ohne Extended-Präfix (UNC bleibt `\\server\share`).
fn plain(path: &Path) -> String {
    let s = path.to_string_lossy().replace('/', "\\");
    if let Some(rest) = s.strip_prefix(EXTENDED_UNC) {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(EXTENDED) {
        rest.to_string()
    } else {
        s
    }
}

fn is_drive_root(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 3 && b[1] == b':' && b[2] == b'\\'
}

/// Normalisierter Schlüssel einer Datei: Extended-Präfix entfernt, `\` als Trenner,
/// Laufwerksbuchstabe groß, Rest kleingeschrieben, kein abschließendes `\`.
pub fn path_key(path: &Path) -> String {
    let mut s = plain(path);
    while s.ends_with('\\') && !is_drive_root(&s) {
        s.pop();
    }
    let mut key = s.to_lowercase();
    if key.as_bytes().get(1) == Some(&b':') {
        key[..1].make_ascii_uppercase();
    }
    key
}

/// Wie [`path_key`], aber mit abschließendem `\` (Ordner).
pub fn dir_key(path: &Path) -> String {
    let mut key = path_key(path);
    if !key.ends_with('\\') {
        key.push('\\');
    }
    key
}

/// Halboffener Bereich `[start, end)` aller Schlüssel unterhalb eines Ordner-Schlüssels.
pub fn prefix_range(dir_key: &str) -> (String, String) {
    (dir_key.to_string(), format!("{dir_key}{}", char::MAX))
}

/// `key` liegt auf oder unterhalb von `prefix_key` (beide aus [`path_key`]).
pub fn is_under(key: &str, prefix_key: &str) -> bool {
    key == prefix_key
        || key
            .strip_prefix(prefix_key.trim_end_matches('\\'))
            .is_some_and(|rest| rest.starts_with('\\'))
}

/// Pfad relativ zur Wurzel (Groß-/Kleinschreibung der Wurzel egal, Schreibweise des Rests bleibt).
/// `None`, wenn `path` nicht unterhalb von `root` liegt; für `path == root` ein leerer Pfad.
pub fn relative_to(root: &Path, path: &Path) -> Option<PathBuf> {
    let (root, path) = (plain(root), plain(path));
    let mut root_parts = root.split('\\').filter(|p| !p.is_empty());
    let mut path_parts = path.split('\\').filter(|p| !p.is_empty());
    for r in root_parts.by_ref() {
        if path_parts.next()?.to_lowercase() != r.to_lowercase() {
            return None;
        }
    }
    Some(path_parts.collect::<PathBuf>())
}

/// Pfad mit Extended-Length-Präfix für das Dateisystem (relative Pfade bleiben unverändert).
pub fn extended(path: &Path) -> PathBuf {
    let s = path.to_string_lossy().replace('/', "\\");
    if s.starts_with(EXTENDED) {
        PathBuf::from(s)
    } else if let Some(rest) = s.strip_prefix(r"\\") {
        PathBuf::from(format!("{EXTENDED_UNC}{rest}"))
    } else if s.as_bytes().get(1) == Some(&b':') {
        PathBuf::from(format!("{EXTENDED}{s}"))
    } else {
        PathBuf::from(s)
    }
}

/// Darstellbarer Pfad (ungültiges UTF-16 wird durch Ersatzzeichen ersetzt, ohne Extended-Präfix).
pub fn display(path: &Path) -> String {
    plain(path)
}

fn app_dir(var: &str, override_sub: &str) -> Option<PathBuf> {
    if let Some(home) = std::env::var_os(HOME_OVERRIDE_ENV) {
        return Some(PathBuf::from(home).join(override_sub));
    }
    std::env::var_os(var).map(|base| PathBuf::from(base).join("ordner-cleanup"))
}

/// Verzeichnis für den Index (`%LOCALAPPDATA%\ordner-cleanup`).
pub fn data_dir() -> Option<PathBuf> {
    app_dir("LOCALAPPDATA", "data")
}

/// Verzeichnis für die Config (`%APPDATA%\ordner-cleanup`).
pub fn config_dir() -> Option<PathBuf> {
    app_dir("APPDATA", "config")
}

fn data_file(name: &str) -> Result<PathBuf> {
    let dir = data_dir().context("Datenordner (%LOCALAPPDATA%) nicht ermittelbar")?;
    Ok(dir.join(name))
}

/// Index der gescannten Wurzeln (`index.db`).
pub fn index_path() -> Result<PathBuf> {
    data_file("index.db")
}

/// Register der `apply`-Läufe (`runs.jsonl`).
pub fn registry_path() -> Result<PathBuf> {
    data_file("runs.jsonl")
}

/// Verlauf der Kennzahlen (`history.db`), unabhängig vom Index.
pub fn history_path() -> Result<PathBuf> {
    data_file("history.db")
}

/// Sperrdatei, die `scan` und `run` exklusiv halten.
pub fn scan_lock_path() -> Result<PathBuf> {
    data_file("scan.lock")
}

/// Register der angelegten Aufgaben (`schedules.json`).
pub fn schedules_path() -> Result<PathBuf> {
    data_file("schedules.json")
}

/// Ablage der GUI-Pläne (`plans\_gui`), überschreibbar mit `[gui] plans_dir`. Der Unterstrich ist
/// in Profilnamen verboten, es gibt also keine Kollision mit `plans\<profil>`.
pub fn gui_plans_dir(override_dir: Option<&str>) -> Result<PathBuf> {
    gui_plans_dir_in(data_dir().as_deref(), config_dir().as_deref(), override_dir)
}

fn gui_plans_dir_in(
    data: Option<&Path>,
    config: Option<&Path>,
    override_dir: Option<&str>,
) -> Result<PathBuf> {
    match override_dir.map(str::trim).filter(|d| !d.is_empty()) {
        Some(dir) => Ok(PathBuf::from(crate::config::resolve_in(dir, config))),
        None => Ok(data
            .context("Datenordner (%LOCALAPPDATA%) nicht ermittelbar")?
            .join("plans")
            .join("_gui")),
    }
}

/// Zustand der GUI (Fenster, letzte Auswahl) unter `%APPDATA%\ordner-cleanup\gui`.
pub fn gui_state_dir() -> Result<PathBuf> {
    gui_state_dir_in(config_dir().as_deref())
}

fn gui_state_dir_in(config: Option<&Path>) -> Result<PathBuf> {
    Ok(config
        .context("Config-Ordner (%APPDATA%) nicht ermittelbar")?
        .join("gui"))
}

/// Fehlerprotokoll der GUI (die GUI-exe hat keine Konsole).
pub fn gui_error_log() -> Result<PathBuf> {
    gui_error_log_in(data_dir().as_deref())
}

fn gui_error_log_in(data: Option<&Path>) -> Result<PathBuf> {
    Ok(data
        .context("Datenordner (%LOCALAPPDATA%) nicht ermittelbar")?
        .join("gui-errors.log"))
}

/// Berichte eines Profils (`reports\<profil>`).
pub fn reports_dir(profile: &str) -> Result<PathBuf> {
    Ok(data_file("reports")?.join(profile))
}

/// Pläne eines Profils (`plans\<profil>`).
pub fn plans_dir(profile: &str) -> Result<PathBuf> {
    Ok(data_file("plans")?.join(profile))
}

/// Lauf-Protokoll eines Profils (`runs\<profil>.jsonl`).
pub fn runs_log(profile: &str) -> Result<PathBuf> {
    Ok(data_file("runs")?.join(format!("{profile}.jsonl")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(r"c:\Users\Foo\Bar.TXT", r"C:\users\foo\bar.txt")]
    #[case(r"C:/Users/Foo", r"C:\users\foo")]
    #[case(r"\\?\C:\Users\Foo", r"C:\users\foo")]
    #[case(r"\\?\UNC\Server\Share\Dir", r"\\server\share\dir")]
    #[case(r"\\Server\Share\Dir", r"\\server\share\dir")]
    #[case(r"C:\Ärger\Übung.doc", r"C:\ärger\übung.doc")]
    #[case(r"C:\Users\Foo\", r"C:\users\foo")]
    fn path_key_normalisiert(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(path_key(Path::new(input)), expected);
    }

    #[test]
    fn dir_key_endet_auf_backslash() {
        assert_eq!(dir_key(Path::new(r"C:\Users\Foo")), r"C:\users\foo\");
        assert_eq!(dir_key(Path::new(r"C:\Users\Foo\")), r"C:\users\foo\");
        assert_eq!(dir_key(Path::new(r"C:\")), r"C:\");
    }

    #[test]
    fn prefix_range_enthaelt_nur_unterbaum() {
        let (lo, hi) = prefix_range(&dir_key(Path::new(r"C:\Daten")));
        let inside = path_key(Path::new(r"C:\Daten\a\b.txt"));
        let sibling = path_key(Path::new(r"C:\Daten2\b.txt"));
        assert!(inside >= lo && inside < hi);
        assert!(!(sibling >= lo && sibling < hi));
    }

    #[rstest]
    #[case(r"C:\Daten\x", r"\\?\C:\Daten\x")]
    #[case(r"\\?\C:\Daten\x", r"\\?\C:\Daten\x")]
    #[case(r"\\Server\Share\x", r"\\?\UNC\Server\Share\x")]
    #[case(r"C:/Daten/x", r"\\?\C:\Daten\x")]
    fn extended_setzt_praefix(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(extended(Path::new(input)), PathBuf::from(expected));
    }

    #[test]
    fn display_entfernt_praefix() {
        assert_eq!(display(Path::new(r"\\?\C:\Daten\x")), r"C:\Daten\x");
        assert_eq!(display(Path::new(r"\\?\UNC\S\Sh")), r"\\S\Sh");
    }

    #[rstest]
    #[case(r"C:\daten", r"C:\daten", true)]
    #[case(r"C:\daten\a\b", r"C:\daten", true)]
    #[case(r"C:\daten2\b", r"C:\daten", false)]
    #[case(r"C:\dat", r"C:\daten", false)]
    fn is_under_prueft_ordnergrenzen(
        #[case] key: &str,
        #[case] prefix: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(is_under(key, prefix), expected);
    }

    #[rstest]
    #[case(r"C:\Daten", r"C:\Daten\Sub\Datei.TXT", Some(r"Sub\Datei.TXT"))]
    #[case(r"c:\daten", r"C:\DATEN\Sub\x", Some(r"Sub\x"))]
    #[case(r"\\?\C:\Daten", r"C:\Daten\x", Some("x"))]
    #[case(r"C:\Daten", r"C:\Daten", Some(""))]
    #[case(r"C:\Daten", r"C:\Daten2\x", None)]
    #[case(r"C:\Daten", r"D:\Daten\x", None)]
    fn relative_to_schneidet_wurzel_ab(
        #[case] root: &str,
        #[case] path: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(
            relative_to(Path::new(root), Path::new(path)),
            expected.map(PathBuf::from)
        );
    }

    #[test]
    fn gui_plaene_liegen_standardmaessig_unter_plans_gui() {
        let data = Path::new(r"C:\Data");
        assert_eq!(
            gui_plans_dir_in(Some(data), None, None).unwrap(),
            PathBuf::from(r"C:\Data\plans\_gui")
        );
        // `_` ist in Profilnamen verboten: keine Kollision mit plans\<profil>
        assert!(!crate::config::valid_profile_name("_gui"));
    }

    #[test]
    fn gui_plaene_override_absolut_und_relativ_zur_config() {
        let data = Path::new(r"C:\Data");
        let cfg = Path::new(r"C:\Cfg");
        assert_eq!(
            gui_plans_dir_in(Some(data), Some(cfg), Some(r"D:\Pläne")).unwrap(),
            PathBuf::from(r"D:\Pläne")
        );
        assert_eq!(
            gui_plans_dir_in(Some(data), Some(cfg), Some("pl")).unwrap(),
            PathBuf::from(r"C:\Cfg\pl")
        );
        assert!(gui_plans_dir_in(None, None, None).is_err());
    }

    #[test]
    fn gui_state_und_fehlerlog_liegen_im_richtigen_ordner() {
        assert_eq!(
            gui_state_dir_in(Some(Path::new(r"C:\Cfg"))).unwrap(),
            PathBuf::from(r"C:\Cfg\gui")
        );
        assert_eq!(
            gui_error_log_in(Some(Path::new(r"C:\Data"))).unwrap(),
            PathBuf::from(r"C:\Data\gui-errors.log")
        );
    }

    #[test]
    fn home_override_gilt_fuer_beide_verzeichnisse() {
        // Env-Variable wird nur in diesem Test gesetzt.
        std::env::set_var(HOME_OVERRIDE_ENV, r"C:\Temp\oc-test");
        assert_eq!(data_dir().unwrap(), PathBuf::from(r"C:\Temp\oc-test\data"));
        assert_eq!(
            config_dir().unwrap(),
            PathBuf::from(r"C:\Temp\oc-test\config")
        );
        std::env::remove_var(HOME_OVERRIDE_ENV);
    }
}
