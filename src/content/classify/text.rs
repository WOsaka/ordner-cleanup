//! Textaufbereitung und Schlüsselwort-Suche: Normalisierung (Kleinschreibung, Silbentrennung,
//! Leerraum, Obergrenze) und ein Aho-Corasick-Automat mit Wortgrenzen-Prüfung. Umlaute werden
//! doppelt indexiert: Jedes Schlüsselwort liegt in seiner Schreibweise und in der Form
//! `ä→ae, ö→oe, ü→ue, ß→ss` im Automaten, zählt aber nur einmal.

use std::collections::HashMap;

use aho_corasick::{AhoCorasick, MatchKind};

/// Bereinigter Text mit erhaltener Groß-/Kleinschreibung: Silbentrennung am Zeilenende
/// entfernt, Leerraum zu einem Leerzeichen, höchstens `max_chars` Zeichen.
pub fn clean(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len().min(max_chars * 2));
    let mut count = 0usize;
    let mut pending_space = false;
    let mut i = 0usize;
    while i < chars.len() && count < max_chars {
        let c = chars[i];
        if c == '\u{00AD}' {
            i += 1;
            continue;
        }
        // Trennstrich am Zeilenende: Buchstabe, '-', Zeilenumbruch, Kleinbuchstabe
        if c == '-' && i > 0 && chars[i - 1].is_alphabetic() {
            let mut j = i + 1;
            while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t' || chars[j] == '\r') {
                j += 1;
            }
            if j < chars.len() && chars[j] == '\n' {
                let mut k = j + 1;
                while k < chars.len() && chars[k].is_whitespace() {
                    k += 1;
                }
                if k < chars.len() && chars[k].is_lowercase() {
                    i = k;
                    continue;
                }
            }
        }
        if c.is_whitespace() || c.is_control() {
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
                count += 1;
            }
            pending_space = false;
            out.push(c);
            count += 1;
        }
        i += 1;
    }
    out
}

/// Kleinschreibung für die Suche.
pub fn fold(text: &str) -> String {
    text.to_lowercase()
}

/// `ä→ae, ö→oe, ü→ue, ß→ss` (nur Kleinbuchstaben, für Schlüsselwörter und Dateinamen).
pub fn expand_umlauts(s: &str) -> String {
    s.chars()
        .fold(String::with_capacity(s.len() + 4), |mut out, c| {
            match c {
                'ä' => out.push_str("ae"),
                'ö' => out.push_str("oe"),
                'ü' => out.push_str("ue"),
                'ß' => out.push_str("ss"),
                c => out.push(c),
            }
            out
        })
}

/// Dateiname als Suchtext: ohne Endung, Trenner (`_ - . ( )`) zu Leerzeichen.
pub fn name_text(file_name: &str) -> String {
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem);
    fold(
        &stem
            .chars()
            .map(|c| {
                if matches!(c, '_' | '-' | '.' | '(' | ')') {
                    ' '
                } else {
                    c
                }
            })
            .collect::<String>(),
    )
}

pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Worin ein Treffer gefunden wurde.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Keyword { category: usize, index: usize },
    Exclude { category: usize },
}

/// Automat über alle Schlüsselwörter und Ausschlusswörter aller Kategorien.
pub struct KeywordMatcher {
    automaton: AhoCorasick,
    /// Zu jedem Automaten-Muster die Slots, die es meldet.
    slots: Vec<Vec<Slot>>,
}

impl KeywordMatcher {
    /// `entries`: Slot und kleingeschriebenes Schlüsselwort.
    pub fn new(entries: &[(Slot, String)]) -> Self {
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut patterns: Vec<String> = Vec::new();
        let mut slots: Vec<Vec<Slot>> = Vec::new();
        for (slot, keyword) in entries {
            for form in [keyword.clone(), expand_umlauts(keyword)] {
                let id = *index.entry(form.clone()).or_insert_with(|| {
                    patterns.push(form);
                    slots.push(Vec::new());
                    patterns.len() - 1
                });
                if !slots[id].contains(slot) {
                    slots[id].push(*slot);
                }
            }
        }
        let automaton = AhoCorasick::builder()
            .match_kind(MatchKind::Standard)
            .build(&patterns)
            .expect("Automat aus gültigen Mustern");
        Self { automaton, slots }
    }

