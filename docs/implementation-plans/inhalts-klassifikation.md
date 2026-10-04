---
title: "Implementation Plan: Inhaltsbasierte Klassifikation (Phase 6a)"
feature_spec: docs/features/inhalts-klassifikation.md
status: implemented   # pending-approval | approved | implemented
created: 2026-10-04
updated: 2026-10-04
---

# Implementation Plan: Inhaltsbasierte Klassifikation (Phase 6a)

## Summary

Phase 6a besteht aus fünf Schichten, die aufeinander aufbauen:

1. **Extraktion** (`src/content/extract/`): Je Dateityp ein Extraktor, der Text und Metadaten liest. Abgedeckt sind PDF, Office, Bild (EXIF inkl. Kamera und GPS, PNG-Maße), Video (`mvhd`) und OCR. Fremde Dateien gelten als nicht vertrauenswürdig: Es gibt Obergrenzen, Panics werden je Datei abgefangen, und Cloud-only-Dateien werden nie geöffnet.
2. **Klassifikation** (`src/content/classify/`): rein und deterministisch. Aus normalisiertem Text, Dateiname und Metadaten berechnet sie Kategorie, Konfidenz, Treffer und Felder. Grundlage sind die eingebauten Kategorien und die Datei `categories.toml`. Optional fragt sie danach das lokale LLM.
3. **Cache und Befehl `classify`** (`src/index/content_cache.rs`, `src/app/classify.rs`): Schema v3 bringt den Inhalts-Cache, den DPAPI-verschlüsselten OCR-Text-Cache und `files.first_seen`. Der Lauf ist inkrementell, parallel, abbrechbar und läuft mit niedriger Priorität.
4. **Regel-Engine** (`src/rules/`, `src/change/rules.rs`): neue Bedingungen, Platzhalter mit Ersatzwert und neue Skip-Gründe. `plan rules` klassifiziert fehlende Einträge selbst.
5. **Ausgabe und Betrieb**: Abschnitt „Inhalte“ im Bericht, `classify` im Profil und in `run`, „zum Prüfen“ in der Benachrichtigung.

Ein Spike-Schritt 0 klärt zuerst die drei größten Unbekannten: die PDF-Bibliothek, die WinRT-OCR aus Rust und die strukturierte Ausgabe von Ollama.

## Entscheidungen aus Spec und Codeanalyse

