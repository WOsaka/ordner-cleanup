use std::ffi::OsString;
use std::io;
use std::path::Path;

use crate::paths;
use crate::platform::FileAttrs;

/// Zeitstempel sind 100-ns-Ticks seit der Unix-Epoche.
pub const TICKS_PER_SEC: i64 = 10_000_000;
const FILETIME_UNIX_DIFF: u64 = 116_444_736_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    /// Symlink oder Junction (wird nie betreten).
    Link {
        dir: bool,
    },
}

#[derive(Debug, Clone)]
pub struct RawEntry {
    pub name: OsString,
    pub kind: EntryKind,
    pub size: u64,
    pub mtime: i64,
    pub ctime: i64,
    pub attrs: FileAttrs,
    pub link_target: Option<String>,
}

#[derive(Debug, Default)]
pub struct DirListing {
    pub entries: Vec<RawEntry>,
    /// Einträge, deren Metadaten nicht gelesen werden konnten.
    pub errors: Vec<(OsString, io::Error)>,
}

/// Lesender Zugriff auf Verzeichnisse. Öffnet nie Dateiinhalte.
pub trait DirSource: Sync {
    fn read_dir(&self, path: &Path) -> io::Result<DirListing>;
}

pub fn filetime_to_ticks(filetime: u64) -> i64 {
    filetime.saturating_sub(FILETIME_UNIX_DIFF) as i64
}

/// Echtes Dateisystem. `std::fs::read_dir` liefert unter Windows die Find-Daten,
/// ohne die Dateien zu öffnen (kein OneDrive-Recall).
pub struct StdDirSource;

impl DirSource for StdDirSource {
    fn read_dir(&self, path: &Path) -> io::Result<DirListing> {
        use std::os::windows::fs::MetadataExt;

        let mut listing = DirListing::default();
        for entry in std::fs::read_dir(paths::extended(path))? {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    listing.errors.push((OsString::new(), e));
                    continue;
                }
            };
            let name = entry.file_name();
            let (file_type, meta) = match (entry.file_type(), entry.metadata()) {
                (Ok(t), Ok(m)) => (t, m),
                (Err(e), _) | (_, Err(e)) => {
                    listing.errors.push((name, e));
                    continue;
                }
            };
            let attrs = FileAttrs(meta.file_attributes());
            let is_dir = attrs.0 & FileAttrs::DIRECTORY != 0;
            let (kind, link_target) = if file_type.is_symlink() {
                let target = std::fs::read_link(entry.path())
                    .ok()
                    .map(|t| paths::display(&t));
                (EntryKind::Link { dir: is_dir }, target)
            } else if is_dir {
                (EntryKind::Dir, None)
            } else {
                (EntryKind::File, None)
            };
            listing.entries.push(RawEntry {
                name,
                kind,
                size: meta.file_size(),
                mtime: filetime_to_ticks(meta.last_write_time()),
                ctime: filetime_to_ticks(meta.creation_time()),
                attrs,
                link_target,
            });
        }
        Ok(listing)
    }
}

#[cfg(test)]
pub use fake::FakeDirSource;

#[cfg(test)]
mod fake {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// Verzeichnisbaum im Speicher; protokolliert jeden `read_dir`-Aufruf.
    #[derive(Default)]
    pub struct FakeDirSource {
        dirs: HashMap<String, Result<Vec<RawEntry>, io::ErrorKind>>,
        log: Mutex<Vec<String>>,
    }

    impl FakeDirSource {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn dir(mut self, path: &str, entries: Vec<RawEntry>) -> Self {
            self.dirs
                .insert(paths::path_key(Path::new(path)), Ok(entries));
            self
        }

        pub fn failing(mut self, path: &str, kind: io::ErrorKind) -> Self {
            self.dirs
                .insert(paths::path_key(Path::new(path)), Err(kind));
            self
        }

        /// Alle gelesenen Verzeichnisse (als `path_key`).
        pub fn reads(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl DirSource for FakeDirSource {
        fn read_dir(&self, path: &Path) -> io::Result<DirListing> {
            let key = paths::path_key(path);
            self.log.lock().unwrap().push(key.clone());
            match self.dirs.get(&key) {
                Some(Ok(entries)) => Ok(DirListing {
                    entries: entries.clone(),
                    errors: vec![],
                }),
                Some(Err(kind)) => Err(io::Error::from(*kind)),
                None => Err(io::Error::from(io::ErrorKind::NotFound)),
            }
        }
    }

    fn entry(name: &str, kind: EntryKind, size: u64, attrs: u32) -> RawEntry {
        RawEntry {
            name: name.into(),
            kind,
            size,
            mtime: 100,
            ctime: 50,
            attrs: FileAttrs(attrs),
            link_target: None,
        }
    }

    pub fn file(name: &str, size: u64) -> RawEntry {
        entry(name, EntryKind::File, size, 0x20)
    }

    pub fn dir(name: &str) -> RawEntry {
        entry(name, EntryKind::Dir, 0, FileAttrs::DIRECTORY)
    }

    pub fn cloud_file(name: &str, size: u64) -> RawEntry {
        entry(
            name,
            EntryKind::File,
            size,
            0x20 | FileAttrs::RECALL_ON_DATA_ACCESS | FileAttrs::REPARSE_POINT,
        )
    }

    pub fn junction(name: &str, target: &str) -> RawEntry {
        let mut e = entry(
            name,
            EntryKind::Link { dir: true },
            0,
            FileAttrs::DIRECTORY | FileAttrs::REPARSE_POINT,
        );
        e.link_target = Some(target.to_string());
        e
    }
}

#[cfg(test)]
pub use fake::{cloud_file, dir as fake_dir, file as fake_file, junction as fake_junction};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_umrechnung() {
        // 1970-01-01 in FILETIME
        assert_eq!(filetime_to_ticks(116_444_736_000_000_000), 0);
        assert_eq!(
            filetime_to_ticks(116_444_736_000_000_000 + TICKS_PER_SEC as u64),
            TICKS_PER_SEC
        );
        assert_eq!(filetime_to_ticks(0), 0);
    }

    #[test]
    fn std_source_listet_dateien_ordner_und_groessen() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "12345").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let listing = StdDirSource.read_dir(dir.path()).unwrap();
        assert!(listing.errors.is_empty());
        let a = listing.entries.iter().find(|e| e.name == "a.txt").unwrap();
        assert_eq!((a.kind, a.size), (EntryKind::File, 5));
        assert!(a.mtime > 0);
        let sub = listing.entries.iter().find(|e| e.name == "sub").unwrap();
        assert_eq!(sub.kind, EntryKind::Dir);
    }

    #[test]
    fn std_source_meldet_fehlenden_ordner() {
        let dir = tempfile::tempdir().unwrap();
        let err = StdDirSource
            .read_dir(&dir.path().join("fehlt"))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn std_source_erkennt_junctions_als_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("ziel");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("link");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(status.status.success(), "mklink fehlgeschlagen");
        let listing = StdDirSource.read_dir(dir.path()).unwrap();
        let l = listing.entries.iter().find(|e| e.name == "link").unwrap();
        assert_eq!(l.kind, EntryKind::Link { dir: true });
        assert!(l.link_target.as_deref().unwrap().ends_with("ziel"));
        let z = listing.entries.iter().find(|e| e.name == "ziel").unwrap();
        assert_eq!(z.kind, EntryKind::Dir);
    }
}
