//! Einzelne Dateien aus der Quarantäne zurückholen. Die Liste kommt aus den Journalen; das
//! Zurückholen schreibt je Eintrag ein `undo_done`, aber kein `undo_start`/`undo_end` – der
//! Lauf behält seinen Status, nur die Größe in der Quarantäne sinkt.

use std::io;
use std::path::Path;

use chrono::{DateTime, Utc};

use super::fsops::FsOps;
use super::journal::{self, Dest};
use super::undo::{collect_ops, move_is_sane, summarize, OpKind, RunStatus, UndoError};
use super::{quarantine, RunId};
use crate::paths;

/// Ein Eintrag in der Quarantäne-Ansicht: ein ausgeführter, noch nicht zurückgeholter Move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantineItem {
    pub run: RunId,
    pub action: u32,
    /// `from` aus dem Intent: dorthin kommt die Datei zurück.
    pub origin: String,
    /// `to` aus dem Intent: Ablageort in der Quarantäne.
    pub stored: String,
    pub size: u64,
    /// Nur Dateien aus `dedupe` (xxh3-128); bei Ordnern ein Fingerprint, der nicht geprüft wird.
    pub hash: Option<String>,
    pub is_dir: bool,
    /// Nur bei Ordnern, per Verzeichnis-Durchlauf ohne Dateien zu öffnen.
    pub file_count: Option<u64>,
    pub started: Option<String>,
    pub expires: Option<DateTime<Utc>>,
    /// Liegt noch in der Quarantäne.
    pub present: bool,
    pub cloud_only: bool,
}

#[derive(Debug, Default)]
pub struct QuarantineListing {
    pub items: Vec<QuarantineItem>,
    /// Journale, die nicht gelesen werden konnten.
    pub unreadable: usize,
}

/// Alle Quarantäne-Moves der Wurzel, die ausgeführt und weder zurückgedreht noch gelöscht
/// sind. Der Inhalt von Dateien wird nie gelesen.
pub fn list_quarantine(
    root: &Path,
    quarantine_days: u32,
    fs: &dyn FsOps,
) -> Result<QuarantineListing, UndoError> {
    let dir = paths::extended(&quarantine::journal_dir(root));
    let listing = match std::fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(QuarantineListing::default()),
        Err(e) => return Err(e.into()),
    };
    let root_key = paths::path_key(root);
    let mut runs: Vec<(RunId, std::path::PathBuf)> = listing
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let run = RunId::parse(name.strip_suffix(".jsonl")?).ok()?;
            Some((run, entry.path()))
        })
        .collect();
    runs.sort();
    let mut out = QuarantineListing::default();
    for (run, path) in runs {
        let Ok(entries) = journal::read(&path) else {
            out.unreadable += 1;
            continue;
        };
        let summary = summarize(run.clone(), &entries, quarantine_days);
        if summary.status == RunStatus::Purged {
            continue;
        }
        let stop = quarantine::run_dir(root, &run);
        for op in collect_ops(&entries) {
            if !op.done || op.failed || op.undone {
                continue;
            }
            let OpKind::Move {
                from,
                to,
                size,
                dest: Dest::Quarantine,
                is_dir,
                hash,
            } = op.kind
            else {
                continue;
            };
            if !move_is_sane(&root_key, &stop, &from, &to) {
                continue;
            }
            let stored = Path::new(&to);
            let meta = fs.metadata(stored).ok();
            out.items.push(QuarantineItem {
                run: run.clone(),
                action: op.action,
                origin: from,
                size,
                hash,
                is_dir,
                file_count: if is_dir && meta.is_some() {
                    count_files(fs, stored)
                } else {
                    None
                },
                started: summary.started.clone(),
                expires: summary.expires,
                present: meta.is_some(),
                cloud_only: meta.is_some_and(|m| m.is_cloud_only()),
                stored: to,
            });
        }
    }
    Ok(out)
}

