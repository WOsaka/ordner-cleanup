use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use xxhash_rust::xxh3::Xxh3;

use crate::paths;

/// Länge der Stichproben am Anfang und Ende der Datei.
pub const SAMPLE: u64 = 64 * 1024;
/// Bis zu dieser Größe deckt der Teil-Hash die ganze Datei ab (Voll-Hash = Teil-Hash).
pub const SMALL_MAX: u64 = 2 * SAMPLE;
const BUFFER: usize = 1024 * 1024;

const FILE_SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;

/// Öffnet nur lesend, ohne andere Programme zu blockieren.
pub fn open_read(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_SEQUENTIAL_SCAN)
        .open(paths::extended(path))
}

/// Datei ist von einem anderen Programm exklusiv geöffnet.
pub fn is_locked(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
    )
}

fn digest(hasher: &Xxh3) -> Vec<u8> {
    hasher.digest128().to_be_bytes().to_vec()
}

fn changed() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Datei hat sich während des Lesens geändert",
    )
}

/// Hash über Größe sowie Anfang und Ende (bzw. den ganzen Inhalt bei kleinen Dateien).
pub fn partial_hash(file: &mut File, size: u64) -> io::Result<Vec<u8>> {
    let mut hasher = Xxh3::new();
    hasher.update(&size.to_le_bytes());
    file.seek(SeekFrom::Start(0))?;
    if size <= SMALL_MAX {
        let mut data = Vec::with_capacity(size as usize);
        file.take(size).read_to_end(&mut data)?;
        if data.len() as u64 != size {
            return Err(changed());
        }
        hasher.update(&data);
    } else {
        let mut head = vec![0u8; SAMPLE as usize];
        file.read_exact(&mut head)?;
        file.seek(SeekFrom::Start(size - SAMPLE))?;
        let mut tail = vec![0u8; SAMPLE as usize];
        file.read_exact(&mut tail)?;
        hasher.update(&head);
        hasher.update(&tail);
    }
    Ok(digest(&hasher))
}

/// Hash über den gesamten Inhalt (Streaming). Kleine Dateien: identisch zum Teil-Hash.
pub fn full_hash(file: &mut File, size: u64) -> io::Result<Vec<u8>> {
    if size <= SMALL_MAX {
        return partial_hash(file, size);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Xxh3::new();
    let mut buf = vec![0u8; BUFFER];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    if total != size {
        return Err(changed());
    }
    Ok(digest(&hasher))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make(dir: &Path, name: &str, content: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    fn hashes(path: &Path) -> (Vec<u8>, Vec<u8>) {
        let size = std::fs::metadata(path).unwrap().len();
        let mut f = open_read(path).unwrap();
        (
            partial_hash(&mut f, size).unwrap(),
            full_hash(&mut f, size).unwrap(),
        )
    }

    #[test]
    fn kleine_dateien_voll_hash_gleich_teil_hash() {
        let d = tempfile::tempdir().unwrap();
        let (p, f) = hashes(&make(d.path(), "a", b"hallo welt"));
        assert_eq!(p, f);
        assert_eq!(p.len(), 16);
    }

    #[test]
    fn grenze_bei_128_kib() {
        let d = tempfile::tempdir().unwrap();
        let (p, f) = hashes(&make(d.path(), "a", &vec![7u8; SMALL_MAX as usize]));
        assert_eq!(p, f);
        let (p, f) = hashes(&make(d.path(), "b", &vec![7u8; SMALL_MAX as usize + 1]));
        assert_ne!(p, f);
    }

    #[test]
    fn gleicher_inhalt_gleicher_hash_anderer_inhalt_anderer() {
        let d = tempfile::tempdir().unwrap();
        let a = hashes(&make(d.path(), "a", b"abc"));
        let b = hashes(&make(d.path(), "b", b"abc"));
        let c = hashes(&make(d.path(), "c", b"abd"));
        assert_eq!(a, b);
        assert_ne!(a.0, c.0);
        assert_ne!(a.1, c.1);
    }

    #[test]
    fn aenderung_in_der_mitte_betrifft_nur_den_voll_hash() {
        let d = tempfile::tempdir().unwrap();
        let mut content = vec![1u8; 400 * 1024];
        let a = hashes(&make(d.path(), "a", &content));
        content[200 * 1024] = 2;
        let b = hashes(&make(d.path(), "b", &content));
        assert_eq!(a.0, b.0, "Teil-Hash sieht nur Anfang und Ende");
        assert_ne!(a.1, b.1, "Voll-Hash sieht die Mitte");
    }

    #[test]
    fn aenderung_am_ende_betrifft_den_teil_hash() {
        let d = tempfile::tempdir().unwrap();
        let mut content = vec![1u8; 400 * 1024];
        let a = hashes(&make(d.path(), "a", &content));
        *content.last_mut().unwrap() = 9;
        let b = hashes(&make(d.path(), "b", &content));
        assert_ne!(a.0, b.0);
    }

    #[test]
    fn kuerzere_datei_als_erwartet_ist_fehler() {
        let d = tempfile::tempdir().unwrap();
        let p = make(d.path(), "a", b"kurz");
        let mut f = open_read(&p).unwrap();
        assert!(partial_hash(&mut f, 100).is_err());
        let p = make(d.path(), "b", &vec![0u8; 300 * 1024]);
        let mut f = open_read(&p).unwrap();
        assert!(full_hash(&mut f, 400 * 1024).is_err());
    }

    #[test]
    fn exklusiv_geoeffnete_datei_ist_gesperrt() {
        let d = tempfile::tempdir().unwrap();
        let p = make(d.path(), "a", b"x");
        let holder = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&p)
            .unwrap();
        let err = open_read(&p).unwrap_err();
        assert!(is_locked(&err), "{err:?}");
        drop(holder);
        assert!(open_read(&p).is_ok());
    }

    #[test]
    fn schreibender_prozess_blockiert_nicht() {
        let d = tempfile::tempdir().unwrap();
        let p = make(d.path(), "a", b"x");
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_ALL)
            .open(&p)
            .unwrap();
        writer.write_all(b"y").unwrap();
        assert!(open_read(&p).is_ok());
    }
}
