//! Dateizugriffe für Apply und Undo hinter einem kleinen Trait (wie `DirSource` in Phase 1),
//! damit Sperren, Fehler und Abstürze in Tests gezielt ausgelöst werden können.

use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::paths;
use crate::platform::windows::{file_identity, move_no_replace, set_dir_times_and_attrs};
use crate::platform::FileAttrs;
use crate::scan::hasher;
use crate::scan::source::filetime_to_ticks;
#[cfg(test)]
use crate::scan::source::TICKS_PER_SEC;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    pub mtime_ticks: i64,
    /// Erstellungszeit (für die Wiederherstellung entfernter Ordner).
    pub ctime_ticks: i64,
    pub attrs: FileAttrs,
    /// Symlink oder Junction.
    pub is_link: bool,
    pub is_dir: bool,
}

impl FileMeta {
    pub fn is_cloud_only(&self) -> bool {
        self.attrs.is_cloud_only()
    }

    pub fn is_reparse_point(&self) -> bool {
        self.attrs.0 & FileAttrs::REPARSE_POINT != 0
    }
}

fn file_meta(meta: &std::fs::Metadata) -> FileMeta {
    let attrs = FileAttrs(meta.file_attributes());
    FileMeta {
        size: meta.file_size(),
        mtime_ticks: filetime_to_ticks(meta.last_write_time()),
        ctime_ticks: filetime_to_ticks(meta.creation_time()),
        attrs,
        is_link: meta.file_type().is_symlink(),
        is_dir: attrs.0 & FileAttrs::DIRECTORY != 0,
    }
}

pub trait FsOps {
    /// Metadaten ohne Links zu folgen und ohne den Inhalt zu öffnen (kein Cloud-Recall).
    fn metadata(&self, path: &Path) -> io::Result<FileMeta>;
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    /// Verschiebt auf demselben Volume und überschreibt nie.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Voll-Hash (xxh3-128) wie in Phase 1. Nur für lokale Dateien aufrufen.
    fn hash(&self, path: &Path, size: u64) -> io::Result<Vec<u8>>;
    fn volume_serial(&self, path: &Path) -> io::Result<u32>;
    /// Löscht einen Ordner samt Inhalt endgültig (nur `purge`).
    fn remove_dir_all(&self, path: &Path) -> io::Result<()>;
    /// Entfernt einen **leeren** Ordner; bei Inhalt schlägt der Aufruf atomar fehl.
    fn remove_dir(&self, path: &Path) -> io::Result<()>;
    /// Legt genau einen Ordner an (nicht rekursiv); ein vorhandener ist `AlreadyExists`.
    fn create_dir(&self, path: &Path) -> io::Result<()>;
    /// Einträge eines Ordners mit Metadaten aus den Find-Daten, ohne etwas zu öffnen
    /// (kein Cloud-Recall). Links werden nicht verfolgt.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<(PathBuf, FileMeta)>>;
    /// Setzt Attribute (nur die setzbaren Bits), Änderungs- und Erstellungszeit eines Ordners.
    fn set_dir_meta(
        &self,
        path: &Path,
        attrs: u32,
        mtime_ticks: i64,
        ctime_ticks: i64,
    ) -> io::Result<()>;

    fn exists(&self, path: &Path) -> bool {
        self.metadata(path).is_ok()
    }
}

pub struct RealFs;

impl FsOps for RealFs {
    fn metadata(&self, path: &Path) -> io::Result<FileMeta> {
        std::fs::symlink_metadata(paths::extended(path)).map(|m| file_meta(&m))
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(paths::extended(path))
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        move_no_replace(from, to)
    }

    fn hash(&self, path: &Path, size: u64) -> io::Result<Vec<u8>> {
        let mut file = hasher::open_read(path)?;
        hasher::full_hash(&mut file, size)
    }

    fn volume_serial(&self, path: &Path) -> io::Result<u32> {
        file_identity(path).map(|i| i.volume_serial)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(paths::extended(path))
    }

    fn remove_dir(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir(paths::extended(path))
    }

    fn create_dir(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir(paths::extended(path))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<(PathBuf, FileMeta)>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(paths::extended(path))? {
            let entry = entry?;
            let meta = file_meta(&entry.metadata()?);
            entries.push((path.join(entry.file_name()), meta));
        }
        Ok(entries)
    }

    fn set_dir_meta(
        &self,
        path: &Path,
        attrs: u32,
        mtime_ticks: i64,
        ctime_ticks: i64,
    ) -> io::Result<()> {
        set_dir_times_and_attrs(path, attrs, mtime_ticks, ctime_ticks)
    }
}

#[cfg(test)]
pub mod testing {
    //! `RealFs` mit gezielt ausgelösten Fehlern, simulierten Platzhaltern und Abstürzen.

    use std::collections::HashSet;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::{FileMeta, FsOps, RealFs};
    use crate::paths;
    use crate::platform::FileAttrs;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Op {
        Hash,
        Rename,
        RemoveDir,
        CreateDir,
    }

