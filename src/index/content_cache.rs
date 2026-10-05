//! Inhalts-Cache (Schema v3, Phase 6a): Kategorie, Konfidenz und Felder je Datei sowie der
//! DPAPI-verschlüsselte OCR-Text. Beide Tabellen hängen nicht an `files.generation` und
//! überdauern deshalb Re-Scans. Die Gültigkeitsprüfung (Größe, mtime, Versionen) liegt beim
//! Aufrufer; hier werden nur Zeilen gelesen und geschrieben. Es wird nie Volltext gespeichert,
//! OCR-Text nur als Blob, den der Aufrufer schon verschlüsselt hat.

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};

use super::{Index, Result};
use crate::paths;

/// Eine Zeile aus `content_cache`. JSON-Spalten bleiben Strings; die Typen dazu liegen in
/// `crate::content`.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedContent {
    pub path_key: String,
    pub size: i64,
    pub mtime: i64,
    pub extractor_version: i64,
    pub defs_fingerprint: String,
    pub status: String,
    pub category: Option<String>,
    pub confidence: Option<f64>,
    pub category2: Option<String>,
    pub confidence2: Option<f64>,
    pub source: Option<String>,
    pub hits: Option<String>,
    pub fields: String,
    pub field_sources: String,
    pub text_source: Option<String>,
    pub llm_model: Option<String>,
    pub classified_at: String,
}

/// Eine Zeile aus `ocr_text`; `data` ist schon verschlüsselt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrTextEntry {
    pub path_key: String,
    pub size: i64,
    pub mtime: i64,
    pub ocr_version: i64,
    pub languages: String,
    pub pages: i64,
    pub data: Vec<u8>,
}

/// Ein verschlüsselter OCR-Text samt Seitenzahl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrTextHit {
    pub pages: i64,
    pub data: Vec<u8>,
}

impl Index {
    /// Alle Cache-Zeilen unterhalb der Wurzel, nach `path_key`.
    pub fn content_lookup(&self, root_key: &str) -> Result<HashMap<String, CachedContent>> {
        let (lo, hi) = paths::prefix_range(root_key);
        let mut stmt = self.conn().prepare(
            "SELECT path_key, size, mtime, extractor_version, defs_fingerprint, status, category,
                    confidence, category2, confidence2, source, hits, fields, field_sources,
                    text_source, llm_model, classified_at
             FROM content_cache WHERE path_key >= ?1 AND path_key < ?2",
        )?;
        let rows = stmt.query_map(params![lo, hi], |r| {
            Ok(CachedContent {
                path_key: r.get(0)?,
                size: r.get(1)?,
                mtime: r.get(2)?,
                extractor_version: r.get(3)?,
                defs_fingerprint: r.get(4)?,
                status: r.get(5)?,
                category: r.get(6)?,
                confidence: r.get(7)?,
                category2: r.get(8)?,
                confidence2: r.get(9)?,
                source: r.get(10)?,
                hits: r.get(11)?,
                fields: r.get(12)?,
                field_sources: r.get(13)?,
                text_source: r.get(14)?,
                llm_model: r.get(15)?,
                classified_at: r.get(16)?,
            })
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let row = row?;
            map.insert(row.path_key.clone(), row);
        }
        Ok(map)
    }

    /// Schreibt alle Zeilen in **einer** Transaktion (ein Block beim Abbruch-Verlust).
    pub fn content_store(&mut self, records: &[CachedContent]) -> Result<()> {
        let tx = self.conn_mut().transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO content_cache(path_key, size, mtime, extractor_version,
                    defs_fingerprint, status, category, confidence, category2, confidence2,
                    source, hits, fields, field_sources, text_source, llm_model, classified_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
                 ON CONFLICT(path_key) DO UPDATE SET
                   size=excluded.size, mtime=excluded.mtime,
                   extractor_version=excluded.extractor_version,
                   defs_fingerprint=excluded.defs_fingerprint, status=excluded.status,
                   category=excluded.category, confidence=excluded.confidence,
                   category2=excluded.category2, confidence2=excluded.confidence2,
                   source=excluded.source, hits=excluded.hits, fields=excluded.fields,
                   field_sources=excluded.field_sources, text_source=excluded.text_source,
                   llm_model=excluded.llm_model, classified_at=excluded.classified_at",
            )?;
            for c in records {
                stmt.execute(params![
                    c.path_key,
                    c.size,
                    c.mtime,
                    c.extractor_version,
                    c.defs_fingerprint,
                    c.status,
                    c.category,
                    c.confidence,
                    c.category2,
                    c.confidence2,
                    c.source,
                    c.hits,
                    c.fields,
                    c.field_sources,
                    c.text_source,
                    c.llm_model,
                    c.classified_at
                ])?;
            }
        }
        Ok(tx.commit()?)
    }

