pub mod classify;
pub mod source;
pub mod walker;

#[cfg(test)]
mod incremental_tests;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;

use crate::config::Config;
use crate::index::{DirRecord, FileRecord, Index, RootRun, RootStatus};
use crate::paths;
use classify::{Classifier, DefaultPaths};
use source::DirSource;
use walker::{Progress, ScanEvent, WalkCtx};

const FILE_BATCH: usize = 5_000;
const DIR_BATCH: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOutcome {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub errors: u64,
    pub aborted: bool,
}

/// Umgebung eines Scans (alles außer Index und Wurzel).
pub struct ScanEnv<'a> {
    pub source: &'a dyn DirSource,
    pub config: &'a Config,
    pub default_paths: &'a DefaultPaths,
    pub cancel: &'a AtomicBool,
    pub progress: &'a Progress,
    pub now: &'a str,
}

/// Scannt `root` in den Index. Bei gesetztem `cancel` wird der Lauf sauber beendet
/// (Status `aborted`, kein Prune).
pub fn scan(index: &mut Index, root: &Path, env: &ScanEnv<'_>) -> Result<ScanOutcome> {
    let ScanEnv {
        source,
        config,
        default_paths,
        cancel,
        progress,
        now,
    } = *env;
    let classifier = Classifier::new(root, config, default_paths)?;
    let prev = index.load_previous(&paths::dir_key(root))?;
    let run = index.begin_root(&paths::display(root), &paths::dir_key(root), now)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.threads)
        .build()
        .context("Thread-Pool konnte nicht erstellt werden")?;
    let (tx, rx) = crossbeam_channel::bounded::<ScanEvent>(1024);

    let written = std::thread::scope(|s| {
        let writer = s.spawn(|| write_events(index, &run, rx));
        {
            let ctx = WalkCtx {
                source,
                classifier: &classifier,
                cancel,
                progress,
                prev: &prev,
                tx: &tx,
            };
            pool.install(|| walker::walk(&ctx, root));
        }
        drop(tx);
        writer.join().expect("Writer-Thread ist abgestürzt")
    });

    let aborted = cancel.load(Ordering::Relaxed);
    let errors = progress.errors.load(Ordering::Relaxed);
    let status = if aborted || written.is_err() {
        RootStatus::Aborted
    } else {
        RootStatus::Complete
    };
    index.finish_root(&run, status, errors as i64, now)?;
    written?;
    Ok(ScanOutcome {
        files: progress.files.load(Ordering::Relaxed),
        dirs: progress.dirs.load(Ordering::Relaxed),
        bytes: progress.bytes.load(Ordering::Relaxed),
        errors,
        aborted,
    })
}

