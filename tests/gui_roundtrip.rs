//! Rundreisen zwischen der Operationen-Schicht (die die GUI nutzt) und der echten CLI. Die
//! Operationen lesen `ORDNER_CLEANUP_HOME` aus der Prozessumgebung; deshalb laufen die Tests
//! hintereinander (Sperre) und teilen sich einen Datenordner, der je Test neu angelegt wird.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use assert_cmd::Command;
use ordner_cleanup::change::plan::Plan;
use ordner_cleanup::ops::apply::{apply_check, apply_execute};
use ordner_cleanup::ops::plan::{plan, save_subset, PlanKindRequest, PlanOut, PlanRequest};
use ordner_cleanup::ops::runs::{runs, undo_check, undo_execute};
use ordner_cleanup::ops::scan::{scan, ScanRequest};
use ordner_cleanup::ops::target::TargetSpec;
use ordner_cleanup::ops::OpCtx;
use ordner_cleanup::paths::{self, HOME_OVERRIDE_ENV};

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

    fn scan(&self) {
        let ctx = OpCtx::default();
        let report = scan(
            &ScanRequest {
                target: TargetSpec::Path {
                    path: self.root().to_path_buf(),
                    force: false,
                },
                exclude: vec![],
                summary_only: vec![],
                no_default_excludes: true,
                reset_index: false,
                threads: None,
                template: None,
            },
            &ctx,
        )
        .unwrap();
        assert!(report.complete());
    }

    fn plan_dedupe(&self, out: PlanOut) -> ordner_cleanup::ops::plan::PlanOutcome {
        plan(
            &PlanRequest {
                target: TargetSpec::Path {
                    path: self.root().to_path_buf(),
                    force: false,
                },
                kind: PlanKindRequest::Dedupe {
                    keep: "oldest".parse().unwrap(),
                },
                out,
            },
            &OpCtx::default(),
        )
        .unwrap()
    }
}

/// Drei Kopien („alt“ bleibt) plus eine einzelne Datei.
fn three_copies(env: &Env) -> (PathBuf, PathBuf, PathBuf) {
    let keep = env.write("a/alt.txt", "gleicher inhalt", 3000);
    let b = env.write("b/kopie1.txt", "gleicher inhalt", 2000);
    let c = env.write("c/kopie2.txt", "gleicher inhalt", 1000);
    env.write("d/einzig.txt", "etwas anderes", 500);
    (keep, b, c)
}

#[test]
fn gui_teilplan_laesst_sich_per_cli_anwenden_und_zurueckdrehen() {
    let env = Env::new();
    let (keep, b, c) = three_copies(&env);
    env.scan();

    // GUI-Plan in den Plan-Ordner der GUI, dann nur eine der zwei Kopien auswählen
    let outcome = env.plan_dedupe(PlanOut::GuiDir);
    let file = outcome.saved.clone().unwrap();
    assert!(file.starts_with(paths::gui_plans_dir(None).unwrap()));
    assert_eq!(outcome.plan.actions.len(), 2);
    let original_bytes = std::fs::read(&file).unwrap();
    let wanted = outcome
        .plan
        .actions
        .iter()
        .find(|a| Path::new(&a.path).file_name() == b.file_name())
        .unwrap()
        .id;
    let keep_ids: HashSet<u32> = [wanted].into();
    let subset_file = save_subset(&file, &outcome.plan, &keep_ids, &Default::default()).unwrap();

    assert_eq!(
        std::fs::read(&file).unwrap(),
        original_bytes,
        "Original bleibt bytegleich"
    );
    let subset = Plan::load(&subset_file).unwrap();
    assert_eq!(subset.actions.len(), 1);
    assert_eq!(subset.params["auswahl"], "1/2");

    // Die CLI wendet den Teilplan an: nur die gewählte Kopie verschwindet
    env.bin()
        .arg("apply")
        .arg(&subset_file)
        .arg("--yes")
        .assert()
        .success();
    assert!(!b.exists() && c.exists() && keep.exists());

    // ... und dreht den Lauf zurück
    let listing = runs(Some(env.root())).unwrap();
    let run = listing[0].1[0].run.clone();
    env.bin()
        .args(["undo", &run.to_string(), "--yes", "--root"])
        .arg(env.root())
        .assert()
        .success();
    assert!(b.exists() && c.exists());
}

