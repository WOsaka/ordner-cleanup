use super::*;
use crate::health::score::{score, Weights};

fn area(folder: &str, junk: u64) -> Area {
    let metrics = Metrics {
        files: 1000,
        size: 1_000_000,
        local_bytes: 1_000_000,
        junk_files: junk,
        ..Metrics::default()
    };
    let score = score(&metrics, &Weights::default());
    Area {
        folder: folder.to_string(),
        metrics,
        score,
    }
}

fn snapshot(junk: u64) -> Snapshot {
    Snapshot {
        root: area("", junk),
        folders: vec![area("Docs", junk), area("Fotos", 0)],
    }
}

fn meta(root_key: &str, fp: &str, at: &str) -> SnapshotMeta {
    SnapshotMeta {
        root_key: root_key.to_string(),
        root_path: root_key.to_uppercase(),
        taken_at: at.to_string(),
        scan_finished_at: Some(at.to_string()),
        tool_version: "0.1.0".into(),
        metrics_version: METRICS_VERSION,
        config_fp: fp.to_string(),
        template: None,
        profile: None,
        scan_errors: 0,
    }
}

const ROOT: &str = r"d:\daten\";

#[test]
fn schreiben_und_lesen() {
    let mut h = History::open_in_memory().unwrap();
    let snap = snapshot(10);
    let id = h
        .write(&snap, &meta(ROOT, "fp1", "2026-10-01T10:00:00Z"))
        .unwrap();
    let (stored, kind) = h.latest_comparable(ROOT, "fp1", None).unwrap().unwrap();
    assert_eq!(kind, Comparable::Full);
    assert_eq!(stored.id, id);
    assert_eq!(stored.snapshot, snap);
    assert_eq!(stored.meta.config_fp, "fp1");
    assert_eq!(h.folders(ROOT).unwrap(), ["Docs", "Fotos"]);
}

#[test]
fn leerer_verlauf_hat_keinen_vergleichslauf() {
    let h = History::open_in_memory().unwrap();
    assert!(h.latest_comparable(ROOT, "fp", None).unwrap().is_none());
    assert!(h.series(ROOT, "", 10).unwrap().is_empty());
}

#[test]
fn gleicher_abdruck_ist_voll_anderer_eingeschraenkt() {
    let mut h = History::open_in_memory().unwrap();
    h.write(&snapshot(0), &meta(ROOT, "alt", "2026-10-01T10:00:00Z"))
        .unwrap();
    let new = h
        .write(&snapshot(5), &meta(ROOT, "neu", "2026-10-02T10:00:00Z"))
        .unwrap();
    let (s, kind) = h
        .latest_comparable(ROOT, "neu", Some(i64::MAX))
        .unwrap()
        .unwrap();
    assert_eq!((s.id, kind), (new, Comparable::Full));
    // Abdruck, den es nur früher gab: ein Lauf mit „neu“ wäre jetzt nur eingeschränkt vergleichbar
    let (s, kind) = h.latest_comparable(ROOT, "dritter", None).unwrap().unwrap();
    assert_eq!((s.id, kind), (new, Comparable::Limited));
    // Der Lauf mit dem alten Abdruck bleibt voll vergleichbar, auch wenn ein neuerer existiert
    let (s, kind) = h.latest_comparable(ROOT, "alt", None).unwrap().unwrap();
    assert_eq!(kind, Comparable::Full);
    assert_eq!(s.meta.config_fp, "alt");
}

#[test]
fn andere_formelversion_ist_nur_eingeschraenkt_vergleichbar() {
    let mut h = History::open_in_memory().unwrap();
    let mut m = meta(ROOT, "fp", "2026-10-01T10:00:00Z");
    m.metrics_version = METRICS_VERSION + 1;
    h.write(&snapshot(0), &m).unwrap();
    let (_, kind) = h.latest_comparable(ROOT, "fp", None).unwrap().unwrap();
    assert_eq!(kind, Comparable::Limited);
}