    #[derive(Default)]
    pub struct FaultyFs {
        cloud: Mutex<HashSet<String>>,
        failing: Mutex<HashSet<(Op, String)>>,
        /// Alle Pfade, deren Inhalt gelesen wurde (Nachweis: Platzhalter werden nie gehasht).
        hashed: Mutex<Vec<String>>,
        renames: AtomicUsize,
        panic_before_rename: Mutex<Option<usize>>,
        panic_after_rename: Mutex<Option<usize>>,
    }

    impl FaultyFs {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn cloud_only(self, path: &Path) -> Self {
            self.cloud.lock().unwrap().insert(paths::path_key(path));
            self
        }

        pub fn fail(self, op: Op, path: &Path) -> Self {
            self.failing
                .lock()
                .unwrap()
                .insert((op, paths::path_key(path)));
            self
        }

        /// Simuliert einen Prozessabbruch vor bzw. nach dem n-ten Move (1-basiert).
        pub fn crash_before_rename(self, n: usize) -> Self {
            *self.panic_before_rename.lock().unwrap() = Some(n);
            self
        }

        pub fn crash_after_rename(self, n: usize) -> Self {
            *self.panic_after_rename.lock().unwrap() = Some(n);
            self
        }

        pub fn disarm(&self) {
            *self.panic_before_rename.lock().unwrap() = None;
            *self.panic_after_rename.lock().unwrap() = None;
            self.failing.lock().unwrap().clear();
        }

        pub fn hashed(&self) -> Vec<String> {
            self.hashed.lock().unwrap().clone()
        }

        fn is_failing(&self, op: Op, path: &Path) -> bool {
            self.failing
                .lock()
                .unwrap()
                .contains(&(op, paths::path_key(path)))
        }
    }

    fn locked() -> io::Error {
        io::Error::from_raw_os_error(32) // ERROR_SHARING_VIOLATION
    }

    impl FsOps for FaultyFs {
        fn metadata(&self, path: &Path) -> io::Result<FileMeta> {
            let mut meta = RealFs.metadata(path)?;
            if self.cloud.lock().unwrap().contains(&paths::path_key(path)) {
                meta.attrs = FileAttrs(meta.attrs.0 | FileAttrs::RECALL_ON_DATA_ACCESS);
            }
            Ok(meta)
        }

        fn create_dir_all(&self, path: &Path) -> io::Result<()> {
            RealFs.create_dir_all(path)
        }

        fn remove_dir(&self, path: &Path) -> io::Result<()> {
            if self.is_failing(Op::RemoveDir, path) {
                return Err(locked());
            }
            RealFs.remove_dir(path)
        }

        fn create_dir(&self, path: &Path) -> io::Result<()> {
            if self.is_failing(Op::CreateDir, path) {
                return Err(locked());
            }
            RealFs.create_dir(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<(PathBuf, FileMeta)>> {
            let mut entries = RealFs.read_dir(path)?;
            let cloud = self.cloud.lock().unwrap();
            for (p, meta) in &mut entries {
                if cloud.contains(&paths::path_key(p)) {
                    meta.attrs = FileAttrs(meta.attrs.0 | FileAttrs::RECALL_ON_DATA_ACCESS);
                }
            }
            Ok(entries)
        }

        fn set_dir_meta(
            &self,
            path: &Path,
            attrs: u32,
            mtime_ticks: i64,
            ctime_ticks: i64,
        ) -> io::Result<()> {
            RealFs.set_dir_meta(path, attrs, mtime_ticks, ctime_ticks)
        }

        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            let n = self.renames.fetch_add(1, Ordering::SeqCst) + 1;
            if *self.panic_before_rename.lock().unwrap() == Some(n) {
                panic!("simulierter Absturz vor Move {n}");
            }
            if self.is_failing(Op::Rename, from) {
                return Err(locked());
            }
            RealFs.rename(from, to)?;
            if *self.panic_after_rename.lock().unwrap() == Some(n) {
                panic!("simulierter Absturz nach Move {n}");
            }
            Ok(())
        }

        fn hash(&self, path: &Path, size: u64) -> io::Result<Vec<u8>> {
            self.hashed.lock().unwrap().push(paths::path_key(path));
            if self.is_failing(Op::Hash, path) {
                return Err(locked());
            }
            RealFs.hash(path, size)
        }

        fn volume_serial(&self, path: &Path) -> io::Result<u32> {
            RealFs.volume_serial(path)
        }

        fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
            RealFs.remove_dir_all(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::source::TICKS_PER_SEC;

    #[test]
    fn metadata_folgt_keinen_links_und_kennt_ordner() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "abc").unwrap();
        let meta = RealFs.metadata(&file).unwrap();
        assert_eq!(meta.size, 3);
        assert!(!meta.is_dir && !meta.is_link && !meta.is_cloud_only());
        assert!(meta.mtime_ticks > 0);
        assert!(RealFs.metadata(dir.path()).unwrap().is_dir);
        assert!(RealFs.metadata(&dir.path().join("fehlt")).is_err());
        assert!(!RealFs.exists(&dir.path().join("fehlt")));
    }

