use std::path::Path;

use super::check::{check, CheckResult, DeviationKind};
use super::*;
use crate::index::RootStatus;
use crate::paths;
use crate::platform::FileAttrs;
use crate::report::rows::{DRow, FRow};
use crate::report::ReportRoot;

const ROOT: &str = r"Z:\Root";

fn root() -> ReportRoot {
    ReportRoot {
        path: ROOT.to_string(),
        dir_key: paths::dir_key(Path::new(ROOT)),
        scanned_at: None,
        status: RootStatus::Complete,
    }
}

fn dir(path: &str) -> DRow {
    let p = Path::new(path);
    DRow {
        path: path.to_string(),
        key: paths::dir_key(p),
        parent: p.parent().map(paths::dir_key),
        depth: paths::path_key(p).matches('\\').count() as i64,
        summary: false,
        is_link: false,
        direct_entries: 1,
        summary_size: 0,
        summary_files: 0,
    }
}

fn file(path: &str) -> FRow {
    let p = Path::new(path);
    FRow {
        path: path.to_string(),
        dir_key: paths::dir_key(p.parent().unwrap()),
        name: p.file_name().unwrap().to_string_lossy().into_owned(),
        ext: None,
        size: 1,
        mtime: 0,
        attrs: FileAttrs(0x20),
        cloud: false,
        is_link: false,
        full_hash: None,
        counted: true,
    }
}

/// Ordner `dirs` (die Wurzel und alle Zwischenordner kommen automatisch dazu) und Dateien.
fn run(t: &Template, dirs: &[&str], files: &[&str]) -> CheckResult {
    let mut all: std::collections::BTreeSet<String> = [ROOT.to_string()].into();
    for d in dirs {
        let mut cur = Path::new(d);
        loop {
            all.insert(cur.display().to_string());
            match cur.parent() {
                Some(p) if p != Path::new(ROOT) && p.starts_with(ROOT) => cur = p,
                _ => break,
            }
        }
    }
    let dirs: Vec<DRow> = all.iter().map(|d| dir(d)).collect();
    let files: Vec<FRow> = files.iter().map(|f| file(f)).collect();
    check(&files, &dirs, &root(), t)
}

fn kinds(r: &CheckResult) -> Vec<(DeviationKind, String)> {
    r.deviations
        .iter()
        .map(|d| (d.kind, d.path.clone()))
        .collect()
}

fn para() -> Template {
    Template::builtin("para").unwrap()
}

// ------------------------------------------------------------ Laden

#[test]
fn eingebaute_vorlagen_sind_gueltig() {
    assert_eq!(builtin_names(), ["para", "johnny-decimal"]);
    let para = Template::builtin("PARA").unwrap();
    assert_eq!(para.name, "PARA");
    assert_eq!(para.levels[0].required.len(), 4);
    assert!(!para.levels[0].allow_other && !para.levels[0].files);
    let jd = Template::builtin("johnny-decimal").unwrap();
    assert_eq!(jd.levels.len(), 3);
    assert!(Template::builtin("gibtsnicht").is_none());
}

#[test]
fn ebenen_ohne_listen_sind_frei_mit_listen_streng() {
    let t = Template::parse("[[levels]]\nfiles = false\n[[levels]]\nrequired = ['a']").unwrap();
    assert!(t.levels[0].allow_other && !t.levels[0].files);
    assert!(!t.levels[1].allow_other && t.levels[1].files);
    let t = Template::parse("[[levels]]\nallowed = ['a']\nallow_other = true").unwrap();
    assert!(t.levels[0].allow_other);
}

#[test]
fn ungueltige_regex_nennt_die_zeile() {
    let text = "name = 'X'\n[[levels]]\npatterns = ['^ok', '(kaputt']\n";
    let err = Template::parse(text).unwrap_err().to_string();
    assert!(err.contains("Zeile 3"), "{err}");
    assert!(err.contains("(kaputt"), "{err}");
}