#[test]
fn gui_plan_doppelter_ordner_laesst_sich_per_cli_anwenden_und_zurueckdrehen() {
    let env = Env::new();
    for dir in ["Projekt", "Kopie von Projekt", "Backup Projekt"] {
        env.write(&format!("{dir}/a.txt"), "alpha alpha", 3000);
        env.write(&format!("{dir}/sub/b.txt"), "bravo bravo", 2000);
    }
    env.write("Anderes/x.txt", "ganz etwas anderes", 500);
    env.scan();

    let outcome = plan(
        &PlanRequest {
            target: TargetSpec::Path {
                path: env.root().to_path_buf(),
                force: false,
            },
            kind: PlanKindRequest::DedupeDirs {
                keep: "oldest".parse().unwrap(),
                min_size: None,
            },
            out: PlanOut::GuiDir,
        },
        &OpCtx::default(),
    )
    .unwrap();
    assert_eq!(outcome.plan.actions.len(), 2);
    let file = outcome.saved.clone().unwrap();
    // Nur eine der zwei Kopien auswählen („Kopie von Projekt“).
    let wanted = outcome
        .plan
        .actions
        .iter()
        .find(|a| a.path.ends_with("Kopie von Projekt"))
        .unwrap()
        .id;
    let subset_file = save_subset(
        &file,
        &outcome.plan,
        &HashSet::from([wanted]),
        &Default::default(),
    )
    .unwrap();

    env.bin()
        .arg("apply")
        .arg(&subset_file)
        .arg("--yes")
        .assert()
        .success();
    assert!(!env.root().join("Kopie von Projekt").exists());
    assert!(env.root().join("Backup Projekt").exists() && env.root().join("Projekt").exists());

    let listing = runs(Some(env.root())).unwrap();
    let run = listing[0].1[0].run.clone();
    env.bin()
        .args(["undo", &run.to_string(), "--yes", "--root"])
        .arg(env.root())
        .assert()
        .success();
    assert!(env
        .root()
        .join("Kopie von Projekt")
        .join("sub")
        .join("b.txt")
        .exists());
}

#[test]
fn cli_plan_laesst_sich_ueber_ops_anwenden_und_zurueckdrehen() {
    let env = Env::new();
    let (_keep, b, c) = three_copies(&env);
    env.scan();

    let plan_file = env.out.path().join("plan.json");
    env.bin()
        .args(["plan", "dedupe"])
        .arg(env.root())
        .arg("--out")
        .arg(&plan_file)
        .assert()
        .success();
    let plan = Plan::load(&plan_file).unwrap();

    let check = apply_check(&plan).unwrap();
    assert_eq!(check.actions, 2);
    assert!(check.limit.is_none());
    let ctx = OpCtx::default();
    let result = apply_execute(&plan, "plan.json", false, &ctx).unwrap();
    assert_eq!(result.outcome.executed(), 2);
    assert!(!b.exists() && !c.exists());

    // Der Lauf steht im Register und in `runs`
    let listing = runs(None).unwrap();
    assert!(listing
        .iter()
        .any(|(_, r)| r.iter().any(|s| s.run == result.outcome.run)));

    // Undo über die CLI wie bei einem reinen CLI-Lauf
    env.bin()
        .args(["undo", &result.outcome.run.to_string(), "--yes"])
        .assert()
        .success();
    assert!(b.exists() && c.exists());
}

#[test]
fn undo_ueber_ops_funktioniert_fuer_einen_cli_lauf() {
    let env = Env::new();
    let (_keep, b, c) = three_copies(&env);
    env.scan();
    let plan_file = env.out.path().join("plan.json");
    env.bin()
        .args(["plan", "dedupe"])
        .arg(env.root())
        .arg("--out")
        .arg(&plan_file)
        .assert()
        .success();
    env.bin()
        .arg("apply")
        .arg(&plan_file)
        .arg("--yes")
        .assert()
        .success();
    assert!(!b.exists() && !c.exists());

    let run = runs(Some(env.root())).unwrap()[0].1[0].run.clone();
    let check = undo_check(&run, None).unwrap();
    assert!(!check.already_undone() && !check.purged());
    let outcome = undo_execute(&run, &check.root, &OpCtx::default()).unwrap();
    assert_eq!(outcome.restored(), 2);
    assert!(b.exists() && c.exists());
    assert!(undo_check(&run, None).unwrap().already_undone());
}

#[test]
fn belegte_scan_sperre_wird_zu_busy() {
    let env = Env::new();
    three_copies(&env);
    let _lock = ordner_cleanup::ops::acquire_scan_lock().unwrap();
    let err = scan(
        &ScanRequest {
            target: TargetSpec::Path {
                path: env.root().to_path_buf(),
                force: false,
            },
            exclude: vec![],
            summary_only: vec![],
            no_default_excludes: true,
            reset_index: false,
            threads: None,
            template: None,
        },
        &OpCtx::default(),
    )
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<ordner_cleanup::ops::Error>(),
        Some(ordner_cleanup::ops::Error::Busy(_))
    ));
}

