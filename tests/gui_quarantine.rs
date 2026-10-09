//! Rundreise der Quarantäne-Ansicht: Liste und Zurückholen über die Operationen-Schicht (wie
//! die GUI sie nutzt), Anwenden, `undo` und `purge` über die echte CLI. Die Operationen lesen
//! `ORDNER_CLEANUP_HOME` aus der Prozessumgebung; deshalb laufen die Tests hintereinander.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use assert_cmd::Command;
use ordner_cleanup::change::restore::ItemOutcome;
use ordner_cleanup::ops::plan::{plan, PlanKindRequest, PlanOut, PlanRequest};
use ordner_cleanup::ops::quarantine::{
    quarantine_list, restore_check, restore_execute, QuarantineList, Selected,
};
use ordner_cleanup::ops::runs::runs;
use ordner_cleanup::ops::scan::scan;
use ordner_cleanup::ops::target::TargetSpec;
use ordner_cleanup::ops::OpCtx;
use ordner_cleanup::paths::HOME_OVERRIDE_ENV;
use predicates::str::contains;

static SERIAL: Mutex<()> = Mutex::new(());

struct Env {
    _guard: std::sync::MutexGuard<'static, ()>,
    home: tempfile::TempDir,
    tree: tempfile::TempDir,
    out: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var(HOME_OVERRIDE_ENV, home.path());
        Self {
            _guard: guard,
            home,
            tree: tempfile::tempdir().unwrap(),
            out: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.tree.path()
    }

    fn bin(&self) -> Command {
        let mut cmd = Command::cargo_bin("ordner-cleanup").unwrap();
        cmd.env(HOME_OVERRIDE_ENV, self.home.path())
            .current_dir(self.out.path());
        cmd
    }

    fn write(&self, rel: &str, content: &str, age_secs: u64) -> PathBuf {
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(age_secs))
            .unwrap();
        path
    }

    fn target(&self) -> TargetSpec {
        TargetSpec::Path {
            path: self.root().to_path_buf(),
            force: false,
        }
    }

    /// Drei Kopien (die älteste bleibt), gescannt und per CLI angewendet.
    fn applied(&self) -> (PathBuf, PathBuf, PathBuf) {
        let keep = self.write("a/alt.txt", "gleicher inhalt", 3000);
        let b = self.write("b/kopie1.txt", "gleicher inhalt", 2000);
        let c = self.write("c/kopie2.txt", "gleicher inhalt", 1000);
        self.write("d/einzig.txt", "etwas anderes", 500);
        let report = scan(
            &ordner_cleanup::ops::scan::ScanRequest {
                target: self.target(),
                exclude: vec![],
                summary_only: vec![],
                no_default_excludes: true,
                reset_index: false,
                threads: None,
                template: None,
            },
            &OpCtx::default(),
        )
        .unwrap();
        assert!(report.complete());
        let outcome = plan(
            &PlanRequest {
                target: self.target(),
                kind: PlanKindRequest::Dedupe {
                    keep: "oldest".parse().unwrap(),
                },
                out: PlanOut::GuiDir,
            },
            &OpCtx::default(),
        )
        .unwrap();
        self.bin()
            .arg("apply")
            .arg(outcome.saved.unwrap())
            .arg("--yes")
            .assert()
            .success();
        assert!(keep.exists() && !b.exists() && !c.exists());
        (keep, b, c)
    }

    /// Lässt den Lauf ablaufen: Startzeit im Journal auf 2020 setzen (wie `cli_apply_undo`).
    fn age_run(&self) {
        let journal = self
            .root()
            .join(".ordner-cleanup")
            .join("journal")
            .join(format!("{}.jsonl", run_id(self)));
        let text = std::fs::read_to_string(&journal).unwrap();
        let start = text.find("\"started\":\"").unwrap() + "\"started\":\"".len();
        let end = start + text[start..].find('"').unwrap();
        let aged = format!("{}2020-01-01T00:00:00Z{}", &text[..start], &text[end..]);
        std::fs::write(&journal, aged).unwrap();
    }

    fn list(&self) -> QuarantineList {
        quarantine_list(Some(self.root())).unwrap()
    }

    fn quarantine_bytes(&self) -> u64 {
        runs(Some(self.root())).unwrap()[0].1[0].bytes
    }

    /// Wählt den Eintrag, dessen Ursprung auf `name` endet.
    fn select(&self, name: &str) -> Selected {
        let list = self.list();
        let group = &list.roots[0];
        let item = group
            .items
            .iter()
            .find(|i| i.origin.ends_with(name))
            .unwrap_or_else(|| panic!("{name} nicht in der Liste"));
        Selected {
            root: group.root.clone(),
            run: item.run.clone(),
            action: item.action,
        }
    }
}