| Thema | Entscheidung |
|---|---|
| Modulname | `src/scan/classify.rs` gibt es schon (Scan-Klassifikation für Excludes und Summary). Das neue Modul heißt deshalb `src/content/`, der Befehl wie in der Spec `classify`. Im Code heißen die Inhaltskategorien `ContentCategory`, damit sie nicht mit den `junk_categories` aus Phase 3 verwechselt werden. |
| Cache-Ort | In der vorhandenen `index.db` (Schema v3), wie `exif_cache` aus Phase 4: eigene Tabellen ohne `generation`, damit Re-Scans sie nicht löschen; Bereinigung über `content_prune(root)` wie `exif_prune`. `--reset-index` löscht die Caches mit. |
| OCR-Text-Cache | Entscheidung nach Review (siehe Spec): Gespeichert wird nur OCR-Text, verschlüsselt mit `CryptProtectData` (DPAPI, Scope aktueller Benutzer, ohne UI) in der Tabelle `ocr_text`. Er ist gültig bei gleicher Größe, mtime, `OCR_VERSION` und Sprachliste. Text aus Textlayer und Office wird nie gespeichert, weil er schnell neu gelesen ist. Mit `cache_ocr_text = false` wird weder geschrieben noch gelesen, und vorhandene Einträge werden beim nächsten `classify` gelöscht. |
| Ankunftszeit | `files.ctime` (NTFS-Erstellzeit) gibt es seit Phase 1. Neu ist `files.first_seen`, das der Scan nur für Schlüssel setzt, die im Vorzustand (`load_previous`) fehlen, **und** nur wenn die Wurzel schon einmal vollständig gescannt war. Beim Upsert wird die Spalte nicht überschrieben. Daraus folgt `arrived = max(ctime, first_seen)`. |
| Klassifikationsablauf | Je Datei: (1) Metadaten-Extraktor nach Endung, (2) Text (Textlayer bzw. Office; OCR nur ohne brauchbaren Textlayer bzw. bei Bildern nach der OCR-Regel unten), (3) Normalisierung, (4) Kategorie-Bewertung, (5) Feld-Muster, (6) optional LLM für das, was danach fehlt. Ergebnis ist ein `ContentRecord`. |
| OCR bei Bildern | Fotos per OCR zu lesen ist teuer: 10.000 Fotos ergeben Stunden. Deshalb wird ein Bild nur per OCR gelesen, wenn es **kein** Kamera-EXIF (`Make`/`Model`) hat **oder** auf `[classify] ocr_image_globs` passt (Default `["**/Scans/**", "**/*scan*", "**/*dokument*"]`). Danach gilt: Liefert die OCR mindestens `DOC_MIN_WORDS` = 25 Wörter und eine Dokumentkategorie, gewinnt diese. Sonst greifen `foto` bzw. `screenshot`. |
| Brauchbarer Textlayer | Mindestens 20 Wörter pro gelesener Seite im Schnitt bzw. 50 Wörter insgesamt; sonst gilt das PDF als gescannt (OCR, falls aktiv). |
| Normalisierung | Kleinschreibung, Umlaute **doppelt** indexiert (`ä` bleibt `ä` und wird zusätzlich als `ae` gefunden: Schlüsselwörter werden in beiden Formen in den Automaten gelegt), Silbentrennung am Zeilenende entfernt, Leerraum zu einem Leerzeichen, Obergrenze `max_text_chars`. |
| Schlüsselwort-Suche | `aho-corasick` (steckt schon über `regex` im Baum, wird direkt eingebunden) über alle Schlüsselwörter aller Kategorien mit Wortgrenzen-Prüfung. Jedes Schlüsselwort zählt **einmal** je Datei. Muster (`patterns`) sind Regex und zählen ebenfalls einmal. Treffer im Dateinamen zählen mit demselben Gewicht. |
| Konfidenz (`CLASSIFIER_VERSION = 1`) | `best` und `second` sind die Punktwerte der zwei stärksten Kategorien. `stärke = min(1, best / 8)`, `abstand = 1 − (second / best)²`, `konfidenz = stärke · abstand`, auf 2 Stellen gerundet. Bei `best = 0` gibt es keine Kategorie. Ausschlusswörter setzen den Punktwert einer Kategorie auf 0. Die Konstanten (8, Quadrat) werden in Schritt 13 am Testkorpus kalibriert und stehen im README. Monoton in `best`, deterministisch. |
| Metadaten-Kategorien | `foto`: Kamera-EXIF vorhanden, Konfidenz 0,95. `screenshot`: Name passt auf `^(screenshot|bildschirmfoto|screen shot|bildschirmaufnahme)` (0,95) oder PNG ohne EXIF mit gängiger Bildschirmauflösung (0,8). Beide sind als eingebaute Kategorien mit `kind = "meta"` beschrieben und lassen sich per `disable_builtin` abschalten. |
| Felder | Muster in `src/content/classify/fields.rs`. **Datum:** beschriftet („Rechnungsdatum“, „Datum“, „Date“, „vom“) vor unbeschriftet. Formate `dd.mm.yyyy`, `d. Monat yyyy` (DE/EN) und `yyyy-mm-dd`. Plausibel nur zwischen 1990 und heute + 1 Jahr. **Betrag:** beschriftet („Gesamtbetrag“, „Endbetrag“, „Rechnungsbetrag“, „Total“, „Summe“), Dezimalkomma oder -punkt, wird zu `123,45` normalisiert. **Nummer:** „Rechnungs-Nr.“, „Rechnungsnummer“, „Invoice No“, „Vertragsnummer“, „Kundennummer“. **Absender:** nur `[[senders]]`, sonst LLM. **Titel:** LLM vor Metadaten-Titel. Metadaten-Titel, die auf die Müll-Liste passen („Microsoft Word - …“, „Untitled“, „Dokument1“, gleich Dateiname), werden verworfen. |
| Platzhalter-Ersatz | Syntax `{feld|Ersatz}`. Der Ersatz ist eine Token-Folge ohne `/` und darf Platzhalter enthalten, aber keinen weiteren Ersatz. Das bleibt eindeutig, weil `|` in Windows-Namen verboten ist. Fehlt ein Feld ohne Ersatz, entsteht `TargetError::MissingField(name)`. Bei `{exif.date}` gilt weiter `exif_fallback` aus Phase 4, ein `|` dort ist ein Fehler. |
| Feldwerte bereinigen | `<>:"/\|?*` und Steuerzeichen werden zu Leerzeichen, Leerraum wird zusammengefasst, Punkte und Leerzeichen am Ende werden entfernt. Länge: `doc.title` 60, sonst 40 Zeichen, am Wortende gekürzt. Danach folgt die Normalisierung aus Phase 4 (nur für den Dateinamen). |
| Regel-Auswertung | Erste passende Regel gewinnt (unverändert). Drei Durchgänge in `plan_rules`: (1) billige Phase-4-Bedingungen, damit feststeht, für welche Dateien eine Inhaltsbedingung überhaupt geprüft werden muss; (2) für diese Dateien Inhalts-Cache laden bzw. per `ContentSource` klassifizieren; (3) nur wenn eine Kandidaten-Regel `text_regex` hat: Text je Datei **einmal** lesen, alle `text_regex` der Kandidaten-Regeln auswerten, nur die Ergebnisse (Bitmenge je Datei) behalten, Text verwerfen. Danach läuft die bekannte Zuordnung mit vollständigen Daten. |
| Skip-Gründe | Neu in `SkipReason`: `LowConfidence`, `NotClassified`, `MissingField`, `TooRecentArrival` (für `min_dwell`, damit es sich von `TooRecent` aus Phase 3 unterscheidet). `Skipped` bekommt `detail: Option<String>` (`serde(default, skip_serializing_if)`), z. B. `doc.sender` oder `rechnung 0.62 / mahnung 0.41`. Ältere Pläne bleiben lesbar. Pläne mit `detail` kann ein älteres Programm wegen `deny_unknown_fields` nicht lesen; das ist akzeptiert, weil Pläne kurzlebig sind. |
| Low-Confidence-Zuordnung | Passt keine Regel und ist mindestens eine Regel **nur** an `min_confidence` gescheitert (alle anderen Bedingungen erfüllt, Kategorie passt, Konfidenz zu klein), wird die Datei als `skipped: low-confidence` geführt statt als „ohne Regel“. Analog gilt `not-classified` (Cache fehlt und `--no-classify` oder Datei nicht lesbar) und `too-recent-arrival`. |
| Kategorienamen prüfen | `RuleSet::parse` bekommt die bekannten Kategorienamen (`&KnownCategories`). `plan rules` lädt deshalb zuerst die Kategorie-Definitionen; Fehler darin brechen vor den Regeln ab. Regeln ohne Inhaltsbedingung brauchen keine gültige `categories.toml` (fehlende Datei: nur eingebaute Kategorien). |
| LLM-Anbindung | Ollama `POST /api/chat`, `stream: false`, `format` = JSON-Schema (structured outputs), `options: {temperature: 0, seed: 1}`. **Eine** Anfrage je Datei, die alle benötigten Aufgaben bündelt. Antwortschema: `{category, confidence, date, sender, amount, number, title}`, jeweils nullable. HTTP über `ureq` 3, synchron, ohne Proxy, ohne Weiterleitungen. Der Endpoint wird beim Laden der Config geprüft: Schema `http`, Host `127.0.0.1`, `::1`/`[::1]` oder `localhost`. Erreichbarkeit prüft einmal je Lauf `GET /api/tags`, dabei auch, ob das Modell installiert ist. Ist es nicht erreichbar, gibt es eine Warnung, und das LLM bleibt für den Rest des Laufs aus. |
| Wann das LLM gefragt wird | Nur für Dateien mit Text und nur mit eingeschaltetem `[llm]` und ohne `--no-llm`. Aufgabe `category` bei Konfidenz unter `min_confidence`, Aufgabe `fields` bei fehlendem `doc.date` oder `doc.sender`, Aufgabe `title` immer. Das Ergebnis wird mit Modellname gecacht und nur bei anderem Modell, `--force` oder geändertem Text neu angefragt. Eine Änderung der Kategorie-Definitionen macht LLM-Kategorien ungültig, LLM-Felder bleiben. |
| LLM-Ergebnis prüfen | Die Kategorie muss in der bekannten Liste stehen, sonst `unbekannt`. Konfidenz = `min(angabe, max_confidence)`. Datum plausibel wie bei den Mustern, Betrag als Zahl, Texte bereinigt wie Feldwerte. Was nicht passt, wird verworfen; das regelbasierte Ergebnis bleibt. |
| OCR | WinRT über die vorhandene `windows`-Crate: `Windows.Media.Ocr.OcrEngine::TryCreateFromLanguage` je Sprache aus `ocr_languages` (die erste verfügbare erkennt, bei sehr wenig Text wird die nächste versucht). Bilder laufen über `Windows.Graphics.Imaging.BitmapDecoder`, PDFs über `Windows.Data.Pdf.PdfDocument` → `RenderToStreamAsync` mit 2-facher Skalierung, begrenzt auf `OcrEngine::MaxImageDimension`. Async-Aufrufe blockierend mit `.get()`. Jeder OCR-Thread initialisiert die WinRT-Umgebung als MTA. Hinter einem Trait `Ocr`, damit Tests ohne Sprachpaket laufen. |
| Parallelität und Priorität | rayon-Pool mit `[classify] threads` (0 = halbe Kernanzahl, mindestens 1). OCR und LLM sind teurer und laufen in einem eigenen kleinen Pool (OCR: 2 Threads, LLM: 1 Anfrage gleichzeitig). Während `classify` wird die Prozesspriorität auf `BELOW_NORMAL_PRIORITY_CLASS` gesetzt (`SetPriorityClass`) und danach zurückgesetzt. |
| Abbruch | Gleiches Muster wie beim Scan (`ctrlc`-Flag). Ergebnisse werden in Blöcken zu 200 Einträgen in einer Transaktion geschrieben, ein Abbruch verliert also höchstens den laufenden Block. Exit-Code wie beim abgebrochenen Scan. |
| Sperre | `classify` nimmt die globale `ScanLock` aus Phase 5. `plan rules` mit automatischer Klassifikation nimmt sie auch, aber nur, wenn wirklich klassifiziert werden muss. |
| Geodaten | `assets/geo/cities15000.tsv`: aus GeoNames `cities15000.txt` abgeleitet, Spalten `name`, `country`, `lat`, `lon`, etwa 33.000 Zeilen, etwa 900 KB. Eingebunden mit `include_str!` und beim ersten Zugriff in ein `Vec` geparst (`LazyLock`). Die Suche ist linear mit Vorfilter über ein Breitenband (±0,5°). Das ist schnell genug, weil sie je Foto einmal läuft und gecacht wird. Höchstens 50 km Entfernung (Haversine), sonst nur `country`, wenn eine Stadt desselben Landes innerhalb von 300 km liegt, sonst kein Feld. Das Erzeugungsskript liegt unter `tools/geonames.ps1`, die Lizenz in `assets/geo/LICENSE`. |
| Video | Eigener, begrenzter ISO-BMFF-Parser (`moov` → `mvhd`, Version 0/1). Er liest höchstens 64 MB vom Anfang und, falls `moov` am Ende liegt, 64 MB vom Ende. Zeitbasis 1904-01-01 UTC, Wert 0 bedeutet kein Datum. Ausgabe in lokaler Zeit. |
| Office | Crate `zip` (nur `deflate`) + vorhandenes `quick-xml`. docx: `word/document.xml` (`w:t`, Absätze als Zeilen). xlsx: `xl/sharedStrings.xml` + Inline-Strings. pptx: `ppt/slides/slide*.xml` (`a:t`). Metadaten aus `docProps/core.xml`. Grenzen: höchstens 2.000 Einträge, 50 MB entpackt je Eintrag, 200 MB insgesamt, Verhältnis entpackt/gepackt ≤ 100. Verschlüsselte Office-Dateien sind OLE-Container statt ZIP: erkennt man an der OLE-Signatur `D0 CF 11 E0`, Status `unreadable: encrypted`. |
| PDF | Auswahl in Spike 0a zwischen `pdf-extract` (Text) + `lopdf` (Metadaten, Verschlüsselung, Seitenzahl) und `lopdf` allein mit eigener Textextraktion. Pflicht sind Verhalten bei Fehlern ohne Abort (Panics werden gefangen, Endlosschleifen durch einen Zeitwächter je Datei abgefangen, siehe Risiken), Seitenbegrenzung und reines Rust. |