#[test]
fn plan_ohne_speichern_schreibt_keine_datei() {
    let env = Env::new();
    three_copies(&env);
    env.scan();
    let outcome = env.plan_dedupe(PlanOut::DontSave);
    assert!(outcome.saved.is_none());
    assert_eq!(outcome.plan.actions.len(), 2);
    let leftovers: Vec<_> = std::fs::read_dir(env.out.path()).unwrap().collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert!(!paths::gui_plans_dir(None).unwrap().exists());
}

#[test]
fn obergrenze_wird_von_apply_execute_selbst_geprueft() {
    let env = Env::new();
    let (_keep, b, _c) = three_copies(&env);
    env.scan();
    let plan_file = env.out.path().join("plan.json");
    env.bin()
        .args(["plan", "dedupe"])
        .arg(env.root())
        .arg("--out")
        .arg(&plan_file)
        .assert()
        .success();
    // Unter einer (vorgetäuschten) OneDrive-Wurzel mit Obergrenze 1 Datei
    let cfg_dir = paths::config_dir().unwrap();
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("config.toml"), "onedrive_max_move_files = 1\n").unwrap();
    std::env::set_var("OneDrive", env.root());

    let plan = Plan::load(&plan_file).unwrap();
    let check = apply_check(&plan).unwrap();
    assert!(check.limit.is_some());
    let err = apply_execute(&plan, "plan.json", false, &OpCtx::default()).unwrap_err();
    assert!(matches!(
        err.downcast_ref::<ordner_cleanup::ops::Error>(),
        Some(ordner_cleanup::ops::Error::LimitExceeded(_))
    ));
    assert!(b.exists(), "nichts wurde verändert");

    let result = apply_execute(&plan, "plan.json", true, &OpCtx::default()).unwrap();
    assert_eq!(result.outcome.executed(), 2);
    std::env::remove_var("OneDrive");
}

#[test]
fn mit_settings_gespeicherte_regel_wird_von_der_cli_genutzt() {
    use ordner_cleanup::settings::fields::Value;
    use ordner_cleanup::settings::validate::rules_text;
    use ordner_cleanup::settings::{rules_doc, tables, EditableFile};

    let env = Env::new();
    env.write("Downloads/bericht.pdf", "pdf-inhalt", 5000);
    env.write("Downloads/foto.jpg", "jpg-inhalt", 4000);
    env.scan();

    // Regel wie im Regel-Editor anlegen und mit der Validierung der CLI speichern
    let rules_path = paths::config_dir().unwrap().join("rules.toml");
    let mut file = EditableFile::load_or_empty(&rules_path).unwrap();
    let i = tables::add(
        file.doc_mut(),
        rules_doc::KEY,
        "pdfs",
        &rules_doc::new_rule_defaults(),
    )
    .unwrap();
    tables::set_field(
        file.doc_mut(),
        rules_doc::KEY,
        i,
        "ext",
        &Value::List(vec!["pdf".into()]),
    );
    tables::set_field(
        file.doc_mut(),
        rules_doc::KEY,
        i,
        "target",
        &Value::Text("Dokumente/".into()),
    );
    let config = ordner_cleanup::config::Config::default();
    file.save(&|t| rules_text(t, &config), false).unwrap();

    // Die CLI plant mit dieser Datei
    let plan_file = env.out.path().join("regeln.json");
    env.bin()
        .args(["plan", "rules"])
        .arg(env.root())
        .arg("--out")
        .arg(&plan_file)
        .assert()
        .success();
    let plan = Plan::load(&plan_file).unwrap();
    assert_eq!(plan.actions.len(), 1);
    assert_eq!(plan.actions[0].rule.as_deref(), Some("pdfs"));
    assert!(plan.actions[0].path.ends_with("bericht.pdf"));

    // Die Vorschau der GUI (Entwurf als Text, nicht gespeichert) liefert dasselbe
    let preview = plan_preview_with_text(&env, &std::fs::read_to_string(&rules_path).unwrap());
    assert_eq!(preview.plan.actions.len(), 1);
    assert!(preview.saved.is_none());
}

fn plan_preview_with_text(env: &Env, text: &str) -> ordner_cleanup::ops::plan::PlanOutcome {
    plan(
        &PlanRequest {
            target: TargetSpec::Path {
                path: env.root().to_path_buf(),
                force: false,
            },
            kind: PlanKindRequest::Rules {
                rules_file: None,
                rules_text: Some(text.to_string()),
                only: vec![],
                no_classify: true,
                lock_held: false,
            },
            out: PlanOut::DontSave,
        },
        &OpCtx::default(),
    )
    .unwrap()
}