    /// Entfernt Zeilen (Inhalt und OCR) unter der Wurzel, zu denen es keine Datei im Index
    /// mehr gibt.
    pub fn content_prune(&mut self, root_key: &str) -> Result<usize> {
        let (lo, hi) = paths::prefix_range(root_key);
        let mut removed = 0;
        for table in ["content_cache", "ocr_text"] {
            removed += self.conn().execute(
                &format!(
                    "DELETE FROM {table} WHERE path_key >= ?1 AND path_key < ?2
                     AND NOT EXISTS (SELECT 1 FROM files WHERE files.path_key = {table}.path_key)"
                ),
                params![lo, hi],
            )?;
        }
        Ok(removed)
    }

    /// Löscht Inhalts- und OCR-Text-Cache der Wurzel (`classify --clear`). Liefert die Zahl
    /// der gelöschten Inhaltszeilen.
    pub fn content_clear(&mut self, root_key: &str) -> Result<usize> {
        let (lo, hi) = paths::prefix_range(root_key);
        let tx = self.conn_mut().transaction()?;
        tx.execute(
            "DELETE FROM ocr_text WHERE path_key >= ?1 AND path_key < ?2",
            params![lo, hi],
        )?;
        let n = tx.execute(
            "DELETE FROM content_cache WHERE path_key >= ?1 AND path_key < ?2",
            params![lo, hi],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// Löscht nur den OCR-Text-Cache der Wurzel (`cache_ocr_text = false`).
    pub fn ocr_text_clear(&mut self, root_key: &str) -> Result<usize> {
        let (lo, hi) = paths::prefix_range(root_key);
        Ok(self.conn().execute(
            "DELETE FROM ocr_text WHERE path_key >= ?1 AND path_key < ?2",
            params![lo, hi],
        )?)
    }

    /// Verschlüsselten OCR-Text holen, falls Größe, mtime, OCR-Version und Sprachen passen.
    pub fn ocr_text_get(
        &self,
        key: &str,
        size: i64,
        mtime: i64,
        ocr_version: i64,
        languages: &str,
    ) -> Result<Option<OcrTextHit>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT pages, data FROM ocr_text
                 WHERE path_key = ?1 AND size = ?2 AND mtime = ?3
                   AND ocr_version = ?4 AND languages = ?5",
                params![key, size, mtime, ocr_version, languages],
                |r| {
                    Ok(OcrTextHit {
                        pages: r.get(0)?,
                        data: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn ocr_text_put(&mut self, e: &OcrTextEntry) -> Result<()> {
        self.conn().execute(
            "INSERT INTO ocr_text(path_key, size, mtime, ocr_version, languages, pages, data)
             VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(path_key) DO UPDATE SET
               size=excluded.size, mtime=excluded.mtime, ocr_version=excluded.ocr_version,
               languages=excluded.languages, pages=excluded.pages, data=excluded.data",
            params![
                e.path_key,
                e.size,
                e.mtime,
                e.ocr_version,
                e.languages,
                e.pages,
                e.data
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::FileRecord;
    use std::path::Path;

    fn row(key: &str, size: i64) -> CachedContent {
        CachedContent {
            path_key: key.into(),
            size,
            mtime: 5,
            extractor_version: 1,
            defs_fingerprint: "fp".into(),
            status: "ok".into(),
            category: Some("rechnung".into()),
            confidence: Some(0.9),
            category2: None,
            confidence2: None,
            source: Some("rules".into()),
            hits: Some("[\"rechnung\"]".into()),
            fields: "{}".into(),
            field_sources: "{}".into(),
            text_source: Some("layer".into()),
            llm_model: None,
            classified_at: "t".into(),
        }
    }

    fn file(index: &mut Index, path: &str) {
        let p = Path::new(path);
        index
            .upsert_files(
                &[FileRecord {
                    dir_key: paths::dir_key(p.parent().unwrap()),
                    path: path.into(),
                    path_key: paths::path_key(p),
                    name: "x".into(),
                    ..FileRecord::default()
                }],
                1,
            )
            .unwrap();
    }

    #[test]
    fn speichern_lesen_und_ueberschreiben() {
        let mut index = Index::open_in_memory().unwrap();
        index
            .content_store(&[row(r"C:\r\a.pdf", 1), row(r"C:\andere\b.pdf", 2)])
            .unwrap();
        let got = index.content_lookup(r"C:\r\").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[r"C:\r\a.pdf"], row(r"C:\r\a.pdf", 1));
        index.content_store(&[row(r"C:\r\a.pdf", 9)]).unwrap();
        assert_eq!(
            index.content_lookup(r"C:\r\").unwrap()[r"C:\r\a.pdf"].size,
            9
        );
    }

    #[test]
    fn prune_entfernt_nur_verwaiste_zeilen_der_wurzel() {
        let mut index = Index::open_in_memory().unwrap();
        file(&mut index, r"C:\R\da.pdf");
        let da = paths::path_key(Path::new(r"C:\R\da.pdf"));
        index
            .content_store(&[row(&da, 1), row(r"C:\r\weg.pdf", 1), row(r"C:\x\f.pdf", 1)])
            .unwrap();
        assert_eq!(index.content_prune(r"C:\r\").unwrap(), 1);
        let left = index.content_lookup(r"C:\").unwrap();
        assert!(left.contains_key(&da) && left.contains_key(r"C:\x\f.pdf"));
        assert_eq!(left.len(), 2);
    }

    #[test]
    fn clear_loescht_inhalt_und_ocr_der_wurzel() {
        let mut index = Index::open_in_memory().unwrap();
        index
            .content_store(&[row(r"C:\r\a", 1), row(r"C:\x\a", 1)])
            .unwrap();
        for k in [r"C:\r\a", r"C:\x\a"] {
            index
                .ocr_text_put(&OcrTextEntry {
                    path_key: k.into(),
                    size: 1,
                    mtime: 5,
                    ocr_version: 1,
                    languages: "de".into(),
                    pages: 1,
                    data: vec![1, 2],
                })
                .unwrap();
        }
        assert_eq!(index.content_clear(r"C:\r\").unwrap(), 1);
        assert!(index
            .ocr_text_get(r"C:\r\a", 1, 5, 1, "de")
            .unwrap()
            .is_none());
        assert!(index
            .ocr_text_get(r"C:\x\a", 1, 5, 1, "de")
            .unwrap()
            .is_some());
    }

    #[test]
    fn ocr_text_gilt_nur_bei_gleichen_schluesseln() {
        let mut index = Index::open_in_memory().unwrap();
        index
            .ocr_text_put(&OcrTextEntry {
                path_key: "k".into(),
                size: 10,
                mtime: 20,
                ocr_version: 1,
                languages: "de,en".into(),
                pages: 3,
                data: vec![7; 4],
            })
            .unwrap();
        let hit = index
            .ocr_text_get("k", 10, 20, 1, "de,en")
            .unwrap()
            .unwrap();
        assert_eq!((hit.pages, hit.data), (3, vec![7; 4]));
        for (s, m, v, l) in [
            (11, 20, 1, "de,en"),
            (10, 21, 1, "de,en"),
            (10, 20, 2, "de,en"),
            (10, 20, 1, "de"),
        ] {
            assert!(index.ocr_text_get("k", s, m, v, l).unwrap().is_none());
        }
        assert_eq!(index.ocr_text_clear(r"z:\").unwrap(), 0);
        assert_eq!(index.ocr_text_clear("k").unwrap(), 1);
    }
}
