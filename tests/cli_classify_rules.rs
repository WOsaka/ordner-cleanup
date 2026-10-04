//! Ende-zu-Ende von `plan rules` mit Inhaltsbedingungen über die echte CLI:
//! scan → plan rules (klassifiziert selbst) → apply → undo, Idempotenz, `--no-classify`,
//! unsichere Kategorien, Wartezeit. OCR und LLM sind aus.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use assert_cmd::assert::Assert;
use assert_cmd::Command;
use ordner_cleanup::content::extract::pdf::testing::pdf_with_pages;
use ordner_cleanup::paths;
use predicates::str::contains;

const INVOICE: &str = "Rechnung Rechnungsnummer: RE-2026-0042 Rechnungsdatum: 30.09.2026 Zahlbar bis 30.10.2026 Zahlungsziel 14 Tage Gesamtbetrag: 119,00 EUR Vielen Dank fuer Ihren Auftrag";
const WEAK: &str = "Brief wegen der Rechnung von letzter Woche mit freundlichen Gruessen";

const RULES: &str = r#"
[[rules]]
name     = "rechnungen"
glob     = "Downloads/**"
category = "rechnung"
target   = "Finanzen/{doc.date:%Y}/{doc.date:%Y-%m-%d}_{doc.number|{name}}.{ext}"
"#;

type Snapshot = BTreeMap<String, (u64, SystemTime, Vec<u8>)>;

struct Env {
    home: tempfile::TempDir,
    tree: tempfile::TempDir,
    out: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let env = Self {
            home: tempfile::tempdir().unwrap(),
            tree: tempfile::tempdir().unwrap(),
            out: tempfile::tempdir().unwrap(),
        };
        let cfg = env.home.path().join("config");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("config.toml"), "[classify]\nocr = false\n").unwrap();
        env.rules(RULES);
        env
    }

    fn root(&self) -> &Path {
        self.tree.path()
    }

    fn bin(&self) -> Command {
        let mut cmd = Command::cargo_bin("ordner-cleanup").unwrap();
        cmd.env("ORDNER_CLEANUP_HOME", self.home.path())
            .env_remove("OneDrive")
            .env_remove("OneDriveConsumer")
            .env_remove("OneDriveCommercial")
            .current_dir(self.out.path());
        cmd
    }

    fn rules(&self, text: &str) {
        std::fs::write(self.home.path().join("config").join("rules.toml"), text).unwrap();
    }

    fn write_bytes(&self, rel: &str, content: &[u8]) -> PathBuf {
        let path = self.root().join(rel);
        std::fs::create_dir_all(paths::extended(path.parent().unwrap())).unwrap();
        std::fs::write(paths::extended(&path), content).unwrap();
        path
    }

    fn scan(&self) {
        self.bin().arg("scan").arg(self.root()).assert().success();
    }

    fn plan(&self, name: &str, extra: &[&str]) -> (PathBuf, Assert) {
        let plan = self.out.path().join(name);
        self.scan();
        let assert = self
            .bin()
            .args(["plan", "rules"])
            .arg(self.root())
            .arg("--out")
            .arg(&plan)
            .args(extra)
            .assert();
        (plan, assert)
    }

    fn snapshot(&self) -> Snapshot {
        fn walk(dir: &Path, root: &Path, out: &mut Snapshot) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.file_name().is_some_and(|n| n == ".ordner-cleanup") {
                    continue;
                }
                let meta = std::fs::metadata(&path).unwrap();
                let key = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if meta.is_dir() {
                    out.insert(key, (0, SystemTime::UNIX_EPOCH, Vec::new()));
                    walk(&path, root, out);
                } else {
                    out.insert(
                        key,
                        (
                            meta.len(),
                            meta.modified().unwrap(),
                            std::fs::read(&path).unwrap(),
                        ),
                    );
                }
            }
        }
        let root = paths::extended(self.root());
        let mut out = Snapshot::new();
        walk(&root, &root, &mut out);
        out
    }

    fn corpus(&self) {
        self.write_bytes(
            "Downloads/scan0012.pdf",
            &pdf_with_pages(&[INVOICE], None, None),
        );
        self.write_bytes("Downloads/Notiz.txt", b"nichts");
    }
}