    #[test]
    fn rename_ueberschreibt_nie_und_erhaelt_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
        std::fs::write(&a, "inhalt a").unwrap();
        std::fs::write(&b, "inhalt b").unwrap();
        let mtime = RealFs.metadata(&a).unwrap().mtime_ticks;
        let err = RealFs.rename(&a, &b).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "inhalt b");
        assert!(a.exists());

        let c = dir.path().join("c.txt");
        RealFs.rename(&a, &c).unwrap();
        assert!(!a.exists());
        assert_eq!(RealFs.metadata(&c).unwrap().mtime_ticks, mtime);
    }

    #[test]
    fn hash_stimmt_mit_phase_1_ueberein() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.bin"), dir.path().join("b.bin"));
        std::fs::write(&a, vec![7u8; 300 * 1024]).unwrap();
        std::fs::write(&b, vec![7u8; 300 * 1024]).unwrap();
        let size = 300 * 1024;
        assert_eq!(
            RealFs.hash(&a, size).unwrap(),
            RealFs.hash(&b, size).unwrap()
        );
        assert_eq!(RealFs.hash(&a, size).unwrap().len(), 16);
    }

    #[test]
    fn volume_serial_ist_pro_volume_gleich() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(
            RealFs.volume_serial(&file).unwrap(),
            RealFs.volume_serial(dir.path()).unwrap()
        );
    }

    #[test]
    fn remove_dir_entfernt_nur_leere_ordner() {
        let dir = tempfile::tempdir().unwrap();
        let leer = dir.path().join("leer");
        let voll = dir.path().join("voll");
        std::fs::create_dir_all(&leer).unwrap();
        std::fs::create_dir_all(&voll).unwrap();
        std::fs::write(voll.join("a.txt"), "x").unwrap();

        RealFs.remove_dir(&leer).unwrap();
        assert!(!leer.exists());

        assert!(RealFs.remove_dir(&voll).is_err());
        assert!(voll.join("a.txt").exists(), "nichts darf verloren gehen");
    }

    #[test]
    fn remove_dir_entfernt_keine_dateien() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(RealFs.remove_dir(&file).is_err());
        assert!(file.exists());
    }

    #[test]
    fn create_dir_legt_nicht_rekursiv_an_und_ueberschreibt_nichts() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        RealFs.create_dir(&a).unwrap();
        assert!(a.is_dir());
        assert_eq!(
            RealFs.create_dir(&a).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        let tief = dir.path().join("x").join("y");
        assert_eq!(
            RealFs.create_dir(&tief).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(!dir.path().join("x").exists());
    }

    #[test]
    fn read_dir_liefert_eintraege_mit_metadaten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "abc").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let mut entries = RealFs.read_dir(dir.path()).unwrap();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, dir.path().join("a.txt"));
        assert_eq!(entries[0].1.size, 3);
        assert!(!entries[0].1.is_dir);
        assert_eq!(entries[1].0, dir.path().join("sub"));
        assert!(entries[1].1.is_dir);
        assert!(RealFs.read_dir(&dir.path().join("fehlt")).is_err());
    }

    #[test]
    fn metadata_liefert_die_erstellungszeit() {
        let dir = tempfile::tempdir().unwrap();
        let meta = RealFs.metadata(dir.path()).unwrap();
        assert!(meta.ctime_ticks > 0);
    }

    #[test]
    fn set_dir_meta_stellt_zeiten_und_attribute_wieder_her() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("ordner");
        std::fs::create_dir(&sub).unwrap();
        let mtime = 1_600_000_000 * TICKS_PER_SEC;
        let ctime = 1_500_000_000 * TICKS_PER_SEC;
        RealFs
            .set_dir_meta(&sub, FileAttrs::HIDDEN | FileAttrs::DIRECTORY, mtime, ctime)
            .unwrap();
        let meta = RealFs.metadata(&sub).unwrap();
        assert_eq!(meta.mtime_ticks, mtime);
        assert_eq!(meta.ctime_ticks, ctime);
        assert!(meta.attrs.is_hidden());
        assert!(meta.is_dir);
    }

    #[test]
    fn set_dir_meta_ohne_attribute_laesst_den_ordner_normal() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("ordner");
        std::fs::create_dir(&sub).unwrap();
        RealFs
            .set_dir_meta(&sub, FileAttrs::DIRECTORY, TICKS_PER_SEC, TICKS_PER_SEC)
            .unwrap();
        let meta = RealFs.metadata(&sub).unwrap();
        assert!(!meta.attrs.is_hidden() && !meta.attrs.is_system());
        assert!(RealFs
            .set_dir_meta(&dir.path().join("fehlt"), 0, 0, 0)
            .is_err());
    }

    #[test]
    fn symlink_oder_junction_ist_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("ziel");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("junction");
        // Junction über mklink /J (braucht keine Adminrechte).
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("Junction nicht erzeugbar, Test übersprungen");
            return;
        }
        let meta = RealFs.metadata(&link).unwrap();
        assert!(meta.is_link && meta.is_reparse_point());
    }
}
