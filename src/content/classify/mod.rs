//! Klassifikation: rein und deterministisch. Aus Text, Dateiname und Metadaten berechnet sie
//! Kategorie, Konfidenz, zweitbeste Kategorie und die ausschlaggebenden Treffer.
//!
//! Konfidenz (`CLASSIFIER_VERSION = 1`): `best` und `second` sind die Punktwerte der zwei
//! stärksten Kategorien (Summe der Gewichte aller verschiedenen Treffer; Ausschlusswörter
//! setzen den Wert auf 0). `stärke = min(1, best / STRENGTH_FULL)`,
//! `abstand = 1 − (second / best)²`, `konfidenz = stärke · abstand`, auf 2 Stellen gerundet.
//! Bei `best = 0` gibt es keine Kategorie. Die zweitbeste Kategorie bekommt
//! `min(1, second / STRENGTH_FULL) · (second / best)`.

pub mod defs;
pub mod fields;
pub mod text;

use regex::Regex;

use super::Fields;
use defs::{CategoryDefs, Kind};
use text::{KeywordMatcher, Slot};

/// Punktwert, ab dem die Stärke 1 erreicht (Kalibrierung: Schritt 13 des Plans).
pub const STRENGTH_FULL: f64 = 8.0;
/// Mindestzahl Wörter, damit ein per OCR gelesenes Bild als Dokument gilt.
pub const DOC_MIN_WORDS: usize = 25;
/// Höchstens so viele Treffer werden zur Erklärung gespeichert.
pub const MAX_HITS: usize = 8;

/// Ergebnis der Zuordnung einer Datei.
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    pub category: Option<String>,
    pub confidence: f32,
    pub second: Option<(String, f32)>,
    /// Ausschlaggebende Treffer der besten Kategorie, stärkste zuerst.
    pub hits: Vec<String>,
}

impl Classification {
    pub fn none() -> Self {
        Self {
            category: None,
            confidence: 0.0,
            second: None,
            hits: Vec::new(),
        }
    }
}

pub struct ClassifyInput<'a> {
    /// Dateiname mit Endung
    pub name: &'a str,
    /// Endung, klein, ohne Punkt
    pub ext: &'a str,
    /// Bereinigter Text (siehe [`text::clean`]), falls vorhanden
    pub text: Option<&'a str>,
    /// Extrahierte Metadaten (`exif.camera`, `img.width` …)
    pub meta: &'a Fields,
}

fn round2(x: f64) -> f32 {
    ((x * 100.0).round() / 100.0) as f32
}

fn strength(score: f64) -> f64 {
    (score / STRENGTH_FULL).min(1.0)
}

/// Konfidenz aus bestem und zweitbestem Punktwert.
pub fn confidence(best: u32, second: u32) -> f32 {
    if best == 0 {
        return 0.0;
    }
    let ratio = f64::from(second) / f64::from(best);
    round2(strength(f64::from(best)) * (1.0 - ratio * ratio))
}

fn second_confidence(best: u32, second: u32) -> f32 {
    if best == 0 || second == 0 {
        return 0.0;
    }
    round2(strength(f64::from(second)) * (f64::from(second) / f64::from(best)))
}

/// Gängige Bildschirmauflösungen (Breite × Höhe, in beiden Ausrichtungen geprüft).
const SCREEN_SIZES: &[(u32, u32)] = &[
    (1280, 720),
    (1280, 800),
    (1366, 768),
    (1440, 900),
    (1536, 864),
    (1600, 900),
    (1680, 1050),
    (1920, 1080),
    (1920, 1200),
    (2048, 1152),
    (2560, 1080),
    (2560, 1440),
    (2560, 1600),
    (2880, 1800),
    (3440, 1440),
    (3840, 2160),
    (750, 1334),
    (828, 1792),
    (1080, 1920),
    (1080, 2340),
    (1080, 2400),
    (1125, 2436),
    (1170, 2532),
    (1179, 2556),
    (1284, 2778),
    (1290, 2796),
    (1440, 3200),
];

pub struct Classifier {
    defs: CategoryDefs,
    matcher: KeywordMatcher,
    screenshot_name: Regex,
}

struct Scored {
    index: usize,
    score: u32,
    hits: Vec<(u32, String)>,
}

