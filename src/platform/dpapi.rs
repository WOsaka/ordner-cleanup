//! Windows-DPAPI (`CryptProtectData`): verschlüsselt Daten an das Benutzerkonto. Dient dem
//! OCR-Text-Cache; ohne Benutzeroberfläche, damit nie ein Dialog aufgeht.

use std::ffi::c_void;
use std::io;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr().cast_mut(),
    }
}

/// Kopiert das von Windows belegte Ausgabe-Blob und gibt es frei.
fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if out.pbData.is_null() {
        return Vec::new();
    }
    // SAFETY: Windows liefert `cbData` gültige Bytes ab `pbData`; der Puffer wird danach
    // genau einmal mit `LocalFree` freigegeben.
    unsafe {
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData.cast::<c_void>());
        bytes
    }
}

/// Verschlüsselt `data` für den aktuellen Benutzer.
pub fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = blob(data);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: `input` zeigt auf `data` (lebt über den Aufruf), die übrigen Zeiger sind
    // null (erlaubt), `out` ist ein gültiges Ausgabe-Blob.
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(take(out))
}

/// Entschlüsselt Daten aus [`protect`]. Manipulierte oder fremde Daten ergeben einen Fehler.
pub fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = blob(data);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: wie bei `protect`.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(take(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rundlauf_und_kein_klartext_im_blob() {
        let text = "Rechnung Nr. 4711 Gesamtbetrag 123,45 Ümläute".as_bytes();
        let enc = protect(text).unwrap();
        assert_ne!(enc, text);
        assert!(
            !enc.windows(7).any(|w| w == b"Rechnun"),
            "Klartext im verschlüsselten Blob"
        );
        assert_eq!(unprotect(&enc).unwrap(), text);
    }

    #[test]
    fn leere_daten_laufen_rund() {
        let enc = protect(b"").unwrap();
        assert!(unprotect(&enc).unwrap().is_empty());
    }

    #[test]
    fn manipulierte_oder_fremde_daten_sind_ein_fehler() {
        let mut enc = protect(b"geheim").unwrap();
        let mid = enc.len() / 2;
        enc[mid] ^= 0xFF;
        assert!(unprotect(&enc).is_err());
        assert!(unprotect(b"das ist kein dpapi-blob").is_err());
        assert!(unprotect(&[]).is_err());
    }
}
