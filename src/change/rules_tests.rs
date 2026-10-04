//! Tests des Planers `plan rules` mit In-Memory-Index und Fakes für EXIF und Inhaltsvergleich.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use chrono::{NaiveDate, NaiveDateTime, TimeZone};

use super::plan::{ActionType, PlanKind};
use super::protect::{ProtectPaths, Protector};
use super::rules::{plan_rules, ContentSource, ExifResult, ExifSource, RulesEnv, RulesPlan};
use super::SkipReason;
use crate::config::Config;
use crate::index::{DirRecord, FileRecord, FileRow, Index};
use crate::paths;
use crate::rules::RuleSet;
use crate::scan::source::TICKS_PER_SEC;

const ROOT: &str = r"Z:\Root";
/// 2026-10-03 00:00:00 UTC
const NOW: i64 = 1_790_985_600 * TICKS_PER_SEC;
const DAY: i64 = 86_400 * TICKS_PER_SEC;

fn local_ticks(y: i32, m: u32, d: u32) -> i64 {
    let local = NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap();
    chrono::Local
        .from_local_datetime(&local)
        .unwrap()
        .timestamp()
        * TICKS_PER_SEC
}

fn dt(y: i32, m: u32, d: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(10, 0, 0)
        .unwrap()
}

#[derive(Clone)]
struct F {
    path: &'static str,
    size: i64,
    mtime: i64,
    cloud: bool,
    link: bool,
}

fn file(path: &'static str) -> F {
    F {
        path,
        size: 100,
        mtime: local_ticks(2026, 3, 7),
        cloud: false,
        link: false,
    }
}

impl F {
    fn size(mut self, size: i64) -> Self {
        self.size = size;
        self
    }
    fn mtime(mut self, mtime: i64) -> Self {
        self.mtime = mtime;
        self
    }
    fn cloud(mut self) -> Self {
        self.cloud = true;
        self
    }
    fn link(mut self) -> Self {
        self.link = true;
        self
    }
}

/// Index mit allen Ordnern auf dem Weg zu den Dateien plus `extra_dirs` (leere Ordner).
fn seed(files: &[F], extra_dirs: &[&str]) -> Index {
    let mut index = Index::open_in_memory().unwrap();
    let root_key = paths::dir_key(Path::new(ROOT));
    let run = index.begin_root(ROOT, &root_key, "t").unwrap();
    let mut dirs: Vec<String> = vec![ROOT.to_string()];
    for d in extra_dirs {
        dirs.push((*d).to_string());
    }
    for f in files {
        let mut p = Path::new(f.path).parent();
        while let Some(dir) = p {
            let s = dir.to_string_lossy().into_owned();
            if s.len() <= ROOT.len() {
                break;
            }
            dirs.push(s);
            p = dir.parent();
        }
    }
    dirs.sort();
    dirs.dedup();
    let records: Vec<DirRecord> = dirs
        .iter()
        .map(|d| {
            let p = Path::new(d);
            DirRecord {
                path: d.clone(),
                path_key: paths::dir_key(p),
                parent_key: (d != ROOT).then(|| paths::dir_key(p.parent().unwrap())),
                mode: "full".into(),
                attrs: 0x10,
                direct_entries: 1,
                ..DirRecord::default()
            }
        })
        .collect();
    index.upsert_dirs(&records, run.generation).unwrap();
    let file_records: Vec<FileRecord> = files
        .iter()
        .map(|f| {
            let p = Path::new(f.path);
            FileRecord {
                dir_key: paths::dir_key(p.parent().unwrap()),
                path: f.path.to_string(),
                path_key: paths::path_key(p),
                name: p.file_name().unwrap().to_string_lossy().into_owned(),
                size: f.size,
                mtime: f.mtime,
                attrs: if f.cloud { 0x40_0000 } else { 0x20 },
                cloud_only: f.cloud,
                is_link: f.link,
                ..FileRecord::default()
            }
        })
        .collect();
    index.upsert_files(&file_records, run.generation).unwrap();
    index
}

