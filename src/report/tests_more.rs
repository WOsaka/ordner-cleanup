use std::path::Path;

use super::tests::{fixture, params, report_of, run_scan, write};
use super::*;
use crate::scan::source::{cloud_file, fake_dir, fake_file, FakeDirSource, StdDirSource};

#[test]
fn alter_klassen_und_alte_dateien() {
    let r = report_of(&fixture());
    let class = |label: &str| r.age.classes.iter().find(|c| c.label == label).unwrap();
    assert_eq!(class("> 3 Jahre").count, 1);
    assert_eq!(class("> 3 Jahre").size, 5000);
    // 10 Dateien, ohne das alte Video und ohne das zweite Hardlink-Geschwister
    assert_eq!(class("< 1 Monat").count, 8);
    assert_eq!(r.age.classes.len(), 5);
    assert_eq!(r.age.old_files.len(), 1);
    assert!(r.age.old_files[0].path.ends_with("video.mp4"));
    assert!(r.age.old_files[0].age_days >= 1199);
    assert_eq!(r.age.old_total_size, 5000);
}

#[test]
fn exakte_duplikate_mit_verschwendetem_platz_hardlinks_nicht() {
    let r = report_of(&fixture());
    assert_eq!(r.duplicates.group_count, 1);
    assert_eq!(r.duplicates.total_wasted, 100);
    let g = &r.duplicates.groups[0];
    assert_eq!((g.size, g.instances, g.files.len()), (100, 2, 2));
}

#[test]
fn aehnliche_dateien_markieren_exakte_duplikate() {
    let r = report_of(&fixture());
    assert_eq!(r.similar.len(), 1);
    let g = &r.similar[0];
    assert!(g.dir.ends_with("docs"));
    assert_eq!(g.files.len(), 3);
    let exact: Vec<(&str, bool)> = g
        .files
        .iter()
        .map(|f| (f.name.as_str(), f.exact_duplicate))
        .collect();
    assert!(exact.contains(&("Angebot.docx", true)));
    assert!(exact.contains(&("Angebot_v2.docx", true)));
    assert!(exact.contains(&("Angebot - Kopie (3).docx", false)));
}

#[test]
fn struktur_findet_leere_und_ein_datei_ordner() {
    let r = report_of(&fixture());
    let with = |issue: &str| -> Vec<String> {
        r.structure
            .iter()
            .filter(|s| s.issue == issue)
            .map(|s| s.path.rsplit('\\').next().unwrap().to_string())
            .collect()
    };
    assert_eq!(with("empty"), vec!["empty"]);
    assert_eq!(with("single_file"), vec!["big", "single"]);
    assert!(with("too_deep").is_empty());
    assert!(r
        .structure
        .iter()
        .all(|s| !s.path.ends_with("node_modules")));
}

#[test]
fn problemdateien_werden_gelistet() {
    let r = report_of(&fixture());
    let find = |name: &str| r.problems.iter().find(|p| p.path.ends_with(name));
    assert!(find("~$Angebot.docx")
        .unwrap()
        .problems
        .contains(&"office_lock".to_string()));
    assert!(find("zero.txt")
        .unwrap()
        .problems
        .contains(&"zero_byte".to_string()));
    assert!(find("readme.txt").is_none());
    assert!(r.problems.iter().all(|p| !p.path.contains("node_modules")));
}

#[test]
fn versteckte_dateien_werden_markiert() {
    let f = fixture();
    let readme = f.root.join("readme.txt");
    let ok = std::process::Command::new("attrib")
        .arg("+h")
        .arg(&readme)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        return;
    }
    let mut index = f.index;
    run_scan(&mut index, &StdDirSource, &f.root);
    let root = select_root(&index, Some(&f.root)).unwrap();
    let r = build(&index, &root, &params(50)).unwrap();
    let item = r
        .top_files
        .iter()
        .find(|i| i.path.ends_with("readme.txt"))
        .unwrap();
    assert!(item.hidden && !item.system);
    assert_eq!(r.overview.hidden_files, 1);
}

