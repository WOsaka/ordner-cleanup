//! Aufnahmedatum aus dem EXIF-Header von Bildern (`DateTimeOriginal`, sonst
//! `DateTimeDigitized`). Fremde Bilddaten sind nicht vertrauenswürdig: Es wird nur mit
//! Obergrenze gelesen, und jeder Fehler (auch ein Panic im Parser) gilt als „kein EXIF“.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime};

use super::hasher;
use crate::paths;
use crate::platform::FileAttrs;

/// Es werden höchstens so viele Bytes vom Dateianfang gelesen.
pub const MAX_READ: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum ExifError {
    /// Cloud-only-Platzhalter oder Link: wird nicht geöffnet (kein Download).
    NotLocal,
    Io(io::Error),
}

impl std::fmt::Display for ExifError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLocal => f.write_str("Cloud-Platzhalter oder Link"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ExifError {}

/// Lesen mit fester Obergrenze: ab `limit` Bytes ein Fehler statt weiterer Daten.
struct LimitedReader<R> {
    inner: R,
    pos: u64,
    limit: u64,
}

impl<R: Read + Seek> Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.limit {
            return Err(io::Error::other("Obergrenze für das Lesen erreicht"));
        }
        let max = buf.len().min((self.limit - self.pos) as usize);
        let n = self.inner.read(&mut buf[..max])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for LimitedReader<R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.pos = self.inner.seek(from)?;
        Ok(self.pos)
    }
}

/// Aufnahmedatum der Datei. `Ok(None)`: kein (lesbares) EXIF, kaputte oder zu große Daten.
/// Cloud-only-Dateien und Links werden live erkannt und nie geöffnet.
pub fn read_taken(path: &Path) -> Result<Option<NaiveDateTime>, ExifError> {
    read_taken_limited(path, MAX_READ)
}

fn read_taken_limited(path: &Path, limit: u64) -> Result<Option<NaiveDateTime>, ExifError> {
    let meta = std::fs::symlink_metadata(paths::extended(path)).map_err(ExifError::Io)?;
    {
        use std::os::windows::fs::MetadataExt;
        let attrs = FileAttrs(meta.file_attributes());
        if attrs.is_cloud_only() || meta.file_type().is_symlink() || attrs.0 & FileAttrs::REPARSE_POINT != 0 {
            return Err(ExifError::NotLocal);
        }
    }
    let file: File = hasher::open_read(path).map_err(ExifError::Io)?;
    let mut reader = BufReader::new(LimitedReader {
        inner: file,
        pos: 0,
        limit,
    });
    // Ein Panic im Parser darf nie den Lauf beenden.
    let parsed = catch_unwind(AssertUnwindSafe(|| {
        exif::Reader::new().read_from_container(&mut reader)
    }));
    let Ok(Ok(exif)) = parsed else {
        return Ok(None);
    };
    for tag in [exif::Tag::DateTimeOriginal, exif::Tag::DateTimeDigitized] {
        let Some(field) = exif.get_field(tag, exif::In::PRIMARY) else {
            continue;
        };
        if let exif::Value::Ascii(values) = &field.value {
            if let Some(date) = values.first().and_then(|v| parse_exif_datetime(v)) {
                return Ok(Some(date));
            }
        }
    }
    Ok(None)
}

fn parse_exif_datetime(ascii: &[u8]) -> Option<NaiveDateTime> {
    let dt = exif::DateTime::from_ascii(ascii).ok()?;
    NaiveDate::from_ymd_opt(i32::from(dt.year), u32::from(dt.month), u32::from(dt.day))?
        .and_hms_opt(u32::from(dt.hour), u32::from(dt.minute), u32::from(dt.second))
}

/// Minimale EXIF-Bilder für Tests (auch Integrationstests), ohne Binär-Fixtures.
#[doc(hidden)]
pub mod testing {
    use chrono::NaiveDateTime;

    pub const DATE_TIME_ORIGINAL: u16 = 0x9003;
    pub const DATE_TIME_DIGITIZED: u16 = 0x9004;