/// Fake: Aufnahmedaten nach Pfad (kleingeschrieben, siehe `paths::path_key`); zählt Aufrufe.
#[derive(Default)]
struct FakeExif {
    dates: HashMap<String, ExifResult>,
    asked: Mutex<Vec<String>>,
}

impl FakeExif {
    fn with(mut self, path: &str, result: ExifResult) -> Self {
        self.dates.insert(paths::path_key(Path::new(path)), result);
        self
    }
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl ExifSource for FakeExif {
    fn taken(&self, _index: &mut Index, files: &[&FileRow]) -> Vec<ExifResult> {
        let mut asked = self.asked.lock().unwrap();
        files
            .iter()
            .map(|f| {
                asked.push(f.key.clone());
                self.dates.get(&f.key).copied().unwrap_or(ExifResult::NoDate)
            })
            .collect()
    }
}

/// Fake: Dateien mit gleicher ID haben denselben Inhalt.
#[derive(Default)]
struct FakeContent {
    ids: HashMap<String, u32>,
}

impl FakeContent {
    fn same(mut self, paths_with_same_content: &[&str]) -> Self {
        let id = self.ids.len() as u32 + 1;
        for p in paths_with_same_content {
            self.ids.insert(paths::path_key(Path::new(p)), id);
        }
        self
    }
}

impl ContentSource for FakeContent {
    fn same_content(&self, a: &FileRow, b: &FileRow) -> bool {
        match (self.ids.get(&a.key), self.ids.get(&b.key)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
    }
}

fn parse(text: &str) -> RuleSet {
    RuleSet::parse(text, Path::new("rules.toml")).unwrap()
}

fn protector(config: &Config) -> Protector {
    Protector::new(Path::new(ROOT), config, &ProtectPaths::default())
}

fn run_full(
    index: &mut Index,
    rules: &RuleSet,
    exif: &FakeExif,
    content: &FakeContent,
    config: &Config,
) -> RulesPlan {
    let protector = protector(config);
    let env = RulesEnv {
        protector: &protector,
        exif,
        content,
        created: "t",
        now_ticks: NOW,
    };
    let result = plan_rules(index, Path::new(ROOT), rules, &env).unwrap();
    result.plan.validate().unwrap();
    assert_eq!(result.plan.kind, PlanKind::Rules);
    result
}

fn run(index: &mut Index, rules: &RuleSet) -> RulesPlan {
    run_full(
        index,
        rules,
        &FakeExif::default(),
        &FakeContent::default(),
        &Config::default(),
    )
}

/// Aktionen als `(Quelle relativ, Ziel relativ)`.
fn moves(plan: &RulesPlan) -> Vec<(String, String)> {
    let strip = |p: &str| p.trim_start_matches(&format!("{ROOT}\\")).to_string();
    plan.plan
        .actions
        .iter()
        .map(|a| (strip(&a.path), strip(a.target.as_deref().unwrap())))
        .collect()
}

fn mv(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

fn skipped(plan: &RulesPlan) -> Vec<(String, SkipReason)> {
    plan.plan
        .skipped
        .iter()
        .map(|s| {
            (
                s.path.trim_start_matches(&format!("{ROOT}\\")).to_string(),
                s.reason,
            )
        })
        .collect()
}

const PDF_RULE: &str = r#"
[[rules]]
name = "pdf"
ext = ["pdf"]
target = "Dokumente/"
"#;

#[test]
fn nichts_wird_veraendert_und_der_plan_ist_gueltig() {
    let mut index = seed(&[file(r"Z:\Root\Downloads\a.pdf")], &[]);
    let p = run(&mut index, &parse(PDF_RULE));
    assert_eq!(moves(&p), [mv(r"Downloads\a.pdf", r"Dokumente\a.pdf")]);
    let a = &p.plan.actions[0];
    assert_eq!(a.action, ActionType::Move);
    assert_eq!(a.rule.as_deref(), Some("pdf"));
    assert_eq!(a.reason, "rule:pdf");
    assert_eq!(a.id, 1);
    assert_eq!(p.bytes, 100);
    assert_eq!(p.plan.params["rules_file"], "rules.toml");
}

#[test]
fn die_erste_passende_regel_gewinnt() {
    let rules = parse(
        r#"
[[rules]]
name = "a"
ext = ["pdf"]
target = "A/"
[[rules]]
name = "b"
ext = ["pdf"]
target = "B/"
"#,
    );
    let mut index = seed(&[file(r"Z:\Root\x.pdf")], &[]);
    let p = run(&mut index, &rules);
    assert_eq!(moves(&p), [mv("x.pdf", r"A\x.pdf")]);
    assert_eq!(p.per_rule[0].actions, 1);
    assert_eq!(p.per_rule[1].actions, 0);
}

#[test]
fn rule_filter_beruecksichtigt_nur_die_gewaehlten_regeln() {
    let rules = parse(
        r#"
[[rules]]
name = "a"
ext = ["pdf"]
target = "A/"
[[rules]]
name = "b"
ext = ["txt"]
target = "B/"
"#,
    )
    .select(&["b".to_string()])
    .unwrap();
    let mut index = seed(&[file(r"Z:\Root\x.pdf"), file(r"Z:\Root\y.txt")], &[]);
    let p = run(&mut index, &rules);
    assert_eq!(moves(&p), [mv("y.txt", r"B\y.txt")]);
    assert_eq!(p.unmatched, 1);
}

#[test]
fn bedingungen_sind_und_verknuepft() {
    let rules = parse(
        r#"
[[rules]]
name = "r"
glob = "Downloads/**"
ext = ["pdf"]
min_size = "50B"
max_size = "500B"
min_age = "7d"
target = "Ziel/"
"#,
    );
    let old = NOW - 30 * DAY;
    let mut index = seed(
        &[
            file(r"Z:\Root\Downloads\ok.pdf").mtime(old),
            file(r"Z:\Root\Downloads\jung.pdf").mtime(NOW - DAY),
            file(r"Z:\Root\Downloads\klein.pdf").mtime(old).size(10),
            file(r"Z:\Root\Downloads\gross.pdf").mtime(old).size(501),
            file(r"Z:\Root\Downloads\falsch.txt").mtime(old),
            file(r"Z:\Root\Anders\ok.pdf").mtime(old),
        ],
        &[],
    );
    let p = run(&mut index, &rules);
    assert_eq!(moves(&p), [mv(r"Downloads\ok.pdf", r"Ziel\ok.pdf")]);
    assert_eq!(p.unmatched, 5);
}

#[test]
fn platzhalter_parent_jahr_und_regex_gruppen() {
    let rules = parse(
        r#"
[[rules]]
name = "r"
name_regex = '^(\w+)_(\d+)\.txt$'
target = "{parent}/{1}/{year}-{month}-{day}_{2}.{ext}"
"#,
    );
    let mut index = seed(&[file(r"Z:\Root\Eingang\Rechnung_4711.txt")], &[]);
    let p = run(&mut index, &rules);
    assert_eq!(
        moves(&p),
        [mv(
            r"Eingang\Rechnung_4711.txt",
            r"Eingang\Rechnung\2026-03-07_4711.txt"
        )]
    );
}

#[test]
fn parent_einer_datei_in_der_wurzel_ist_der_wurzelname() {
    let rules = parse(
        r#"
[[rules]]
name = "r"
ext = ["txt"]
target = "{parent}/"
"#,
    );
    let mut index = seed(&[file(r"Z:\Root\a.txt")], &[]);
    let p = run(&mut index, &rules);
    assert_eq!(moves(&p), [mv("a.txt", r"Root\a.txt")]);
}

const FOTO_RULE: &str = r#"
[[rules]]
name = "fotos"
ext = ["jpg"]
target = "Fotos/{exif.date:%Y}/{name}.{ext}"
"#;

#[test]
fn exif_datum_hat_vorrang_vor_mtime() {
    let mut index = seed(&[file(r"Z:\Root\in\a.jpg")], &[]);
    let exif = FakeExif::default().with(r"Z:\Root\in\a.jpg", ExifResult::Date(dt(2019, 8, 15)));
    let p = run_full(
        &mut index,
        &parse(FOTO_RULE),
        &exif,
        &FakeContent::default(),
        &Config::default(),
    );
    assert_eq!(moves(&p), [mv(r"in\a.jpg", r"Fotos\2019\a.jpg")]);
}

#[test]
fn ohne_exif_greift_der_fallback_mtime_oder_skip() {
    let mut index = seed(&[file(r"Z:\Root\in\a.jpg")], &[]);
    let p = run(&mut index, &parse(FOTO_RULE));
    assert_eq!(moves(&p), [mv(r"in\a.jpg", r"Fotos\2026\a.jpg")]);

    let skip_rule = FOTO_RULE.to_string() + "exif_fallback = \"skip\"\n";
    let p = run(&mut index, &parse(&skip_rule));
    assert!(p.plan.actions.is_empty());
    assert_eq!(skipped(&p), [(r"in\a.jpg".into(), SkipReason::NoExifDate)]);
}

#[test]
fn exif_wird_nur_fuer_noetige_dateien_abgefragt() {
    let mut index = seed(
        &[
            file(r"Z:\Root\a.jpg"),
            file(r"Z:\Root\b.txt"),
            file(r"Z:\Root\c.jpg").cloud(),
        ],
        &[],
    );
    let exif = FakeExif::default();
    let rules = parse(
        r#"
[[rules]]
name = "fotos"
ext = ["jpg"]
target = "Fotos/{exif.date:%Y}/"
[[rules]]
name = "txt"
ext = ["txt"]
target = "T/"
"#,
    );
    let p = run_full(&mut index, &rules, &exif, &FakeContent::default(), &Config::default());
    assert_eq!(exif.asked(), [paths::path_key(Path::new(r"Z:\Root\a.jpg"))]);
    assert_eq!(
        skipped(&p),
        [("c.jpg".into(), SkipReason::CloudPlaceholder)]
    );
}

#[test]
fn cloud_antwort_der_exif_quelle_ueberspringt_die_datei() {
    let mut index = seed(&[file(r"Z:\Root\a.jpg")], &[]);
    let exif = FakeExif::default().with(r"Z:\Root\a.jpg", ExifResult::Cloud);
    let p = run_full(
        &mut index,
        &parse(FOTO_RULE),
        &exif,
        &FakeContent::default(),
        &Config::default(),
    );
    assert_eq!(skipped(&p), [("a.jpg".into(), SkipReason::CloudPlaceholder)]);
}

#[test]
fn datei_die_schon_am_ziel_liegt_ergibt_keine_aktion() {
    let mut index = seed(&[file(r"Z:\Root\Dokumente\a.pdf")], &[]);
    let p = run(&mut index, &parse(PDF_RULE));
    assert!(p.plan.actions.is_empty() && p.plan.skipped.is_empty());
}

#[test]
fn ziel_das_sich_nur_in_der_schreibweise_unterscheidet_wird_umbenannt() {
    let rules = parse(
        r#"
[[rules]]
name = "klein"
ext = ["jpg"]
target = "./{name}.{ext}"
lowercase_ext = true
"#,
    );
    let mut index = seed(&[file(r"Z:\Root\Fotos\foto.JPG")], &[]);
    let p = run(&mut index, &rules);
    assert_eq!(moves(&p), [mv(r"Fotos\foto.JPG", r"Fotos\foto.jpg")]);

    // Nach dem Umbenennen und Scan ist der nächste Plan leer.
    let mut after = seed(&[file(r"Z:\Root\Fotos\foto.jpg")], &[]);
    assert!(run(&mut after, &rules).plan.actions.is_empty());
}

#[test]
fn vorhandener_ordner_in_anderer_schreibweise_wird_weiterverwendet() {
    let rules = parse(
        r#"
[[rules]]
name = "r"
ext = ["pdf"]
target = "dokumente/"
"#,
    );
    let mut index = seed(
        &[
            file(r"Z:\Root\x\a.pdf"),
            file(r"Z:\Root\Dokumente\schon.pdf"),
        ],
        &[],
    );
    let p = run(&mut index, &rules);
    assert_eq!(moves(&p), [mv(r"x\a.pdf", r"Dokumente\a.pdf")]);
    let mut after = seed(
        &[
            file(r"Z:\Root\Dokumente\a.pdf"),
            file(r"Z:\Root\Dokumente\schon.pdf"),
        ],
        &[],
    );
    assert!(run(&mut after, &rules).plan.actions.is_empty(), "idempotent");
}

#[test]
fn identischer_inhalt_am_ziel_ist_ein_duplikat() {
    let mut index = seed(
        &[
            file(r"Z:\Root\x\a.pdf"),
            file(r"Z:\Root\Dokumente\a.pdf"),
        ],
        &[],
    );
    let content = FakeContent::default().same(&[r"Z:\Root\x\a.pdf", r"Z:\Root\Dokumente\a.pdf"]);
    let p = run_full(
        &mut index,
        &parse(PDF_RULE),
        &FakeExif::default(),
        &content,
        &Config::default(),
    );
    assert!(p.plan.actions.is_empty());
    assert_eq!(
        skipped(&p),
        [(r"x\a.pdf".into(), SkipReason::DuplicateAtTarget)]
    );
}

#[test]
fn anderer_inhalt_am_ziel_wird_durchnummeriert() {
    let mut index = seed(
        &[
            file(r"Z:\Root\x\a.pdf"),
            file(r"Z:\Root\Dokumente\a.pdf").size(7),
            file(r"Z:\Root\Dokumente\a (2).pdf").size(8),
        ],
        &[],
    );
    let p = run(&mut index, &parse(PDF_RULE));
    assert_eq!(moves(&p), [mv(r"x\a.pdf", r"Dokumente\a (3).pdf")]);
}

#[test]
fn mehrere_quellen_mit_gleichem_ziel_werden_in_stabiler_reihenfolge_nummeriert() {
    let files = [
        file(r"Z:\Root\c\a.pdf"),
        file(r"Z:\Root\a\a.pdf"),
        file(r"Z:\Root\b\A.PDF"),
    ];
    let mut first = seed(&files, &[]);
    let mut second = seed(&[files[2].clone(), files[0].clone(), files[1].clone()], &[]);
    let rules = parse(PDF_RULE);
    let p1 = run(&mut first, &rules);
    let p2 = run(&mut second, &rules);
    assert_eq!(
        moves(&p1),
        [
            mv(r"a\a.pdf", r"Dokumente\a.pdf"),
            mv(r"b\A.PDF", r"Dokumente\A (2).PDF"),
            mv(r"c\a.pdf", r"Dokumente\a (3).pdf"),
        ]
    );
    assert_eq!(p1.plan, p2.plan, "zweimal geplant = identischer Plan");
}

#[test]
fn nach_dem_durchnummerieren_ist_der_naechste_lauf_leer() {
    let rules = parse(
        r#"
[[rules]]
name = "pdf"
ext = ["pdf"]
target = "Dokumente/"
strip_copy_suffix = true
"#,
    );
    let mut after = seed(
        &[
            file(r"Z:\Root\Dokumente\a.pdf").size(1),
            file(r"Z:\Root\Dokumente\a (2).pdf").size(2),
            file(r"Z:\Root\Dokumente\a (3).pdf").size(3),
        ],
        &[],
    );
    let p = run(&mut after, &rules);
    assert!(
        p.plan.actions.is_empty(),
        "kein Hin und Her: {:?}",
        moves(&p)
    );
}

#[test]
fn ordner_am_ziel_und_datei_im_zielpfad_sind_ungueltige_ziele() {
    let mut index = seed(
        &[file(r"Z:\Root\x\a.pdf"), file(r"Z:\Root\y\b.pdf")],
        &[r"Z:\Root\Dokumente\a.pdf"],
    );
    let p = run(&mut index, &parse(PDF_RULE));
    assert_eq!(
        skipped(&p).into_iter().find(|s| s.0 == r"x\a.pdf"),
        Some((r"x\a.pdf".into(), SkipReason::InvalidTarget)),
        "Ordner liegt am Zielpfad"
    );
    assert_eq!(moves(&p), [mv(r"y\b.pdf", r"Dokumente\b.pdf")]);

    let mut index = seed(
        &[file(r"Z:\Root\x\a.pdf"), file(r"Z:\Root\Dokumente")],
        &[],
    );
    let p = run(&mut index, &parse(PDF_RULE));
    assert_eq!(skipped(&p), [(r"x\a.pdf".into(), SkipReason::InvalidTarget)]);
}

#[test]
fn ungueltige_namen_und_gruppen_werden_uebersprungen() {
    let rules = parse(
        r#"
[[rules]]
name = "r"
name_regex = '^(.*)\.txt$'
target = "{1}/"
"#,
    );
    let mut index = seed(
        &[
            file(r"Z:\Root\ok.txt"),
            file(r"Z:\Root\CON.txt"),
            file(r"Z:\Root\leer .txt"),
            file(r"Z:\Root\punkt..txt"),
            file(r"Z:\Root\.txt"),
        ],
        &[],
    );
    let p = run(&mut index, &rules);
    assert_eq!(moves(&p), [mv("ok.txt", r"ok\ok.txt")]);
    let reasons: Vec<_> = skipped(&p).into_iter().map(|s| s.1).collect();
    assert!(reasons.iter().all(|r| *r == SkipReason::InvalidTarget), "{reasons:?}");
    assert_eq!(reasons.len(), 4);
}

#[test]
fn punkt_punkt_aus_einer_gruppe_kann_nicht_ausbrechen() {
    let rules = parse(
        r#"
[[rules]]
name = "r"
name_regex = '^x(.*)\.txt$'
target = "{1}/"
"#,
    );
    let mut index = seed(&[file(r"Z:\Root\x...txt")], &[]);
    let p = run(&mut index, &rules);
    assert!(p.plan.actions.is_empty());
    assert_eq!(skipped(&p), [("x...txt".into(), SkipReason::InvalidTarget)]);
}

#[test]
fn ziele_ueber_260_zeichen_werden_uebersprungen() {
    let long = "l".repeat(250);
    let rules = parse(&format!(
        "[[rules]]\nname = \"r\"\next = [\"pdf\"]\ntarget = \"{long}/\"\n"
    ));
    let mut index = seed(&[file(r"Z:\Root\a.pdf")], &[]);
    let p = run(&mut index, &rules);
    assert_eq!(skipped(&p), [("a.pdf".into(), SkipReason::PathTooLong)]);
}

#[test]
fn geschuetzte_quelle_und_geschuetztes_ziel_werden_uebersprungen() {
    let config = Config {
        protected_paths: vec![r"Z:\Root\Dokumente\Tabu".into(), r"Z:\Root\p".into()],
        ..Config::default()
    };
    let rules = parse(
        r#"
[[rules]]
name = "tabu"
glob = "t/**"
target = "Dokumente/Tabu/"
[[rules]]
name = "pdf"
ext = ["pdf"]
target = "Dokumente/"
"#,
    );
    let mut index = seed(
        &[
            file(r"Z:\Root\t\a.txt"),
            file(r"Z:\Root\p\b.pdf"),
            file(r"Z:\Root\q\c.pdf"),
        ],
        &[],
    );
    let p = run_full(
        &mut index,
        &rules,
        &FakeExif::default(),
        &FakeContent::default(),
        &config,
    );
    assert_eq!(moves(&p), [mv(r"q\c.pdf", r"Dokumente\c.pdf")]);
    assert_eq!(
        skipped(&p),
        [
            (r"p\b.pdf".into(), SkipReason::Protected),
            (r"t\a.txt".into(), SkipReason::Protected)
        ]
    );
}

#[test]
fn ziel_unter_archiv_oder_werkzeugordner_ist_geschuetzt() {
    for target in ["_Archiv/neu/", ".ordner-cleanup/x/"] {
        let rules = parse(&format!(
            "[[rules]]\nname = \"r\"\next = [\"pdf\"]\ntarget = \"{target}\"\n"
        ));
        let mut index = seed(&[file(r"Z:\Root\a.pdf")], &[]);
        let p = run(&mut index, &rules);
        assert!(p.plan.actions.is_empty(), "{target}");
        assert_eq!(skipped(&p), [("a.pdf".into(), SkipReason::Protected)]);
    }
}

#[test]
fn archiv_und_werkzeugordner_sind_keine_quellen() {
    let mut index = seed(
        &[
            file(r"Z:\Root\_Archiv\2020\a.pdf"),
            file(r"Z:\Root\.ordner-cleanup\q\b.pdf"),
        ],
        &[],
    );
    let p = run(&mut index, &parse(PDF_RULE));
    assert!(p.plan.actions.is_empty() && p.plan.skipped.is_empty());
    assert_eq!(p.unmatched, 0);
}

#[test]
fn cloud_platzhalter_und_links_werden_nicht_bewegt() {
    let mut index = seed(
        &[
            file(r"Z:\Root\a.pdf").cloud(),
            file(r"Z:\Root\b.pdf").link(),
            file(r"Z:\Root\c.pdf"),
        ],
        &[],
    );
    let p = run(&mut index, &parse(PDF_RULE));
    assert_eq!(moves(&p), [mv("c.pdf", r"Dokumente\c.pdf")]);
    assert_eq!(
        skipped(&p),
        [
            ("a.pdf".into(), SkipReason::CloudPlaceholder),
            ("b.pdf".into(), SkipReason::Link)
        ]
    );
}

#[test]
fn nicht_passende_dateien_werden_gezaehlt_und_beispiele_gesammelt() {
    let mut index = seed(
        &[
            file(r"Z:\Root\a.pdf"),
            file(r"Z:\Root\b.pdf"),
            file(r"Z:\Root\c.pdf"),
            file(r"Z:\Root\d.pdf"),
            file(r"Z:\Root\e.txt"),
        ],
        &[],
    );
    let p = run(&mut index, &parse(PDF_RULE));
    assert_eq!(p.unmatched, 1);
    assert_eq!(p.per_rule[0].actions, 4);
    assert_eq!(p.per_rule[0].bytes, 400);
    assert_eq!(
        p.per_rule[0].examples[0],
        ("a.pdf".to_string(), "Dokumente/a.pdf".to_string())
    );
    assert_eq!(p.per_rule[0].examples.len(), 3);
}

#[test]
fn ziel_eines_anderen_laufs_wird_nicht_zur_quelle_einer_aktion() {
    // a.pdf soll nach Dokumente\a.pdf; Dokumente\a.pdf selbst ist als Quelle gar nicht betroffen.
    let rules = parse(
        r#"
[[rules]]
name = "pdf"
ext = ["pdf"]
target = "Dokumente/{name}.{ext}"
"#,
    );
    let mut index = seed(
        &[file(r"Z:\Root\a.pdf"), file(r"Z:\Root\Dokumente\a.pdf").size(5)],
        &[],
    );
    let p = run(&mut index, &rules);
    assert_eq!(moves(&p), [mv("a.pdf", r"Dokumente\a (2).pdf")]);
}

#[test]
fn stabilitaetshinweis_wenn_die_regel_ihre_eigenen_ziele_verschieben_wuerde() {
    let rules = parse(
        r#"
[[rules]]
name = "eltern"
ext = ["txt"]
target = "./{parent}_{name}.{ext}"
"#,
    );
    let mut index = seed(&[file(r"Z:\Root\Ordner\a.txt")], &[]);
    let p = run(&mut index, &rules);
    assert_eq!(moves(&p), [mv(r"Ordner\a.txt", r"Ordner\Ordner_a.txt")]);
    assert_eq!(p.notes.len(), 1, "{:?}", p.notes);
    assert!(p.notes[0].contains("eltern"), "{}", p.notes[0]);
}

#[test]
fn stabilitaetshinweis_bei_einer_kette_von_regel_a_zu_regel_b() {
    let rules = parse(
        r#"
[[rules]]
name = "a"
glob = "Eingang/**"
target = "Zwischen/"
[[rules]]
name = "b"
glob = "Zwischen/**"
target = "Final/"
"#,
    );
    let mut index = seed(
        &[file(r"Z:\Root\Eingang\x.txt"), file(r"Z:\Root\Eingang\y.txt")],
        &[],
    );
    let p = run(&mut index, &rules);
    assert_eq!(p.plan.actions.len(), 2);
    assert_eq!(p.notes.len(), 1, "einmal je Regelpaar: {:?}", p.notes);
    assert!(p.notes[0].contains("„a“") && p.notes[0].contains("„b“"));
}

#[test]
fn stabile_regeln_erzeugen_keinen_hinweis() {
    let mut index = seed(&[file(r"Z:\Root\x\a.pdf")], &[]);
    assert!(run(&mut index, &parse(PDF_RULE)).notes.is_empty());
}

#[test]
fn skipped_ist_nach_pfad_sortiert_und_ids_sind_fortlaufend() {
    let mut index = seed(
        &[
            file(r"Z:\Root\b\2.pdf"),
            file(r"Z:\Root\a\1.pdf"),
            file(r"Z:\Root\c\3.pdf").cloud(),
            file(r"Z:\Root\a\0.pdf").cloud(),
        ],
        &[],
    );
    let p = run(&mut index, &parse(PDF_RULE));
    let ids: Vec<_> = p.plan.actions.iter().map(|a| a.id).collect();
    assert_eq!(ids, [1, 2]);
    let order: Vec<_> = skipped(&p).into_iter().map(|s| s.0).collect();
    assert_eq!(order, [r"a\0.pdf", r"c\3.pdf"]);
}

mod cached_exif {
    use super::*;
    use crate::change::rules::CachedExif;
    use crate::scan::exif::testing::{jpeg_with_date, DATE_TIME_ORIGINAL};

    fn row(path: &Path) -> FileRow {
        let meta = std::fs::metadata(path).unwrap();
        FileRow {
            path: path.to_string_lossy().into_owned(),
            key: paths::path_key(path),
            dir_key: paths::dir_key(path.parent().unwrap()),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            size: meta.len() as i64,
            mtime: 1234,
            attrs: 0x20,
            cloud_only: false,
            is_link: false,
        }
    }

    #[test]
    fn liest_einmal_und_beantwortet_den_zweiten_lauf_aus_dem_cache() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("a.jpg");
        let none = dir.path().join("b.jpg");
        std::fs::write(&photo, jpeg_with_date(DATE_TIME_ORIGINAL, dt(2019, 8, 15))).unwrap();
        std::fs::write(&none, b"kein bild").unwrap();
        let (a, b) = (row(&photo), row(&none));
        let mut index = Index::open_in_memory().unwrap();

        let first = CachedExif.taken(&mut index, &[&a, &b]);
        assert_eq!(first, [ExifResult::Date(dt(2019, 8, 15)), ExifResult::NoDate]);

        // Dateien weg: Ein Treffer im Cache liest nichts mehr.
        std::fs::remove_file(&photo).unwrap();
        std::fs::remove_file(&none).unwrap();
        let second = CachedExif.taken(&mut index, &[&a, &b]);
        assert_eq!(second, first);
    }

    #[test]
    fn geaenderte_datei_wird_neu_gelesen() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("a.jpg");
        std::fs::write(&photo, jpeg_with_date(DATE_TIME_ORIGINAL, dt(2019, 8, 15))).unwrap();
        let mut a = row(&photo);
        let mut index = Index::open_in_memory().unwrap();
        CachedExif.taken(&mut index, &[&a]);
        std::fs::write(&photo, jpeg_with_date(DATE_TIME_ORIGINAL, dt(2020, 1, 2))).unwrap();
        a.mtime += 1;
        assert_eq!(
            CachedExif.taken(&mut index, &[&a]),
            [ExifResult::Date(dt(2020, 1, 2))]
        );
    }

    #[test]
    fn platzhalter_wird_gemeldet_und_nicht_gemerkt() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("a.jpg");
        std::fs::write(&photo, jpeg_with_date(DATE_TIME_ORIGINAL, dt(2019, 8, 15))).unwrap();
        let wide: Vec<u16> = photo.as_os_str().encode_wide().chain(Some(0)).collect();
        assert_ne!(unsafe { SetFileAttributesW(wide.as_ptr(), 0x1000) }, 0);
        let a = row(&photo);
        let mut index = Index::open_in_memory().unwrap();
        assert_eq!(CachedExif.taken(&mut index, &[&a]), [ExifResult::Cloud]);
        assert!(index
            .exif_lookup(&[(a.key.as_str(), a.size, a.mtime)])
            .unwrap()
            .is_empty());
    }
}