/// Einziger Schreiber: sammelt Ereignisse zu Batches und schreibt sie in Transaktionen.
/// Nach einem Datenbankfehler wird weiter gelesen (damit der Walker nicht blockiert),
/// aber nichts mehr geschrieben.
fn write_events(index: &mut Index, run: &RootRun, rx: Receiver<ScanEvent>) -> Result<()> {
    let mut files: Vec<FileRecord> = Vec::new();
    let mut dirs: Vec<DirRecord> = Vec::new();
    let mut failure: Option<anyhow::Error> = None;

    let flush_files = |index: &mut Index, files: &mut Vec<FileRecord>| -> Result<()> {
        index.upsert_files(files, run.generation)?;
        files.clear();
        Ok(())
    };
    let flush_dirs = |index: &mut Index, dirs: &mut Vec<DirRecord>| -> Result<()> {
        index.upsert_dirs(dirs, run.generation)?;
        dirs.clear();
        Ok(())
    };

    for event in rx {
        if failure.is_some() {
            continue;
        }
        let result = match event {
            ScanEvent::Files(batch) => {
                files.extend(batch);
                if files.len() >= FILE_BATCH {
                    flush_files(index, &mut files)
                } else {
                    Ok(())
                }
            }
            ScanEvent::Dir(d) => {
                dirs.push(d);
                if dirs.len() >= DIR_BATCH {
                    flush_dirs(index, &mut dirs)
                } else {
                    Ok(())
                }
            }
            ScanEvent::Error(e) => index.record_error(run, &e).map_err(Into::into),
        };
        if let Err(e) = result {
            failure = Some(e);
        }
    }
    if failure.is_none() {
        let result = flush_files(index, &mut files).and_then(|()| flush_dirs(index, &mut dirs));
        if let Err(e) = result {
            failure = Some(e);
        }
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::source::{
        cloud_file, fake_dir, fake_file, fake_junction, FakeDirSource, StdDirSource,
    };
    use std::io::ErrorKind;

    fn run_scan(
        index: &mut Index,
        source: &dyn DirSource,
        root: &str,
        cancel: &AtomicBool,
    ) -> ScanOutcome {
        let config = Config {
            threads: 2,
            ..Config::default()
        };
        let env = ScanEnv {
            source,
            config: &config,
            default_paths: &DefaultPaths::default(),
            cancel,
            progress: &Progress::default(),
            now: "t",
        };
        scan(index, Path::new(root), &env).unwrap()
    }

    fn q<T: rusqlite::types::FromSql>(index: &Index, sql: &str) -> T {
        index.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn basic_source() -> FakeDirSource {
        FakeDirSource::new()
            .dir(r"C:\R", vec![fake_file("a.txt", 10), fake_dir("sub")])
            .dir(r"C:\R\sub", vec![fake_file("b.TXT", 20)])
    }

    #[test]
    fn erfasst_dateien_ordner_und_groessen() {
        let mut index = Index::open_in_memory().unwrap();
        let out = run_scan(
            &mut index,
            &basic_source(),
            r"C:\R",
            &AtomicBool::new(false),
        );
        assert_eq!((out.files, out.dirs, out.bytes, out.errors), (2, 2, 30, 0));
        assert!(!out.aborted);
        assert_eq!(q::<i64>(&index, "SELECT COUNT(*) FROM files"), 2);
        assert_eq!(q::<i64>(&index, "SELECT SUM(size) FROM files"), 30);
        assert_eq!(q::<i64>(&index, "SELECT COUNT(*) FROM dirs"), 2);
        assert_eq!(
            q::<String>(&index, "SELECT ext FROM files WHERE name='b.TXT'"),
            "txt"
        );
        assert_eq!(
            q::<String>(&index, "SELECT parent_key FROM dirs WHERE depth=1"),
            r"C:\r\"
        );
        assert_eq!(index.roots().unwrap()[0].status, RootStatus::Complete);
    }

    #[test]
    fn cloud_platzhalter_werden_erfasst_aber_nie_geoeffnet() {
        let source = FakeDirSource::new().dir(
            r"C:\R",
            vec![cloud_file("wolke.docx", 500), fake_file("lokal.docx", 5)],
        );
        let mut index = Index::open_in_memory().unwrap();
        run_scan(&mut index, &source, r"C:\R", &AtomicBool::new(false));
        assert_eq!(
            q::<i64>(
                &index,
                "SELECT cloud_only FROM files WHERE name='wolke.docx'"
            ),
            1
        );
        assert_eq!(
            q::<i64>(
                &index,
                "SELECT cloud_only FROM files WHERE name='lokal.docx'"
            ),
            0
        );
        // Die Quelle kennt nur read_dir: gelesen wurde ausschließlich das Wurzelverzeichnis.
        assert_eq!(source.reads(), vec![r"C:\r".to_string()]);
        assert_eq!(
            q::<i64>(
                &index,
                "SELECT COUNT(*) FROM files WHERE hash_status IS NOT NULL"
            ),
            0
        );
    }

    #[test]
    fn links_werden_erfasst_aber_nicht_betreten() {
        let source = FakeDirSource::new().dir(
            r"C:\R",
            vec![fake_junction("link", r"C:\Anderswo"), fake_file("a", 1)],
        );
        let mut index = Index::open_in_memory().unwrap();
        let out = run_scan(&mut index, &source, r"C:\R", &AtomicBool::new(false));
        assert_eq!(source.reads(), vec![r"C:\r".to_string()]);
        assert_eq!(out.bytes, 1);
        assert_eq!(
            q::<String>(&index, "SELECT link_target FROM dirs WHERE is_link=1"),
            r"C:\Anderswo"
        );
    }

    #[test]
    fn summary_only_speichert_nur_summe() {
        let source = FakeDirSource::new()
            .dir(r"C:\R", vec![fake_file("a", 1), fake_dir("node_modules")])
            .dir(
                r"C:\R\node_modules",
                vec![
                    fake_file("x.js", 100),
                    fake_dir("pkg"),
                    fake_junction("l", "C:\\z"),
                ],
            )
            .dir(r"C:\R\node_modules\pkg", vec![fake_file("y.js", 200)]);
        let mut index = Index::open_in_memory().unwrap();
        run_scan(&mut index, &source, r"C:\R", &AtomicBool::new(false));
        assert_eq!(q::<i64>(&index, "SELECT COUNT(*) FROM files"), 1);
        assert_eq!(
            q::<i64>(&index, "SELECT summary_size FROM dirs WHERE mode='summary'"),
            300
        );
        assert_eq!(
            q::<i64>(
                &index,
                "SELECT summary_files FROM dirs WHERE mode='summary'"
            ),
            2
        );
        assert_eq!(
            q::<i64>(
                &index,
                "SELECT direct_entries FROM dirs WHERE mode='summary'"
            ),
            3
        );
    }

    #[test]
    fn ausgeschlossene_ordner_werden_nicht_betreten() {
        let source = FakeDirSource::new()
            .dir(r"C:\R", vec![fake_dir("$Recycle.Bin"), fake_file("a", 1)])
            .dir(r"C:\R\$Recycle.Bin", vec![fake_file("geheim", 9)]);
        let mut index = Index::open_in_memory().unwrap();
        run_scan(&mut index, &source, r"C:\R", &AtomicBool::new(false));
        assert_eq!(source.reads(), vec![r"C:\r".to_string()]);
        assert_eq!(q::<i64>(&index, "SELECT COUNT(*) FROM dirs"), 1);
    }

    #[test]
    fn zugriff_verweigert_wird_protokolliert_und_scan_laeuft_weiter() {
        let source = FakeDirSource::new()
            .dir(r"C:\R", vec![fake_dir("gesperrt"), fake_dir("ok")])
            .failing(r"C:\R\gesperrt", ErrorKind::PermissionDenied)
            .dir(r"C:\R\ok", vec![fake_file("f", 7)]);
        let mut index = Index::open_in_memory().unwrap();
        let out = run_scan(&mut index, &source, r"C:\R", &AtomicBool::new(false));
        assert_eq!((out.errors, out.files), (1, 1));
        assert_eq!(
            q::<String>(&index, "SELECT kind FROM errors"),
            "access_denied"
        );
        assert_eq!(
            q::<String>(&index, "SELECT path FROM errors"),
            r"C:\R\gesperrt"
        );
        assert_eq!(index.roots().unwrap()[0].error_count, 1);
    }

    #[test]
    fn verschwundener_ordner_ist_warnung_vanished() {
        let source = FakeDirSource::new().dir(r"C:\R", vec![fake_dir("weg")]);
        let mut index = Index::open_in_memory().unwrap();
        let out = run_scan(&mut index, &source, r"C:\R", &AtomicBool::new(false));
        assert_eq!(out.errors, 1);
        assert_eq!(q::<String>(&index, "SELECT kind FROM errors"), "vanished");
    }

    #[test]
    fn abbruch_markiert_aborted_und_prunt_nicht() {
        let mut index = Index::open_in_memory().unwrap();
        run_scan(
            &mut index,
            &basic_source(),
            r"C:\R",
            &AtomicBool::new(false),
        );
        let out = run_scan(&mut index, &basic_source(), r"C:\R", &AtomicBool::new(true));
        assert!(out.aborted);
        assert_eq!(index.roots().unwrap()[0].status, RootStatus::Aborted);
        assert_eq!(q::<i64>(&index, "SELECT COUNT(*) FROM files"), 2);
    }

    #[test]
    fn echter_baum_stimmt_mit_unabhaengiger_zaehlung() {
        let tmp = tempfile::tempdir().unwrap();
        let mut expected_files = 0u64;
        let mut expected_bytes = 0u64;
        for d in 0..4 {
            let dir = tmp.path().join(format!("d{d}")).join("tief");
            std::fs::create_dir_all(&dir).unwrap();
            for f in 0..5 {
                let content = vec![b'x'; d * 10 + f + 1];
                expected_bytes += content.len() as u64;
                expected_files += 1;
                std::fs::write(dir.join(format!("f{f}.dat")), content).unwrap();
            }
        }
        let mut index = Index::open_in_memory().unwrap();
        let root = tmp.path().to_string_lossy().into_owned();
        let out = run_scan(&mut index, &StdDirSource, &root, &AtomicBool::new(false));
        assert_eq!(
            (out.files, out.bytes, out.errors),
            (expected_files, expected_bytes, 0)
        );
        assert_eq!(
            q::<i64>(&index, "SELECT COUNT(*) FROM files") as u64,
            expected_files
        );
        assert_eq!(
            q::<i64>(&index, "SELECT SUM(size) FROM files") as u64,
            expected_bytes
        );
        assert_eq!(q::<i64>(&index, "SELECT COUNT(*) FROM dirs"), 1 + 4 + 4);
    }
}
