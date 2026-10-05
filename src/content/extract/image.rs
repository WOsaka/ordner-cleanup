//! Bilder: EXIF-Kamera (`Make`/`Model`), GPS und Aufnahmedatum, bei PNG die Maße aus dem
//! IHDR-Header. Liest höchstens [`MAX_READ`] vom Dateianfang; jeder Parserfehler (auch ein
//! Panic) gilt als „kein EXIF“.

use std::io::{BufReader, Read};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use super::{status_for, Extracted};
use crate::content::Status;
use crate::scan::exif::{open_local, MAX_READ};

/// Bereinigt einen EXIF-Text: Steuerzeichen und Nullbytes entfernt, Leerraum zusammengefasst.
pub fn clean_text(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `Apple` + `iPhone 15` → `Apple iPhone 15`; steckt die Marke schon im Modell
/// (`Canon` + `Canon EOS 80D`), wird sie nicht verdoppelt.
pub fn camera_name(make: &str, model: &str) -> Option<String> {
    let (make, model) = (clean_text(make), clean_text(model));
    let name = match (make.is_empty(), model.is_empty()) {
        (true, true) => return None,
        (false, true) => make,
        (true, false) => model,
        (false, false) => {
            if model.to_lowercase().starts_with(&make.to_lowercase()) {
                model
            } else {
                format!("{make} {model}")
            }
        }
    };
    Some(name)
}

fn ascii(exif: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    match &field.value {
        exif::Value::Ascii(values) => values
            .first()
            .map(|v| String::from_utf8_lossy(v).into_owned()),
        _ => None,
    }
}

fn degrees(exif: &exif::Exif, tag: exif::Tag, ref_tag: exif::Tag, negative: u8) -> Option<f64> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    let exif::Value::Rational(parts) = &field.value else {
        return None;
    };
    if parts.len() < 3 || parts.iter().take(3).any(|r| r.denom == 0) {
        return None;
    }
    let value = parts[0].to_f64() + parts[1].to_f64() / 60.0 + parts[2].to_f64() / 3600.0;
    let reference = ascii(exif, ref_tag)?;
    let sign = if reference.as_bytes().first() == Some(&negative) {
        -1.0
    } else {
        1.0
    };
    Some(sign * value)
}

/// GPS-Position; `0/0` und Werte außerhalb des gültigen Bereichs gelten als „kein GPS“.
pub fn gps(exif: &exif::Exif) -> Option<(f64, f64)> {
    let lat = degrees(
        exif,
        exif::Tag::GPSLatitude,
        exif::Tag::GPSLatitudeRef,
        b'S',
    )?;
    let lon = degrees(
        exif,
        exif::Tag::GPSLongitude,
        exif::Tag::GPSLongitudeRef,
        b'W',
    )?;
    let valid = lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
        && !(lat == 0.0 && lon == 0.0);
    valid.then_some((lat, lon))
}