impl Classifier {
    pub fn new(defs: CategoryDefs) -> Self {
        let mut entries = Vec::new();
        for (ci, c) in defs.categories.iter().enumerate() {
            if c.kind != Kind::Text {
                continue;
            }
            for (ki, (kw, _)) in c.keywords.iter().enumerate() {
                entries.push((
                    Slot::Keyword {
                        category: ci,
                        index: ki,
                    },
                    kw.clone(),
                ));
            }
            for ex in &c.exclude_keywords {
                entries.push((Slot::Exclude { category: ci }, ex.clone()));
            }
        }
        Self {
            matcher: KeywordMatcher::new(&entries),
            defs,
            screenshot_name: Regex::new(
                r"(?i)^(screenshot|bildschirmfoto|screen shot|bildschirmaufnahme)",
            )
            .unwrap(),
        }
    }

    pub fn defs(&self) -> &CategoryDefs {
        &self.defs
    }

    pub fn fingerprint(&self) -> &str {
        self.defs.fingerprint()
    }

    /// Punktwerte aller Textkategorien, die für die Endung gelten, absteigend.
    fn score(&self, input: &ClassifyInput<'_>) -> Vec<Scored> {
        let name_folded = text::name_text(input.name);
        let text_folded = input.text.map(text::fold);
        let hits_text: Vec<Slot> = text_folded
            .as_deref()
            .map(|t| self.matcher.find(t))
            .unwrap_or_default();
        let hits_name: Vec<Slot> = self.matcher.find(&name_folded);

        let mut scored = Vec::new();
        for (ci, c) in self.defs.categories.iter().enumerate() {
            if c.kind != Kind::Text || !c.applies_to(input.ext) {
                continue;
            }
            let excluded = hits_text
                .iter()
                .chain(&hits_name)
                .any(|s| *s == Slot::Exclude { category: ci });
            if excluded {
                continue;
            }
            let mut hits: Vec<(u32, String)> = Vec::new();
            for (ki, (kw, weight)) in c.keywords.iter().enumerate() {
                let slot = Slot::Keyword {
                    category: ci,
                    index: ki,
                };
                if hits_text.contains(&slot) {
                    hits.push((*weight, kw.clone()));
                } else if hits_name.contains(&slot) {
                    hits.push((*weight, format!("{kw} (Dateiname)")));
                }
            }
            for p in &c.patterns {
                let in_text = input.text.is_some_and(|t| p.regex.is_match(t));
                if in_text || p.regex.is_match(&name_folded) {
                    hits.push((p.weight, pattern_label(&p.source)));
                }
            }
            let score: u32 = hits.iter().map(|(w, _)| w).sum();
            if score > 0 {
                hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                scored.push(Scored {
                    index: ci,
                    score,
                    hits,
                });
            }
        }
        scored.sort_by(|a, b| {
            b.score.cmp(&a.score).then_with(|| {
                self.defs.categories[a.index]
                    .name
                    .cmp(&self.defs.categories[b.index].name)
            })
        });
        scored
    }

    /// Ergebnis aus Text und Dateiname; `None`, wenn keine Textkategorie trifft.
    fn classify_text(&self, input: &ClassifyInput<'_>) -> Option<Classification> {
        let scored = self.score(input);
        let best = scored.first()?;
        let second = scored.get(1);
        let second_score = second.map_or(0, |s| s.score);
        Some(Classification {
            category: Some(self.defs.categories[best.index].name.clone()),
            confidence: confidence(best.score, second_score),
            second: second.map(|s| {
                (
                    self.defs.categories[s.index].name.clone(),
                    second_confidence(best.score, s.score),
                )
            }),
            hits: best
                .hits
                .iter()
                .take(MAX_HITS)
                .map(|(_, h)| h.clone())
                .collect(),
        })
    }

