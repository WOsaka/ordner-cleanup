use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::str::contains;

fn bin(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ordner-cleanup").unwrap();
    cmd.env("ORDNER_CLEANUP_HOME", home);
    cmd
}

fn sample_tree(dir: &Path) {
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::write(dir.join("a").join("x.txt"), "inhalt-eins").unwrap();
    std::fs::write(dir.join("a").join("z.txt"), "inhalt-drei").unwrap();
    std::fs::write(dir.join("y.txt"), "inhalt-zwei").unwrap();
    std::fs::write(dir.join("a").join("m.tmp"), "x").unwrap();
}

fn write_config(home: &Path, text: &str) {
    let dir = home.join("config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), text).unwrap();
}

fn profile_config(root: &Path, extra: &str) -> String {
    format!("[profiles.t]\nroot = '{}'\n{extra}\n", root.display())
}

fn snapshot_of(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path.strip_prefix(base).unwrap().display().to_string();
            if path.is_dir() {
                out.push((format!("{rel}\\"), vec![]));
                walk(base, &path, out);
            } else {
                out.push((rel, std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn data(home: &Path) -> PathBuf {
    home.join("data")
}

fn log_of(home: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(data(home).join("runs").join("t.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn latest_html(home: &Path) -> String {
    let path = data(home).join("reports").join("t").join("latest.html");
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn run_erzeugt_momentaufnahme_bericht_und_plaene_ohne_den_baum_zu_aendern() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(
        home.path(),
        &profile_config(tree.path(), "plans = [\"junk\", \"empty-dirs\"]"),
    );
    let before = snapshot_of(tree.path());

    bin(home.path())
        .args(["run", "--profile", "t"])
        .assert()
        .success()
        .stdout(contains("Health-Score"));

    assert_eq!(
        snapshot_of(tree.path()),
        before,
        "run darf nichts verändern"
    );
    let reports = files_in(&data(home.path()).join("reports").join("t"));
    assert_eq!(reports.len(), 2, "{reports:?}");
    assert!(reports.contains(&"latest.html".to_string()));
    assert!(latest_html(home.path()).contains("report-data"));
    let plans = files_in(&data(home.path()).join("plans").join("t"));
    assert_eq!(plans.len(), 2, "{plans:?}");
    assert!(plans.iter().any(|p| p.ends_with("-junk.json")));
    assert!(plans.iter().any(|p| p.ends_with("-empty-dirs.json")));

    let log = log_of(home.path());
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["status"], "ok");
    assert!(log[0]["score"].as_u64().is_some());
    let junk = log[0]["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "junk")
        .unwrap();
    assert_eq!(junk["actions"], 1);
    assert!(log[0]["report"].as_str().unwrap().ends_with("latest.html"));
    bin(home.path())
        .args(["history", "--profile", "t"])
        .assert()
        .success();
}

#[test]
fn run_behaelt_nur_die_letzten_zwoelf_berichte_und_plaene() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(
        home.path(),
        &profile_config(tree.path(), "plans = [\"junk\"]"),
    );
    let reports = data(home.path()).join("reports").join("t");
    let plans = data(home.path()).join("plans").join("t");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::create_dir_all(&plans).unwrap();
    for day in 1..=14 {
        std::fs::write(reports.join(format!("202001{day:02}-000000.html")), "alt").unwrap();
        std::fs::write(plans.join(format!("202001{day:02}-000000-junk.json")), "{}").unwrap();
    }
    bin(home.path())
        .args(["run", "--profile", "t"])
        .assert()
        .success();
    let r = files_in(&reports);
    assert_eq!(r.len(), 12 + 1, "{r:?}");
    assert!(r.contains(&"latest.html".to_string()));
    assert!(!r.contains(&"20200101-000000.html".to_string()));
    assert!(r.contains(&"20200114-000000.html".to_string()));
    assert_eq!(files_in(&plans).len(), 12, "{:?}", files_in(&plans));
}

#[test]
fn run_mit_fehlender_wurzel_endet_ohne_momentaufnahme_mit_protokoll() {
    let home = tempfile::tempdir().unwrap();
    write_config(
        home.path(),
        "[profiles.t]\nroot = 'C:\\gibt\\es\\garantiert\\nicht'\n",
    );
    bin(home.path())
        .args(["run", "--profile", "t"])
        .assert()
        .code(1)
        .stderr(contains("nicht erreichbar"));
    let log = log_of(home.path());
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["status"], "root-missing");
    assert!(log[0]["errors"][0]
        .as_str()
        .unwrap()
        .contains("nicht erreichbar"));
    assert!(!data(home.path()).join("history.db").exists());
}

#[test]
fn run_ueberspringt_bei_belegter_sperre_und_der_naechste_bericht_nennt_den_ausfall() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(home.path(), &profile_config(tree.path(), ""));
    let lock = ordner_cleanup::scan::lock::ScanLock::acquire(&data(home.path()).join("scan.lock"))
        .unwrap();
    bin(home.path())
        .args(["run", "--profile", "t"])
        .assert()
        .code(3);
    drop(lock);
    let log = log_of(home.path());
    assert_eq!(log[0]["status"], "skipped-locked");
    assert!(
        !data(home.path()).join("history.db").exists(),
        "ohne Scan keine Momentaufnahme"
    );

    bin(home.path())
        .args(["run", "--profile", "t"])
        .assert()
        .success();
    assert!(
        latest_html(home.path()).contains("ist ausgefallen"),
        "der Bericht nennt den ausgefallenen Lauf"
    );
    assert_eq!(log_of(home.path()).len(), 2);
}

#[test]
fn fehlende_regeldatei_laesst_scan_und_bericht_entstehen_und_endet_mit_exit_2() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(
        home.path(),
        &profile_config(
            tree.path(),
            "plans = [\"rules\", \"junk\"]\nrules_file = \"fehlt.toml\"",
        ),
    );
    bin(home.path())
        .args(["run", "--profile", "t"])
        .assert()
        .code(2);
    assert!(latest_html(home.path()).contains("report-data"));
    let log = log_of(home.path());
    assert_eq!(log[0]["status"], "partial");
    let plans = log[0]["plans"].as_array().unwrap();
    assert!(plans[0]["error"].as_str().unwrap().contains("Regeldatei"));
    assert_eq!(
        plans[1]["error"],
        serde_json::Value::Null,
        "der nächste Plan läuft trotzdem"
    );
    let files = files_in(&data(home.path()).join("plans").join("t"));
    assert_eq!(files.len(), 1, "{files:?}");
}

#[test]
fn run_mit_unbekanntem_profil_scheitert() {
    let home = tempfile::tempdir().unwrap();
    bin(home.path())
        .args(["run", "--profile", "nope"])
        .assert()
        .code(1)
        .stderr(contains("Unbekanntes Profil"));
    bin(home.path()).arg("run").assert().failure();
}

fn pe_subsystem(exe: &Path) -> u16 {
    let bytes = std::fs::read(exe).unwrap();
    let pe = u32::from_le_bytes(bytes[0x3C..0x40].try_into().unwrap()) as usize;
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
    let optional = pe + 24;
    u16::from_le_bytes(bytes[optional + 68..optional + 70].try_into().unwrap())
}

#[test]
fn hintergrundprogramm_hat_kein_konsolenfenster_und_laeuft_wie_die_konsolen_exe() {
    const GUI: u16 = 2;
    const CONSOLE: u16 = 3;
    let main = assert_cmd::cargo::cargo_bin("ordner-cleanup");
    let bg = assert_cmd::cargo::cargo_bin("ordner-cleanup-bg");
    assert_eq!(pe_subsystem(&main), CONSOLE);
    assert_eq!(pe_subsystem(&bg), GUI);

    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(home.path(), &profile_config(tree.path(), ""));
    Command::cargo_bin("ordner-cleanup-bg")
        .unwrap()
        .env("ORDNER_CLEANUP_HOME", home.path())
        .args(["run", "--profile", "t"])
        .assert()
        .success();
    assert_eq!(log_of(home.path())[0]["status"], "ok");
}

fn notify_lines(file: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn run_notify(home: &Path, notify_file: &Path, notify: bool) -> assert_cmd::assert::Assert {
    let mut cmd = bin(home);
    cmd.env(ordner_cleanup::platform::toast::TEST_LOG_ENV, notify_file)
        .args(["run", "--profile", "t"]);
    if notify {
        cmd.arg("--notify");
    }
    cmd.assert()
}

#[test]
fn notify_zeigt_bei_neuen_plaenen_und_klick_oeffnet_den_bericht() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(
        home.path(),
        &profile_config(tree.path(), "plans = [\"junk\"]"),
    );
    let file = home.path().join("toasts.jsonl");
    run_notify(home.path(), &file, true).success();
    let toasts = notify_lines(&file);
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert_eq!(toasts[0]["title"], "t");
    assert!(toasts[0]["body"]
        .as_str()
        .unwrap()
        .contains("1 Plan bereit"));
    assert!(toasts[0]["open"].as_str().unwrap().ends_with("latest.html"));
    assert_eq!(log_of(home.path())[0]["notified"], true);
}

#[test]
fn ohne_notify_bleibt_der_lauf_still() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(
        home.path(),
        &profile_config(tree.path(), "plans = [\"junk\"]"),
    );
    let file = home.path().join("toasts.jsonl");
    run_notify(home.path(), &file, false).success();
    assert!(!file.exists());
    assert_eq!(log_of(home.path())[0]["notified"], false);
}

#[test]
fn ruhiger_lauf_zeigt_auch_mit_notify_nichts() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    std::fs::remove_file(tree.path().join("a").join("m.tmp")).unwrap();
    write_config(home.path(), &profile_config(tree.path(), ""));
    let file = home.path().join("toasts.jsonl");
    run_notify(home.path(), &file, true).success();
    assert!(!file.exists());
}

#[test]
fn fehlende_wurzel_meldet_nicht_erreichbar_und_oeffnet_das_protokoll() {
    let home = tempfile::tempdir().unwrap();
    write_config(
        home.path(),
        "[profiles.t]\nroot = 'C:\\gibt\\es\\garantiert\\nicht'\n",
    );
    let file = home.path().join("toasts.jsonl");
    run_notify(home.path(), &file, true).code(1);
    let toasts = notify_lines(&file);
    assert_eq!(toasts.len(), 1);
    assert!(toasts[0]["title"]
        .as_str()
        .unwrap()
        .contains("nicht erreichbar"));
    assert!(toasts[0]["open"].as_str().unwrap().ends_with("t.jsonl"));
}

#[test]
fn uebersprungen_meldet_erst_bei_wiederholung() {
    let home = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    sample_tree(tree.path());
    write_config(home.path(), &profile_config(tree.path(), ""));
    let file = home.path().join("toasts.jsonl");
    let _lock = ordner_cleanup::scan::lock::ScanLock::acquire(&data(home.path()).join("scan.lock"))
        .unwrap();
    run_notify(home.path(), &file, true).code(3);
    assert!(!file.exists(), "ein einzelner Ausfall bleibt still");
    run_notify(home.path(), &file, true).code(3);
    let toasts = notify_lines(&file);
    assert_eq!(toasts.len(), 1);
    assert!(toasts[0]["title"].as_str().unwrap().contains("wiederholt"));
}
