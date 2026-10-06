//! OneDrive-Obergrenze: Ein großer Plan unter einer OneDrive-Wurzel würde einen großen
//! Sync auslösen. `apply` verweigert ihn ohne `--allow-large`. Gilt für alle Pläne;
//! `remove-dir` zählt nicht (kein Inhalt wird bewegt).

use std::path::{Path, PathBuf};

use bytesize::ByteSize;

use super::plan::{ActionType, Plan};
use crate::config::Config;
use crate::paths;

/// Der Pfad liegt auf oder unter einer der OneDrive-Wurzeln.
pub fn under_onedrive(path: &Path, onedrive_roots: &[PathBuf]) -> bool {
    let key = paths::path_key(path);
    onedrive_roots
        .iter()
        .any(|r| paths::is_under(&key, &paths::path_key(r)))
}

/// Anzahl Dateien und Bytes, die der Plan bewegt (Quarantäne und `move`; Ordner-Moves zählen
/// mit ihrer Dateianzahl und Gesamtgröße).
pub fn move_volume(plan: &Plan) -> (u64, u64) {
    plan.actions
        .iter()
        .filter(|a| a.action != ActionType::RemoveDir)
        .fold((0, 0), |(files, bytes), a| {
            let count = if a.is_dir { a.files.unwrap_or(0) } else { 1 };
            (files + count, bytes + a.size)
        })
}

/// Meldung, wenn der Plan unter OneDrive die Obergrenze aus der Config überschreitet.
pub fn exceeds(plan: &Plan, onedrive_roots: &[PathBuf], config: &Config) -> Option<String> {
    if !under_onedrive(Path::new(&plan.root), onedrive_roots) {
        return None;
    }
    let (files, bytes) = move_volume(plan);
    let (max_files, max_bytes) = (
        config.onedrive_max_move_files,
        config.onedrive_max_move_bytes_value(),
    );
    (files > max_files || bytes > max_bytes).then(|| {
        format!(
            "Der Plan bewegt {files} Dateien ({}) unter OneDrive; die Obergrenze liegt bei \
             {max_files} Dateien oder {}. Das löst einen großen Sync aus. Mit `--allow-large` \
             trotzdem ausführen.",
            ByteSize::b(bytes),
            ByteSize::b(max_bytes)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::plan::{PlanKind, PlannedAction, PLAN_VERSION};

    fn action(id: u32, ty: ActionType, size: u64) -> PlannedAction {
        PlannedAction {
            id,
            action: ty,
            path: format!(r"D:\Daten\datei{id}.txt"),
            size,
            mtime_ticks: 0,
            mtime: String::new(),
            hash: None,
            keep: None,
            keep_hash: None,
            reason: "test".into(),
            target: None,
            is_dir: false,
            files: None,
            rule: None,
        }
    }

    fn plan(actions: Vec<PlannedAction>) -> Plan {
        Plan {
            version: PLAN_VERSION,
            created: "t".into(),
            kind: PlanKind::Junk,
            root: r"C:\Users\me\OneDrive\Daten".into(),
            keep_strategy: None,
            params: Default::default(),
            protected_paths: Vec::new(),
            actions,
            skipped: vec![],
        }
    }

    fn many(n: u32, size: u64) -> Plan {
        plan(
            (1..=n)
                .map(|i| action(i, ActionType::Quarantine, size))
                .collect(),
        )
    }

    fn onedrive() -> Vec<PathBuf> {
        vec![PathBuf::from(r"C:\Users\me\OneDrive")]
    }

    #[test]
    fn volumen_zaehlt_dateien_und_ordner_moves_aber_keine_remove_dir() {
        let mut dir_move = action(3, ActionType::Move, 700);
        dir_move.is_dir = true;
        dir_move.files = Some(40);
        let p = plan(vec![
            action(1, ActionType::Quarantine, 100),
            action(2, ActionType::Move, 200),
            dir_move,
            action(4, ActionType::RemoveDir, 0),
            action(5, ActionType::RemoveDir, 999_999),
        ]);
        assert_eq!(move_volume(&p), (42, 1000));
        assert_eq!(move_volume(&plan(vec![])), (0, 0));
    }

    #[test]
    fn genau_an_der_grenze_ist_erlaubt_darueber_nicht() {
        let c = Config::default();
        assert!(exceeds(&many(1000, 1), &onedrive(), &c).is_none());
        let msg = exceeds(&many(1001, 1), &onedrive(), &c).unwrap();
        assert!(msg.contains("1001"), "{msg}");
        assert!(msg.contains("--allow-large"), "{msg}");

        assert!(exceeds(&many(1, 5_000_000_000), &onedrive(), &c).is_none());
        let msg = exceeds(&many(1, 5_000_000_001), &onedrive(), &c).unwrap();
        assert!(msg.contains("--allow-large"), "{msg}");
    }

    #[test]
    fn nur_unter_einer_onedrive_wurzel() {
        let c = Config::default();
        let mut p = many(5000, 1);
        assert!(exceeds(&p, &onedrive(), &c).is_some());
        p.root = r"D:\Daten".into();
        assert!(exceeds(&p, &onedrive(), &c).is_none());
        assert!(exceeds(&many(5000, 1), &[], &c).is_none());
        p.root = r"C:\Users\me\OneDrive2\x".into();
        assert!(exceeds(&p, &onedrive(), &c).is_none());
    }

    #[test]
    fn remove_dir_loest_die_obergrenze_nie_aus() {
        let p = plan(
            (1..=5000)
                .map(|i| action(i, ActionType::RemoveDir, 0))
                .collect(),
        );
        assert!(exceeds(&p, &onedrive(), &Config::default()).is_none());
    }

    #[test]
    fn ordner_move_zaehlt_mit_dateianzahl() {
        let mut dir_move = action(1, ActionType::Move, 10);
        dir_move.is_dir = true;
        dir_move.files = Some(1500);
        assert!(exceeds(&plan(vec![dir_move]), &onedrive(), &Config::default()).is_some());
    }

    #[test]
    fn grenzen_kommen_aus_der_config() {
        let c = Config::parse("onedrive_max_move_files = 10\nonedrive_max_move_bytes = \"1KB\"")
            .unwrap();
        assert!(exceeds(&many(10, 1), &onedrive(), &c).is_none());
        assert!(exceeds(&many(11, 1), &onedrive(), &c).is_some());
        assert!(exceeds(&many(2, 600), &onedrive(), &c).is_some());
    }

    #[test]
    fn under_onedrive_ignoriert_schreibweise_und_aehnliche_namen() {
        assert!(under_onedrive(
            Path::new(r"c:\users\ME\onedrive\x"),
            &onedrive()
        ));
        assert!(under_onedrive(
            Path::new(r"C:\Users\me\OneDrive"),
            &onedrive()
        ));
        assert!(!under_onedrive(
            Path::new(r"C:\Users\me\OneDrive2"),
            &onedrive()
        ));
        assert!(!under_onedrive(Path::new(r"C:\Users\me\OneDrive"), &[]));
    }
}