## Files to Create

| File | Purpose |
|---|---|
| `src/content/mod.rs` | Typen: `ContentRecord`, `Status`, `Source` (`Rules`/`Llm`), `Fields` (BTreeMap Feldname → Wert), `Hit`; Konstanten `EXTRACTOR_VERSION`, `CLASSIFIER_VERSION`, `OCR_VERSION`; Feldnamen-Liste (eine Quelle für Regeln und Bericht) |
| `src/content/extract/mod.rs` | `Extracted { text: Option<String>, text_source, meta: Fields, pages, status }`; Auswahl nach Endung; `guarded(path, f)` (Cloud-only/Link prüfen, Größen-Grenze, `catch_unwind`, Zeitwächter) |
| `src/content/extract/pdf.rs` | Textlayer, Metadaten (`Title`, `Author`, `CreationDate`), Seitenzahl, Verschlüsselung |
| `src/content/extract/office.rs` | docx/xlsx/pptx: Text und `core.xml`; ZIP-Grenzen; OLE-Erkennung |
| `src/content/extract/image.rs` | EXIF: Aufnahmedatum, Make/Model (bereinigt), GPS; PNG-IHDR-Maße; nutzt die Lese-Logik aus `scan/exif.rs` (gemeinsamer `LimitedReader`, wird dafür `pub(crate)`) |
| `src/content/extract/video.rs` | `mvhd`-Parser für MP4/MOV/M4V/3GP |
| `src/content/geo.rs` | Eingebettete Städte, `nearest(lat, lon) -> Option<Place>` |
| `src/content/classify/mod.rs` | `Classifier::new(defs) `, `classify(input) -> Classification` (Kategorie, Konfidenz, Top-2, Treffer), rein |
| `src/content/classify/defs.rs` | Laden und Prüfen von eingebauten + eigenen Kategorien, `[[senders]]`, `disable_builtin`; Fehler mit Datei/Zeile (wie `rules/file.rs`); `fingerprint()` (xxh3 über kanonisches JSON) |
| `src/content/classify/text.rs` | Normalisierung, Wortzählung, Aho-Corasick-Automat mit Wortgrenzen |
| `src/content/classify/fields.rs` | Datum, Betrag, Nummer, Absender, Titel-Müllfilter, Bereinigung von Feldwerten |
| `src/content/llm.rs` | `LlmClient` (Trait `Llm` für Tests), Endpoint-Prüfung, Prompt, JSON-Schema, Antwortprüfung |
| `src/content/pipeline.rs` | Datei → `ContentRecord`: Extraktion, OCR-Entscheidung, Klassifikation, Felder, LLM; Traits `Ocr`, `Llm`, `TextCache` werden hereingereicht |
| `src/platform/ocr.rs` | WinRT-OCR für Bild und PDF-Seiten; `available_languages()`; MTA-Initialisierung je Thread |
| `src/platform/dpapi.rs` | `protect(&[u8]) -> Vec<u8>`, `unprotect(&[u8]) -> Vec<u8>` (`CryptProtectData`/`CryptUnprotectData`, `CRYPTPROTECT_UI_FORBIDDEN`) |
| `src/index/content_cache.rs` | `content_lookup`, `content_store` (Block-Transaktion), `content_prune`, `content_clear`; `ocr_text_get/put/clear` |
| `src/app/classify.rs` | `classify_command`: Profil/Pfad, Sperre, Priorität, Fortschritt, Abbruch, Zusammenfassung, `--clear`, `--force`, `--ext`, `--only`, `--no-llm`; außerdem `ensure_classified(index, root, files)` für `plan rules` |
| `src/report/content.rs` | Abschnitt „Inhalte“: Modell (`ContentSection`) und Aufbau aus Index + Cache |
| `assets/categories.toml` | Eingebauter Kategorie-Startsatz (gleiches Format wie die Nutzerdatei, `include_str!`) |
| `assets/geo/cities15000.tsv`, `assets/geo/LICENSE` | Geodaten (CC BY 4.0) mit Namensnennung |
| `tools/geonames.ps1` | Erzeugt die TSV aus der GeoNames-Datei (einmalig, dokumentiert) |
| `tests/support/corpus.rs` | Erzeugt den synthetischen Testkorpus zur Testlaufzeit (PDF per `lopdf`, Office per `zip`, JPEG/PNG mit EXIF per Byte-Bausteinen, MP4 mit `mvhd`) samt erwarteter Ergebnisliste |
| `tests/fixtures/content/` | Wenige kleine, synthetische Bild-Scans (PNG, gescanntes PDF) für OCR-Tests; ohne echte Daten, erzeugt einmal mit Skript, je < 100 KB |
| `tests/cli_classify.rs` | End-to-End: `scan` → `classify` → `report` → `plan rules` → `apply` → `undo` auf dem Korpus |
| `tests/perf_classify.rs` | Wiederholungslauf mit 10.000 Dateien aus dem Cache (Zeitgrenze, wie `perf_plans.rs`) |