    /// Alle Slots, deren Schlüsselwort mit Wortgrenzen in `folded` vorkommt (jeder einmal).
    pub fn find(&self, folded: &str) -> Vec<Slot> {
        let mut seen: Vec<Slot> = Vec::new();
        for m in self.automaton.find_overlapping_iter(folded) {
            let (start, end) = (m.start(), m.end());
            let matched = &folded[start..end];
            let left_ok = match matched.chars().next() {
                Some(first) if first.is_alphanumeric() => !folded[..start]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric),
                _ => true,
            };
            let right_ok = match matched.chars().next_back() {
                Some(last) if last.is_alphanumeric() => !folded[end..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric),
                _ => true,
            };
            if left_ok && right_ok {
                for slot in &self.slots[m.pattern().as_usize()] {
                    if !seen.contains(slot) {
                        seen.push(*slot);
                    }
                }
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_entfernt_silbentrennung_und_normalisiert_leerraum() {
        assert_eq!(
            clean("Rech-\nnung  Nr.\t12\r\n\r\nEnde", 1000),
            "Rechnung Nr. 12 Ende"
        );
        assert_eq!(
            clean("Vor-\n  Name", 1000),
            "Vor- Name",
            "Großbuchstabe: kein Trennstrich"
        );
        assert_eq!(clean("E-Mail\nan uns", 1000), "E-Mail an uns");
        assert_eq!(clean("sch\u{00AD}ön", 1000), "schön");
        assert_eq!(clean("  \n ", 1000), "");
    }

    #[test]
    fn clean_begrenzt_zeichen() {
        let t = "äöü ".repeat(100);
        assert!(clean(&t, 10).chars().count() <= 10);
    }

    #[test]
    fn umlaute_werden_erweitert() {
        assert_eq!(expand_umlauts("überfällig größe"), "ueberfaellig groesse");
    }

    #[test]
    fn dateiname_als_suchtext() {
        assert_eq!(
            name_text("Rechnung_2026-09 (3).PDF"),
            "rechnung 2026 09  3 "
        );
        assert_eq!(name_text("ohne_endung"), "ohne endung");
    }

    fn matcher() -> KeywordMatcher {
        let kw = |c, i, s: &str| {
            (
                Slot::Keyword {
                    category: c,
                    index: i,
                },
                s.to_string(),
            )
        };
        KeywordMatcher::new(&[
            kw(0, 0, "rechnung"),
            kw(0, 1, "rechnungsnummer"),
            kw(1, 0, "überfällig"),
            kw(1, 1, "rechnung"),
            (Slot::Exclude { category: 0 }, "angebot".into()),
        ])
    }

    fn slots(text: &str) -> Vec<Slot> {
        let mut v = matcher().find(&fold(text));
        v.sort_by_key(|s| format!("{s:?}"));
        v
    }

    #[test]
    fn wortgrenzen_und_ganze_woerter() {
        let k0 = Slot::Keyword {
            category: 0,
            index: 0,
        };
        assert!(slots("Ihre Rechnung liegt bei").contains(&k0));
        assert!(!slots("Abrechnungszeitraum").contains(&k0));
        assert!(
            !slots("Rechnungsdatum").contains(&k0),
            "Kompositum ist ein anderes Wort"
        );
        assert!(slots("Rechnung.").contains(&k0));
        assert!(slots("(Rechnung)").contains(&k0));
        assert!(slots("RECHNUNG").contains(&k0));
    }

    #[test]
    fn keyword_in_mehreren_kategorien_und_umlaut_form() {
        let found = slots("rechnung ist ueberfaellig");
        assert!(found.contains(&Slot::Keyword {
            category: 0,
            index: 0
        }));
        assert!(found.contains(&Slot::Keyword {
            category: 1,
            index: 1
        }));
        assert!(
            found.contains(&Slot::Keyword {
                category: 1,
                index: 0
            }),
            "ae-Form"
        );
        assert!(slots("überfällig").contains(&Slot::Keyword {
            category: 1,
            index: 0
        }));
    }

    #[test]
    fn jedes_keyword_zaehlt_einmal() {
        let found = slots("rechnung rechnung rechnung");
        let n = found
            .iter()
            .filter(|s| {
                **s == Slot::Keyword {
                    category: 0,
                    index: 0,
                }
            })
            .count();
        assert_eq!(n, 1);
    }

    #[test]
    fn ueberlappende_treffer_werden_gefunden() {
        let found = slots("rechnungsnummer 5");
        assert!(found.contains(&Slot::Keyword {
            category: 0,
            index: 1
        }));
        assert!(!found.contains(&Slot::Keyword {
            category: 0,
            index: 0
        }));
    }

    #[test]
    fn ausschlusswort() {
        assert!(slots("ein angebot").contains(&Slot::Exclude { category: 0 }));
    }

    #[test]
    fn woerter_zaehlen() {
        assert_eq!(word_count("a  b\nc"), 3);
        assert_eq!(word_count(""), 0);
    }
}
