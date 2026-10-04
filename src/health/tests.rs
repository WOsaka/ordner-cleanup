use super::*;
use crate::index::{DupFile, RootStatus};
use crate::platform::FileAttrs;
use crate::scan::source::TICKS_PER_SEC;

const ROOT: &str = r"Z:\Root";
const DAY: i64 = 86_400 * TICKS_PER_SEC;
const NOW: i64 = 4_000 * DAY;

fn file(path: &str, size: u64) -> FRow {
    let p = Path::new(path);
    FRow {
        path: path.to_string(),
        dir_key: paths::dir_key(p.parent().unwrap()),
        name: p.file_name().unwrap().to_string_lossy().into_owned(),
        ext: p.extension().map(|e| e.to_string_lossy().to_lowercase()),
        size,
        mtime: NOW - 10 * DAY,
        attrs: FileAttrs(0x20),
        cloud: false,
        is_link: false,
        full_hash: None,
        counted: true,
    }
}

fn dir(path: &str, entries: u64) -> DRow {
    let p = Path::new(path);
    let depth = paths::path_key(p).matches('\\').count() as i64;
    DRow {
        path: path.to_string(),
        key: paths::dir_key(p),
        parent: p.parent().map(paths::dir_key),
        depth,
        summary: false,
        is_link: false,
        direct_entries: entries,
        summary_size: 0,
        summary_files: 0,
    }
}

fn root() -> ReportRoot {
    ReportRoot {
        path: ROOT.to_string(),
        dir_key: paths::dir_key(Path::new(ROOT)),
        scanned_at: None,
        status: RootStatus::Complete,
    }
}

fn run(files: &[FRow], dirs: &[DRow], dups: &[DupGroup]) -> Snapshot {
    run_with(files, dirs, dups, &Weights::default())
}

fn run_with(files: &[FRow], dirs: &[DRow], dups: &[DupGroup], weights: &Weights) -> Snapshot {
    let config = crate::config::Config::default();
    let categories = config.junk_categories.clone();
    let junk = JunkClassifier::new(
        &crate::change::junk::JunkOptions {
            categories: &categories,
            rules: &[],
            downloads_dirs: &[],
            installer_min_age_days: 90,
            now_ticks: NOW,
        },
        Path::new(ROOT),
    );
    let problem_ctx = ProblemCtx::default();
    let thresholds = Thresholds {
        max_depth: 8,
        huge_entries: 1000,
    };
    compute(
        &Rows {
            files,
            dirs,
            errors: &[],
        },
        dups,
        &root(),
        &MetricsCtx {
            junk: &junk,
            problem_ctx: &problem_ctx,
            thresholds: &thresholds,
            old_after_days: 365,
            now_ticks: NOW,
            weights,
            template: None,
        },
    )
}

fn folder<'a>(s: &'a Snapshot, name: &str) -> &'a Area {
    s.folders
        .iter()
        .find(|a| a.folder == name)
        .unwrap_or_else(|| {
            panic!(
                "Ordner {name} fehlt in {:?}",
                s.folders.iter().map(|a| &a.folder).collect::<Vec<_>>()
            )
        })
}

#[test]
fn leere_wurzel_ergibt_nullen_und_100() {
    let s = run(&[], &[dir(ROOT, 0)], &[]);
    assert_eq!(s.root.metrics.files, 0);
    assert_eq!(s.root.score.total, 100);
    assert!(s.folders.is_empty());
}

#[test]
fn zaehlt_groesse_dateien_ordner_und_cloud() {
    let mut cloud = file(r"Z:\Root\Docs\cloud.docx", 500);
    cloud.cloud = true;
    let mut link = file(r"Z:\Root\Docs\link.lnk", 999);
    link.is_link = true;
    let mut hard = file(r"Z:\Root\Docs\hard.bin", 700);
    hard.counted = false;
    let files = [
        file(r"Z:\Root\top.txt", 10),
        file(r"Z:\Root\Docs\a.docx", 100),
        cloud,
        link,
        hard,
    ];
    let dirs = [dir(ROOT, 2), dir(r"Z:\Root\Docs", 4)];
    let s = run(&files, &dirs, &[]);
    let m = &s.root.metrics;
    assert_eq!(m.files, 4, "Links zählen nicht");
    assert_eq!(m.size, 10 + 100 + 500, "Hardlink-Geschwister nur einmal");
    assert_eq!(m.local_bytes, 110, "Cloud-Platzhalter sind nicht lokal");
    assert_eq!((m.cloud_files, m.cloud_bytes), (1, 500));
    assert_eq!(m.dirs, 2);
    let docs = folder(&s, "Docs");
    assert_eq!(docs.metrics.files, 3);
    assert_eq!(docs.metrics.dirs, 1);
    assert_eq!(
        s.folders.len(),
        1,
        "Dateien in der Wurzel sind keine Ordnerzeile"
    );
}