## Files to Modify

| File | Change Description |
|---|---|
| `Cargo.toml` | Neu: PDF-Crate(s) laut Spike, `zip` (default-features aus, `deflate`), `aho-corasick`, `ureq` (default-features aus, `json`); `windows`-Features `Media_Ocr`, `Graphics_Imaging`, `Data_Pdf`, `Storage`, `Storage_Streams`, `Globalization`; `windows-sys`-Features `Win32_Security_Cryptography`, `Win32_System_Threading`; dev: `lopdf` (falls nicht schon Abhängigkeit) |
| `src/lib.rs` | `pub mod content;` |
| `src/index/schema.sql`, `src/index/mod.rs` | Schema v3: `files.first_seen`, Tabellen `content_cache`, `ocr_text`; Migration v2→v3 (und v1→v2→v3 nacheinander) |
| `src/index/store.rs` | `FileRecord.first_seen`; Upsert setzt die Spalte nur beim Einfügen |
| `src/index/listing.rs` | `FileRow` um `ctime: Option<i64>` und `first_seen: Option<i64>` erweitern |
| `src/scan/mod.rs` / `src/scan/walker.rs` | `first_seen` setzen (neuer Schlüssel und Wurzel schon einmal `complete`) |
| `src/scan/exif.rs` | `LimitedReader` und Öffnen mit Cloud-Prüfung als `pub(crate)` teilen; Aufnahmedatum-Logik bleibt |
| `src/platform/mod.rs` | `pub mod ocr; pub mod dpapi;` + Priorität setzen/zurücksetzen (`ProcessPriorityGuard`) |
| `src/config.rs` | `[classify]` (`ClassifyConfig`) und `[llm]` (`LlmConfig`) mit Defaults und Prüfung (Endpoint nur Loopback, Größen/Zeiten parsen); `Profile` um `classify: Option<bool>`, `llm: Option<bool>`, `categories_file: Option<String>`; `with_profile` übernimmt sie |
| `src/cli.rs` | `Command::Classify(ClassifyArgs)`; `PlanRulesArgs.no_classify` |
| `src/app/mod.rs` | Dispatch `classify`; `build_report` setzt `report.content` |
| `src/app/plan.rs` | `plan_rules_command`: Kategorien laden, an `RuleSet::parse` geben, `RulesEnv.content` (`CachedContent`) und `RulesEnv.text` (`LiveText`) |
| `src/app/run.rs` | Mit `profile.classify == Some(true)` läuft `classify` nach dem Scan, solange die Sperre noch gehalten wird; Fehler machen den Lauf nur `partial`; `RunRecord.review` setzen |
| `src/runlog.rs` | `RunRecord.review: Option<usize>` (serde default) |
| `src/notify.rs` | Zusatz „N Dateien zum Prüfen“, wenn `review` gegenüber dem letzten Lauf gestiegen ist |
| `src/rules/condition.rs` | `Candidate` um `content: Option<&ContentView>`, `arrived_ticks: Option<i64>`, `text_hits: Option<&BitSet>`; `Conditions` um `category`, `min_confidence`, `category_source`, `fields`, `fields_regex`, `text_regex`, `min_dwell_days`; `matches` liefert `MatchResult { Match(caps), NoMatch, NearMiss(SkipReason, detail) }`; `needs_content()`, `needs_text()` |
| `src/rules/template.rs` | Tokens `Field { name, format, fallback }` für `category`, `doc.*`, `exif.camera/city/country/lat/lon`, `video.date`; Parser für `|`-Ersatz; `TargetError::MissingField`; Prüfung bekannter Felder |
| `src/rules/file.rs`, `src/rules/mod.rs` | Neue Schlüssel in `RawRule`; Umwandlung; `RuleSet::parse(text, source, &KnownCategories)`; `Rule::target` bekommt die Feldwerte |
| `src/change/mod.rs`, `src/change/plan.rs` | Neue `SkipReason`-Varianten mit Texten; `Skipped.detail` |
| `src/change/rules.rs` | Drei Durchgänge (siehe Entscheidungen); Traits `ContentLookup` und `TextSource` in `RulesEnv`; Statistik „zum Prüfen“ |
| `src/report/mod.rs`, `src/report/html.rs`, `templates/report.html`, `assets/report.css`, `src/report/terminal.rs`, `src/report/csv.rs`, `src/report/json.rs` | Abschnitt „Inhalte“, Terminal-Kurzfassung, Spalten `category`, `confidence`, `content_source`, `doc_date`, `doc_sender` im Datei-Export |
| `README.md` | Befehl `classify`, `categories.toml`, neue Bedingungen und Platzhalter, Konfidenzformel, LLM-Einrichtung (Ollama, empfohlenes Modell), OCR-Sprachpakete, Datenschutz (Felder, DPAPI-OCR-Cache, `--clear`), GeoNames-Namensnennung |
| `docs/roadmap.md` | Status Phase 6a |

## Data Model Changes

### Index-Schema v3
```sql
ALTER TABLE files ADD COLUMN first_seen INTEGER;   -- 100-ns-Ticks; NULL = schon beim ersten Scan da

CREATE TABLE content_cache (
    path_key TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    extractor_version INTEGER NOT NULL,
    defs_fingerprint TEXT NOT NULL,     -- Kategorie-Definitionen bei der Zuordnung
    status TEXT NOT NULL,               -- ok | unreadable:<grund> | unsupported | too-large
    category TEXT,
    confidence REAL,
    category2 TEXT,
    confidence2 REAL,
    source TEXT,                        -- rules | llm
    hits TEXT,                          -- JSON-Liste der ausschlaggebenden Treffer
    fields TEXT NOT NULL DEFAULT '{}',  -- JSON: {"doc.date": "2026-09-30", "exif.city": "Lisbon", …}
    field_sources TEXT NOT NULL DEFAULT '{}', -- JSON: Feld → pattern | meta | llm
    text_source TEXT,                   -- layer | office | ocr | none
    llm_model TEXT,
    classified_at TEXT NOT NULL
);

CREATE TABLE ocr_text (
    path_key TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    ocr_version INTEGER NOT NULL,
    languages TEXT NOT NULL,            -- z. B. "de,en"
    pages INTEGER NOT NULL,
    data BLOB NOT NULL                  -- DPAPI-verschlüsselter UTF-8-Text
);
```
`SCHEMA_VERSION = 3`. Migration v2→v3 läuft in einer Transaktion. v1-Indizes durchlaufen v1→v2→v3.