#[test]
fn unbekannter_schluessel_nennt_die_zeile() {
    let err = Template::parse("name = 'X'\n[[levels]]\nrequierd = ['a']\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("Zeile 3"), "{err}");
    assert!(err.contains("requierd"), "{err}");
    let err = Template::parse("name = 'X'\nfoo = 1\n[[levels]]\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("Zeile 2") && err.contains("foo"), "{err}");
}

#[test]
fn falscher_typ_nennt_die_zeile() {
    let err = Template::parse("[[levels]]\nfiles = 'ja'\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("Zeile 2"), "{err}");
}

#[test]
fn namen_werden_geprueft() {
    for (text, needle) in [
        ("[[levels]]\nrequired = ['']", "leeren Namen"),
        ("[[levels]]\nrequired = ['a/b']", "Pfadtrenner"),
        ("[[levels]]\nrequired = ['A', 'a']", "doppelt"),
        ("max_depth = 0\n[[levels]]", "max_depth"),
        ("max_depth = 1\n[[levels]]\n[[levels]]", "max_depth"),
        ("name = 'leer'", "beschreibt nichts"),
    ] {
        let err = Template::parse(text).unwrap_err().to_string();
        assert!(err.contains(needle), "{text}: {err}");
    }
}

#[test]
fn aufloesen_nach_name_oder_pfad() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("meine.toml");
    std::fs::write(&file, "name = 'Meine'\n[[levels]]\nrequired = ['x']\n").unwrap();

    let builtin = resolve("PARA", None).unwrap();
    assert!(builtin.id.starts_with("para:"));
    let absolute = resolve(&file.display().to_string(), None).unwrap();
    assert_eq!(absolute.template.name, "Meine");
    let relative = resolve("meine.toml", Some(dir.path())).unwrap();
    assert_eq!(
        relative.id.split(':').next_back(),
        absolute.id.split(':').next_back()
    );

    // Der Hash folgt dem Inhalt
    std::fs::write(&file, "name = 'Meine'\n[[levels]]\nrequired = ['y']\n").unwrap();
    let changed = resolve("meine.toml", Some(dir.path())).unwrap();
    assert_ne!(changed.id, relative.id);

    let err = resolve("gibts-nicht", None).unwrap_err().to_string();
    assert!(
        err.contains("para") && err.contains("johnny-decimal"),
        "{err}"
    );
}

#[test]
fn fehler_in_der_datei_nennen_datei_und_zeile() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("kaputt.toml"),
        "[[levels]]\npatterns = ['(']\n",
    )
    .unwrap();
    let err = format!(
        "{:#}",
        resolve("kaputt.toml", Some(dir.path())).unwrap_err()
    );
    assert!(
        err.contains("kaputt.toml") && err.contains("Zeile 2"),
        "{err}"
    );
}

// ------------------------------------------------------------ PARA

#[test]
fn para_passende_struktur_hat_keine_abweichungen() {
    let r = run(
        &para(),
        &[
            r"Z:\Root\1 Projekte\Alpha",
            r"Z:\Root\2 Bereiche",
            r"Z:\Root\3 Ressourcen",
            r"Z:\Root\4 Archiv",
            r"Z:\Root\_Archiv\2020",
            r"Z:\Root\.ordner-cleanup\q",
        ],
        &[
            r"Z:\Root\1 Projekte\Alpha\a.txt",
            r"Z:\Root\_Archiv\2020\b.txt",
        ],
    );
    assert!(r.deviations.is_empty(), "{:?}", r.deviations);
    assert_eq!(
        r.checked, 4,
        "vier Ordner der Ebene 1, nichts Darunterliegendes"
    );
}

#[test]
fn para_spec_beispiel_fehlend_unerwartet_und_dateien_oben() {
    let r = run(
        &para(),
        &[
            r"Z:\Root\1 Projekte",
            r"Z:\Root\2 Bereiche",
            r"Z:\Root\3 Ressourcen",
            r"Z:\Root\Sonstiges",
        ],
        &[r"Z:\Root\notiz.txt", r"Z:\Root\1 Projekte\ok.txt"],
    );
    assert_eq!(
        kinds(&r),
        [
            (
                DeviationKind::MissingRequired,
                r"Z:\Root\4 Archiv".to_string()
            ),
            (
                DeviationKind::UnexpectedDir,
                r"Z:\Root\Sonstiges".to_string()
            ),
            (
                DeviationKind::FileNotAllowed,
                r"Z:\Root\notiz.txt".to_string()
            ),
        ]
    );
    // geprüft: 4 Ordner (3 Pflicht + Sonstiges), 1 Datei oben, 1 fehlender Pflicht-Platz
    assert_eq!(r.checked, 6);
    // Zuordnung: Fehlendes und Datei gehören zur Wurzel, „Sonstiges“ zu seiner eigenen Zeile
    let root = r.by_bucket[""];
    assert_eq!(root, (2, 2));
    assert_eq!(
        r.by_bucket[&paths::dir_key(Path::new(r"Z:\Root\Sonstiges"))],
        (1, 1)
    );
}

