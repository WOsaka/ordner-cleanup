use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetDriveTypeW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

use crate::paths;

const DRIVE_REMOTE: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    Local,
    Remote,
    Unc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub volume_serial: u32,
    pub file_index: u64,
    pub nlinks: u32,
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

pub fn drive_kind(path: &Path) -> DriveKind {
    let plain = paths::display(path);
    if plain.starts_with(r"\\") {
        return DriveKind::Unc;
    }
    let Some(letter) = plain.chars().next().filter(|c| c.is_ascii_alphabetic()) else {
        return DriveKind::Local;
    };
    let root = wide(Path::new(&format!(r"{letter}:\")));
    // SAFETY: `root` ist nullterminiert und lebt über den Aufruf.
    if unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE {
        DriveKind::Remote
    } else {
        DriveKind::Local
    }
}

/// Liest Volume-Seriennummer, File-ID und Link-Anzahl; öffnet nur mit
/// `FILE_READ_ATTRIBUTES` (löst keinen Cloud-Recall aus).
pub fn file_identity(path: &Path) -> io::Result<FileIdentity> {
    let name = wide(&paths::extended(path));
    // SAFETY: `name` ist nullterminiert; das Handle wird unten geschlossen.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: reine Zahlenstruktur; `handle` ist gültig.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    let err = io::Error::last_os_error();
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return Err(err);
    }
    Ok(FileIdentity {
        volume_serial: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        nlinks: info.nNumberOfLinks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unc_pfad_ist_unc() {
        assert_eq!(drive_kind(Path::new(r"\\server\share\x")), DriveKind::Unc);
        assert_eq!(
            drive_kind(Path::new(r"\\?\UNC\server\share\x")),
            DriveKind::Unc
        );
    }

    #[test]
    fn temp_ordner_ist_lokal() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(drive_kind(dir.path()), DriveKind::Local);
    }

    #[test]
    fn hardlinks_teilen_identitaet() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        let c = dir.path().join("c.txt");
        std::fs::write(&a, "x").unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        std::fs::write(&c, "x").unwrap();
        let (ia, ib, ic) = (
            file_identity(&a).unwrap(),
            file_identity(&b).unwrap(),
            file_identity(&c).unwrap(),
        );
        assert_eq!(ia, ib);
        assert_eq!(ia.nlinks, 2);
        assert_ne!(ia.file_index, ic.file_index);
        assert_eq!(ic.nlinks, 1);
    }

    #[test]
    fn fehlende_datei_ist_fehler() {
        assert!(file_identity(Path::new(r"C:\gibt\es\nicht.txt")).is_err());
    }
}
