//! Exklusive Sperre für die Dauer eines Scans: eine Datei, die mit `share_mode(0)` geöffnet wird.
//! Solange ein Prozess sie hält, scheitert jedes weitere Öffnen; stirbt der Prozess, gibt das
//! Betriebssystem sie frei (kein Aufräumen einer veralteten Sperre nötig).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("Ein anderer Scan läuft bereits (Sperre {}).", .0.display())]
    Busy(PathBuf),
    #[error("Sperrdatei {} nicht nutzbar: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Hält die Sperre bis zum `Drop`.
#[derive(Debug)]
pub struct ScanLock {
    _file: File,
}

impl ScanLock {
    pub fn acquire(path: &Path) -> Result<ScanLock, LockError> {
        let io = |source| LockError::Io {
            path: path.to_path_buf(),
            source,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        match OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .share_mode(0)
            .open(path)
        {
            Ok(mut file) => {
                // Nur zur Diagnose: wer hält die Sperre?
                let _ = writeln!(file, "pid {}", std::process::id());
                Ok(ScanLock { _file: file })
            }
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
                ) =>
            {
                Err(LockError::Busy(path.to_path_buf()))
            }
            Err(e) => Err(io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zweite_sperre_scheitert_bis_die_erste_frei_ist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("scan.lock");
        let first = ScanLock::acquire(&path).unwrap();
        match ScanLock::acquire(&path) {
            Err(LockError::Busy(p)) => assert_eq!(p, path),
            other => panic!("unerwartet: {:?}", other.err()),
        }
        drop(first);
        assert!(ScanLock::acquire(&path).is_ok());
    }

    #[test]
    fn fehlermeldung_ist_klar() {
        let text = LockError::Busy(PathBuf::from(r"C:\x\scan.lock")).to_string();
        assert!(text.contains("anderer Scan läuft"), "{text}");
    }
}