#[test]
fn wahrscheinliche_duplikate_sind_als_nicht_verifiziert_markiert() {
    let source = FakeDirSource::new()
        .dir(
            r"C:\R",
            vec![fake_dir("a"), fake_dir("b"), fake_file("Bericht.docx", 900)],
        )
        .dir(
            r"C:\R\a",
            vec![
                cloud_file("Bericht.docx", 900),
                cloud_file("anders.docx", 10),
            ],
        )
        .dir(
            r"C:\R\b",
            vec![
                cloud_file("BERICHT.DOCX", 900),
                cloud_file("anders.docx", 11),
            ],
        );
    let mut index = Index::open_in_memory().unwrap();
    run_scan(&mut index, &source, Path::new(r"C:\R"));
    let root = select_root(&index, Some(Path::new(r"C:\R"))).unwrap();
    let r = build(&index, &root, &params(10)).unwrap();
    assert_eq!(r.probable_duplicates.len(), 1);
    let g = &r.probable_duplicates[0];
    assert!(!g.verified);
    assert_eq!((g.size, g.files.len()), (900, 3));
    assert_eq!(g.files.iter().filter(|f| f.cloud_only).count(), 2);
    assert_eq!(r.duplicates.group_count, 0);
    assert_eq!(r.overview.cloud_only_files, 4);
    assert_eq!(r.overview.cloud_only_size, 900 + 10 + 900 + 11);
}

#[test]
fn fehler_erscheinen_und_nicht_lesbare_ordner_sind_nicht_leer() {
    let source = FakeDirSource::new()
        .dir(r"C:\R", vec![fake_dir("gesperrt"), fake_dir("leer")])
        .failing(r"C:\R\gesperrt", std::io::ErrorKind::PermissionDenied)
        .dir(r"C:\R\leer", vec![]);
    let mut index = Index::open_in_memory().unwrap();
    run_scan(&mut index, &source, Path::new(r"C:\R"));
    let root = select_root(&index, Some(Path::new(r"C:\R"))).unwrap();
    let r = build(&index, &root, &params(10)).unwrap();
    assert_eq!(r.errors.len(), 1);
    assert_eq!(r.errors[0].kind, "access_denied");
    assert_eq!(r.overview.error_count, 1);
    let empty: Vec<&str> = r
        .structure
        .iter()
        .filter(|s| s.issue == "empty")
        .map(|s| s.path.as_str())
        .collect();
    assert_eq!(empty, vec![r"C:\R\leer"]);
}

#[test]
fn bericht_fuer_unterordner_bezieht_sich_nur_auf_diesen() {
    let f = fixture();
    let sub = f.root.join("docs");
    let root = select_root(&f.index, Some(&sub)).unwrap();
    let r = build(&f.index, &root, &params(50)).unwrap();
    assert_eq!(r.overview.files, 4);
    assert_eq!(r.overview.total_size, 260);
    assert_eq!(r.size_tree.size, 260);
    assert_eq!(r.duplicates.group_count, 1);
}

#[test]
fn json_enthaelt_alle_abschnitte() {
    let r = report_of(&fixture());
    let value: serde_json::Value = serde_json::from_str(&json::to_string(&r).unwrap()).unwrap();
    for key in [
        "meta",
        "overview",
        "size_tree",
        "top_files",
        "top_dirs",
        "file_types",
        "age",
        "duplicates",
        "probable_duplicates",
        "similar",
        "structure",
        "problems",
        "errors",
    ] {
        assert!(value.get(key).is_some(), "Abschnitt {key} fehlt");
    }
    assert_eq!(value["overview"]["files"], 12);
    assert_eq!(value["duplicates"]["total_wasted"], 100);
}

#[test]
fn csv_je_eine_datei_pro_liste_mit_bom_und_semikolon() {
    let r = report_of(&fixture());
    let out = tempfile::tempdir().unwrap();
    let written = csv::write(&r, out.path()).unwrap();
    let names: Vec<String> = written
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for expected in [
        "duplicates.csv",
        "similar.csv",
        "problems.csv",
        "old-files.csv",
        "top-files.csv",
        "empty-dirs.csv",
        "errors.csv",
    ] {
        assert!(names.contains(&expected.to_string()), "{expected}");
    }
    let bytes = std::fs::read(out.path().join("duplicates.csv")).unwrap();
    assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
    let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
    let mut lines = text.lines();
    assert_eq!(
        lines.next().unwrap(),
        "Gruppe;Größe (Bytes);Verschwendet (Bytes);Instanzen;Pfad;Geändert (Unix-Sekunden);Hardlinks"
    );
    assert_eq!(lines.count(), 2);
    let empty = std::fs::read_to_string(out.path().join("errors.csv")).unwrap();
    assert_eq!(
        empty.trim_start_matches('\u{feff}').lines().count(),
        1,
        "nur Kopfzeile"
    );
}

