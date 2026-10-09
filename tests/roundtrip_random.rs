//! Rundreise über die echte CLI mit zufälligen Bäumen: alle Planer (dedupe, dedupe-dirs, junk,
//! versions, archive, rules, empty-dirs) in zufälliger Reihenfolge, jeweils scan → plan → apply, danach
//! alle Läufe rückwärts per `undo`. Erwartet wird der Ausgangsbaum byteidentisch (Pfade, Größen,
//! mtime, Inhalt).
//!
//! Der Zufall ist deterministisch (Seed je Fall). Einen fehlschlagenden Seed reproduziert man mit
//! `cargo test --test roundtrip_random -- rundreise::seed_<n>_<n>`; mehr Seeds mit
//! `ROUNDTRIP_SEEDS=200 cargo test --test roundtrip_random -- --ignored`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use assert_cmd::Command;
use ordner_cleanup::paths;

/// Pfad → (Größe, mtime, Inhalt); Ordner nur mit Existenz (ihre mtime ändert Windows selbst).
type Snapshot = BTreeMap<String, (u64, SystemTime, Vec<u8>)>;

const DAY: u64 = 86_400;

const RULES: &str = r#"
[[rules]]
name = "pdf"
ext = ["pdf"]
target = "Dokumente/{year}/"
strip_copy_suffix = true
"#;

/// xorshift64*: klein, deterministisch, ohne zusätzliche Abhängigkeit.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

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
        let config = env.home.path().join("config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("rules.toml"), RULES).unwrap();
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

    fn write(&self, rel: &str, content: &[u8], age_days: u64) {
        let path = self.root().join(rel);
        std::fs::create_dir_all(paths::extended(path.parent().unwrap())).unwrap();
        std::fs::write(paths::extended(&path), content).unwrap();
        if age_days > 0 {
            std::fs::File::options()
                .write(true)
                .open(paths::extended(&path))
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(age_days * DAY))
                .unwrap();
        }
    }

    fn mkdir(&self, rel: &str) {
        std::fs::create_dir_all(paths::extended(&self.root().join(rel))).unwrap();
    }

    /// `scan` und `plan <aktion>`; liefert Planpfad und Anzahl der Aktionen.
    fn plan(&self, action: &str, name: &str) -> (PathBuf, usize) {
        let plan = self.out.path().join(name);
        self.bin().arg("scan").arg(self.root()).assert().success();
        self.bin()
            .args(["plan", action])
            .arg(self.root())
            .arg("--out")
            .arg(&plan)
            .assert()
            .success();
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&plan).unwrap()).unwrap();
        let count = json["actions"].as_array().unwrap().len();
        (plan, count)
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
}

/// Lauf-ID aus „Lauf <id>: …“.
fn run_id(text: &str) -> String {
    let rest = text.split("Lauf ").nth(1).expect("Zeile „Lauf …“ fehlt");
    rest.split(':').next().unwrap().trim().to_string()
}

const DIRS: &[&str] = &[
    "Projekt",
    "Übergröße Ordner",
    "Fotos",
    "a",
    "Daten 2019",
    "Ärger & Co",
    "Downloads",
];