#[test]
fn nur_summe_ordner_zaehlen_mit_ihrer_summe() {
    let mut sum = dir(r"Z:\Root\Big", 0);
    sum.summary = true;
    sum.summary_files = 1000;
    sum.summary_size = 5_000;
    let s = run(&[], &[dir(ROOT, 1), sum], &[]);
    assert_eq!(folder(&s, "Big").metrics.files, 1000);
    assert_eq!(s.root.metrics.size, 5_000);
    assert_eq!(
        s.root.metrics.empty_dirs, 0,
        "Summenordner gelten nicht als leer"
    );
}

#[test]
fn muell_wird_gezaehlt_cloud_und_links_nicht() {
    let mut cloud_tmp = file(r"Z:\Root\Docs\cloud.tmp", 50);
    cloud_tmp.cloud = true;
    let files = [
        file(r"Z:\Root\Docs\a.tmp", 40),
        file(r"Z:\Root\Docs\Thumbs.db", 60),
        file(r"Z:\Root\Docs\ok.docx", 900),
        cloud_tmp,
    ];
    let s = run(&files, &[dir(ROOT, 1), dir(r"Z:\Root\Docs", 4)], &[]);
    assert_eq!(s.root.metrics.junk_files, 2);
    assert_eq!(s.root.metrics.junk_bytes, 100);
}

#[test]
fn problemdateien_je_art() {
    let files = [
        file(r"Z:\Root\a\leer.txt", 0),
        file(r"Z:\Root\a\~$lock.docx", 10),
        file(r"Z:\Root\a\fein.txt", 10),
    ];
    let s = run(&files, &[dir(ROOT, 1), dir(r"Z:\Root\a", 3)], &[]);
    let m = &s.root.metrics;
    assert_eq!(m.problem_files, 2);
    assert_eq!(m.problems_by_kind.get("zero_byte"), Some(&1));
    assert_eq!(m.problems_by_kind.get("office_lock"), Some(&1));
}

#[test]
fn struktur_leere_ordner_und_befunde() {
    let dirs = [
        dir(ROOT, 2),
        dir(r"Z:\Root\leer", 0),
        dir(r"Z:\Root\voll", 1),
    ];
    let files = [file(r"Z:\Root\voll\x.txt", 1)];
    let s = run(&files, &dirs, &[]);
    assert_eq!(s.root.metrics.empty_dirs, 1);
    // leerer Ordner + Ordner mit nur einer Datei
    assert_eq!(s.root.metrics.structure_issues, 2);
    assert_eq!(folder(&s, "leer").metrics.empty_dirs, 1);
    assert_eq!(folder(&s, "voll").metrics.empty_dirs, 0);
}

#[test]
fn versionen_je_ordner_ohne_archiv() {
    let files = [
        file(r"Z:\Root\a\Bericht.docx", 1),
        file(r"Z:\Root\a\Bericht - Kopie.docx", 1),
        file(r"Z:\Root\a\Bericht_v2.docx", 1),
        file(r"Z:\Root\b\Plan.docx", 1),
        file(r"Z:\Root\b\Plan - Kopie.docx", 1),
        file(r"Z:\Root\_Archiv\Versionen\Alt.docx", 1),
        file(r"Z:\Root\_Archiv\Versionen\Alt - Kopie.docx", 1),
    ];
    let dirs = [
        dir(ROOT, 3),
        dir(r"Z:\Root", 3),
        dir(r"Z:\Root", 2),
        dir(r"Z:\Root\_Archiv", 1),
    ];
    let s = run(&files, &dirs, &[]);
    assert_eq!(s.root.metrics.version_excess, 3);
    assert_eq!(folder(&s, "a").metrics.version_excess, 2);
    assert_eq!(folder(&s, "b").metrics.version_excess, 1);
    assert_eq!(folder(&s, "_Archiv").metrics.version_excess, 0);
}

fn dup(size: i64, paths: &[&str]) -> DupGroup {
    DupGroup {
        size,
        hash: vec![1],
        instances: paths.len(),
        wasted: size * (paths.len() as i64 - 1),
        files: paths
            .iter()
            .map(|p| DupFile {
                path: p.to_string(),
                mtime: 0,
                nlinks: 1,
                identity: None,
            })
            .collect(),
    }
}