/// Dateien unterhalb von `dir`, nur über Verzeichniseinträge (nichts wird geöffnet).
fn count_files(fs: &dyn FsOps, dir: &Path) -> Option<u64> {
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for (path, meta) in fs.read_dir(&current).ok()? {
            if meta.is_dir && !meta.is_reparse_point() {
                stack.push(path);
            } else {
                count += 1;
            }
        }
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::fsops::testing::FaultyFs;
    use crate::change::fsops::RealFs;
    use crate::change::journal::{Entry, JournalWriter};
    use crate::change::test_support::{fx, run_with, Fx, RUN};
    use crate::change::undo::{purge_run, undo_run, UndoEnv};
    use std::sync::atomic::AtomicBool;

    const RUN2: &str = "20261004-120000-cd34";
    static NEVER: AtomicBool = AtomicBool::new(false);

    fn run_id(text: &str) -> RunId {
        RunId::parse(text).unwrap()
    }

    fn env() -> UndoEnv<'static> {
        UndoEnv {
            fs: &RealFs,
            now: "2026-10-03T11:00:00Z",
            cancel: &NEVER,
        }
    }

    fn list(fx: &Fx) -> QuarantineListing {
        list_quarantine(&fx.root, 30, &RealFs).unwrap()
    }

    /// Zwei Duplikate im Lauf `RUN` in die Quarantäne verschoben.
    fn applied() -> Fx {
        let fx = fx();
        fx.write("a/orig.txt", "gleicher inhalt");
        fx.write("b/kopie.txt", "gleicher inhalt");
        fx.write("c/sub/kopie2.txt", "gleicher inhalt");
        let plan = fx.plan(&[
            ("b/kopie.txt", "a/orig.txt"),
            ("c/sub/kopie2.txt", "a/orig.txt"),
        ]);
        let out = run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        assert_eq!(out.executed(), 2);
        fx
    }

    fn origins(listing: &QuarantineListing, fx: &Fx) -> Vec<String> {
        let mut found: Vec<String> = listing
            .items
            .iter()
            .map(|i| {
                paths::relative_to(&fx.root, Path::new(&i.origin))
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        found.sort();
        found
    }

    #[test]
    fn liste_zeigt_nur_quarantaene_moves() {
        let fx = applied();
        fx.write("alt/x.txt", "x");
        let plan = fx.move_file_plan(&[("alt/x.txt", "alt/x.txt")]);
        run_with(&fx, &plan, &RealFs, &fx.protector(), RUN2).unwrap();
        let listing = list(&fx);
        assert_eq!(origins(&listing, &fx), ["b/kopie.txt", "c/sub/kopie2.txt"]);
        assert_eq!(listing.unreadable, 0);
        let item = &listing.items[0];
        assert_eq!(item.run, run_id(RUN));
        assert_eq!(item.size, "gleicher inhalt".len() as u64);
        assert!(item.hash.is_some() && item.present && !item.is_dir && !item.cloud_only);
        assert_eq!(item.file_count, None);
        assert!(item.started.is_some() && item.expires.is_some());
    }

    #[test]
    fn zurueckgedrehte_und_geloeschte_laeufe_fehlen() {
        let fx = applied();
        undo_run(&fx.root, &run_id(RUN), &env()).unwrap();
        assert!(list(&fx).items.is_empty());

        let fx = applied();
        purge_run(&fx.root, &run_id(RUN), &env()).unwrap();
        assert!(list(&fx).items.is_empty());
    }

    #[test]
    fn teilweise_zurueckgedrehter_lauf_zeigt_nur_den_rest() {
        let fx = applied();
        let path = quarantine::journal_path(&fx.root, &run_id(RUN));
        JournalWriter::open_append(&path)
            .unwrap()
            .append(&Entry::UndoDone {
                run: run_id(RUN),
                action: 1,
                to: None,
            })
            .unwrap();
        assert_eq!(origins(&list(&fx), &fx), ["c/sub/kopie2.txt"]);
    }

    #[test]
    fn uebersprungene_aktionen_fehlen() {
        let fx = fx();
        fx.write("a/orig.txt", "gleicher inhalt");
        fx.write("b/kopie.txt", "gleicher inhalt");
        fx.write("c/kopie2.txt", "gleicher inhalt");
        let plan = fx.plan(&[
            ("b/kopie.txt", "a/orig.txt"),
            ("c/kopie2.txt", "a/orig.txt"),
        ]);
        std::fs::write(fx.root.join("c/kopie2.txt"), "geändert und länger").unwrap();
        run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        assert_eq!(origins(&list(&fx), &fx), ["b/kopie.txt"]);
    }

    #[test]
    fn von_hand_geloeschte_datei_ist_nicht_present() {
        let fx = applied();
        std::fs::remove_file(fx.quarantined(RUN, "b/kopie.txt")).unwrap();
        let listing = list(&fx);
        assert_eq!(listing.items.len(), 2);
        let gone = listing
            .items
            .iter()
            .find(|i| i.origin.ends_with("kopie.txt"))
            .unwrap();
        assert!(!gone.present);
        assert!(listing.items.iter().filter(|i| i.present).count() == 1);
    }

    #[test]
    fn ordner_eintrag_hat_dateianzahl() {
        let fx = fx();
        fx.write("a/1.txt", "eins");
        fx.write("a/sub/2.txt", "zwei");
        fx.write("b/1.txt", "eins");
        fx.write("b/sub/2.txt", "zwei");
        let plan = fx.dir_dedupe_plan(&[("b", "a")]);
        run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        let listing = list(&fx);
        assert_eq!(listing.items.len(), 1);
        let item = &listing.items[0];
        assert!(item.is_dir && item.present);
        assert_eq!(item.file_count, Some(2));
    }

    #[test]
    fn cloud_only_wird_nie_gelesen() {
        let fx = applied();
        let faulty = FaultyFs::new().cloud_only(&fx.quarantined(RUN, "b/kopie.txt"));
        let listing = list_quarantine(&fx.root, 30, &faulty).unwrap();
        let cloud: Vec<_> = listing.items.iter().filter(|i| i.cloud_only).collect();
        assert_eq!(cloud.len(), 1);
        assert!(cloud[0].origin.ends_with("kopie.txt"));
        assert!(faulty.hashed().is_empty());
    }

    #[test]
    fn unlesbares_journal_wird_gezaehlt() {
        let fx = applied();
        std::fs::write(
            quarantine::journal_path(&fx.root, &run_id(RUN2)),
            "kein json\n{\"t\":\"run_end\"}\n",
        )
        .unwrap();
        let listing = list(&fx);
        assert_eq!(listing.unreadable, 1);
        assert_eq!(listing.items.len(), 2);
    }

    #[test]
    fn eintrag_ausserhalb_der_wurzel_faellt_weg() {
        let fx = applied();
        let path = quarantine::journal_path(&fx.root, &run_id(RUN));
        let mut entries = journal::read(&path).unwrap();
        for e in &mut entries {
            if let Entry::Intent {
                from, action: 1, ..
            } = e
            {
                *from = r"C:\Windows\System32\drivers\etc\hosts".into();
            }
        }
        std::fs::remove_file(&path).unwrap();
        let mut w = JournalWriter::create(&path).unwrap();
        for e in &entries {
            w.append(e).unwrap();
        }
        assert_eq!(origins(&list(&fx), &fx), ["c/sub/kopie2.txt"]);
    }
}