#[test]
fn csv_schuetzt_vor_formel_injektion() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "=HYPERLINK(1).txt", b"");
    write(tmp.path(), "normal.txt", b"x");
    let mut index = Index::open_in_memory().unwrap();
    run_scan(&mut index, &StdDirSource, tmp.path());
    let root = select_root(&index, Some(tmp.path())).unwrap();
    let r = build(&index, &root, &params(10)).unwrap();
    let all = csv::render_all(&r).unwrap();
    let (_, problems) = all.iter().find(|(n, _)| *n == "problems.csv").unwrap();
    let text = String::from_utf8_lossy(problems);
    // Der Pfad beginnt mit Laufwerk, der Name ist mitten im Pfad: kein Präfix nötig.
    assert!(text.contains("=HYPERLINK(1).txt"));
    // Felder, die selbst mit '=' beginnen, bekommen ein Apostroph.
    let mut r2 = r.clone();
    r2.errors.push(ErrorItem {
        path: "=cmd|' /C calc'!A0".into(),
        kind: "io".into(),
        message: "@SUM(1)".into(),
    });
    let all = csv::render_all(&r2).unwrap();
    let (_, errors) = all.iter().find(|(n, _)| *n == "errors.csv").unwrap();
    let text = String::from_utf8_lossy(errors);
    assert!(text.contains("'=cmd|"));
    assert!(text.contains("'@SUM(1)"));
}

#[test]
fn terminal_zusammenfassung_nennt_die_kennzahlen() {
    let r = report_of(&fixture());
    let text = terminal::render(&r);
    assert!(text.contains(&r.meta.root));
    assert!(text.contains("Dateien:          12"));
    assert!(text.contains("Exakte Gruppen:   1"));
    assert!(text.contains("Leerer Ordner: 1"));
    assert!(text.contains("0-Byte-Datei: 1"));
    assert!(!text.contains("ACHTUNG"));
}

#[test]
fn terminal_warnt_bei_abgebrochenem_scan() {
    let mut r = report_of(&fixture());
    r.meta.scan_status = "aborted".into();
    assert!(terminal::render(&r).contains("ACHTUNG"));
}

#[test]
fn report_ohne_scan_ist_klarer_fehler() {
    let index = Index::open_in_memory().unwrap();
    let err = select_root(&index, None).unwrap_err().to_string();
    assert!(err.contains("Kein Scan"));
    let err = select_root(&index, Some(Path::new(r"C:\Daten")))
        .unwrap_err()
        .to_string();
    assert!(err.contains(r"Kein Scan für C:\Daten vorhanden"));
    assert!(err.contains("scan"));
}

#[test]
fn mehrere_wurzeln_ohne_auswahl_listet_sie_auf() {
    let mut index = Index::open_in_memory().unwrap();
    for path in [r"C:\A", r"C:\B"] {
        index
            .begin_root(path, &crate::paths::dir_key(Path::new(path)), "t")
            .unwrap();
    }
    let err = select_root(&index, None).unwrap_err().to_string();
    assert!(err.contains(r"C:\A") && err.contains(r"C:\B"));
    assert!(select_root(&index, Some(Path::new(r"C:\B"))).is_ok());
    assert!(select_root(&index, Some(Path::new(r"C:\C"))).is_err());
}

#[test]
fn zielordner_nicht_beschreibbar_ist_klarer_fehler() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("ich-bin-eine-datei");
    std::fs::write(&file, "x").unwrap();
    let err = prepare_out_dir(Some(&file), "20261002-120000")
        .unwrap_err()
        .to_string();
    assert!(err.contains("Zielordner nicht beschreibbar"));
    let ok = prepare_out_dir(Some(tmp.path()), "20261002-120000").unwrap();
    assert!(ok.ends_with("ordner-cleanup-report-20261002-120000"));
    assert!(ok.is_dir());
}

#[test]
fn formate_parsen() {
    assert_eq!(Format::parse_list(&[]).unwrap().len(), 3);
    assert_eq!(
        Format::parse_list(&["JSON".into(), "csv".into(), "json".into()]).unwrap(),
        vec![Format::Json, Format::Csv]
    );
    assert!(Format::parse_list(&["pdf".into()]).is_err());
}

#[test]
fn write_all_schreibt_gewaehlte_formate() {
    let r = report_of(&fixture());
    let out = tempfile::tempdir().unwrap();
    let written = write_all(&r, &[Format::Json], out.path()).unwrap();
    assert_eq!(written.len(), 1);
    assert!(out.path().join("report.json").is_file());
    assert!(!out.path().join("duplicates.csv").exists());
}
