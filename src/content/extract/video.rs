//! Videos (MP4/MOV/M4V/3GP): Aufnahmezeit aus der `mvhd`-Box in `moov`. Eigener, begrenzter
//! ISO-BMFF-Parser: Er springt über die Boxen der obersten Ebene (`mdat` wird nie gelesen)
//! und liest nur den Inhalt von `moov` bis zu einer Obergrenze.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Local, NaiveDateTime};

use super::{status_for, Extracted};
use crate::content::Status;
use crate::scan::exif::open_local;

/// Sekunden zwischen 1904-01-01 und 1970-01-01 (UTC).
const MAC_TO_UNIX: u64 = 2_082_844_800;
/// Mehr als so viele Bytes `moov` werden nicht gelesen.
const MAX_MOOV: u64 = 64 * 1024 * 1024;
/// Obergrenze für Boxen auf oberster Ebene (Schutz vor Endlosschleifen).
const MAX_TOP_BOXES: usize = 10_000;

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes(b[..4].try_into().unwrap())
}
fn be64(b: &[u8]) -> u64 {
    u64::from_be_bytes(b[..8].try_into().unwrap())
}

/// `mvhd`-Nutzdaten (nach dem Box-Header) → Erstellzeit in Sekunden seit 1904, 0 = unbekannt.
fn mvhd_creation(payload: &[u8]) -> Option<u64> {
    let version = *payload.first()?;
    match version {
        0 if payload.len() >= 8 => Some(u64::from(be32(&payload[4..]))),
        1 if payload.len() >= 12 => Some(be64(&payload[4..])),
        _ => None,
    }
}

/// Sucht `mvhd` unter den Kind-Boxen von `moov`.
fn find_mvhd(moov: &[u8]) -> Option<u64> {
    let mut pos = 0usize;
    while pos + 8 <= moov.len() {
        let size = be32(&moov[pos..]) as usize;
        let kind = &moov[pos + 4..pos + 8];
        let (header, total) = match size {
            0 => (8, moov.len() - pos),
            1 if pos + 16 <= moov.len() => (16, be64(&moov[pos + 8..]) as usize),
            1 => return None,
            n => (8, n),
        };
        if total < header || pos.checked_add(total)? > moov.len() {
            return None;
        }
        if kind == b"mvhd" {
            return mvhd_creation(&moov[pos + header..pos + total]);
        }
        pos += total;
    }
    None
}

/// Liest die `mvhd`-Erstellzeit (Sekunden seit 1904) aus beliebigem Lesestrom.
pub fn read_creation<R: Read + Seek>(r: &mut R) -> Option<u64> {
    let len = r.seek(SeekFrom::End(0)).ok()?;
    let mut pos = 0u64;
    for _ in 0..MAX_TOP_BOXES {
        if pos + 8 > len {
            return None;
        }
        r.seek(SeekFrom::Start(pos)).ok()?;
        let mut head = [0u8; 16];
        let n = r.read(&mut head).ok()?;
        if n < 8 {
            return None;
        }
        let size = u64::from(be32(&head));
        let kind = [head[4], head[5], head[6], head[7]];
        let (header, total) = match size {
            0 => (8, len - pos),
            1 if n >= 16 => (16, be64(&head[8..])),
            1 => return None,
            s => (8, s),
        };
        if total < header || pos.checked_add(total)? > len {
            return None;
        }
        if &kind == b"moov" {
            let body = total - header;
            if body > MAX_MOOV {
                return None;
            }
            r.seek(SeekFrom::Start(pos + header)).ok()?;
            let mut buf = vec![0u8; body as usize];
            r.read_exact(&mut buf).ok()?;
            return find_mvhd(&buf);
        }
        pos += total;
    }
    None
}

/// Lokale Aufnahmezeit; 0 (= 1904-01-01) bedeutet „unbekannt".
pub fn local_time(mac_seconds: u64) -> Option<NaiveDateTime> {
    if mac_seconds == 0 || mac_seconds < MAC_TO_UNIX {
        return None;
    }
    let unix = i64::try_from(mac_seconds - MAC_TO_UNIX).ok()?;
    DateTime::from_timestamp(unix, 0).map(|utc| utc.with_timezone(&Local).naive_local())
}

pub fn extract(path: &Path) -> Extracted {
    let mut file = match open_local(path, u64::MAX) {
        Ok(f) => f,
        Err(e) => return Extracted::new(status_for(&e)),
    };
    let mut out = Extracted::new(Status::Ok);
    if let Some(taken) = read_creation(&mut file).and_then(local_time) {
        out.meta.insert(
            "video.date".into(),
            taken.format("%Y-%m-%dT%H:%M:%S").to_string(),
        );
    }
    out
}