    /// JPEG (Start, ein EXIF-Segment, Ende) mit genau einem Datumsfeld.
    pub fn jpeg_with_date(tag: u16, date: NaiveDateTime) -> Vec<u8> {
        let text = format!("{}\0", date.format("%Y:%m:%d %H:%M:%S"));
        let mut tiff: Vec<u8> = vec![b'M', b'M', 0, 42, 0, 0, 0, 8];
        // IFD0: ein Eintrag (Zeiger auf das Exif-IFD bei Offset 26).
        tiff.extend([0, 1, 0x87, 0x69, 0, 4, 0, 0, 0, 1, 0, 0, 0, 26, 0, 0, 0, 0]);
        // Exif-IFD: ein ASCII-Eintrag mit 20 Bytes bei Offset 44.
        tiff.extend([0, 1]);
        tiff.extend(tag.to_be_bytes());
        tiff.extend([0, 2, 0, 0, 0, 20, 0, 0, 0, 44, 0, 0, 0, 0]);
        tiff.extend(text.as_bytes());
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(tiff);
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend(((app1.len() + 2) as u16).to_be_bytes());
        jpeg.extend(app1);
        jpeg.extend([0xFF, 0xD9]);
        jpeg
    }

    /// JPEG ohne EXIF.
    pub fn jpeg_without_exif() -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, 0xD9]
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn dt(y: i32, m: u32, d: u32, h: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, 30, 15)
            .unwrap()
    }

    fn write(dir: &tempfile::TempDir, name: &str, data: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    #[test]
    fn liest_date_time_original() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(&dir, "a.jpg", &jpeg_with_date(DATE_TIME_ORIGINAL, dt(2024, 8, 15, 10)));
        assert_eq!(read_taken(&file).unwrap(), Some(dt(2024, 8, 15, 10)));
    }

    #[test]
    fn date_time_digitized_ist_der_zweite_wunsch() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(&dir, "a.jpg", &jpeg_with_date(DATE_TIME_DIGITIZED, dt(2023, 1, 2, 3)));
        assert_eq!(read_taken(&file).unwrap(), Some(dt(2023, 1, 2, 3)));
    }

    #[test]
    fn ohne_exif_abgeschnitten_und_nicht_bild_ergibt_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_taken(&write(&dir, "n.jpg", &jpeg_without_exif())).unwrap(), None);
        let full = jpeg_with_date(DATE_TIME_ORIGINAL, dt(2024, 8, 15, 10));
        let cut = write(&dir, "cut.jpg", &full[..40]);
        assert_eq!(read_taken(&cut).unwrap(), None);
        assert_eq!(read_taken(&write(&dir, "x.txt", b"Hallo Welt")).unwrap(), None);
        assert_eq!(read_taken(&write(&dir, "leer.jpg", b"")).unwrap(), None);
    }

    #[test]
    fn ungueltiges_datum_ergibt_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut data = jpeg_with_date(DATE_TIME_ORIGINAL, dt(2024, 8, 15, 10));
        let pos = data.windows(4).position(|w| w == b"2024").unwrap();
        data[pos..pos + 10].copy_from_slice(b"0000:00:00");
        assert_eq!(read_taken(&write(&dir, "z.jpg", &data)).unwrap(), None);
    }

    #[test]
    fn obergrenze_verhindert_das_lesen_weiter_hinten() {
        let dir = tempfile::tempdir().unwrap();
        let file = write(&dir, "a.jpg", &jpeg_with_date(DATE_TIME_ORIGINAL, dt(2024, 8, 15, 10)));
        assert_eq!(read_taken_limited(&file, 30).unwrap(), None);
        assert!(read_taken_limited(&file, 4096).unwrap().is_some());
    }

    #[test]
    fn zufaellige_bytes_bringen_den_parser_nicht_zu_fall() {
        let dir = tempfile::tempdir().unwrap();
        let mut data = jpeg_with_date(DATE_TIME_ORIGINAL, dt(2024, 8, 15, 10));
        for i in 0..data.len() {
            let original = data[i];
            data[i] = original.wrapping_add(0x55);
            let file = write(&dir, "f.jpg", &data);
            assert!(read_taken(&file).is_ok(), "Byte {i}");
            data[i] = original;
        }
    }

    #[test]
    fn platzhalter_wird_nicht_geoeffnet() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;
        let dir = tempfile::tempdir().unwrap();
        let file = write(&dir, "a.jpg", &jpeg_with_date(DATE_TIME_ORIGINAL, dt(2024, 8, 15, 10)));
        let wide: Vec<u16> = file.as_os_str().encode_wide().chain(Some(0)).collect();
        // FILE_ATTRIBUTE_OFFLINE: wie ein Cloud-Platzhalter, aber mit lesbarem Inhalt.
        assert_ne!(unsafe { SetFileAttributesW(wide.as_ptr(), 0x1000) }, 0);
        assert!(matches!(read_taken(&file), Err(ExifError::NotLocal)));
    }

    #[test]
    fn fehlende_datei_ist_ein_io_fehler() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_taken(&dir.path().join("fehlt.jpg")),
            Err(ExifError::Io(_))
        ));
    }
}