Gültigkeit eines `content_cache`-Eintrags:
- **Vollständig gültig:** Größe, mtime und `extractor_version` sind gleich, `defs_fingerprint` ist gleich.
- **Neu zuordnen:** Nur der Fingerabdruck ist anders. Der Text wird neu gelesen (OCR aus `ocr_text`). LLM-Felder werden übernommen, die LLM-Kategorie wird neu bestimmt.
- **Neu extrahieren:** Größe, mtime oder `extractor_version` sind anders.

### Inhaltssicht für Regeln
```rust
pub struct ContentView<'a> {
    pub status_ok: bool,
    pub category: Option<&'a str>,
    pub confidence: f32,
    pub category2: Option<(&'a str, f32)>,
    pub source: Source,
    pub fields: &'a BTreeMap<String, String>,
}
```

### Kategorie-Datei (`categories.toml`, eingebaut: `assets/categories.toml`)
```toml
disable_builtin = ["medizin"]

[[categories]]
name             = "rechnung"            # [a-z0-9_-]+, eindeutig
kind             = "text"                # text (Default) | meta (nur eingebaut: foto, screenshot)
description      = "Rechnung, Invoice"   # für den LLM-Prompt
keywords         = { "rechnung" = 3, "rechnungsnummer" = 4 }
patterns         = { '(?i)rechnungs?-?nr\.?\s*[:#]?\s*[\w-]+' = 4 }
exclude_keywords = ["angebot"]
ext              = ["pdf", "docx", "jpg", "png"]   # leer = alle Dokument- und Bildtypen

[[senders]]
name  = "Telekom"
match = ["Telekom Deutschland GmbH", "telekom.de"]
```
Prüfregeln:
- unbekannte Schlüssel sind Fehler (`deny_unknown_fields`)
- Gewichte 1–10
- Regex muss kompilieren
- doppelte Namen sind Fehler (Groß-/Kleinschreibung egal)
- `disable_builtin` darf nur eingebaute Namen enthalten
- `kind = "meta"` ist für eigene Kategorien ein Fehler

Die Fehlermeldung nennt Datei, Zeile und Kategorie.

### Config
```toml
[classify]
categories_file = "categories.toml"
min_confidence  = 0.8
max_file_size   = "100MB"
ocr             = true
ocr_languages   = ["de", "en"]
ocr_max_pages   = 5
ocr_image_globs = ["**/Scans/**", "**/*scan*", "**/*dokument*"]
cache_ocr_text  = true
max_text_chars  = 200000
threads         = 0

[llm]
enabled         = false
endpoint        = "http://127.0.0.1:11434"
model           = "qwen2.5:7b"     # Default nach Spike 0c
timeout         = "60s"
max_input_chars = 6000
tasks           = ["category", "fields", "title"]
max_confidence  = 0.85
```
Profil (Erweiterung von Phase 5): `classify = true`, `llm = true|false` (überschreibt `[llm] enabled`), `categories_file = "…"`. Das präzisiert die Spec („Profile können `[classify]`/`[llm]` überschreiben“) auf die drei Werte, die sich je Ordner sinnvoll unterscheiden. Weitere Werte kommen später, wenn Bedarf besteht.

### Plan-Format
`version: 2` bleibt. `skipped[].reason` bekommt neue Werte `low-confidence`, `not-classified`, `missing-field`, `too-recent-arrival`. `skipped[].detail` ist optional. Aktionen bleiben `move` mit `rule`.

## API / Interface Changes

### CLI
```
ordner-cleanup classify [<pfad>] [--profile <name>] [--no-llm] [--force] [--ext pdf,jpg]
                        [--only <glob>] [--clear]
ordner-cleanup plan rules … [--no-classify]
```
- `classify` ohne Index für die Wurzel ergibt einen Fehler mit Hinweis auf `scan` (Exit 1).
- `--clear` schließt alle anderen Schalter außer Pfad/Profil aus.
- `--only` ist ein Glob relativ zur Wurzel (wie `glob` in Regeln). Er dient zum Abstimmen der Kategorien an einer Auswahl.

Ausgabe von `classify` (Beispiel):
```
Klassifiziert: C:\Users\Oskar\Downloads
  1.204 Dateien geprüft: 87 analysiert, 1.093 aus dem Cache, 24 übersprungen (Cloud-only 20, zu groß 4)
  Kategorien geändert: 312 Dateien neu zugeordnet (OCR-Text aus dem Cache)
  Quellen: Text 61, OCR 9, Metadaten 17, LLM 12
  rechnung 143 · kontoauszug 52 · vertrag 18 · foto 610 · screenshot 77 · … · ohne Kategorie 211
  Zum Prüfen: 19 (Liste im Bericht)   Nicht lesbar: 3 (verschlüsselt 2, beschädigt 1)
  Warnung: LLM nicht erreichbar (http://127.0.0.1:11434), ohne LLM weitergemacht
  Dauer 0:42
```

### Bibliothek (Auszug)
```rust
// content
pub struct Classifier { /* Automat, Regex, Gewichte */ }
impl Classifier {
    pub fn new(defs: &CategoryDefs) -> Self;
    pub fn classify(&self, input: &ClassifyInput) -> Classification;
}
pub struct ClassifyInput<'a> { pub name: &'a str, pub ext: &'a str, pub text: Option<&'a str>, pub meta: &'a Fields }
pub fn extract_fields(text: &str, meta: &Fields, senders: &[Sender], today: NaiveDate) -> (Fields, FieldSources);

pub trait Ocr: Sync { fn image(&self, path: &Path) -> Result<String, OcrError>; fn pdf(&self, path: &Path, max_pages: usize) -> Result<String, OcrError>; }
pub trait Llm: Sync { fn ask(&self, req: &LlmRequest) -> Result<LlmAnswer, LlmError>; }

pub fn process_file(file: &FileRow, ctx: &PipelineCtx) -> ContentRecord;

// index
impl Index {
    pub fn content_lookup(&self, root_key: &str) -> Result<HashMap<String, CachedContent>>;
    pub fn content_store(&mut self, records: &[ContentRecord]) -> Result<()>;
    pub fn content_prune(&mut self, root_key: &str) -> Result<usize>;
    pub fn content_clear(&mut self, root_key: &str) -> Result<usize>;
    pub fn ocr_text_get(&self, key: &str, size: i64, mtime: i64, langs: &str) -> Result<Option<Vec<u8>>>;
    pub fn ocr_text_put(&mut self, entry: &OcrTextEntry) -> Result<()>;
}

// change::rules
pub trait ContentLookup: Sync {
    /// Sorgt dafür, dass `files` klassifiziert sind (bei Bedarf über `classify`), und liefert die Sicht.
    fn content(&self, index: &mut Index, files: &[&FileRow]) -> Result<HashMap<String, CachedContent>, IndexError>;
}
pub trait TextSource: Sync { fn text(&self, index: &Index, file: &FileRow) -> Option<String>; }
```

