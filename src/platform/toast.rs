//! Windows-Benachrichtigung (WinRT-Toast) für geplante Läufe.
//!
//! Die App-ID (AUMID) wird ohne Adminrechte unter `HKCU\Software\Classes\AppUserModelId`
//! registriert; sie bestimmt den Absender („Ordner-Cleanup“). Ein Klick öffnet die Datei über
//! `activationType="protocol"` mit einer `file:///`-URI, ohne dass unser Programm läuft.

use std::io;
use std::path::{Path, PathBuf};

use windows::core::HSTRING;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};

pub const AUMID: &str = "WOsaka.OrdnerCleanup";
pub const DISPLAY_NAME: &str = "Ordner-Cleanup";

/// Umgebungsvariable für Tests: statt eines Toasts wird eine JSON-Zeile je Benachrichtigung an
/// diese Datei angehängt. So lässt sich `run --notify` ohne Eingriff ins Windows-Benachrichtigungs-
/// system prüfen.
pub const TEST_LOG_ENV: &str = "ORDNER_CLEANUP_NOTIFY_LOG";

/// Zeigt eine Benachrichtigung; ein Klick öffnet `open`.
pub trait Notifier {
    fn notify(&self, title: &str, body: &str, open: &Path) -> io::Result<()>;
}

/// Der Notifier für den echten Betrieb (bzw. die Testdatei, falls die Umgebungsvariable gesetzt ist).
pub fn system_notifier() -> Box<dyn Notifier> {
    match std::env::var_os(TEST_LOG_ENV) {
        Some(file) => Box::new(FileNotifier(PathBuf::from(file))),
        None => Box::new(WinToast),
    }
}

pub struct WinToast;

impl Notifier for WinToast {
    fn notify(&self, title: &str, body: &str, open: &Path) -> io::Result<()> {
        register_aumid()?;
        show(&toast_xml(title, body, &file_uri(open))).map_err(io::Error::other)
    }
}

/// Schreibt die Benachrichtigung als JSON-Zeile in eine Datei (Test-Notifier).
pub struct FileNotifier(pub PathBuf);

impl Notifier for FileNotifier {
    fn notify(&self, title: &str, body: &str, open: &Path) -> io::Result<()> {
        use std::io::Write;
        let line = serde_json::json!({
            "title": title,
            "body": body,
            "open": open.display().to_string(),
        });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.0)?;
        writeln!(file, "{line}")
    }
}

fn show(xml: &str) -> windows::core::Result<()> {
    // Schon initialisiert oder anderer Apartment-Modus ist in Ordnung.
    // SAFETY: Aufruf ohne Zeiger; der Rückgabewert wird absichtlich ignoriert.
    let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Registriert die App-ID mit Anzeigenamen im Benutzerbereich (idempotent, ohne Adminrechte).
pub fn register_aumid() -> io::Result<()> {
    let subkey = wide(&format!(r"Software\Classes\AppUserModelId\{AUMID}"));
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: Alle Zeiger zeigen auf lebende, nullterminierte bzw. initialisierte Puffer.
    unsafe {
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let name = wide("DisplayName");
        let value = wide(DISPLAY_NAME);
        let status = RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        );
        RegCloseKey(key);
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
    }
    Ok(())
}

/// `file:///C:/Ordner/mit%20Leerzeichen/latest.html`
pub fn file_uri(path: &Path) -> String {
    let plain = crate::paths::display(path).replace('\\', "/");
    let mut out = String::from("file:///");
    for b in plain.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn escape(text: &str) -> String {
    quick_xml::escape::escape(text).into_owned()
}

/// Toast-XML mit Aktivierung über die Datei-URI.
pub fn toast_xml(title: &str, body: &str, uri: &str) -> String {
    format!(
        "<toast activationType=\"protocol\" launch=\"{}\"><visual><binding template=\"ToastGeneric\">\
         <text>{}</text><text>{}</text></binding></visual></toast>",
        escape(uri),
        escape(title),
        escape(body)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_kodiert_leerzeichen_und_umlaute() {
        assert_eq!(
            file_uri(Path::new(r"C:\Users\Max Müller\a&b\latest.html")),
            "file:///C:/Users/Max%20M%C3%BCller/a%26b/latest.html"
        );
        assert_eq!(
            file_uri(Path::new(r"\\?\C:\x\y.html")),
            "file:///C:/x/y.html"
        );
    }

    #[test]
    fn xml_ist_escaped_und_wohlgeformt() {
        let xml = toast_xml("A & <B>", "\"x\" 'y' > z", "file:///C:/a%26b?x=1&y=2");
        assert!(xml.contains("A &amp; &lt;B&gt;"), "{xml}");
        assert!(xml.contains("&amp;y=2"), "{xml}");
        assert!(!xml.contains("<B>"));
        let mut reader = quick_xml::Reader::from_str(&xml);
        loop {
            match reader.read_event().unwrap() {
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
    }

    #[test]
    fn datei_notifier_haengt_json_zeilen_an() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("n.jsonl");
        let n = FileNotifier(file.clone());
        n.notify("T", "B", Path::new(r"C:\r\latest.html")).unwrap();
        n.notify("T2", "B2", Path::new(r"C:\r\x.html")).unwrap();
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&file)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["title"], "T");
        assert_eq!(lines[1]["open"], r"C:\r\x.html");
    }

    /// Echter Toast und echte Registrierung: nur von Hand (`cargo test -- --ignored toast`).
    #[test]
    #[ignore = "zeigt eine echte Windows-Benachrichtigung"]
    fn toast_wird_angezeigt() {
        register_aumid().unwrap();
        WinToast
            .notify(
                "ordner-cleanup Test",
                "Testbenachrichtigung – ein Klick öffnet die Datei.",
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("README.md")
                    .as_path(),
            )
            .unwrap();
    }
}
