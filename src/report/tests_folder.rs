//! Abschnitt „Ordner-Duplikate“: exakte Gruppen und teilweise gleiche Paare.

use super::tests::{params, run_scan, write};
use super::*;
use crate::scan::source::StdDirSource;

struct Tree {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    index: Index,
}

/// `Projekt` und `Kopie von Projekt` (exakt gleich), `T1`/`T2` (85 % gemeinsam),
/// `U1`/`U2` (75 % gemeinsam).
fn tree() -> Tree {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path().to_path_buf();
    for dir in ["Projekt", "Kopie von Projekt"] {
        write(&r, &format!("{dir}/a.txt"), &[b'a'; 100]);
        write(&r, &format!("{dir}/sub/b.txt"), &[b'b'; 200]);
    }
    write(&r, "T1/gross.bin", &[b'g'; 850]);
    write(&r, "T1/nur1.bin", &[b'1'; 150]);
    write(&r, "T2/gross.bin", &[b'g'; 850]);
    write(&r, "T2/nur2.bin", &[b'2'; 150]);
    write(&r, "U1/gross.bin", &[b'h'; 750]);
    write(&r, "U1/nur1.bin", &[b'3'; 250]);
    write(&r, "U2/gross.bin", &[b'h'; 750]);
    write(&r, "U2/nur2.bin", &[b'4'; 250]);
    let mut index = Index::open_in_memory().unwrap();
    run_scan(&mut index, &StdDirSource, &r);
    Tree {
        _tmp: tmp,
        root: r,
        index,
    }
}

fn report(t: &Tree, threshold: f64) -> Report {
    let root = select_root(&t.index, Some(&t.root)).unwrap();
    let mut p = params(50);
    p.partial_threshold = threshold;
    build(&t.index, &root, &p).unwrap()
}

fn name(path: &str) -> &str {
    path.rsplit('\\').next().unwrap()
}

#[test]
fn exakte_ordner_gruppen_nur_oberste_ebene_mit_platzgewinn() {
    let r = report(&tree(), 0.8);
    let f = &r.folder_duplicates;
    assert_eq!(
        f.groups.len(),
        1,
        "sub/sub zählt nicht extra: {:?}",
        f.groups
    );
    let g = &f.groups[0];
    let mut dirs: Vec<&str> = g.dirs.iter().map(|d| name(d)).collect();
    dirs.sort();
    assert_eq!(dirs, ["Kopie von Projekt", "Projekt"]);
    assert_eq!((g.files, g.bytes, g.reclaimable), (2, 300, 300));
    assert_eq!(f.total_reclaimable, 300);
}

#[test]
fn teilweise_gleich_ab_der_schwelle_85_ja_75_nein() {
    let r = report(&tree(), 0.8);
    let f = &r.folder_duplicates;
    assert_eq!(f.partial_threshold, 0.8);
    assert_eq!(f.partial.len(), 1, "{:?}", f.partial);
    let p = &f.partial[0];
    let mut names = [name(&p.a), name(&p.b)];
    names.sort();
    assert_eq!(names, ["T1", "T2"]);
    assert_eq!(p.shared_bytes, 850);
    assert!((p.ratio - 0.85).abs() < 1e-9);
    assert!(!f.partial_incomplete);
}

#[test]
fn niedrigere_schwelle_nimmt_das_75_prozent_paar_auf() {
    let r = report(&tree(), 0.7);
    assert_eq!(r.folder_duplicates.partial.len(), 2);
}

#[test]
fn exakte_gruppe_erscheint_nicht_auch_als_teilweise_gleich() {
    let r = report(&tree(), 0.5);
    assert!(r
        .folder_duplicates
        .partial
        .iter()
        .all(|p| !name(&p.a).contains("Projekt") || !name(&p.b).contains("Projekt")));
}

#[test]
fn csv_hat_ordner_duplikate_und_teilweise_gleich() {
    let r = report(&tree(), 0.8);
    let all = csv::render_all(&r).unwrap();
    let text = |file: &str| {
        let (_, bytes) = all.iter().find(|(n, _)| *n == file).unwrap();
        String::from_utf8(bytes[3..].to_vec()).unwrap()
    };
    let groups = text("folder-duplicates.csv");
    let mut lines = groups.lines();
    assert_eq!(
        lines.next().unwrap(),
        "Gruppe;Ordner;Dateien;Größe (Bytes);Platzgewinn (Bytes)"
    );
    assert_eq!(lines.count(), 2, "eine Zeile je Ordner der Gruppe");
    let partial = text("folder-partial.csv");
    let mut lines = partial.lines();
    assert_eq!(
        lines.next().unwrap(),
        "Ordner A;Ordner B;Gemeinsame Bytes;Anteil"
    );
    assert_eq!(lines.count(), 1);
}

#[test]
fn json_html_und_terminal_nennen_den_abschnitt() {
    let r = report(&tree(), 0.8);
    let value: serde_json::Value = serde_json::from_str(&json::to_string(&r).unwrap()).unwrap();
    assert_eq!(value["folder_duplicates"]["total_reclaimable"], 300);
    assert_eq!(
        value["folder_duplicates"]["groups"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let text = terminal::render(&r);
    assert!(text.contains("Doppelte Ordner:"), "{text}");
    let html = html::render(&r).unwrap();
    assert!(html.contains(r#"data-slot="folder-duplicates""#));
    assert!(html.contains("Ordner-Duplikate"));
}

#[test]
fn kopie_in_der_quarantaene_ist_kein_doppelter_ordner() {
    let mut t = tree();
    let root = t.root.clone();
    write(
        &root,
        ".ordner-cleanup/quarantine/r1/Projekt/a.txt",
        &[b'a'; 100],
    );
    write(
        &root,
        ".ordner-cleanup/quarantine/r1/Projekt/sub/b.txt",
        &[b'b'; 200],
    );
    run_scan(&mut t.index, &StdDirSource, &root);

    let f = &report(&t, 0.8).folder_duplicates;

    assert_eq!(f.groups.len(), 1, "{:?}", f.groups);
    assert_eq!(f.groups[0].dirs.len(), 2, "{:?}", f.groups[0].dirs);
    assert!(
        f.groups[0]
            .dirs
            .iter()
            .all(|d| !d.contains(".ordner-cleanup")),
        "{:?}",
        f.groups[0].dirs
    );
}