#[test]
fn duplikate_gehen_in_den_ordner_jeder_weiteren_instanz() {
    let g = dup(
        100,
        &[r"Z:\Root\b\y.bin", r"Z:\Root\a\x.bin", r"Z:\Root\b\x.bin"],
    );
    let files = [
        file(r"Z:\Root\a\x.bin", 100),
        file(r"Z:\Root\b\x.bin", 100),
        file(r"Z:\Root\b\y.bin", 100),
    ];
    let s = run(&files, &[dir(ROOT, 2)], &[g]);
    assert_eq!(s.root.metrics.dup_wasted, 200);
    assert_eq!(
        s.root.metrics.dup_groups, 1,
        "die Gruppe zählt in der Wurzel einmal"
    );
    assert_eq!(
        folder(&s, "a").metrics.dup_wasted,
        0,
        "die erste Instanz zählt nicht"
    );
    assert_eq!(folder(&s, "a").metrics.dup_groups, 0);
    assert_eq!(folder(&s, "b").metrics.dup_wasted, 200);
    assert_eq!(folder(&s, "b").metrics.dup_groups, 1);
}

#[test]
fn hardlinks_zaehlen_nicht_als_verschwendet() {
    let mut g = dup(
        100,
        &[r"Z:\Root\a\x.bin", r"Z:\Root\a\y.bin", r"Z:\Root\b\z.bin"],
    );
    g.files[0].identity = Some((1, 7));
    g.files[1].identity = Some((1, 7));
    g.files[2].identity = Some((1, 9));
    let s = run(&[file(r"Z:\Root\a\x.bin", 100)], &[dir(ROOT, 2)], &[g]);
    assert_eq!(s.root.metrics.dup_wasted, 100);
}

#[test]
fn alte_daten_nur_ausserhalb_von_archiv_und_ohne_abzug() {
    let mut old = file(r"Z:\Root\a\alt.txt", 500);
    old.mtime = NOW - 800 * DAY;
    let mut archived = file(r"Z:\Root\_Archiv\2020\alt.txt", 300);
    archived.mtime = NOW - 800 * DAY;
    let s = run(
        &[old, archived, file(r"Z:\Root\a\neu.txt", 100)],
        &[dir(ROOT, 2)],
        &[],
    );
    assert_eq!(s.root.metrics.old_bytes, 500);
    assert_eq!(s.root.score.total, 100);
}

#[test]
fn mehr_als_200_ordner_ergeben_sonstige() {
    let mut files = Vec::new();
    for i in 0..205u64 {
        files.push(file(&format!(r"Z:\Root\f{i:03}\x.txt"), 1000 + i));
    }
    let s = run(&files, &[dir(ROOT, 205)], &[]);
    assert_eq!(s.folders.len(), MAX_FOLDERS + 1);
    let other = s.folders.last().unwrap();
    assert_eq!(other.folder, OTHER_FOLDER);
    assert_eq!(other.metrics.files, 5);
    // die kleinsten fünf (i = 0..5) landen in „Sonstige“
    assert_eq!(other.metrics.size, (0..5).map(|i| 1000 + i).sum::<u64>());
    assert_eq!(s.root.metrics.files, 205);
    // größte zuerst
    assert_eq!(s.folders[0].folder, "f204");
}

#[test]
fn deterministisch() {
    let files = [
        file(r"Z:\Root\a\x.tmp", 5),
        file(r"Z:\Root\b\y.docx", 50),
        file(r"Z:\Root\b\y - Kopie.docx", 50),
    ];
    let dirs = [dir(ROOT, 2), dir(r"Z:\Root\a", 1), dir(r"Z:\Root\b", 2)];
    assert_eq!(run(&files, &dirs, &[]), run(&files, &dirs, &[]));
}

#[test]
fn mehr_muell_senkt_den_score_nicht() {
    let mut files = vec![file(r"Z:\Root\a\ok.docx", 1000)];
    let dirs = [dir(ROOT, 1), dir(r"Z:\Root\a", 40)];
    let mut last = 100;
    for i in 0..40 {
        files.push(file(&format!(r"Z:\Root\a\{i}.tmp"), 10));
        let total = run(&files, &dirs, &[]).root.score.total;
        assert!(total <= last, "{i}: {total} > {last}");
        last = total;
    }
    assert!(last < 100);
}

#[test]
fn gewichte_wirken_auf_den_score() {
    let files = [file(r"Z:\Root\a\x.tmp", 1000)];
    let dirs = [dir(ROOT, 1), dir(r"Z:\Root\a", 1)];
    let normal = run(&files, &dirs, &[]).root.score.total;
    let no_junk = run_with(
        &files,
        &dirs,
        &[],
        &Weights {
            junk: 0,
            ..Weights::default()
        },
    )
    .root
    .score
    .total;
    assert!(no_junk > normal);
}
