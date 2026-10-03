//! Orte von Quarantäne und Journal sowie die Bildung der Ziel-Pfade.
//!
//! Layout je Wurzel: `<wurzel>\.ordner-cleanup\journal\<run-id>.jsonl` und
//! `<wurzel>\.ordner-cleanup\quarantine\<run-id>\<relativer Pfad>`.

use std::io;
use std::path::{Component, Path, PathBuf};

use super::fsops::FsOps;
use super::protect::TOOL_DIR;
use super::{RunId, SkipReason};
use crate::paths;

/// Obergrenze für Ziel-Pfade (`\\?\` erlaubt rund 32 767 Zeichen; Rest ist Reserve für
/// Suffixe wie ` (2)`).
pub const MAX_TARGET_LEN: usize = 32_000;

pub fn tool_dir(root: &Path) -> PathBuf {
    root.join(TOOL_DIR)
}

pub fn journal_dir(root: &Path) -> PathBuf {
    tool_dir(root).join("journal")
}

pub fn journal_path(root: &Path, run: &RunId) -> PathBuf {
    journal_dir(root).join(format!("{run}.jsonl"))
}

pub fn quarantine_root(root: &Path) -> PathBuf {
    tool_dir(root).join("quarantine")
}

pub fn run_dir(root: &Path, run: &RunId) -> PathBuf {
    quarantine_root(root).join(run.as_str())
}

/// Ziel einer Quelldatei in der Quarantäne; der relative Pfad bleibt erhalten.
pub fn target_for(root: &Path, run: &RunId, source: &Path) -> Result<PathBuf, SkipReason> {
    let relative = paths::relative_to(root, source)
        .filter(|r| r.components().next().is_some())
        .ok_or(SkipReason::OutsideRoot)?;
    // `..` würde aus der Quarantäne ausbrechen; verwandte Pfade kommen nur aus manipulierten Plänen.
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(SkipReason::OutsideRoot);
    }
    let target = run_dir(root, run).join(relative);
    if paths::display(&target).chars().count() > MAX_TARGET_LEN {
        return Err(SkipReason::TooLong);
    }
    Ok(target)
}

/// Prüft schon beim Planen, ob das Ziel später passen würde.
pub fn fits(root: &Path, source: &Path) -> Result<(), SkipReason> {
    let probe = RunId::parse("00000000-000000-0000").map_err(|_| SkipReason::OutsideRoot)?;
    target_for(root, &probe, source).map(|_| ())
}

/// Liefert `target` oder, wenn es belegt ist, `name (2).ext`, `name (3).ext`, …
pub fn unique_target(fs: &dyn FsOps, target: PathBuf) -> Option<PathBuf> {
    if !fs.exists(&target) {
        return Some(target);
    }
    let stem = target.file_stem()?.to_string_lossy().into_owned();
    let ext = target
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..10_000)
        .map(|n| target.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|candidate| !fs.exists(candidate))
}

/// Die Tool-Ordner dürfen keine Links sein, sonst könnte die Quarantäne umgeleitet werden.
pub fn ensure_plain_dirs(fs: &dyn FsOps, root: &Path, run: &RunId) -> io::Result<()> {
    for dir in [
        tool_dir(root),
        quarantine_root(root),
        run_dir(root, run),
        journal_dir(root),
    ] {
        if let Ok(meta) = fs.metadata(&dir) {
            if meta.is_reparse_point() || !meta.is_dir {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "{} ist ein Link oder kein Ordner; Quarantäne wird nicht angelegt",
                        paths::display(&dir)
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::fsops::RealFs;
    use rstest::rstest;

    fn run() -> RunId {
        RunId::parse("20261003-120000-ab12").unwrap()
    }

    #[test]
    fn ziel_behaelt_den_relativen_pfad() {
        let target = target_for(
            Path::new(r"D:\Daten"),
            &run(),
            Path::new(r"D:\Daten\Sub\Ordner\Datei.TXT"),
        )
        .unwrap();
        assert_eq!(
            target,
            PathBuf::from(
                r"D:\Daten\.ordner-cleanup\quarantine\20261003-120000-ab12\Sub\Ordner\Datei.TXT"
            )
        );
    }

    #[rstest]
    #[case(r"D:\Anders\x.txt")]
    #[case(r"D:\Daten")]
    #[case(r"D:\Daten\..\Anders\x.txt")]
    #[case(r"D:\Daten\sub\..\..\x.txt")]
    #[case(r"D:\Daten\.\x.txt")]
    fn pfade_ausserhalb_der_wurzel_sind_abgelehnt(#[case] source: &str) {
        assert_eq!(
            target_for(Path::new(r"D:\Daten"), &run(), Path::new(source)),
            Err(SkipReason::OutsideRoot)
        );
    }

    #[test]
    fn zu_lange_ziele_werden_abgelehnt() {
        let long = format!(r"D:\Daten\{}\x.txt", "a".repeat(MAX_TARGET_LEN));
        assert_eq!(
            target_for(Path::new(r"D:\Daten"), &run(), Path::new(&long)),
            Err(SkipReason::TooLong)
        );
        assert_eq!(
            fits(Path::new(r"D:\Daten"), Path::new(&long)),
            Err(SkipReason::TooLong)
        );
        assert!(fits(Path::new(r"D:\Daten"), Path::new(r"D:\Daten\x.txt")).is_ok());
    }

    #[test]
    fn pfade_von_journal_und_quarantaene() {
        let root = Path::new(r"D:\Daten");
        assert_eq!(
            journal_path(root, &run()),
            PathBuf::from(r"D:\Daten\.ordner-cleanup\journal\20261003-120000-ab12.jsonl")
        );
        assert_eq!(
            run_dir(root, &run()),
            PathBuf::from(r"D:\Daten\.ordner-cleanup\quarantine\20261003-120000-ab12")
        );
    }

    #[test]
    fn belegtes_ziel_bekommt_eindeutigen_namen_und_wird_nie_ueberschrieben() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.txt");
        assert_eq!(unique_target(&RealFs, target.clone()), Some(target.clone()));
        std::fs::write(&target, "x").unwrap();
        let second = unique_target(&RealFs, target.clone()).unwrap();
        assert_eq!(second, dir.path().join("a (2).txt"));
        std::fs::write(&second, "y").unwrap();
        assert_eq!(
            unique_target(&RealFs, target).unwrap(),
            dir.path().join("a (3).txt")
        );
        // Ohne Endung
        let plain = dir.path().join("README");
        std::fs::write(&plain, "x").unwrap();
        assert_eq!(
            unique_target(&RealFs, plain).unwrap(),
            dir.path().join("README (2)")
        );
    }

    #[test]
    fn tool_ordner_als_junction_wird_abgelehnt() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let link = dir.path().join(TOOL_DIR);
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(elsewhere.path())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("Junction nicht erzeugbar, Test übersprungen");
            return;
        }
        assert!(ensure_plain_dirs(&RealFs, dir.path(), &run()).is_err());
    }

    #[test]
    fn normale_oder_fehlende_tool_ordner_sind_ok() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ensure_plain_dirs(&RealFs, dir.path(), &run()).is_ok());
        std::fs::create_dir_all(run_dir(dir.path(), &run())).unwrap();
        assert!(ensure_plain_dirs(&RealFs, dir.path(), &run()).is_ok());
    }
}