/// MP4 mit `ftyp`, `mdat` (Platzhalter) und `moov/mvhd` für Tests.
#[doc(hidden)]
pub mod testing {
    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut b = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend(kind);
        b.extend(payload);
        b
    }

    /// `mac_seconds` = Sekunden seit 1904 (Version 0 bei `v1 = false`, sonst Version 1).
    pub fn mp4(mac_seconds: u64, v1: bool, moov_at_end: bool) -> Vec<u8> {
        let mut mvhd = Vec::new();
        if v1 {
            mvhd.extend([1, 0, 0, 0]);
            mvhd.extend(mac_seconds.to_be_bytes());
            mvhd.extend([0u8; 8 + 4 + 8 + 80]);
        } else {
            mvhd.extend([0, 0, 0, 0]);
            mvhd.extend((mac_seconds as u32).to_be_bytes());
            mvhd.extend([0u8; 4 + 4 + 4 + 80]);
        }
        let moov = boxed(b"moov", &boxed(b"mvhd", &mvhd));
        let mut out = boxed(b"ftyp", b"isom\0\0\0\0isom");
        if !moov_at_end {
            out.extend(&moov);
        }
        out.extend(boxed(b"mdat", &[0xAB; 5000]));
        if moov_at_end {
            out.extend(&moov);
        }
        out
    }

    /// Sekunden seit 1904 zu einem UTC-Zeitpunkt.
    pub fn mac_seconds(y: i32, m: u32, d: u32, h: u32) -> u64 {
        let unix = chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        (unix + 2_082_844_800) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use std::io::Cursor;

    fn utc(y: i32, m: u32, d: u32, h: u32) -> chrono::DateTime<chrono::Utc> {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_utc()
    }

    #[test]
    fn version_0_und_1_liefern_dieselbe_zeit() {
        let secs = mac_seconds(2025, 7, 14, 12);
        for v1 in [false, true] {
            for end in [false, true] {
                let got = read_creation(&mut Cursor::new(mp4(secs, v1, end)));
                assert_eq!(got, Some(secs), "v1={v1} end={end}");
            }
        }
    }

    #[test]
    fn zeit_wird_in_lokale_zeit_umgerechnet() {
        let secs = mac_seconds(2025, 7, 14, 12);
        let expected = utc(2025, 7, 14, 12).with_timezone(&Local).naive_local();
        assert_eq!(local_time(secs), Some(expected));
    }

    #[test]
    fn null_bedeutet_kein_datum() {
        assert_eq!(local_time(0), None);
        let mut c = Cursor::new(mp4(0, false, false));
        assert_eq!(read_creation(&mut c).and_then(local_time), None);
    }

    #[test]
    fn abgeschnittene_und_kaputte_dateien_sind_none() {
        let full = mp4(mac_seconds(2025, 7, 14, 12), false, false);
        for cut in [0, 4, 7, 20, 40, full.len() - 3] {
            let _ = read_creation(&mut Cursor::new(full[..cut].to_vec()));
        }
        assert_eq!(
            read_creation(&mut Cursor::new(b"kein video".to_vec())),
            None
        );
        // moov mit unsinniger Größe
        let mut bad = boxed_moov_with_size(u32::MAX);
        bad.extend([0; 16]);
        assert_eq!(read_creation(&mut Cursor::new(bad)), None);
    }

    fn boxed_moov_with_size(size: u32) -> Vec<u8> {
        let mut b = size.to_be_bytes().to_vec();
        b.extend(b"moov");
        b
    }

    #[test]
    fn zufaellige_bytes_bringen_den_parser_nicht_zu_fall() {
        let mut data = mp4(mac_seconds(2025, 7, 14, 12), true, true);
        for i in (0..data.len()).step_by(7) {
            let original = data[i];
            data[i] = original.wrapping_add(0x55);
            let _ = read_creation(&mut Cursor::new(data.clone()));
            data[i] = original;
        }
    }

    #[test]
    fn datei_extraktion_liefert_video_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.mp4");
        std::fs::write(&path, mp4(mac_seconds(2025, 7, 14, 12), false, true)).unwrap();
        let out = extract(&path);
        let expected = utc(2025, 7, 14, 12)
            .with_timezone(&Local)
            .naive_local()
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();
        assert_eq!(out.meta["video.date"], expected);
        let none = dir.path().join("n.mp4");
        std::fs::write(&none, mp4(0, false, false)).unwrap();
        assert!(!extract(&none).meta.contains_key("video.date"));
    }
}
