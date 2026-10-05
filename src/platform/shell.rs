//! Explorer-Integration für die GUI: Datei im Explorer zeigen, mit dem Standardprogramm öffnen,
//! Ordner-/Dateidialog und eine Startmenü-Verknüpfung.

use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::FileAttrs;
use crate::paths;

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// Attribute ohne Links zu folgen und ohne den Inhalt zu öffnen (kein Cloud-Recall).
fn attrs_of(path: &Path) -> Option<FileAttrs> {
    std::fs::symlink_metadata(paths::extended(path))
        .ok()
        .map(|m| FileAttrs(m.file_attributes()))
}

/// Eine lokal vorhandene Datei bzw. ein Ordner; Cloud-only-Platzhalter nicht (Öffnen würde einen
/// Download auslösen).
pub fn can_open(path: &Path) -> bool {
    attrs_of(path).is_some_and(|a| !a.is_cloud_only())
}

/// Öffnet mit dem Standardprogramm (wie ein Doppelklick im Explorer).
pub fn open(path: &Path) -> Result<()> {
    match attrs_of(path) {
        None => bail!("{} existiert nicht mehr", paths::display(path)),
        Some(a) if a.is_cloud_only() => bail!(
            "{} liegt nur in der Cloud; Öffnen würde einen Download auslösen",
            paths::display(path)
        ),
        Some(_) => {}
    }
    // ShellExecute kommt mit dem Präfix `\\?\` bei vielen Handlern nicht zurecht; er wird nur
    // für Pfade über der Längengrenze verwendet.
    let plain = paths::display(path);
    let target = if plain.chars().count() < 250 {
        PathBuf::from(plain)
    } else {
        paths::extended(path)
    };
    let file = wide(target.as_os_str());
    let verb = wide(std::ffi::OsStr::new("open"));
    // SAFETY: Zeiger auf nullterminierte Puffer, die den Aufruf überdauern.
    let result = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        )
    };
    // Werte bis 32 sind Fehler (Dokumentation von ShellExecute).
    if (result as isize) <= 32 {
        bail!("{} ließ sich nicht öffnen", paths::display(path));
    }
    Ok(())
}

/// Zeigt die Datei im Explorer (markiert); ein Ordner wird geöffnet.
pub fn reveal(path: &Path) -> Result<()> {
    let shown = paths::display(path);
    let mut cmd = std::process::Command::new("explorer.exe");
    if path.is_dir() {
        cmd.arg(&shown);
    } else {
        cmd.raw_arg(format!("/select,\"{shown}\""));
    }
    // Explorer meldet auch bei Erfolg oft Exit-Code 1; nur ein Startfehler zählt.
    cmd.spawn()
        .context("Explorer konnte nicht gestartet werden")?;
    Ok(())
}

/// Öffnet eine Textdatei im Editor (Notepad); fehlt die Datei, wird sie leer angelegt.
pub fn edit(path: &Path) -> Result<()> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("{} nicht anlegbar", paths::display(dir)))?;
        }
        std::fs::write(path, "")
            .with_context(|| format!("{} nicht anlegbar", paths::display(path)))?;
    }
    std::process::Command::new("notepad.exe")
        .arg(path)
        .spawn()
        .context("Editor konnte nicht gestartet werden")?;
    Ok(())
}

/// Windows-Ordnerdialog.
pub fn pick_folder(start: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(dir) = start.filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_folder()
}

/// Windows-Dateidialog mit einem Filter (Name, Endungen ohne Punkt).
pub fn pick_file(start: Option<&Path>, filter: Option<(&str, &[&str])>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(dir) = start.filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    if let Some((name, exts)) = filter {
        dialog = dialog.add_filter(name, exts);
    }
    dialog.pick_file()
}

/// Startmenü-Ordner des Benutzers (`%APPDATA%\Microsoft\Windows\Start Menu\Programs`).
pub fn start_menu_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|appdata| {
        PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
    })
}

/// Legt `<dir>\<name>.lnk` auf `target` an (nur im Benutzerprofil, nie mit Adminrechten).
pub fn create_shortcut(dir: &Path, name: &str, target: &Path) -> Result<PathBuf> {
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    std::fs::create_dir_all(dir)
        .with_context(|| format!("{} nicht anlegbar", paths::display(dir)))?;
    let file = dir.join(format!("{name}.lnk"));
    let target_w = wide(target.as_os_str());
    let file_w = wide(file.as_os_str());
    let workdir = target.parent().map(|p| wide(p.as_os_str()));

    // SAFETY: COM wird auf diesem Thread initialisiert und am Ende wieder freigegeben; alle
    // Zeiger zeigen auf nullterminierte Puffer, die die Aufrufe überdauern.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| -> windows::core::Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(PCWSTR(target_w.as_ptr()))?;
            if let Some(dir) = &workdir {
                link.SetWorkingDirectory(PCWSTR(dir.as_ptr()))?;
            }
            let persist: IPersistFile = link.cast()?;
            persist.Save(PCWSTR(file_w.as_ptr()), true)
        })();
        if initialized {
            CoUninitialize();
        }
        result.context("Verknüpfung konnte nicht angelegt werden")?;
    }
    Ok(file)
}

/// Verknüpfung „ordner-cleanup“ im Startmenü des Benutzers, die auf diese exe zeigt.
pub fn create_start_menu_shortcut() -> Result<PathBuf> {
    let dir = start_menu_dir().context("%APPDATA% nicht ermittelbar")?;
    let exe = std::env::current_exe().context("Pfad des Programms nicht ermittelbar")?;
    create_shortcut(&dir, "ordner-cleanup", &exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vorhandene_datei_ist_oeffenbar_fehlende_nicht() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(can_open(&file));
        assert!(can_open(dir.path()));
        assert!(!can_open(&dir.path().join("fehlt.txt")));
    }

    #[test]
    fn oeffnen_einer_fehlenden_datei_meldet_den_pfad() {
        let err = open(Path::new(r"Z:\gibt-es-nicht.txt")).unwrap_err();
        assert!(err.to_string().contains("existiert nicht mehr"), "{err}");
    }

    #[test]
    fn verknuepfung_wird_im_angegebenen_ordner_angelegt() {
        let dir = tempfile::tempdir().unwrap();
        let exe = std::env::current_exe().unwrap();
        let lnk = create_shortcut(dir.path(), "Test", &exe).unwrap();
        assert_eq!(lnk, dir.path().join("Test.lnk"));
        assert!(lnk.is_file());
    }

    #[test]
    fn startmenue_liegt_unter_appdata() {
        if let Some(dir) = start_menu_dir() {
            assert!(dir.ends_with(r"Start Menu\Programs"));
        }
    }
}