#[test]
fn gross_und_kleinschreibung_ist_egal() {
    let r = run(
        &para(),
        &[
            r"Z:\Root\1 PROJEKTE",
            r"Z:\Root\2 bereiche",
            r"Z:\Root\3 Ressourcen",
            r"Z:\Root\4 archiv",
        ],
        &[],
    );
    assert!(r.deviations.is_empty(), "{:?}", r.deviations);
}

#[test]
fn leere_wurzel_meldet_alle_pflicht_ordner() {
    let r = run(&para(), &[], &[]);
    assert_eq!(r.deviations.len(), 4);
    assert!(r
        .deviations
        .iter()
        .all(|d| d.kind == DeviationKind::MissingRequired));
    assert_eq!(r.checked, 4);
}

#[test]
fn links_werden_nicht_geprueft() {
    let mut link = dir(r"Z:\Root\Verknuepfung");
    link.is_link = true;
    let mut f = file(r"Z:\Root\l.lnk");
    f.is_link = true;
    let dirs: Vec<DRow> = [
        r"Z:\Root",
        r"Z:\Root\1 Projekte",
        r"Z:\Root\2 Bereiche",
        r"Z:\Root\3 Ressourcen",
        r"Z:\Root\4 Archiv",
    ]
    .iter()
    .map(|d| dir(d))
    .chain([link])
    .collect();
    let r = check(&[f], &dirs, &root(), &para());
    assert!(r.deviations.is_empty());
}

// ------------------------------------------------------------ Johnny.Decimal

#[test]
fn johnny_decimal_muster_je_ebene() {
    let jd = Template::builtin("johnny-decimal").unwrap();
    let r = run(
        &jd,
        &[
            r"Z:\Root\10-19 Verwaltung\11 Finanzen\11.01 Rechnungen",
            r"Z:\Root\10-19 Verwaltung\11 Finanzen\11.02 Steuern",
            r"Z:\Root\10-19 Verwaltung\12 Recht",
            r"Z:\Root\20-29 Projekte",
            r"Z:\Root\Durcheinander",
            r"Z:\Root\10-19 Verwaltung\Kategorie ohne Zahl",
            r"Z:\Root\10-19 Verwaltung\11 Finanzen\Ablage",
        ],
        &[
            r"Z:\Root\10-19 Verwaltung\11 Finanzen\11.01 Rechnungen\2026.pdf",
            r"Z:\Root\10-19 Verwaltung\11 Finanzen\lose.pdf",
            r"Z:\Root\lose-datei.txt",
        ],
    );
    assert_eq!(
        kinds(&r),
        [
            (
                DeviationKind::BadName,
                r"Z:\Root\10-19 Verwaltung\11 Finanzen\Ablage".to_string()
            ),
            (
                DeviationKind::BadName,
                r"Z:\Root\10-19 Verwaltung\Kategorie ohne Zahl".to_string()
            ),
            (DeviationKind::BadName, r"Z:\Root\Durcheinander".to_string()),
            (
                DeviationKind::FileNotAllowed,
                r"Z:\Root\10-19 Verwaltung\11 Finanzen\lose.pdf".to_string()
            ),
            (
                DeviationKind::FileNotAllowed,
                r"Z:\Root\lose-datei.txt".to_string()
            ),
        ]
    );
}

#[test]
fn bereichsmuster_verlangt_zehnerblock() {
    let jd = Template::builtin("johnny-decimal").unwrap();
    for (name, ok) in [
        ("10-19 A", true),
        ("20-29 B", true),
        ("11-19 C", false),
        ("10-18 D", false),
        ("1019 E", false),
    ] {
        let r = run(&jd, &[&format!(r"Z:\Root\{name}")], &[]);
        assert_eq!(r.deviations.is_empty(), ok, "{name}: {:?}", r.deviations);
    }
}

