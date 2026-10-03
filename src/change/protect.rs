//! Schutzregeln: Pfade, die `plan` nie aufnimmt und `apply` immer verweigert.
//!
//! Systempfade sind fest eingebaut und lassen sich nicht abschalten. `AppData`, Ordnernamen
//! (`.git`, `node_modules`) und Projektordner mit Marker-Dateien gelten per Default, die
//! Config ergänzt weitere Pfade. Wie beim Scan hat die explizit angegebene Wurzel Vorrang:
//! Liegt sie selbst in einem konfigurierbar geschützten Bereich (z. B. unter `AppData`),
//! ist nur diese Regel ausgesetzt.

use std::collections::HashMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use crate::config::Config;
use crate::paths;

/// Ordner für Quarantäne und Journal innerhalb einer Wurzel.
pub const TOOL_DIR: &str = ".ordner-cleanup";

const PROTECTED_NAMES: [&str; 2] = [".git", "node_modules"];
const MARKER_FILES: [&str; 2] = ["cargo.toml", "package.json"];
const MARKER_EXTS: [&str; 2] = ["sln", "csproj"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Protection {
    System(String),
    UserPath(String),
    AppData,
    Name(String),
    ProjectMarker(String),
    ToolDir,
}

impl fmt::Display for Protection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::System(p) => write!(f, "Systempfad ({p})"),
            Self::UserPath(p) => write!(f, "in der Config geschützt ({p})"),
            Self::AppData => write!(f, "AppData"),
            Self::Name(n) => write!(f, "geschützter Ordnername ({n})"),
            Self::ProjectMarker(m) => write!(f, "Projektordner (Marker: {m})"),
            Self::ToolDir => write!(f, "{TOOL_DIR} (Quarantäne/Journal)"),
        }
    }
}

/// Absolute Pfade der eingebauten Regeln (aus Umgebungsvariablen, in Tests frei wählbar).
#[derive(Debug, Clone, Default)]
pub struct ProtectPaths {
    pub system: Vec<PathBuf>,
    pub appdata: Vec<PathBuf>,
}

impl ProtectPaths {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
        let mut system: Vec<PathBuf> = [
            "SystemRoot",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramData",
        ]
        .iter()
        .filter_map(|n| var(n))
        .collect();
        // Feste Rückfallwerte, falls Umgebungsvariablen fehlen oder manipuliert sind.
        system.extend(
            [
                r"C:\Windows",
                r"C:\Program Files",
                r"C:\Program Files (x86)",
                r"C:\ProgramData",
            ]
            .map(PathBuf::from),
        );
        let appdata = var("USERPROFILE")
            .map(|p| vec![p.join("AppData")])
            .unwrap_or_default();
        Self { system, appdata }
    }
}

/// Sucht in einem Ordner nach Projekt-Marker-Dateien (austauschbar für Tests).
pub trait MarkerProbe: Send + Sync {
    /// Name der gefundenen Marker-Datei.
    fn marker_in(&self, dir: &Path) -> Option<String>;
}

pub struct RealMarkerProbe;

impl MarkerProbe for RealMarkerProbe {
    fn marker_in(&self, dir: &Path) -> Option<String> {
        let entries = std::fs::read_dir(paths::extended(dir)).ok()?;
        entries.flatten().find_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            let ext = lower.rsplit_once('.').map(|(_, ext)| ext);
            (MARKER_FILES.contains(&lower.as_str())
                || ext.is_some_and(|e| MARKER_EXTS.contains(&e)))
            .then_some(name)
        })
    }
}

pub struct Protector {
    roots: Vec<String>,
    system: Vec<String>,
    user: Vec<String>,
    appdata: Vec<String>,
    probe: Box<dyn MarkerProbe>,
    marker_cache: Mutex<HashMap<String, Option<String>>>,
}