    /// `foto` und `screenshot` aus Metadaten und Dateiname (nur wenn eingebaut und aktiv).
    fn classify_meta(&self, input: &ClassifyInput<'_>) -> Option<Classification> {
        let active = |name: &str| {
            self.defs
                .get(name)
                .is_some_and(|c| c.kind == Kind::Meta && c.applies_to(input.ext))
        };
        let result = |name: &str, conf: f32, hit: &str| Classification {
            category: Some(name.to_string()),
            confidence: conf,
            second: None,
            hits: vec![hit.to_string()],
        };
        if active("screenshot") && self.screenshot_name.is_match(input.name.trim()) {
            return Some(result("screenshot", 0.95, "Dateiname"));
        }
        let camera = input.meta.contains_key("exif.camera");
        if active("foto") && camera {
            return Some(result("foto", 0.95, "Kamera-EXIF"));
        }
        if active("screenshot") && input.ext == "png" && !camera {
            let dim = |k: &str| input.meta.get(k).and_then(|v| v.parse::<u32>().ok());
            if let (Some(w), Some(h)) = (dim("img.width"), dim("img.height")) {
                if SCREEN_SIZES.contains(&(w, h)) || SCREEN_SIZES.contains(&(h, w)) {
                    return Some(result("screenshot", 0.8, "Bildschirmauflösung"));
                }
            }
        }
        None
    }

    /// Ordnet eine Datei zu. Bei Bildern gewinnt eine Dokumentkategorie nur, wenn genug Text
    /// (≥ [`DOC_MIN_WORDS`] Wörter) gelesen wurde; sonst gelten `foto` bzw. `screenshot`.
    pub fn classify(&self, input: &ClassifyInput<'_>) -> Classification {
        let text_result = self.classify_text(input);
        let meta_result = self.classify_meta(input);
        match (text_result, meta_result) {
            (Some(t), Some(m)) => {
                let words = input.text.map_or(0, text::word_count);
                if words >= DOC_MIN_WORDS {
                    t
                } else {
                    m
                }
            }
            (Some(t), None) => t,
            (None, Some(m)) => m,
            (None, None) => Classification::none(),
        }
    }
}