/// Zufälliger relativer Ordner mit Tiefe 0 bis 3 (`""` = Wurzel).
fn random_dir(rng: &mut Rng) -> String {
    let depth = rng.below(4);
    (0..depth)
        .map(|_| rng.pick(DIRS))
        .collect::<Vec<_>>()
        .join("/")
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// Baut einen Baum, der jedem Planer etwas zu tun gibt: normale Dateien, Duplikate,
/// Müll, Versionsgruppen, leere Ordner, alte Ordner und PDFs für die Regel.
fn random_tree(env: &Env, rng: &mut Rng) {
    let mut n = 0u32;
    let mut unique = |rng: &mut Rng| {
        n += 1;
        let len = 1 + rng.below(60);
        let mut bytes = format!("datei {n} ").into_bytes();
        bytes.extend((0..len).map(|_| b'a' + rng.below(26) as u8));
        bytes
    };

    // Normale Dateien, teils mit Duplikaten an anderer Stelle
    for i in 0..(5 + rng.below(15)) {
        let dir = random_dir(rng);
        let ext = rng.pick(&["txt", "docx", "jpg", "pdf", "csv"]);
        let content = unique(rng);
        let age = if rng.chance(40) {
            1 + rng.below(2000) as u64
        } else {
            0
        };
        env.write(&join(&dir, &format!("datei {i}.{ext}")), &content, age);
        for k in 0..rng.below(3) {
            let copy_dir = random_dir(rng);
            let copy_age = 1 + rng.below(1000) as u64;
            env.write(
                &join(&copy_dir, &format!("kopie {i}-{k}.{ext}")),
                &content,
                copy_age,
            );
        }
    }

    // Müll
    for i in 0..rng.below(5) {
        let dir = random_dir(rng);
        let name = match rng.below(4) {
            0 => "Thumbs.db".to_string(),
            1 => format!("cache{i}.tmp"),
            2 => format!("~$bericht{i}.docx"),
            _ => format!("film{i}.crdownload"),
        };
        let content = unique(rng);
        env.write(&join(&dir, &name), &content, 0);
    }

    // Versionsgruppen (ältere Versionen über der Mindestdauer von 30 Tagen)
    for g in 0..rng.below(3) {
        let dir = random_dir(rng);
        let ext = rng.pick(&["docx", "xlsx"]);
        let versions = 2 + rng.below(3);
        for v in 1..=versions {
            let content = unique(rng);
            let age = ((versions - v) as u64) * (40 + rng.below(200) as u64);
            env.write(
                &join(&dir, &format!("Bericht{g}_v{v}.{ext}")),
                &content,
                age,
            );
        }
    }

    // Alte Ordner (älter als 2 Jahre)
    for o in 0..rng.below(3) {
        let dir = join(&random_dir(rng), &format!("Alt{o}"));
        for f in 0..(1 + rng.below(3)) {
            let sub = if rng.chance(50) { "sub/" } else { "" };
            let content = unique(rng);
            env.write(
                &join(&dir, &format!("{sub}alt{f}.txt")),
                &content,
                (3 * 365 + rng.below(1000)) as u64,
            );
        }
    }

    // Ordnerkopien: Original und 1 bis 2 Kopien, teils mit Unterordner und mit Müll-Zusätzen
    // (Thumbs.db, desktop.ini) in einer Kopie, die beim Vergleich nicht zählen.
    for c in 0..rng.below(3) {
        let mut files = Vec::new();
        for f in 0..(1 + rng.below(3)) {
            let sub = if rng.chance(50) { "tief/" } else { "" };
            let age = 1 + rng.below(500) as u64;
            files.push((format!("{sub}f{f}.txt"), unique(rng), age));
        }
        for k in 0..=(1 + rng.below(2)) {
            let name = match (k, rng.chance(50)) {
                (0, _) => format!("Quelle{c}"),
                (_, true) => format!("Kopie von Quelle{c}"),
                (_, false) => format!("Quelle{c} ({k})"),
            };
            let dir = if k == 0 {
                join(&random_dir(rng), &name)
            } else {
                join(&random_dir(rng), &format!("{name} {k}"))
            };
            for (rel, content, age) in &files {
                env.write(&join(&dir, rel), content, age + k as u64 * 10);
            }
            if k > 0 && rng.chance(50) {
                let junk = unique(rng);
                env.write(&join(&dir, "Thumbs.db"), &junk, 1);
                let ini = unique(rng);
                env.write(&join(&dir, "tief/desktop.ini"), &ini, 1);
            }
        }
    }

    // Leere Ordner, auch verschachtelt
    for e in 0..rng.below(4) {
        let dir = join(&random_dir(rng), &format!("Leer{e}"));
        let nested = if rng.chance(50) { "/innen" } else { "" };
        env.mkdir(&format!("{dir}{nested}"));
    }
}

const ACTIONS: &[&str] = &[
    "dedupe",
    "dedupe-dirs",
    "junk",
    "versions",
    "archive",
    "rules",
    "empty-dirs",
];

fn roundtrip(seed: u64) {
    let mut rng = Rng::new(seed);
    let env = Env::new();
    random_tree(&env, &mut rng);
    let before = env.snapshot();

    let mut order: Vec<&str> = ACTIONS.to_vec();
    for i in (1..order.len()).rev() {
        order.swap(i, rng.below(i + 1));
    }

    let mut runs = Vec::new();
    for (step, action) in order.iter().enumerate() {
        let state = env.snapshot();
        let (plan, count) = env.plan(action, &format!("{step}-{action}.json"));
        assert_eq!(
            env.snapshot(),
            state,
            "seed {seed}: plan {action} darf nichts verändern"
        );
        eprintln!("seed {seed}: {action} → {count} Aktionen");
        if count == 0 {
            continue;
        }
        let out = env.bin().arg("apply").arg(&plan).arg("--yes").assert();
        let output = out.get_output();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert_eq!(
            output.status.code(),
            Some(0),
            "seed {seed}, Reihenfolge {order:?}: apply {action} meldet Fehler oder Übersprungenes\n\
             {stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        runs.push((action, run_id(&stdout)));

        let (_, again) = env.plan(action, &format!("{step}-{action}-again.json"));
        assert_eq!(
            again, 0,
            "seed {seed}, Reihenfolge {order:?}: {action} ist nach apply nicht idempotent"
        );
    }

    for (action, run) in runs.iter().rev() {
        let out = env.bin().args(["undo", run, "--yes"]).assert();
        let output = out.get_output();
        assert_eq!(
            output.status.code(),
            Some(0),
            "seed {seed}, Reihenfolge {order:?}: undo {action} ({run})\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let after = env.snapshot();
    if after != before {
        let missing: Vec<_> = before.keys().filter(|k| !after.contains_key(*k)).collect();
        let extra: Vec<_> = after.keys().filter(|k| !before.contains_key(*k)).collect();
        let changed: Vec<_> = before
            .iter()
            .filter(|(k, v)| after.get(*k).is_some_and(|a| a != *v))
            .map(|(k, _)| k)
            .collect();
        panic!(
            "seed {seed}, Reihenfolge {order:?}: Baum nach undo nicht wie vorher\n\
             fehlt: {missing:?}\nzusätzlich: {extra:?}\ngeändert: {changed:?}"
        );
    }
}

#[rstest::rstest]
fn rundreise(#[values(1, 2, 3, 4, 5, 6, 7, 8)] seed: u64) {
    roundtrip(seed);
}

/// Viele Seeds für gründliche Läufe von Hand (`ROUNDTRIP_SEEDS`, Default 100).
#[test]
#[ignore = "langsam; von Hand mit --ignored starten"]
fn rundreise_viele_seeds() {
    let count: u64 = std::env::var("ROUNDTRIP_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    for seed in 1..=count {
        roundtrip(1000 + seed);
    }
}