fn run_id(env: &Env) -> String {
    runs(Some(env.root())).unwrap()[0].1[0].run.to_string()
}

#[test]
fn zurueckholen_einzeln_dann_undo_holt_den_rest() {
    let env = Env::new();
    let (keep, b, c) = env.applied();
    let list = env.list();
    assert_eq!(list.item_count(), 2);
    assert_eq!((list.unreadable, list.unreachable.len()), (0, 0));
    let before = env.quarantine_bytes();
    assert_eq!(before, 2 * "gleicher inhalt".len() as u64);

    let selection = [env.select("kopie1.txt")];
    let previews = restore_check(&selection).unwrap();
    assert_eq!(previews.len(), 1);
    assert!(!previews[0].1.renamed);

    let results = restore_execute(&selection, &OpCtx::default()).unwrap();
    assert_eq!(results.len(), 1);
    assert!(matches!(
        results[0].2.outcome,
        ItemOutcome::Restored { renamed: false, .. }
    ));
    assert!(b.exists() && !c.exists() && keep.exists());
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "gleicher inhalt");
    assert_eq!(env.list().item_count(), 1);
    assert_eq!(env.quarantine_bytes(), before / 2);

    // Der Lauf bleibt „vollständig“: ein späteres Undo holt nur den Rest.
    env.bin()
        .arg("runs")
        .assert()
        .success()
        .stdout(contains("vollständig"));
    env.bin()
        .args(["undo", &run_id(&env), "--yes", "--root"])
        .arg(env.root())
        .assert()
        .success();
    assert!(b.exists() && c.exists());
    assert_eq!(env.list().item_count(), 0);
}

#[test]
fn belegter_ursprung_ergibt_neuen_namen_und_undo_meldet_keine_kollision() {
    let env = Env::new();
    let (_, b, c) = env.applied();
    std::fs::write(&b, "neue datei").unwrap();

    let selection = [env.select("kopie1.txt")];
    let previews = restore_check(&selection).unwrap();
    assert!(previews[0].1.renamed);
    let results = restore_execute(&selection, &OpCtx::default()).unwrap();
    let ItemOutcome::Restored { to, renamed: true } = &results[0].2.outcome else {
        panic!("erwartet: umbenannt zurückgeholt, war {:?}", results[0].2);
    };
    let renamed = b.with_file_name("kopie1 (2).txt");
    // Das Journal speichert die lange Form des Pfads, das Temp-Verzeichnis kann Kurznamen (8.3) nennen.
    assert_eq!(
        std::fs::canonicalize(to).unwrap(),
        std::fs::canonicalize(&renamed).unwrap()
    );
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "neue datei");
    assert_eq!(
        std::fs::read_to_string(&renamed).unwrap(),
        "gleicher inhalt"
    );

    env.bin()
        .args(["undo", &run_id(&env), "--yes", "--root"])
        .arg(env.root())
        .assert()
        .success();
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "neue datei");
    assert_eq!(
        std::fs::read_to_string(&renamed).unwrap(),
        "gleicher inhalt"
    );
    assert!(c.exists());
}

#[test]
fn purge_nach_dem_zurueckholen_laesst_die_zurueckgeholte_datei_stehen() {
    let env = Env::new();
    let (_, b, c) = env.applied();
    let selection = [env.select("kopie1.txt")];
    restore_execute(&selection, &OpCtx::default()).unwrap();
    assert!(b.exists());
    env.age_run();

    env.bin().args(["purge", "--yes"]).assert().success();
    assert!(b.exists(), "zurückgeholt, nicht mehr in der Quarantäne");
    assert!(!c.exists());
    assert_eq!(env.list().item_count(), 0, "der Rest ist gelöscht");
    env.bin()
        .arg("runs")
        .assert()
        .success()
        .stdout(contains("Quarantäne gelöscht"));
}

#[test]
fn zurueckholen_nach_purge_meldet_fehlt() {
    let env = Env::new();
    env.applied();
    let selection = [env.select("kopie1.txt")];
    env.age_run();
    env.bin().args(["purge", "--yes"]).assert().success();
    let results = restore_execute(&selection, &OpCtx::default()).unwrap();
    assert_eq!(results[0].2.outcome, ItemOutcome::Missing);
    assert!(!env.root().join("b/kopie1.txt").exists());
}