/// Lesbare Kurzform eines Regex-Musters für die Treffer-Liste.
fn pattern_label(source: &str) -> String {
    let short: String = source.chars().take(32).collect();
    format!("Muster {short}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Einfacher Zufallsgenerator für Eigenschaftstests (ohne zusätzliche Abhängigkeit).
    fn lcg(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *seed >> 33
    }

    fn classifier() -> Classifier {
        Classifier::new(CategoryDefs::builtin())
    }

    fn run(c: &Classifier, name: &str, ext: &str, text: Option<&str>) -> Classification {
        let meta = Fields::new();
        c.classify(&ClassifyInput {
            name,
            ext,
            text,
            meta: &meta,
        })
    }

    #[test]
    fn konfidenzformel_werte() {
        assert_eq!(confidence(0, 0), 0.0);
        assert_eq!(confidence(8, 0), 1.0);
        assert_eq!(confidence(16, 0), 1.0);
        assert_eq!(confidence(4, 0), 0.5);
        assert_eq!(confidence(8, 8), 0.0, "Gleichstand");
        assert_eq!(confidence(10, 5), 0.75);
        assert_eq!(confidence(10, 2), 0.96);
    }

    #[test]
    fn konfidenz_ist_monoton_in_best() {
        let mut seed = 42;
        for _ in 0..2000 {
            let second = (lcg(&mut seed) % 20) as u32;
            let best = second + (lcg(&mut seed) % 20) as u32;
            let more = best + 1 + (lcg(&mut seed) % 5) as u32;
            assert!(
                confidence(more, second) >= confidence(best, second),
                "best {best} -> {more}, second {second}"
            );
            assert!((0.0..=1.0).contains(&confidence(more, second)));
        }
    }

    #[test]
    fn zusaetzlicher_treffer_senkt_den_punktwert_nie() {
        let c = classifier();
        let base = "rechnung rechnungsnummer 5";
        let more = "rechnung rechnungsnummer 5 zahlungsziel";
        let a = c.score(&ClassifyInput {
            name: "x.pdf",
            ext: "pdf",
            text: Some(base),
            meta: &Fields::new(),
        });
        let b = c.score(&ClassifyInput {
            name: "x.pdf",
            ext: "pdf",
            text: Some(more),
            meta: &Fields::new(),
        });
        let get = |v: &[Scored]| {
            v.iter()
                .find(|s| c.defs.categories[s.index].name == "rechnung")
                .unwrap()
                .score
        };
        assert!(get(&b) >= get(&a));
    }

    #[test]
    fn rechnung_wird_erkannt_mit_treffern() {
        let c = classifier();
        let text = "Rechnung Nr. 4711 Rechnungsdatum: 30.09.2026 Zahlbar bis 30.10.2026 Gesamtbetrag 119,00 EUR";
        let r = run(&c, "scan0012.pdf", "pdf", Some(text));
        assert_eq!(r.category.as_deref(), Some("rechnung"));
        assert!(r.confidence >= 0.8, "{r:?}");
        assert!(
            r.hits.contains(&"rechnungsdatum".to_string()),
            "{:?}",
            r.hits
        );
    }

    #[test]
    fn gleichstand_ergibt_niedrige_konfidenz() {
        let defs = CategoryDefs::load(
            "[[categories]]\nname = \"a\"\nkeywords = { alpha = 5 }\n[[categories]]\nname = \"b\"\nkeywords = { beta = 5 }\n",
            "c.toml",
        )
        .unwrap();
        let c = Classifier::new(defs);
        let r = run(&c, "x.pdf", "pdf", Some("alpha beta"));
        assert_eq!(
            r.category.as_deref(),
            Some("a"),
            "Name entscheidet bei Gleichstand"
        );
        assert_eq!(r.confidence, 0.0);
        assert_eq!(r.second.as_ref().map(|s| s.0.as_str()), Some("b"));
    }

    #[test]
    fn ausschlusswort_setzt_kategorie_auf_null() {
        let defs = CategoryDefs::load(
            "[[categories]]\nname = \"a\"\nkeywords = { alpha = 9 }\nexclude_keywords = [\"nein\"]\n",
            "c.toml",
        )
        .unwrap();
        let c = Classifier::new(defs);
        assert!(run(&c, "x.pdf", "pdf", Some("alpha")).category.is_some());
        assert_eq!(run(&c, "x.pdf", "pdf", Some("alpha nein")).category, None);
        assert_eq!(
            run(&c, "nein.pdf", "pdf", Some("alpha")).category,
            None,
            "auch im Dateinamen"
        );
    }

    #[test]
    fn endung_schraenkt_kategorien_ein() {
        let defs = CategoryDefs::load(
            "[[categories]]\nname = \"a\"\nkeywords = { alpha = 9 }\next = [\"pdf\"]\n",
            "c.toml",
        )
        .unwrap();
        let c = Classifier::new(defs);
        assert!(run(&c, "x.pdf", "pdf", Some("alpha")).category.is_some());
        assert_eq!(run(&c, "x.docx", "docx", Some("alpha")).category, None);
    }

    #[test]
    fn dateiname_zaehlt_mit_gleichem_gewicht() {
        let c = classifier();
        let r = run(&c, "Mietvertrag_Hauptstrasse.pdf", "pdf", None);
        assert_eq!(r.category.as_deref(), Some("vertrag"));
        assert!(r.hits[0].contains("Dateiname"), "{:?}", r.hits);
    }

    #[test]
    fn keyword_zaehlt_einmal_auch_in_name_und_text() {
        let c = classifier();
        let s = c.score(&ClassifyInput {
            name: "rechnung.pdf",
            ext: "pdf",
            text: Some("rechnung rechnung"),
            meta: &Fields::new(),
        });
        let rechnung = s
            .iter()
            .find(|s| c.defs.categories[s.index].name == "rechnung")
            .unwrap();
        assert_eq!(rechnung.score, 3);
    }

    #[test]
    fn umlaut_und_ae_schreibweise_gleichwertig() {
        let c = classifier();
        let a = run(
            &c,
            "x.pdf",
            "pdf",
            Some("Letzte Mahnung: Ihr Konto ist überfällig"),
        );
        let b = run(
            &c,
            "x.pdf",
            "pdf",
            Some("Letzte Mahnung: Ihr Konto ist ueberfaellig"),
        );
        assert_eq!(a, b);
        assert_eq!(a.category.as_deref(), Some("mahnung"));
    }

    #[test]
    fn foto_aus_kamera_exif() {
        let c = classifier();
        let mut meta = Fields::new();
        meta.insert("exif.camera".into(), "Apple iPhone 15".into());
        let r = c.classify(&ClassifyInput {
            name: "IMG_4711.HEIC",
            ext: "heic",
            text: None,
            meta: &meta,
        });
        assert_eq!(r.category.as_deref(), Some("foto"));
        assert_eq!(r.confidence, 0.95);
    }

    #[test]
    fn screenshot_aus_name_und_aufloesung() {
        let c = classifier();
        let meta = Fields::new();
        for name in [
            "Screenshot 2026-09-01 101010.png",
            "Bildschirmfoto 2026-09-01.png",
            "screen shot 1.png",
        ] {
            let r = c.classify(&ClassifyInput {
                name,
                ext: "png",
                text: None,
                meta: &meta,
            });
            assert_eq!(
                (r.category.as_deref(), r.confidence),
                (Some("screenshot"), 0.95),
                "{name}"
            );
        }
        let mut dims = Fields::new();
        dims.insert("img.width".into(), "1080".into());
        dims.insert("img.height".into(), "2400".into());
        let r = c.classify(&ClassifyInput {
            name: "a.png",
            ext: "png",
            text: None,
            meta: &dims,
        });
        assert_eq!(
            (r.category.as_deref(), r.confidence),
            (Some("screenshot"), 0.8)
        );
        dims.insert("exif.camera".into(), "X".into());
        let r = c.classify(&ClassifyInput {
            name: "a.png",
            ext: "png",
            text: None,
            meta: &dims,
        });
        assert_eq!(
            r.category.as_deref(),
            Some("foto"),
            "Kamera schlägt Auflösung"
        );
        let mut odd = Fields::new();
        odd.insert("img.width".into(), "1234".into());
        odd.insert("img.height".into(), "777".into());
        assert_eq!(
            c.classify(&ClassifyInput {
                name: "a.png",
                ext: "png",
                text: None,
                meta: &odd
            })
            .category,
            None
        );
    }

    #[test]
    fn dokument_foto_gewinnt_nur_mit_genug_text() {
        let c = classifier();
        let mut meta = Fields::new();
        meta.insert("exif.camera".into(), "Apple iPhone 15".into());
        let short = "Rechnung Rechnungsnummer 5 zahlbar bis";
        let r = c.classify(&ClassifyInput {
            name: "IMG_1.jpg",
            ext: "jpg",
            text: Some(short),
            meta: &meta,
        });
        assert_eq!(r.category.as_deref(), Some("foto"), "zu wenig Wörter");
        let long = format!("{short} {}", "wort ".repeat(30));
        let r = c.classify(&ClassifyInput {
            name: "IMG_1.jpg",
            ext: "jpg",
            text: Some(&long),
            meta: &meta,
        });
        assert_eq!(r.category.as_deref(), Some("rechnung"));
    }

    #[test]
    fn meta_kategorien_lassen_sich_abschalten() {
        let defs =
            CategoryDefs::load("disable_builtin = [\"foto\", \"screenshot\"]\n", "c.toml").unwrap();
        let c = Classifier::new(defs);
        let mut meta = Fields::new();
        meta.insert("exif.camera".into(), "X".into());
        assert_eq!(
            c.classify(&ClassifyInput {
                name: "a.jpg",
                ext: "jpg",
                text: None,
                meta: &meta
            })
            .category,
            None
        );
        assert_eq!(run(&c, "Screenshot 1.png", "png", None).category, None);
    }

    #[test]
    fn nichts_erkannt() {
        let c = classifier();
        let r = run(&c, "x.pdf", "pdf", Some("Lorem ipsum dolor sit amet"));
        assert_eq!(r, Classification::none());
        assert_eq!(run(&c, "x.mp4", "mp4", None), Classification::none());
    }

    #[test]
    fn deterministisch() {
        let c = classifier();
        let t = "Rechnung Mahnung Zahlungserinnerung Rechnungsnummer 4 überfällig";
        let a = run(&c, "a.pdf", "pdf", Some(t));
        for _ in 0..5 {
            assert_eq!(run(&classifier(), "a.pdf", "pdf", Some(t)), a);
        }
    }

    #[test]
    fn zweite_kategorie_mit_konfidenz() {
        let c = classifier();
        let r = run(
            &c,
            "x.pdf",
            "pdf",
            Some("Mahnung Zahlungserinnerung Rechnung Rechnungsnummer 3"),
        );
        let (name, conf) = r.second.unwrap();
        assert!(!name.is_empty());
        assert!((0.0..=1.0).contains(&conf));
    }
}