#[test]
fn before_schliesst_den_eigenen_lauf_aus() {
    let mut h = History::open_in_memory().unwrap();
    let first = h
        .write(&snapshot(0), &meta(ROOT, "fp", "2026-10-01T10:00:00Z"))
        .unwrap();
    let second = h
        .write(&snapshot(1), &meta(ROOT, "fp", "2026-10-02T10:00:00Z"))
        .unwrap();
    let (s, _) = h
        .latest_comparable(ROOT, "fp", Some(second))
        .unwrap()
        .unwrap();
    assert_eq!(s.id, first);
    assert!(h
        .latest_comparable(ROOT, "fp", Some(first))
        .unwrap()
        .is_none());
}

#[test]
fn andere_wurzel_wird_nicht_verglichen() {
    let mut h = History::open_in_memory().unwrap();
    h.write(&snapshot(0), &meta(r"e:\x\", "fp", "2026-10-01T10:00:00Z"))
        .unwrap();
    assert!(h.latest_comparable(ROOT, "fp", None).unwrap().is_none());
}

#[test]
fn zuletzt_geschriebener_lauf_zaehlt_auch_bei_uhrzeitsprung() {
    let mut h = History::open_in_memory().unwrap();
    h.write(&snapshot(0), &meta(ROOT, "fp", "2026-10-05T10:00:00Z"))
        .unwrap();
    let later_written = h
        .write(&snapshot(3), &meta(ROOT, "fp", "2026-10-01T10:00:00Z"))
        .unwrap();
    let (s, _) = h.latest_comparable(ROOT, "fp", None).unwrap().unwrap();
    assert_eq!(s.id, later_written);
    // Die Reihe ist nach gespeicherter UTC-Zeit sortiert.
    let times: Vec<String> = h
        .series(ROOT, "", 10)
        .unwrap()
        .into_iter()
        .map(|p| p.taken_at)
        .collect();
    assert_eq!(times, ["2026-10-01T10:00:00Z", "2026-10-05T10:00:00Z"]);
}

#[test]
fn reihe_ist_chronologisch_begrenzt_und_je_bereich() {
    let mut h = History::open_in_memory().unwrap();
    for day in 1..=5u64 {
        h.write(
            &snapshot(day * 10),
            &meta(ROOT, "fp", &format!("2026-10-0{day}T10:00:00Z")),
        )
        .unwrap();
    }
    let root = h.series(ROOT, "", 3).unwrap();
    assert_eq!(root.len(), 3);
    assert_eq!(root[0].taken_at, "2026-10-03T10:00:00Z");
    assert_eq!(root[2].metrics.junk_files, 50);
    assert!(root.windows(2).all(|w| w[0].score >= w[1].score));
    let fotos = h.series(ROOT, "Fotos", 10).unwrap();
    assert_eq!(fotos.len(), 5);
    assert!(fotos.iter().all(|p| p.metrics.junk_files == 0));
    assert!(h.series(ROOT, "Gibtsnicht", 10).unwrap().is_empty());
}

#[test]
fn datei_bleibt_nach_dem_schliessen_erhalten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("history.db");
    {
        let mut h = History::open(&path).unwrap();
        h.write(&snapshot(1), &meta(ROOT, "fp", "2026-10-01T10:00:00Z"))
            .unwrap();
    }
    let h = History::open(&path).unwrap();
    assert_eq!(h.series(ROOT, "", 10).unwrap().len(), 1);
}

#[test]
fn beschaedigte_datei_bleibt_unveraendert() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let junk = "das ist keine sqlite-datei, aber lang genug ".repeat(50);
    std::fs::write(&path, &junk).unwrap();
    assert!(matches!(
        History::open(&path),
        Err(HistoryError::Unreadable(_))
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), junk);
}

#[test]
fn neueres_schema_bleibt_unveraendert() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    {
        let h = History::open(&path).unwrap();
        h.conn
            .execute("UPDATE meta SET value='99' WHERE key='schema_version'", [])
            .unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    match History::open(&path) {
        Err(HistoryError::NewerSchema { found: 99, .. }) => {}
        other => panic!("unerwartet: {:?}", other.err()),
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn fremde_datenbank_wird_nicht_angefasst() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute("CREATE TABLE fremd (x INTEGER)", []).unwrap();
    }
    assert!(matches!(
        History::open(&path),
        Err(HistoryError::Unreadable(_))
    ));
    let conn = Connection::open(&path).unwrap();
    let tables: i64 = conn
        .query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tables, 1);
}