fn plan_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn stdout(assert: Assert) -> String {
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

#[test]
fn plan_klassifiziert_selbst_apply_sortiert_ein_undo_stellt_her_und_der_zweite_plan_ist_leer() {
    let env = Env::new();
    env.corpus();
    env.scan();
    let before = env.snapshot();

    let (plan, assert) = env.plan("plan.json", &[]);
    assert.success().stderr(contains("zuerst klassifiziert"));
    assert_eq!(env.snapshot(), before, "plan verändert nichts");
    let json = plan_json(&plan);
    let actions = json["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    assert!(actions[0]["target"]
        .as_str()
        .unwrap()
        .ends_with(r"Finanzen\2026\2026-09-30_RE-2026-0042.pdf"));
    assert_eq!(actions[0]["rule"], "rechnungen");

    let out = stdout(
        env.bin()
            .arg("apply")
            .arg(&plan)
            .arg("--yes")
            .assert()
            .success(),
    );
    assert!(env
        .root()
        .join(r"Finanzen\2026\2026-09-30_RE-2026-0042.pdf")
        .is_file());
    assert!(!env.root().join(r"Downloads\scan0012.pdf").exists());

    // Idempotenz: scan → classify (der Cache ist für den neuen Pfad leer) → plan ohne Aktion
    let (again, assert) = env.plan("plan2.json", &[]);
    assert.success();
    assert!(plan_json(&again)["actions"].as_array().unwrap().is_empty());

    let run = out
        .split("Lauf ")
        .nth(1)
        .and_then(|r| r.split(':').next())
        .unwrap()
        .trim()
        .to_string();
    env.bin().args(["undo", &run, "--yes"]).assert().success();
    assert_eq!(env.snapshot(), before, "undo stellt alles wieder her");
}

#[test]
fn zweiter_plan_liest_den_cache_ohne_neue_klassifikation() {
    let env = Env::new();
    env.corpus();
    env.scan();
    env.bin().arg("classify").arg(env.root()).assert().success();
    let (_, assert) = env.plan("plan.json", &[]);
    let output = assert.success();
    let err = String::from_utf8_lossy(&output.get_output().stderr).into_owned();
    assert!(!err.contains("zuerst klassifiziert"), "{err}");
}

#[test]
fn no_classify_laesst_dateien_als_nicht_klassifiziert_stehen() {
    let env = Env::new();
    env.corpus();
    let (plan, assert) = env.plan("plan.json", &["--no-classify"]);
    assert
        .success()
        .stdout(contains("nicht klassifiziert (keine Aktion): 2"));
    let json = plan_json(&plan);
    assert!(json["actions"].as_array().unwrap().is_empty());
    let skipped = json["skipped"].as_array().unwrap();
    assert!(
        skipped.iter().any(|s| s["reason"] == "not-classified"),
        "{skipped:?}"
    );
}

#[test]
fn unsichere_kategorie_ergibt_keine_aktion_sondern_low_confidence_mit_top2() {
    let env = Env::new();
    env.write_bytes("Downloads/brief.pdf", &pdf_with_pages(&[WEAK], None, None));
    let (plan, assert) = env.plan("plan.json", &[]);
    assert.success().stdout(contains("zum Prüfen"));
    let json = plan_json(&plan);
    assert!(json["actions"].as_array().unwrap().is_empty());
    let skipped = json["skipped"].as_array().unwrap();
    let entry = skipped
        .iter()
        .find(|s| s["reason"] == "low-confidence")
        .unwrap_or_else(|| panic!("{skipped:?}"));
    assert!(
        entry["detail"].as_str().unwrap().starts_with("rechnung 0."),
        "{entry}"
    );
}

#[test]
fn unbekannte_kategorie_in_der_regel_bricht_vor_der_planung_ab() {
    let env = Env::new();
    env.corpus();
    env.rules("[[rules]]\nname = \"x\"\ncategory = \"gibtsnicht\"\ntarget = \"Y/\"\n");
    let (plan, assert) = env.plan("plan.json", &[]);
    assert
        .failure()
        .code(1)
        .stderr(contains("rules.toml:1"))
        .stderr(contains("Regel „x“"))
        .stderr(contains("unbekannte Kategorie"));
    assert!(!plan.exists());
}

#[test]
fn fehlendes_feld_ohne_ersatz_wird_uebersprungen() {
    let env = Env::new();
    env.corpus();
    env.rules("[[rules]]\nname = \"r\"\ncategory = \"rechnung\"\ntarget = \"Von/{doc.sender}/\"\n");
    let (plan, assert) = env.plan("plan.json", &[]);
    assert.success();
    let json = plan_json(&plan);
    assert!(json["actions"].as_array().unwrap().is_empty());
    let skipped = json["skipped"].as_array().unwrap();
    let entry = skipped
        .iter()
        .find(|s| s["reason"] == "missing-field")
        .unwrap();
    assert_eq!(entry["detail"], "doc.sender");
}

#[test]
fn min_dwell_haelt_frisch_angekommene_dateien_zurueck() {
    let env = Env::new();
    env.corpus();
    env.rules(
        "[[rules]]\nname = \"r\"\nglob = \"Downloads/**\"\next = [\"pdf\"]\nmin_dwell = \"14d\"\ntarget = \"Archiv/\"\n",
    );
    // Die Datei wurde gerade erst angelegt (Erstellzeit jetzt), auch wenn der mtime alt wäre.
    let (plan, assert) = env.plan("plan.json", &[]);
    assert.success();
    let json = plan_json(&plan);
    assert!(json["actions"].as_array().unwrap().is_empty());
    assert!(json["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["reason"] == "too-recent-arrival"));
}

#[test]
fn text_regex_und_felder_sortieren_nach_inhalt() {
    let env = Env::new();
    env.write_bytes(
        "Downloads/a.pdf",
        &pdf_with_pages(&["Telekom Deutschland GmbH Rechnung Rechnungsnummer: T-1001 Gesamtbetrag 10,00 EUR Rechnungsdatum 01.09.2026 Zahlungsziel 14 Tage"], None, None),
    );
    env.write_bytes(
        "Downloads/b.pdf",
        &pdf_with_pages(&["Vodafone GmbH Rechnung Rechnungsnummer: V-2002 Gesamtbetrag 20,00 EUR Rechnungsdatum 02.09.2026 Zahlungsziel 14 Tage"], None, None),
    );
    env.rules(
        "[[rules]]\nname = \"telekom\"\nglob = \"Downloads/**\"\ncategory = \"rechnung\"\ntext_regex = '(?i)telekom deutschland gmbh'\ntarget = \"Telekom/\"\n",
    );
    let (plan, assert) = env.plan("plan.json", &[]);
    assert.success();
    let json = plan_json(&plan);
    let actions = json["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert!(actions[0]["path"].as_str().unwrap().ends_with("a.pdf"));
}