## Implementation Sequence

0. **Spikes** (Ergebnisse in „Umsetzungsnotizen“ unten festhalten, kein Produktivcode):
   - **0a PDF:** `pdf-extract` gegen `lopdf` (+ eigene Textextraktion) an 20 echten PDFs aus Downloads/Scans. Gemessen werden Textqualität (Umlaute, Spalten), Laufzeit, Panics/Hänger und verschlüsselte PDFs. Danach fällt die Wahl samt Zeitwächter-Strategie.
   - **0b OCR:** Kleines Rust-Programm mit `windows` 0.62: `OcrEngine` für `de`, Bild-OCR und PDF-Seite über `Windows.Data.Pdf` rendern, MTA in rayon-Threads, Zeit je Seite, Verhalten ohne Sprachpaket.
   - **0c LLM:** Ollama mit `format`-Schema, 3 Modelle (z. B. `qwen2.5:3b`, `qwen2.5:7b`, `llama3.2:3b`), CPU-only. Gemessen werden Zeit je Anfrage und Qualität von Kategorie, Datum, Absender und Titel an 20 Dokumenten. Ergebnis ist der Default für `[llm] model`.
   - Depends on: nothing
1. **Index v3 und `first_seen`:** Schema, Migration (v1→v3, v2→v3), `FileRecord`/`FileRow`-Felder, Scan setzt `first_seen`; `content_cache.rs` mit Lookup/Store/Prune/Clear und `ocr_text` (noch ohne DPAPI: Bytes rein/raus).
   - Files: `src/index/*`, `src/scan/mod.rs`, `src/scan/walker.rs`
   - Depends on: nothing
2. **DPAPI und Priorität:** `platform/dpapi.rs` (Rundlauf-Test; manipulierte Daten ergeben einen Fehler, kein Panic), `ProcessPriorityGuard`.
   - Files: `src/platform/*`, `Cargo.toml`
   - Depends on: nothing
3. **Extraktoren ohne OCR:** `content/mod.rs`, `extract/{mod,pdf,office,image,video}.rs` mit `guarded`, Grenzen, OLE-Erkennung, EXIF-Erweiterung, `mvhd`-Parser. Testkorpus-Generator `tests/support/corpus.rs` (PDF/Office/JPEG/PNG/MP4) entsteht hier mit.
   - Files: `src/content/extract/*`, `src/scan/exif.rs`, `tests/support/corpus.rs`, `Cargo.toml`
   - Depends on: Spike 0a
4. **Geodaten:** `tools/geonames.ps1`, `assets/geo/*`, `content/geo.rs` (Lissabon → Lisbon/PT, offene See → nichts, 0/0 → kein GPS).
   - Files: `src/content/geo.rs`, `assets/geo/*`, `tools/geonames.ps1`
   - Depends on: nothing
5. **Kategorien und Felder:** `classify/{defs,text,mod,fields}.rs`, `assets/categories.toml` mit 14 eingebauten Kategorien, Fingerabdruck, Konfidenzformel, Metadaten-Kategorien, Feldmuster, Bereinigung. Reine Unit-Tests (Tabellen mit Texten → erwartete Kategorie/Felder).
   - Files: `src/content/classify/*`, `assets/categories.toml`, `Cargo.toml` (`aho-corasick`)
   - Depends on: Step 3 (Typen)
6. **OCR:** `platform/ocr.rs` hinter Trait `Ocr`; Sprachpakete prüfen; OCR-Entscheidung für PDF (Textlayer zu dünn) und Bild (`ocr_image_globs`, kein Kamera-EXIF). Tests mit `tests/fixtures/content/` laufen nur, wenn das `de`-Paket vorhanden ist, sonst `eprintln!`-Hinweis und Rückkehr (kein `#[ignore]`, damit sie auf Entwickler-PCs laufen).
   - Files: `src/platform/ocr.rs`, `tests/fixtures/content/*`, `Cargo.toml`
   - Depends on: Spike 0b, Step 3
7. **LLM-Client:** Config-Prüfung (Loopback), `ureq`-Agent ohne Proxy und Weiterleitungen, `/api/tags`-Prüfung, Prompt + JSON-Schema, Antwortprüfung. Tests gegen einen Mini-HTTP-Server auf `127.0.0.1:0` im Test (gültige Antwort, kaputtes JSON, unbekannte Kategorie, Timeout, 404 Modell fehlt).
   - Files: `src/content/llm.rs`, `src/config.rs`, `Cargo.toml`
   - Depends on: Spike 0c, Step 5
8. **Pipeline und Befehl `classify`:** `content/pipeline.rs` (Extraktion → OCR mit DPAPI-Cache → Klassifikation → Felder → LLM), `app/classify.rs` (inkrementelle Auswahl inkl. „neu zuordnen“, rayon + OCR-/LLM-Pools, Fortschritt, Abbruch, Block-Schreiben, Zusammenfassung, `--clear/--force/--ext/--only/--no-llm`), Config `[classify]`, CLI, Profil-Felder, Sperre, Priorität.
   - Files: `src/content/pipeline.rs`, `src/app/classify.rs`, `src/app/mod.rs`, `src/cli.rs`, `src/config.rs`
   - Depends on: Steps 1, 2, 3, 5, 6, 7 (4 für GPS-Felder)
9. **Regel-Engine:** Bedingungen, `MatchResult`, Platzhalter mit `|`-Ersatz, Feldbereinigung, `KnownCategories` beim Parsen, Fehlertexte mit Zeile/Regel. Unit-Tests in `rules/mod.rs`-Stil (rstest-Tabellen für Fehler).
   - Files: `src/rules/*`
   - Depends on: Step 5 (Feldnamen, Kategorienamen)
10. **`plan rules` mit Inhalt:** drei Durchgänge, `ContentLookup` (echte Implementierung ruft `ensure_classified`), `TextSource` (Textlayer/Office live, OCR aus Cache), neue Skip-Gründe mit `detail`, `--no-classify`, Zusammenfassung „zum Prüfen“. Idempotenz-Test.
    - Files: `src/change/rules.rs`, `src/change/mod.rs`, `src/change/plan.rs`, `src/app/plan.rs`, `src/cli.rs`
    - Depends on: Steps 8, 9
11. **Bericht:** `report/content.rs`, HTML-Abschnitt (Kategorie-Tabelle, Quellen, Zum Prüfen ≤ 200, Fotos nach Land/Stadt und Kamera Top 10, Hinweis „noch nicht klassifiziert“), Terminal-Kurzfassung, CSV/JSON-Spalten.
    - Files: `src/report/*`, `templates/report.html`, `assets/report.css`, `src/app/mod.rs`
    - Depends on: Step 8
