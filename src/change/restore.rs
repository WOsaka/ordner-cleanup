//! Einzelne Dateien aus der Quarantäne zurückholen. Die Liste kommt aus den Journalen; das
//! Zurückholen schreibt je Eintrag ein `undo_done`, aber kein `undo_start`/`undo_end` – der
//! Lauf behält seinen Status, nur die Größe in der Quarantäne sinkt.

use std::io;
use std::path::Path;

use chrono::{DateTime, Utc};

use super::fsops::FsOps;
use super::journal::Entry;
use super::journal::{self, Dest};
use super::plan::hex;
use super::undo::{collect_ops, load, move_is_sane, summarize, OpKind, RunStatus, UndoError};
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
        for MoveInfo {
            action,
            from,
            to,
            size,
            is_dir,
            hash,
        } in restorable_moves(root, &run, &entries)
        {
            let stored = Path::new(&to);
            let meta = fs.metadata(stored).ok();
            out.items.push(QuarantineItem {
                run: run.clone(),
                action,
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

/// Ein ausgeführter, noch nicht zurückgeholter Quarantäne-Move aus dem Journal.
struct MoveInfo {
    action: u32,
    from: String,
    to: String,
    size: u64,
    is_dir: bool,
    hash: Option<String>,
}

/// Quarantäne-Moves eines Laufs, die ausgeführt und weder fehlgeschlagen noch zurückgedreht
/// sind und deren Pfade `move_is_sane` bestehen. Gemeinsame Regel für Liste, Vorschau und
/// Zurückholen.
fn restorable_moves(root: &Path, run: &RunId, entries: &[Entry]) -> Vec<MoveInfo> {
    let root_key = paths::path_key(root);
    let stop = quarantine::run_dir(root, run);
    collect_ops(entries)
        .into_iter()
        .filter(|op| op.done && !op.failed && !op.undone)
        .filter_map(|op| match op.kind {
            OpKind::Move {
                from,
                to,
                size,
                dest: Dest::Quarantine,
                is_dir,
                hash,
            } if move_is_sane(&root_key, &stop, &from, &to) => Some(MoveInfo {
                action: op.action,
                from,
                to,
                size,
                is_dir,
                hash,
            }),
            _ => None,
        })
        .collect()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashCheck {
    /// Kein Hash im Journal (nur `dedupe` schreibt einen) oder ein Ordner.
    NotApplicable,
    Match,
    /// Der Inhalt in der Quarantäne weicht ab oder ließ sich nicht prüfen.
    Mismatch,
    /// Platzhalter: der Inhalt wird nicht gelesen, um keinen Download auszulösen.
    CloudOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePreview {
    pub run: RunId,
    pub action: u32,
    /// Ursprünglicher Pfad oder, wenn belegt, `name (2).ext`.
    pub target: std::path::PathBuf,
    pub renamed: bool,
    pub hash: HashCheck,
}

/// Rechnet für eine Auswahl das Ziel aus und prüft bei Dateien aus `dedupe` den Hash.
/// Ändert nichts. Einträge, die nicht (mehr) zurückholbar sind, fehlen im Ergebnis.
pub fn preview_restore(
    root: &Path,
    selection: &[(RunId, u32)],
    fs: &dyn FsOps,
) -> Result<Vec<RestorePreview>, UndoError> {
    let mut runs: Vec<&RunId> = selection.iter().map(|(run, _)| run).collect();
    runs.sort();
    runs.dedup();
    let mut out = Vec::new();
    for run in runs {
        let entries = load(root, run)?;
        if entries.iter().any(|e| matches!(e, Entry::Purged { .. })) {
            continue;
        }
        let moves = restorable_moves(root, run, &entries);
        for (_, action) in selection.iter().filter(|(r, _)| r == run) {
            let Some(item) = moves.iter().find(|m| m.action == *action) else {
                continue;
            };
            let origin = std::path::PathBuf::from(&item.from);
            let target = quarantine::unique_target(fs, origin.clone()).ok_or_else(|| {
                UndoError::Invalid(format!("kein freier Name für {}", paths::display(&origin)))
            })?;
            out.push(RestorePreview {
                run: run.clone(),
                action: *action,
                renamed: target != origin,
                target,
                hash: check_hash(fs, item),
            });
        }
    }
    Ok(out)
}

fn check_hash(fs: &dyn FsOps, item: &MoveInfo) -> HashCheck {
    let (Some(expected), false) = (&item.hash, item.is_dir) else {
        return HashCheck::NotApplicable;
    };
    let stored = Path::new(&item.to);
    match fs.metadata(stored) {
        Ok(meta) if meta.is_cloud_only() => HashCheck::CloudOnly,
        Ok(meta) => match fs.hash(stored, meta.size) {
            Ok(actual) if hex(&actual) == *expected => HashCheck::Match,
            _ => HashCheck::Mismatch,
        },
        Err(_) => HashCheck::NotApplicable,
    }
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

    fn sel(run: &str, actions: &[u32]) -> Vec<(RunId, u32)> {
        actions.iter().map(|&a| (run_id(run), a)).collect()
    }

    fn preview(fx: &Fx, actions: &[u32]) -> Vec<RestorePreview> {
        preview_restore(&fx.root, &sel(RUN, actions), &RealFs).unwrap()
    }

    #[test]
    fn vorschau_freies_ziel_ist_der_ursprung() {
        let fx = applied();
        let p = preview(&fx, &[1, 2]);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].target, fx.root.join("b/kopie.txt"));
        assert_eq!(p[1].target, fx.root.join("c/sub/kopie2.txt"));
        assert!(p.iter().all(|x| !x.renamed && x.hash == HashCheck::Match));
        assert_eq!((p[0].run.clone(), p[0].action), (run_id(RUN), 1));
        assert!(!fx.exists("b/kopie.txt"), "Vorschau ändert nichts");
    }

    #[test]
    fn vorschau_belegtes_ziel_wird_nummeriert() {
        let fx = applied();
        fx.write("b/kopie.txt", "neu");
        let p = preview(&fx, &[1]);
        assert_eq!(p[0].target, fx.root.join("b/kopie (2).txt"));
        assert!(p[0].renamed);
        fx.write("b/kopie (2).txt", "noch neuer");
        assert_eq!(
            preview(&fx, &[1])[0].target,
            fx.root.join("b/kopie (3).txt")
        );
    }

    #[test]
    fn vorschau_erkennt_veraenderten_inhalt() {
        let fx = applied();
        std::fs::write(fx.quarantined(RUN, "b/kopie.txt"), "verändert").unwrap();
        let p = preview(&fx, &[1, 2]);
        assert_eq!(p[0].hash, HashCheck::Mismatch);
        assert_eq!(p[1].hash, HashCheck::Match);
    }

    #[test]
    fn vorschau_ohne_hash_und_bei_ordnern_nicht_anwendbar() {
        let fx = fx();
        fx.write("tmp/x.tmp", "x");
        let plan = fx.junk_plan(&["tmp/x.tmp"]);
        run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        assert_eq!(preview(&fx, &[1])[0].hash, HashCheck::NotApplicable);

        let fx = self::fx();
        fx.write("a/1.txt", "eins");
        fx.write("b/1.txt", "eins");
        let plan = fx.dir_dedupe_plan(&[("b", "a")]);
        run_with(&fx, &plan, &RealFs, &fx.protector(), RUN).unwrap();
        let p = preview(&fx, &[1]);
        assert_eq!(p[0].hash, HashCheck::NotApplicable);
        assert_eq!(p[0].target, fx.root.join("b"));
    }

    #[test]
    fn vorschau_liest_cloud_only_nicht() {
        let fx = applied();
        let faulty = FaultyFs::new().cloud_only(&fx.quarantined(RUN, "b/kopie.txt"));
        let p = preview_restore(&fx.root, &sel(RUN, &[1, 2]), &faulty).unwrap();
        assert_eq!(p[0].hash, HashCheck::CloudOnly);
        assert_eq!(p[1].hash, HashCheck::Match);
        assert_eq!(
            faulty.hashed().len(),
            1,
            "nur die lokale Datei wird gelesen"
        );
    }

    #[test]
    fn vorschau_laesst_unbekannte_und_erledigte_eintraege_weg() {
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
        let p = preview(&fx, &[1, 2, 99]);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].action, 2);
    }
}