/// Entfernt `.` und `..` rein textlich; Extended-Präfixe verschwinden vorher, weil Windows
/// in `\\?\`-Pfaden keine `..` auflöst und `path_key` sie sonst unverändert ließe.
fn lexical_clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in Path::new(&paths::display(path)).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Löst Kurznamen und Links über den nächsten existierenden Vorfahren auf; der nicht
/// existierende Rest bleibt unverändert (manipulierte Pläne können auf Fehlendes zeigen).
fn resolve(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut current = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(paths::extended(current)) {
            let mut resolved = PathBuf::from(paths::display(&real));
            resolved.extend(tail.iter().rev());
            return resolved;
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Der Pfad in Rohform und aufgelöst (ohne Duplikate), jeweils als Schlüssel-Pfad.
fn variants(path: &Path) -> Vec<PathBuf> {
    let cleaned = lexical_clean(path);
    let resolved = resolve(&cleaned);
    if paths::path_key(&cleaned) == paths::path_key(&resolved) {
        vec![cleaned]
    } else {
        vec![cleaned, resolved]
    }
}

fn keys(path: &Path) -> Vec<String> {
    variants(path).iter().map(|p| paths::path_key(p)).collect()
}

impl Protector {
    pub fn new(root: &Path, config: &Config, protect: &ProtectPaths) -> Self {
        let roots = keys(root);
        let system = protect.system.iter().flat_map(|p| keys(p)).collect();
        // Konfigurierbare Regeln, die die Wurzel enthalten, sind ausgesetzt.
        let keep = |list: Vec<String>| -> Vec<String> {
            list.into_iter()
                .filter(|k| !roots.iter().any(|r| paths::is_under(r, k)))
                .collect()
        };
        let user = keep(
            config
                .protected_paths
                .iter()
                .flat_map(|p| keys(Path::new(p)))
                .collect(),
        );
        let appdata = keep(protect.appdata.iter().flat_map(|p| keys(p)).collect());
        Self {
            roots,
            system,
            user,
            appdata,
            probe: Box::new(RealMarkerProbe),
            marker_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_probe(mut self, probe: impl MarkerProbe + 'static) -> Self {
        self.probe = Box::new(probe);
        self
    }

    /// Grund, warum der Pfad nicht angefasst werden darf; `None`, wenn er frei ist.
    pub fn check(&self, path: &Path) -> Option<Protection> {
        variants(path).iter().find_map(|v| self.check_variant(v))
    }

    /// Ein `move` verändert Quelle und Ziel; beide unterliegen denselben Regeln.
    pub fn check_move(&self, source: &Path, target: &Path) -> Option<Protection> {
        self.check(source).or_else(|| self.check(target))
    }

    /// Grund, warum der **Inhalt** eines Ordners nicht angefasst werden darf. Zusätzlich zum
    /// Ordner selbst zählt ein Marker (z. B. `Cargo.toml`) im Ordner, der für Dateien darin
    /// gilt, aber nicht für den Ordnerpfad selbst; ein Probe-Pfad darunter erfasst ihn.
    pub fn check_inside(&self, dir: &Path) -> Option<Protection> {
        self.check(dir).or_else(|| self.check(&dir.join("_")))
    }

    fn check_variant(&self, path: &Path) -> Option<Protection> {
        let key = paths::path_key(path);
        if let Some(p) = self.system.iter().find(|p| paths::is_under(&key, p)) {
            return Some(Protection::System(p.clone()));
        }
        let parts = |s: &str| -> Vec<String> {
            s.split('\\')
                .filter(|p| !p.is_empty())
                .map(String::from)
                .collect()
        };
        if parts(&key).iter().any(|c| c == TOOL_DIR) {
            return Some(Protection::ToolDir);
        }
        if let Some(p) = self.user.iter().find(|p| paths::is_under(&key, p)) {
            return Some(Protection::UserPath(p.clone()));
        }
        if self.appdata.iter().any(|p| paths::is_under(&key, p)) {
            return Some(Protection::AppData);
        }
        // Ordnernamen und Marker zählen nur unterhalb der Wurzel; die Wurzel selbst ist
        // die ausdrückliche Wahl des Nutzers (außer sie trägt selbst einen Marker).
        let root = self.roots.iter().find(|r| paths::is_under(&key, r));
        let below = root.map_or(key.as_str(), |r| &key[r.len()..]);
        if let Some(name) = parts(below)
            .into_iter()
            .find(|c| PROTECTED_NAMES.contains(&c.as_str()))
        {
            return Some(Protection::Name(name));
        }
        for dir in path.ancestors().skip(1) {
            let dir_key = paths::path_key(dir);
            if root.is_some_and(|r| !paths::is_under(&dir_key, r)) {
                break;
            }
            if let Some(marker) = self.marker_in(&dir_key, dir) {
                return Some(Protection::ProjectMarker(marker));
            }
        }
        None
    }

    fn marker_in(&self, dir_key: &str, dir: &Path) -> Option<String> {
        let mut cache = self.marker_cache.lock().ok()?;
        cache
            .entry(dir_key.to_string())
            .or_insert_with(|| self.probe.marker_in(dir))
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::windows::short_path;
    use rstest::rstest;

    struct MapProbe(HashMap<String, String>);

    impl MapProbe {
        fn with(dir: &str, marker: &str) -> Self {
            Self(HashMap::from([(
                paths::path_key(Path::new(dir)),
                marker.to_string(),
            )]))
        }
    }

    impl MarkerProbe for MapProbe {
        fn marker_in(&self, dir: &Path) -> Option<String> {
            self.0.get(&paths::path_key(dir)).cloned()
        }
    }

    fn protect_paths() -> ProtectPaths {
        ProtectPaths {
            system: [
                r"C:\Windows",
                r"C:\Program Files",
                r"C:\Program Files (x86)",
                r"C:\ProgramData",
            ]
            .map(PathBuf::from)
            .to_vec(),
            appdata: vec![PathBuf::from(r"C:\Users\me\AppData")],
        }
    }

    fn protector(root: &str) -> Protector {
        Protector::new(Path::new(root), &Config::default(), &protect_paths())
            .with_probe(MapProbe(HashMap::new()))
    }

    #[rstest]
    #[case(r"C:\Windows\System32\kernel32.dll")]
    #[case(r"c:\windows\notepad.exe")]
    #[case(r"\\?\C:\Windows\x.dll")]
    #[case(r"C:\Program Files\App\a.exe")]
    #[case(r"C:\Program Files (x86)\App\a.exe")]
    #[case(r"C:\ProgramData\x.dat")]
    #[case(r"C:\Windows")]
    #[case(r"C:\Users\..\Windows\x.dll")]
    #[case(r"C:\Users\me\.\..\..\Windows\x.dll")]
    #[case(r"C:/Windows/x.dll")]
    fn systempfade_sind_geschuetzt(#[case] path: &str) {
        let p = protector(r"D:\Daten");
        assert!(
            matches!(p.check(Path::new(path)), Some(Protection::System(_))),
            "{path}"
        );
    }

    #[test]
    fn ziel_eines_moves_wird_wie_die_quelle_geprueft() {
        let p = protector(r"D:\Daten");
        let free = |s: &str| Path::new(s).to_path_buf();
        assert_eq!(
            p.check_move(&free(r"D:\Daten\a"), &free(r"D:\Daten\_Archiv\2020\a")),
            None
        );
        // Quelle geschützt
        assert!(matches!(
            p.check_move(
                &free(r"D:\Daten\proj\node_modules"),
                &free(r"D:\Daten\_Archiv\2020\x")
            ),
            Some(Protection::Name(_))
        ));
        // Ziel geschützt (manipulierter Plan mit Ziel in einem Systempfad)
        assert!(matches!(
            p.check_move(&free(r"D:\Daten\a"), &free(r"C:\Windows\a")),
            Some(Protection::System(_))
        ));
    }

    #[test]
    fn inhalt_eines_ordners_wird_mit_dem_ordner_geprueft() {
        let p = protector(r"D:\Daten");
        assert_eq!(p.check_inside(Path::new(r"D:\Daten\alt\sub")), None);
        assert!(matches!(
            p.check_inside(Path::new(r"D:\Daten\alt\.git")),
            Some(Protection::Name(_))
        ));
        assert!(matches!(
            p.check_inside(Path::new(r"D:\Daten\alt\node_modules")),
            Some(Protection::Name(_))
        ));
    }

    #[test]
    fn inhalt_eines_ordners_mit_projektmarker_ist_geschuetzt() {
        let p = Protector::new(Path::new(r"D:\Daten"), &Config::default(), &protect_paths())
            .with_probe(MapProbe::with(r"D:\Daten\proj", "Cargo.toml"));
        // Der Ordner selbst trägt den Marker: seine Inhalte sind geschützt, ein Probe-Pfad
        // darunter zeigt das, auch wenn der Ordner leer ist oder keine Unterordner hat.
        assert!(matches!(
            p.check_inside(Path::new(r"D:\Daten\proj")),
            Some(Protection::ProjectMarker(_))
        ));
        assert_eq!(p.check_inside(Path::new(r"D:\Daten\andere")), None);
    }

    #[test]
    fn aehnlicher_name_ist_kein_systempfad() {
        let p = protector(r"D:\Daten");
        assert_eq!(p.check(Path::new(r"C:\Windows2\x.txt")), None);
        assert_eq!(p.check(Path::new(r"C:\Program Files Backup\x.txt")), None);
    }

    #[test]
    fn systempfade_gelten_auch_fuer_systemwurzel() {
        // Auch wenn jemand die Wurzel unter C:\Windows legt: fest geschützt.
        let p = protector(r"C:\Windows\Temp");
        assert!(matches!(
            p.check(Path::new(r"C:\Windows\Temp\x.txt")),
            Some(Protection::System(_))
        ));
    }

    #[test]
    fn kurzname_eines_systempfads_wird_aufgeloest() {
        let Some(short) = short_path(Path::new(r"C:\Program Files")) else {
            eprintln!("kein 8.3-Kurzname für Program Files, Test übersprungen");
            return;
        };
        let p = protector(r"D:\Daten");
        let path = short.join("App").join("a.exe");
        assert!(
            matches!(p.check(&path), Some(Protection::System(_))),
            "{}",
            path.display()
        );
    }

    #[test]
    fn config_kann_systempfade_nicht_abschalten() {
        let config = Config::parse("protected_paths = [\"D:\\\\Wichtig\"]").unwrap();
        let p = Protector::new(Path::new(r"D:\Daten"), &config, &protect_paths())
            .with_probe(MapProbe(HashMap::new()));
        assert!(matches!(
            p.check(Path::new(r"C:\Windows\x.dll")),
            Some(Protection::System(_))
        ));
        assert!(matches!(
            p.check(Path::new(r"D:\Wichtig\a.txt")),
            Some(Protection::UserPath(_))
        ));
        assert_eq!(p.check(Path::new(r"D:\Daten\a.txt")), None);
    }

    #[test]
    fn config_pfad_ist_gross_klein_und_praefix_robust() {
        let config = Config::parse("protected_paths = [\"d:\\\\wichtig\"]").unwrap();
        let p = Protector::new(Path::new(r"D:\Daten"), &config, &protect_paths())
            .with_probe(MapProbe(HashMap::new()));
        assert!(p.check(Path::new(r"D:\WICHTIG\a.txt")).is_some());
        assert_eq!(p.check(Path::new(r"D:\Wichtig2\a.txt")), None);
    }

    #[test]
    fn appdata_ist_standardmaessig_geschuetzt() {
        let p = protector(r"C:\Users\me");
        assert_eq!(
            p.check(Path::new(r"C:\Users\me\AppData\Roaming\x.cfg")),
            Some(Protection::AppData)
        );
        assert_eq!(p.check(Path::new(r"C:\Users\me\Dokumente\x.txt")), None);
    }

    #[test]
    fn liegt_die_wurzel_in_appdata_ist_nur_diese_regel_ausgesetzt() {
        let p = protector(r"C:\Users\me\AppData\Local\Temp\test");
        assert_eq!(
            p.check(Path::new(r"C:\Users\me\AppData\Local\Temp\test\a.txt")),
            None
        );
        // Systempfade bleiben trotzdem geschützt.
        assert!(p.check(Path::new(r"C:\Windows\x.dll")).is_some());
    }

    #[rstest]
    #[case(r"D:\Daten\proj\.git\config", ".git")]
    #[case(r"D:\Daten\proj\.GIT\config", ".git")]
    #[case(r"D:\Daten\web\node_modules\a\b.js", "node_modules")]
    #[case(r"D:\Daten\.git", ".git")]
    fn geschuetzte_ordnernamen(#[case] path: &str, #[case] name: &str) {
        let p = protector(r"D:\Daten");
        assert_eq!(
            p.check(Path::new(path)),
            Some(Protection::Name(name.to_string()))
        );
    }

    #[test]
    fn ordnername_in_der_wurzel_selbst_zaehlt_nicht() {
        let p = protector(r"D:\Daten\node_modules");
        assert_eq!(p.check(Path::new(r"D:\Daten\node_modules\a.txt")), None);
    }

    #[test]
    fn projektmarker_schuetzen_den_unterbaum() {
        let p = Protector::new(Path::new(r"D:\Daten"), &Config::default(), &protect_paths())
            .with_probe(MapProbe::with(r"D:\Daten\proj", "Cargo.toml"));
        assert_eq!(
            p.check(Path::new(r"D:\Daten\proj\src\a.rs")),
            Some(Protection::ProjectMarker("Cargo.toml".into()))
        );
        assert_eq!(p.check(Path::new(r"D:\Daten\andere\a.rs")), None);
    }

    #[test]
    fn marker_oberhalb_der_wurzel_zaehlt_nicht_aber_in_der_wurzel_schon() {
        let above = Protector::new(
            Path::new(r"D:\Daten\proj\docs"),
            &Config::default(),
            &protect_paths(),
        )
        .with_probe(MapProbe::with(r"D:\Daten\proj", "package.json"));
        assert_eq!(above.check(Path::new(r"D:\Daten\proj\docs\a.txt")), None);

        let in_root = Protector::new(
            Path::new(r"D:\Daten\proj"),
            &Config::default(),
            &protect_paths(),
        )
        .with_probe(MapProbe::with(r"D:\Daten\proj", "package.json"));
        assert!(in_root.check(Path::new(r"D:\Daten\proj\a.txt")).is_some());
    }

    #[test]
    fn tool_ordner_ist_immer_geschuetzt() {
        let p = protector(r"D:\Daten");
        assert_eq!(
            p.check(Path::new(r"D:\Daten\.ordner-cleanup\quarantine\r1\a.txt")),
            Some(Protection::ToolDir)
        );
    }

    #[test]
    fn echte_marker_dateien_werden_erkannt() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("proj");
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(proj.join("App.CSPROJ"), "").unwrap();
        std::fs::write(proj.join("src").join("a.cs"), "").unwrap();
        std::fs::write(plain.join("a.txt"), "").unwrap();

        let p = Protector::new(dir.path(), &Config::default(), &protect_paths());
        assert_eq!(
            p.check(&proj.join("src").join("a.cs")),
            Some(Protection::ProjectMarker("App.CSPROJ".into()))
        );
        assert_eq!(p.check(&plain.join("a.txt")), None);
    }

    #[test]
    fn kurzname_der_wurzel_umgeht_den_schutz_nicht() {
        let dir = tempfile::Builder::new()
            .prefix("langer-ordnername-")
            .tempdir()
            .unwrap();
        let secret = dir.path().join("wichtig");
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::write(secret.join("a.txt"), "x").unwrap();
        let Some(short) = short_path(&secret) else {
            eprintln!("kein 8.3-Kurzname verfügbar, Test übersprungen");
            return;
        };
        let config = Config {
            protected_paths: vec![secret.to_string_lossy().into_owned()],
            ..Config::default()
        };
        let p = Protector::new(dir.path(), &config, &protect_paths());
        assert!(p.check(&short.join("a.txt")).is_some());
    }
}