12. **Betrieb:** `run` mit `classify = true`, `RunRecord.review`, Benachrichtigungstext; Profil `llm`/`categories_file`.
    - Files: `src/app/run.rs`, `src/runlog.rs`, `src/notify.rs`, `src/config.rs`
    - Depends on: Steps 8, 11
13. **Korpus-Test und Kalibrierung:** `tests/cli_classify.rs` (End-to-End inkl. `apply`/`undo`, Cloud-only-Datei per Attribut-Simulation wie in `windows_specifics.rs`), Korpus-Vergleich gegen die erwartete Liste (100 % auf dem synthetischen Korpus), Konfidenz-Konstanten kalibrieren, `tests/perf_classify.rs`.
    - Files: `tests/*`, ggf. Konstanten in `src/content/classify/mod.rs`
    - Depends on: Steps 10, 11
14. **Doku und manueller Test:** README, Roadmap. Manueller Test auf dem echten Downloads-/Scans-Ordner (ohne und mit LLM): Stichprobe von 100 Dateien bewerten, Plan prüfen (0 falsche Aktionen), `apply` → `undo` mit Snapshot-Vergleich wie in Phase 4. Ergebnis in der Spec dokumentieren.
    - Depends on: Step 13

Schritte 1, 2 und 4 sind voneinander unabhängig und können parallel laufen. Ebenso 6 und 7 nach ihren Spikes.

## Test Strategy

- **Unit:**
  - Normalisierung (Umlaute doppelt, Silbentrennung)
  - Aho-Corasick mit Wortgrenzen (`rechnung` passt nicht in `abrechnungszeitraum`, wenn als ganzes Wort definiert)
  - Konfidenzformel (Monotonie per Eigenschaftstest über zufällige Gewichte, Gleichstand ergibt 0)
  - Prüfung der Kategorie-Datei (rstest-Fehlertabelle mit Zeile und Name)
  - Feldmuster (Tabelle mit 30+ Text-Schnipseln DE/EN, unplausible Daten)
  - Bereinigung von Feldwerten
  - Geodaten
  - `mvhd` (v0/v1, 0-Wert, abgeschnitten)
  - Office (ZIP-Bombe, OLE, kaputtes XML)
  - PDF (verschlüsselt, ohne Textlayer, kaputt)
  - EXIF-Kamera/GPS
  - Template-Parser mit `|`-Ersatz (Fehler: `|` bei `exif.date`, verschachtelter Ersatz, unbekanntes Feld)
  - Bedingungen und `MatchResult` (NearMiss bei Konfidenz, `category_source`)
  - `min_dwell` (ctime alt / first_seen neu, ohne first_seen)
  - Endpoint-Prüfung (`http://192.168.1.5` ist ein Fehler, `http://localhost:11434` gültig)
  - Prüfung der LLM-Antwort
  - DPAPI-Rundlauf
  - Cache-Gültigkeit (vollständig, neu zuordnen, neu extrahieren)
- **Integration:**
  - `Index`-Migration v1/v2→v3 mit Daten
  - `first_seen` über zwei Scans (erster Scan NULL, neue Datei im zweiten Scan gesetzt, Upsert behält den Wert)
  - Pipeline mit Fake-`Ocr`/`Llm` (OCR-Text kommt beim zweiten Lauf aus dem Cache und `Ocr` wird nicht aufgerufen; nach Änderung der Kategorie-Datei wird ohne OCR neu zugeordnet)
  - `plan_rules` mit Fake-`ContentLookup`/`TextSource` (alle neuen Skip-Gründe, Text nur für Kandidaten gelesen, Idempotenz)
  - LLM gegen Mini-Server
  - roher Index-Inhalt enthält keinen OCR-Klartext (Suche nach einem bekannten Wort in der Datei)
- **E2E (`assert_cmd`):**
  - `tests/cli_classify.rs`: `scan` → `classify --no-llm` → Zusammenfassung prüfen → zweiter `classify` liest nichts (Zähler „aus dem Cache“) → `report` enthält „Inhalte“ → `plan rules` mit Inhaltsregeln → `apply --yes` → `undo --yes` → Snapshot identisch
  - `classify` ohne Index → Exit 1
  - Endpoint nicht Loopback → Config-Fehler
  - Ollama-Endpoint ohne Server → Warnung, Exit 0
  - `run --profile` mit `classify = true` (bestehende `cli_run.rs`-Hilfen)
  - `readonly.rs` um `classify` erweitern: kein Byte und keine mtime im Ordner geändert
- **Performance:** `tests/perf_classify.rs`: 10.000 Cache-Treffer unter einer festen Zeitgrenze (Ziel: wenige Sekunden auf dem Entwickler-PC, Grenze großzügig wie in `perf_plans.rs`). 200 Textlayer-PDFs ≥ 20 Dateien/s, als gemessene Zahl im Testprotokoll und nicht als harte Grenze.
- **Manuell:** siehe Schritt 14; zusätzlich OCR mit echtem Scan (Sprachpaket `de`), LLM mit dem Default-Modell und echter Cloud-only-Platzhalter (bisher in Phase 2 bis 4 offen).

## Risks & Open Questions

- **PDF-Parser hängt oder läuft sehr lange bei bösartigen oder kaputten PDFs:** `catch_unwind` hilft nur bei Panics. Abhilfe: Seiten- und Zeichengrenzen und ein Zeitwächter je Datei. Weil Rust-Threads sich nicht abbrechen lassen, läuft die Extraktion in einem Worker-Thread. Bei Zeitüberschreitung (Default 30 s) wird die Datei als `unreadable: timeout` markiert und der Thread aufgegeben, höchstens 4 aufgegebene Threads je Lauf, danach Abbruch des PDF-Teils mit Hinweis. Ob eine Prozess-Isolation (eigener Kindprozess je Charge) nötig ist, klärt Spike 0a.
- **Windows-OCR-Qualität bei kleinem Druck:** Rendering-Skalierung (2×, bis `MaxImageDimension`) und Graustufen werden im Spike verglichen. Bleibt die Qualität schlecht, landen solche Scans unter „Zum Prüfen“ statt falsch einsortiert zu werden. Das Ziel „0 falsche Aktionen“ bleibt damit erreichbar.
- **90 % Trefferquote auf echten Daten:** Die Kalibrierung braucht Iterationen. Der OCR-Text-Cache und `--only` machen sie schnell. Eine Stichprobe wird manuell gelabelt (CSV im Scratchpad, nicht im Repo, weil echte Daten).
- **LLM auf CPU langsam (2–10 s je Dokument):** Es wird nur für den Rest bzw. für Titel gefragt, die Ergebnisse werden gecacht, und `--no-llm` bleibt möglich. Das README nennt die zu erwartende Dauer.
- **Prompt-Injection aus Dokumenten:** Das LLM kann nur Werte liefern, die gegen Schema, Kategorienliste und Plausibilität geprüft und wie jeder Feldwert bereinigt werden. Es löst keine Aktionen aus. Ein `plan` bleibt ein Plan.
- **Wachstum der Binärdatei** (Geodaten ~1 MB, PDF-/ZIP-Crates): akzeptiert, wird nach Schritt 4 und 8 gemessen.
- **`ureq` und Proxy-Umgebungsvariablen:** Der Proxy wird ausdrücklich abgeschaltet, sonst könnte ein `HTTP_PROXY` den Loopback-Verkehr umleiten. Dafür gibt es einen Test.
- **Pläne mit `detail` sind für ältere Programmversionen unlesbar** (`deny_unknown_fields`): akzeptiert, siehe Entscheidungen.
- **Index-Größe:** Etwa 1 KB je Datei im Inhalts-Cache. Der OCR-Text-Cache ist bei 500 Scans × 5 Seiten grob 10–30 MB. Akzeptabel.
- [ ] Welche PDF-Crate? (Spike 0a)
- [ ] Default-Modell für `[llm] model`? (Spike 0c)
- [ ] Endgültige Konstanten der Konfidenzformel (Schritt 13)

