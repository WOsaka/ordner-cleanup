use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
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

/// Verschiebt eine Datei auf demselben Volume und überschreibt nie ein vorhandenes Ziel
/// (anders als `std::fs::rename`). Über Laufwerksgrenzen schlägt der Aufruf fehl, es wird
/// nie kopiert und gelöscht.
pub fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let (from, to) = (wide(&paths::extended(from)), wide(&paths::extended(to)));
    // SAFETY: beide Puffer sind nullterminiert und leben über den Aufruf; Flags 0 =
    // weder Ersetzen noch Kopieren erlaubt.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

const FILETIME_UNIX_DIFF: i64 = 116_444_736_000_000_000;
/// Attribute, die `SetFileAttributesW` für Ordner setzen darf (readonly, hidden, system,
/// archive, not-content-indexed).
const SETTABLE_DIR_ATTRS: u32 = 0x1 | 0x2 | 0x4 | 0x20 | 0x2000;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

fn ticks_to_filetime(ticks: i64) -> FILETIME {
    let value = ticks.saturating_add(FILETIME_UNIX_DIFF).max(0) as u64;
    FILETIME {
        dwLowDateTime: value as u32,
        dwHighDateTime: (value >> 32) as u32,
    }
}

/// Setzt Erstellungs- und Änderungszeit sowie die Attribute eines Ordners (Undo von
/// `remove-dir`). Öffnet nur mit `FILE_WRITE_ATTRIBUTES`.
pub fn set_dir_times_and_attrs(
    path: &Path,
    attrs: u32,
    mtime_ticks: i64,
    ctime_ticks: i64,
) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        SetFileAttributesW, SetFileTime, FILE_WRITE_ATTRIBUTES,
    };
    let name = wide(&paths::extended(path));
    // SAFETY: `name` ist nullterminiert; das Handle wird unten geschlossen.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_WRITE_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let (created, written) = (
        ticks_to_filetime(ctime_ticks),
        ticks_to_filetime(mtime_ticks),
    );
    // SAFETY: `handle` ist gültig, die FILETIME-Zeiger leben über den Aufruf.
    let times_ok = unsafe { SetFileTime(handle, &created, std::ptr::null(), &written) };
    let times_err = io::Error::last_os_error();
    unsafe { CloseHandle(handle) };
    if times_ok == 0 {
        return Err(times_err);
    }
    let settable = attrs & SETTABLE_DIR_ATTRS;
    let value = if settable == 0 {
        FILE_ATTRIBUTE_NORMAL
    } else {
        settable
    };
    // SAFETY: `name` ist nullterminiert.
    if unsafe { SetFileAttributesW(name.as_ptr(), value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Downloads-Ordner des Nutzers über die Known Folder API (`FOLDERID_Downloads`), damit auch
/// umgeleitete Ordner gefunden werden. `None`, wenn Windows keinen Pfad liefert.
pub fn downloads_dir() -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath};

    let mut raw: *mut u16 = std::ptr::null_mut();
    // SAFETY: `FOLDERID_Downloads` ist eine gültige GUID, `raw` nimmt den von Windows
    // allozierten, nullterminierten Pfad auf; er wird unten mit `CoTaskMemFree` freigegeben.
    let hr =
        unsafe { SHGetKnownFolderPath(&FOLDERID_Downloads, 0, std::ptr::null_mut(), &mut raw) };
    if raw.is_null() {
        return None;
    }
    let path = (hr >= 0).then(|| {
        // SAFETY: Bei Erfolg zeigt `raw` auf einen nullterminierten UTF-16-Puffer.
        let len = (0..).take_while(|&i| unsafe { *raw.add(i) } != 0).count();
        let units = unsafe { std::slice::from_raw_parts(raw, len) };
        std::path::PathBuf::from(std::ffi::OsString::from_wide(units))
    });
    // SAFETY: `raw` stammt von `SHGetKnownFolderPath` und wird genau einmal freigegeben.
    unsafe { CoTaskMemFree(raw as *const _) };
    path
}

/// Kurzname (8.3) eines existierenden Pfads, falls Windows einen vergibt.
pub fn short_path(path: &Path) -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let name = wide(path);
    let mut buf = vec![0u16; 1024];
    // SAFETY: `name` ist nullterminiert, `buf` bietet die angegebene Länge.
    let len = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
            name.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as u32,
        )
    } as usize;
    if len == 0 || len > buf.len() {
        return None;
    }
    let short = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buf[..len]));
    (short != path).then_some(short)
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
    fn downloads_ordner_kommt_von_der_known_folder_api() {
        let dir = downloads_dir().expect("Windows kennt immer einen Downloads-Ordner");
        assert!(dir.is_absolute(), "{}", dir.display());
        assert!(
            dir.file_name().is_some(),
            "kein Laufwerks-Stamm erwartet: {}",
            dir.display()
        );
    }

    #[test]
    fn fehlende_datei_ist_fehler() {
        assert!(file_identity(Path::new(r"C:\gibt\es\nicht.txt")).is_err());
    }
}