/// Breite und Höhe aus dem PNG-IHDR (erste 24 Bytes), sonst `None`.
pub fn png_size(head: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if head.len() < 24 || head[..8] != SIGNATURE || &head[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(head[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(head[20..24].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

pub fn extract(path: &Path) -> Extracted {
    let reader = match open_local(path, MAX_READ) {
        Ok(r) => r,
        Err(e) => return Extracted::new(status_for(&e)),
    };
    let mut reader = BufReader::new(reader);
    let mut out = Extracted::new(Status::Ok);

    // Kopf für PNG-Maße, danach EXIF ab Anfang (BufReader hat gepuffert, daher zurückspulen).
    let mut head = [0u8; 24];
    let got = reader.read(&mut head).unwrap_or(0);
    if let Some((w, h)) = png_size(&head[..got]) {
        out.meta.insert("img.width".into(), w.to_string());
        out.meta.insert("img.height".into(), h.to_string());
    }
    if std::io::Seek::seek(&mut reader, std::io::SeekFrom::Start(0)).is_err() {
        return out;
    }

    let parsed = catch_unwind(AssertUnwindSafe(|| {
        exif::Reader::new().read_from_container(&mut reader)
    }));
    let Ok(Ok(exif)) = parsed else {
        return out;
    };
    let make = ascii(&exif, exif::Tag::Make).unwrap_or_default();
    let model = ascii(&exif, exif::Tag::Model).unwrap_or_default();
    if let Some(camera) = camera_name(&make, &model) {
        out.meta.insert("exif.camera".into(), camera);
    }
    match gps(&exif) {
        Some((lat, lon)) => {
            out.meta.insert("exif.has_gps".into(), "true".into());
            out.meta.insert("exif.lat".into(), format!("{lat:.5}"));
            out.meta.insert("exif.lon".into(), format!("{lon:.5}"));
        }
        None => {
            out.meta.insert("exif.has_gps".into(), "false".into());
        }
    }
    out
}

/// Bausteine für Test-JPEGs mit Kamera und GPS (auch für Integrationstests).
#[doc(hidden)]
pub mod testing {
    /// JPEG mit EXIF: Make, Model und optional GPS (Breite/Länge in Grad, mit Vorzeichen).
    pub fn jpeg_with_camera(make: &str, model: &str, gps: Option<(f64, f64)>) -> Vec<u8> {
        // TIFF-Layout (Big Endian), alle Offsets relativ zum TIFF-Anfang:
        // Header(8) | IFD0 | Strings | GPS-IFD | GPS-Daten
        let make_b = format!("{make}\0").into_bytes();
        let model_b = format!("{model}\0").into_bytes();
        let entries: u16 = if gps.is_some() { 3 } else { 2 };
        let ifd0_len = 2 + usize::from(entries) * 12 + 4;
        let make_off = 8 + ifd0_len;
        let model_off = make_off + make_b.len();
        let gps_ifd_off = model_off + model_b.len();

        let mut t: Vec<u8> = vec![b'M', b'M', 0, 42, 0, 0, 0, 8];
        t.extend(entries.to_be_bytes());
        let ascii_entry = |tag: u16, len: usize, off: usize| {
            let mut e = tag.to_be_bytes().to_vec();
            e.extend([0, 2]);
            e.extend((len as u32).to_be_bytes());
            e.extend((off as u32).to_be_bytes());
            e
        };
        t.extend(ascii_entry(0x010F, make_b.len(), make_off));
        t.extend(ascii_entry(0x0110, model_b.len(), model_off));
        if gps.is_some() {
            // GPS-IFD-Zeiger (Typ LONG)
            t.extend([0x88, 0x25, 0, 4, 0, 0, 0, 1]);
            t.extend((gps_ifd_off as u32).to_be_bytes());
        }
        t.extend([0, 0, 0, 0]);
        t.extend(&make_b);
        t.extend(&model_b);
        if let Some((lat, lon)) = gps {
            // GPS-IFD: 4 Einträge (LatRef, Lat, LonRef, Lon)
            let ifd_len = 2 + 4 * 12 + 4;
            let lat_off = gps_ifd_off + ifd_len;
            let lon_off = lat_off + 24;
            t.extend(4u16.to_be_bytes());
            let reference = |tag: u16, c: u8| {
                let mut e = tag.to_be_bytes().to_vec();
                e.extend([0, 2, 0, 0, 0, 2, c, 0, 0, 0]);
                e
            };
            let rational = |tag: u16, off: usize| {
                let mut e = tag.to_be_bytes().to_vec();
                e.extend([0, 5, 0, 0, 0, 3]);
                e.extend((off as u32).to_be_bytes());
                e
            };
            t.extend(reference(0x0001, if lat < 0.0 { b'S' } else { b'N' }));
            t.extend(rational(0x0002, lat_off));
            t.extend(reference(0x0003, if lon < 0.0 { b'W' } else { b'E' }));
            t.extend(rational(0x0004, lon_off));
            t.extend([0, 0, 0, 0]);
            for value in [lat.abs(), lon.abs()] {
                let deg = value.floor();
                let min_total = (value - deg) * 60.0;
                let min = min_total.floor();
                let sec = ((min_total - min) * 60.0 * 1000.0).round();
                for (num, den) in [(deg as u32, 1u32), (min as u32, 1), (sec as u32, 1000)] {
                    t.extend(num.to_be_bytes());
                    t.extend(den.to_be_bytes());
                }
            }
        }
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(t);
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend(((app1.len() + 2) as u16).to_be_bytes());
        jpeg.extend(app1);
        jpeg.extend([0xFF, 0xD9]);
        jpeg
    }

    /// PNG-Kopf (Signatur + IHDR) mit den gegebenen Maßen; kein gültiges Bild, genügt für Tests.
    pub fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut p = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        p.extend(b"IHDR");
        p.extend(width.to_be_bytes());
        p.extend(height.to_be_bytes());
        p.extend([8, 2, 0, 0, 0, 0, 0, 0, 0]);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn write(dir: &tempfile::TempDir, name: &str, data: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn kamera_und_gps_werden_gelesen() {
        let dir = tempfile::tempdir().unwrap();
        // Lissabon: 38.7223 N, 9.1393 W
        let data = jpeg_with_camera("Apple", "iPhone 15", Some((38.7223, -9.1393)));
        let out = extract(&write(&dir, "a.jpg", &data));
        assert_eq!(out.status, Status::Ok);
        assert_eq!(out.meta["exif.camera"], "Apple iPhone 15");
        assert_eq!(out.meta["exif.has_gps"], "true");
        let lat: f64 = out.meta["exif.lat"].parse().unwrap();
        let lon: f64 = out.meta["exif.lon"].parse().unwrap();
        assert!((lat - 38.7223).abs() < 0.001, "{lat}");
        assert!((lon + 9.1393).abs() < 0.001, "{lon}");
    }

    #[test]
    fn ohne_gps_steht_has_gps_false() {
        let dir = tempfile::tempdir().unwrap();
        let data = jpeg_with_camera("Canon", "Canon EOS 80D", None);
        let out = extract(&write(&dir, "a.jpg", &data));
        assert_eq!(out.meta["exif.camera"], "Canon EOS 80D");
        assert_eq!(out.meta["exif.has_gps"], "false");
        assert!(!out.meta.contains_key("exif.lat"));
    }

    #[test]
    fn gps_null_null_gilt_als_kein_gps() {
        let dir = tempfile::tempdir().unwrap();
        let data = jpeg_with_camera("Apple", "iPhone", Some((0.0, 0.0)));
        let out = extract(&write(&dir, "a.jpg", &data));
        assert_eq!(out.meta["exif.has_gps"], "false");
    }

    #[test]
    fn bild_ohne_exif_und_kaputte_daten_sind_ok_ohne_felder() {
        let dir = tempfile::tempdir().unwrap();
        for data in [
            vec![0xFF, 0xD8, 0xFF, 0xD9],
            b"kein bild".to_vec(),
            Vec::new(),
        ] {
            let out = extract(&write(&dir, "a.jpg", &data));
            assert_eq!(out.status, Status::Ok);
            assert!(!out.meta.contains_key("exif.camera"));
        }
    }

    #[test]
    fn png_masse_aus_dem_ihdr() {
        let dir = tempfile::tempdir().unwrap();
        let out = extract(&write(&dir, "s.png", &png_header(1920, 1080)));
        assert_eq!(out.meta["img.width"], "1920");
        assert_eq!(out.meta["img.height"], "1080");
        assert_eq!(png_size(b"zu kurz"), None);
    }

    #[test]
    fn zufaellige_bytes_bringen_den_parser_nicht_zu_fall() {
        let dir = tempfile::tempdir().unwrap();
        let mut data = jpeg_with_camera("Apple", "iPhone 15", Some((38.7, -9.1)));
        for i in 0..data.len() {
            let original = data[i];
            data[i] = original.wrapping_add(0x55);
            let out = extract(&write(&dir, "f.jpg", &data));
            assert_eq!(out.status, Status::Ok, "Byte {i}");
            data[i] = original;
        }
    }

    #[test]
    fn kamera_namen() {
        assert_eq!(
            camera_name("Apple", "iPhone 15").unwrap(),
            "Apple iPhone 15"
        );
        assert_eq!(
            camera_name("Canon", "Canon EOS 80D").unwrap(),
            "Canon EOS 80D"
        );
        assert_eq!(
            camera_name(" SONY\0 ", " ILCE-7M3 ").unwrap(),
            "SONY ILCE-7M3"
        );
        assert_eq!(camera_name("", "Pixel 8").unwrap(), "Pixel 8");
        assert_eq!(camera_name("", " \0"), None);
    }

    #[test]
    fn platzhalter_wird_nicht_geoeffnet() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;
        let dir = tempfile::tempdir().unwrap();
        let file = write(&dir, "a.jpg", &jpeg_with_camera("A", "B", None));
        let wide: Vec<u16> = file.as_os_str().encode_wide().chain(Some(0)).collect();
        assert_ne!(unsafe { SetFileAttributesW(wide.as_ptr(), 0x1000) }, 0);
        assert_eq!(
            extract(&file).status,
            Status::Unreadable("cloud-placeholder".into())
        );
    }
}