## Umsetzungsnotizen (Abweichungen vom Plan)

- **Spikes 0a/0b/0c nicht auf echten Daten gelaufen.** `/feature:code` lief als Hintergrund-Sitzung ohne Zugriff auf echte PDFs/Scans (Datenschutz), ohne Ollama und ohne OCR-Sprachpaket-Test. Entscheidung für 0a: `pdf-extract` (bringt `lopdf` 0.42 mit, wird über `pdf_extract::Document` genutzt, kein zweites `lopdf` im Baum), Worker-Thread mit Zeitwächter, höchstens 4 aufgegebene Worker, 30 Seiten. Die Qualität an echten Rechnungen/Scans bleibt der manuelle Test (Schritt 14); 0b und 0c ebenso.
- **Schritt 1:** `first_seen` wird über einen `meta`-Schlüssel `root_complete:<key>` entschieden (kein zusätzliches Schema); abgebrochene Erstscans zählen nicht als vollständig. `FileRow` hat `ctime` und `first_seen`.
- **Schritt 1:** `ocr_text_get` prüft zusätzlich `ocr_version` und `languages` (Plan: Signatur ohne `ocr_version`). `CachedContent` ist eine flache Zeile (JSON-Spalten als String); die Umwandlung in `ContentRecord` liegt in `content/`.
- **Schritt 3:** `LimitedReader` und `open_local` sind `pub(crate)` in `scan/exif.rs`. Test-Bausteine (`jpeg_with_camera`, `mp4`, `docx`, `pdf_with_pages` …) liegen als `#[doc(hidden)] pub mod testing` in den Extraktor-Dateien, damit `tests/support/corpus.rs` sie nutzen kann.
- **Schritt 4:** Die TSV hat eine fünfte Spalte `population`. Stadtteile großer Städte stehen in GeoNames als eigene Orte (Lissabon → „São Jorge de Arroios“); unter den Orten, die höchstens 10 km weiter weg liegen als der nächste (und höchstens 50 km), gewinnt der einwohnerstärkste.
- **Schritt 6 / Spike 0b (auf dieser Maschine gelaufen, Windows 11 26200, Sprachpakete de-DE und en-US):** Bild-OCR über `Windows.Media.Ocr` läuft in-process und liest den synthetischen Rechnungs-Scan (`tests/fixtures/content/rechnung-scan.jpg`, erzeugt mit `tools/make-ocr-fixtures.ps1`) korrekt. **PDF-Rendering mit `Windows.Data.Pdf` lässt den Prozess beim Beenden mit einer Zugriffsverletzung (0xC0000005) abstürzen**, unabhängig von Apartment-Modell, `Close()`, `RoUninitialize` oder `process::exit`; das reproduziert schon mit einem 40-Zeilen-Programm ohne den Rest des Projekts. Abhilfe: PDF-OCR läuft in einem Hilfsprozess (versteckter Unterbefehl `ordner-cleanup ocr-worker <pdf> --max-pages N --langs de,en`), der eine JSON-Zeile schreibt und sich mit `TerminateProcess` beendet (Exit-Code 0, kein Fehlerdialog). Das setzt auch den Plan-Punkt „Prozess-Isolation“ für den Renderer um (Zeitlimit 180 s je PDF, Abschuss bei Überschreitung). Der Elternprozess sucht `ordner-cleanup.exe` neben dem laufenden Programm.
- **Schritt 6:** Das Trait `Ocr` liegt in `content/ocr.rs` (nicht in `pipeline.rs`), die Windows-Implementierung `WinOcr` in `platform/ocr.rs`. PDF-OCR-Tests stehen in `tests/ocr_worker.rs` (brauchen die Binärdatei über `CARGO_BIN_EXE_`), laufen nur mit dem deutschen Sprachpaket.
- **Schritt 8:** `ContentView` entfällt, Regeln und Bericht nutzen `ContentRecord`. `plan rules` bekommt `ContentLookup` (echte Implementierung `CachedLookup` in `app/classify.rs`, klassifiziert bei Bedarf unter der Scan-Sperre) und `TextProvider` (nicht `TextSource`: der Name gehört schon dem Enum in `content`). `IndexError::Other` für Fehler aus dem Lookup. Das Abbruch-Flag ist jetzt prozessweit (`global_cancel_flag`), weil `ctrlc` nur einen Handler erlaubt und `run` Scan und `classify` hintereinander ausführt.
- **Schritt 8:** Bilder unter 20 KB und Dateien mit Namen „Screenshot …“ werden nicht per OCR gelesen (Zusatz zur Plan-Regel, spart Zeit bei Icons und Bildschirmfotos); `globs` in `ocr_image_globs` gelten weiterhin.
- **Schritt 9:** Die Spec-Beispiel-Regex `'^(?!DE$)'` ist mit der `regex`-Engine nicht möglich (kein Look-around). Spec und README nutzen `'^(?:[^D].|D[^E])$'`. Bei Regeln, die nur an einer weichen Bedingung scheitern, hat `low-confidence` Vorrang vor `too-recent-arrival`. Eine Regel „entscheidet“ ohne Inhalt, wenn sie weder Inhalt, Text noch `min_dwell` braucht; Regeln dahinter werden nicht ausgewertet.
- **Schritt 13:** Die Konfidenz-Konstanten (8, Quadrat) bleiben unverändert: Der synthetische Korpus (`korpus_stimmt_mit_der_erwarteten_liste_ueberein`) stimmt zu 100 % mit der erwarteten Liste überein. Eine Kalibrierung an echten Daten gehört zum manuellen Test. `tests/support/corpus.rs` entfällt; die Bausteine liegen als `testing`-Module bei den Extraktoren.
- **Offen (manuell, Schritt 14):** Test auf dem echten Downloads-/Scans-Ordner ohne und mit LLM (Stichprobe 100 Dateien, ≥ 90 % korrekt, Plan ohne falsche Aktion, `apply` → `undo`), Spikes 0a (PDF-Qualität an echten Rechnungen) und 0c (Ollama-Modellwahl; Default `qwen2.5:7b` ungeprüft), echter Cloud-only-Platzhalter. Ergebnis gehört in die Spec.