// ------------------------------------------------------------ eigene Vorlagen

#[test]
fn max_depth_meldet_nur_den_ersten_zu_tiefen_ordner_je_zweig() {
    let t = Template::parse("max_depth = 2\n[[levels]]\n[[levels]]\n").unwrap();
    let r = run(
        &t,
        &[r"Z:\Root\a\b", r"Z:\Root\a\b\c\d\e", r"Z:\Root\x\y\z"],
        &[r"Z:\Root\a\b\c\datei.txt", r"Z:\Root\a\b\ok.txt"],
    );
    assert_eq!(
        kinds(&r),
        [
            (DeviationKind::TooDeep, r"Z:\Root\a\b\c".to_string()),
            (DeviationKind::TooDeep, r"Z:\Root\x\y\z".to_string()),
        ]
    );
    assert_eq!(r.deviations[0].level, 3);
}

#[test]
fn pflicht_ordner_gelten_in_jedem_ordner_der_ebene_davor() {
    let t = Template::parse("[[levels]]\n[[levels]]\nrequired = ['docs']\nallow_other = true\n")
        .unwrap();
    let r = run(
        &t,
        &[r"Z:\Root\a\docs", r"Z:\Root\b\Docs", r"Z:\Root\c\anderes"],
        &[],
    );
    assert_eq!(
        kinds(&r),
        [(
            DeviationKind::MissingRequired,
            r"Z:\Root\c\docs".to_string()
        )]
    );
    // Der fehlende Ordner gehört zum Ordner „c“
    assert_eq!(r.by_bucket[&paths::dir_key(Path::new(r"Z:\Root\c"))].1, 1);
}

#[test]
fn allowed_und_pflicht_namen_zaehlen_als_erlaubt() {
    let t = Template::parse("[[levels]]\nrequired = ['Pflicht']\nallowed = ['Extra']\n").unwrap();
    let r = run(
        &t,
        &[r"Z:\Root\pflicht", r"Z:\Root\EXTRA", r"Z:\Root\Fremd"],
        &[],
    );
    assert_eq!(
        kinds(&r),
        [(DeviationKind::UnexpectedDir, r"Z:\Root\Fremd".to_string())]
    );
}

#[test]
fn freie_ebenen_werden_nicht_gezaehlt() {
    let t = Template::parse("[[levels]]\nallowed = ['a']\n").unwrap();
    let r = run(
        &t,
        &[r"Z:\Root\a\beliebig\tief\unten"],
        &[r"Z:\Root\a\x.txt"],
    );
    assert!(r.deviations.is_empty());
    assert_eq!(r.checked, 1, "nur die Ebene 1 ist beschrieben");
}

#[test]
fn nur_summe_ordner_verlangen_keine_pflicht_unterordner() {
    let t = Template::parse("[[levels]]\n[[levels]]\nrequired = ['docs']\n").unwrap();
    let mut a = dir(r"Z:\Root\a");
    a.summary = false;
    let mut big = dir(r"Z:\Root\big");
    big.summary = true;
    let dirs = vec![dir(ROOT), a, big, dir(r"Z:\Root\a\docs")];
    let r = check(&[], &dirs, &root(), &t);
    assert!(r.deviations.is_empty(), "{:?}", r.deviations);
}

#[test]
fn mehr_abweichungen_erhoehen_den_anteil_monoton() {
    let t = para();
    let mut last = 0.0;
    for extra in 0..6 {
        let mut dirs = vec![
            r"Z:\Root\1 Projekte".to_string(),
            r"Z:\Root\2 Bereiche".to_string(),
            r"Z:\Root\3 Ressourcen".to_string(),
            r"Z:\Root\4 Archiv".to_string(),
        ];
        dirs.extend((0..extra).map(|i| format!(r"Z:\Root\fremd{i}")));
        let refs: Vec<&str> = dirs.iter().map(String::as_str).collect();
        let r = check_all(&t, &refs);
        let share = r.deviations.len() as f64 / r.checked as f64;
        assert!(share >= last, "{extra}: {share} < {last}");
        last = share;
    }
    assert!(last > 0.0);
}

fn check_all(t: &Template, dirs: &[&str]) -> CheckResult {
    run(t, dirs, &[])
}
