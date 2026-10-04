---
title: "Inhaltsbasierte Klassifikation (Phase 6a)"
status: approved          # draft | approved | implemented
created: 2026-10-04
updated: 2026-10-04
---

# Feature: Inhaltsbasierte Klassifikation (Phase 6a)

## Problem Statement
In Eingangsordnern wie Downloads und Scans landen Dokumente mit Namen wie `scan0012.pdf`, `Dokument (3).pdf` oder `IMG_4711.HEIC`. Die Regel-Engine aus Phase 4 sortiert nur nach Pfad, Name, Endung, Alter, Größe und EXIF-Datum. Was eine Datei *ist* (Rechnung, Vertrag, Kontoauszug, Urlaubsfoto aus Lissabon), bleibt unsichtbar. Deshalb lassen sich diese Ordner nicht automatisch leeren, Dateinamen bleiben nichtssagend, und Fotos lassen sich nicht nach Kamera oder Ort ordnen.

Betroffen ist zunächst der Entwickler selbst, später Kollegen. Phase 6a macht Inhalt und Metadaten für Regeln und Bericht nutzbar, **ohne** das Grundprinzip „Plan → Review → Apply“ aufzuweichen und ohne dass Daten den PC verlassen (siehe [`../roadmap.md`](../roadmap.md), Abschnitt 3).

Die GUI aus Phase 6 der Roadmap ist **nicht** Teil dieser Spec und bekommt eine eigene.

## Goals
- [ ] Neuer Befehl `classify` liest Inhalt und Metadaten unterstützter Dateien, inkrementell und abbrechbar, und legt Kategorie, Konfidenz und extrahierte Felder im Index-Cache ab
- [ ] Ausgewertete Quellen: PDF (Textlayer und Metadaten), Office (docx/xlsx/pptx: Text und Kern-Metadaten), Bilder (EXIF inkl. Kamera und GPS, Ort offline aufgelöst), Videos (Aufnahmedatum MP4/MOV), OCR über die eingebaute Windows-OCR für gescannte PDFs und Fotos von Dokumenten
- [ ] Kategorien: eingebauter Startsatz (DE/EN), per TOML erweiterbar und überschreibbar; jede Zuordnung hat eine Konfidenz und ist erklärbar (welche Treffer)
- [ ] Optional ein **lokales** LLM (Ollama auf Loopback): Kategorie für den Rest, Felder, wenn die Muster nichts finden, Namensvorschlag `{doc.title}`. Ohne LLM funktioniert alles regelbasiert
- [ ] Regel-Engine: neue Bedingungen (`category`, `min_confidence`, `category_source`, `fields`, `fields_regex`, `text_regex`, `min_dwell`) und Platzhalter (`{category}`, `{doc.*}`, `{exif.camera}`, `{exif.city}`, `{exif.country}`, `{video.date}` …) mit Ersatzwert-Syntax
- [ ] Unsichere Zuordnungen erzeugen **nie** eine Aktion: Sie erscheinen im Plan als `skipped: low-confidence` und im Bericht unter „Zum Prüfen“
- [ ] Bericht: Abschnitt „Inhalte“ mit Übersicht nach Kategorien, Quellen, nicht lesbaren Dateien und der Liste „Zum Prüfen“ mit den Top-2-Kategorien
- [ ] Wartezeit bei Eingangsordnern: `min_dwell` misst die Zeit seit Ankunft im Ordner, nicht die mtime
- [ ] Messbar: ≥ 90 % korrekte Kategorie auf dem echten Downloads-/Scans-Ordner, 0 falsche Aktionen, der Eingangsordner lässt sich mit einem Plan leeren; Wiederholungsläufe ohne Änderungen dauern wenige Sekunden

## Out of Scope
- GUI und TUI (eigene Spec, Phase 6b)
- Cloud-LLMs bzw. jede Netzwerkverbindung außer Loopback
- Unverschlüsseltes Speichern von Text; Volltextsuche, `search`-Befehl (gespeichert wird nur OCR-Text, verschlüsselt, als Cache, siehe „Index“)
- Eigener Planer `plan classify` mit fester Zielstruktur (Einsortieren läuft über `plan rules`)
- Auto-Apply in jeder Form (geplante Läufe erzeugen weiter nur Pläne)
- Audio-Metadaten (ID3 usw.)
- Bildinhaltserkennung bei Fotos (Gesichter, Objekte, „Strand“), Ähnlichkeitssuche
- Interaktive Nachfrage bei unsicheren Dateien
- Training oder Feintuning eines Modells; selbstlernende Kategorien
- Weitere Formate: odt/ods, rtf, eml/msg, alte Office-Binärformate (doc/xls/ppt), RAW-Fotos jenseits dessen, was die EXIF-Bibliothek schon liest
- Netzlaufwerke (SMB), Linux/macOS

## User Journey

### Grundablauf
1. Der Nutzer scannt wie gewohnt: `scan <pfad>` bzw. `scan --profile scans`.
2. Er klassifiziert: `ordner-cleanup classify <pfad> | --profile X [--no-llm] [--force] [--ext pdf,jpg]`.
   - Grundlage ist der Index des letzten Scans. Ohne Index bricht der Befehl mit Hinweis auf `scan` ab.
   - Analysiert werden nur neue oder geänderte Dateien (Cache-Schlüssel: Pfad, Größe, mtime, Extraktor-Version) sowie Dateien, deren Kategorie sich durch geänderte Kategorie-Definitionen ändern kann. `--force` analysiert alles neu.
   - Fortschrittsanzeige mit Anzahl und aktueller Quelle (Text/OCR/LLM). Ctrl+C bricht sauber ab; bis dahin Analysiertes bleibt im Cache.
   - Zusammenfassung: analysiert / aus Cache / übersprungen nach Grund, Verteilung nach Kategorie, Anzahl „Zum Prüfen“, nicht lesbar, Quellen (Text, Metadaten, OCR, LLM), Dauer. War das LLM eingeschaltet, aber nicht erreichbar, steht das als Warnung darin.
3. `report` enthält den neuen Abschnitt „Inhalte“ (siehe unten). CSV/JSON bekommen Spalten für Kategorie, Konfidenz, Quelle und die wichtigsten Felder.
4. Der Nutzer schreibt Regeln mit den neuen Bedingungen und Platzhaltern (Beispiel unten) und erzeugt wie gewohnt einen Plan: `plan rules <pfad>`. Ist der Inhalts-Cache für Dateien, die eine Regel mit Inhaltsbedingung prüfen muss, veraltet oder leer, ruft `plan rules` `classify` für genau diese Dateien automatisch auf (mit Hinweis in der Ausgabe; `--no-classify` schaltet das ab, betroffene Dateien erscheinen dann als `skipped: not-classified`).
5. Unsichere Dateien stehen im Plan als `skipped: low-confidence` und im Bericht unter „Zum Prüfen“. Der Nutzer benennt sie von Hand, ergänzt eine eigene Kategorie oder senkt `min_confidence` für eine Regel.
6. `apply` und `undo` funktionieren unverändert wie in Phase 2–4.

### Geplante Läufe und Profile (Erweiterung Phase 5)
- Profile bekommen den optionalen Schlüssel `classify = true`. Dann führt `run --profile X` nach `scan` und vor dem Bericht `classify` aus. Die Benachrichtigung nennt zusätzlich „N Dateien zum Prüfen“, wenn es neue gibt.
- `--profile` funktioniert auch bei `classify`.

### Beispiel-Regeln
```toml
[[rules]]
name           = "rechnungen"
glob           = "Downloads/**"
category       = ["rechnung", "mahnung"]
min_confidence = 0.85                     # optional, Default aus [classify]
min_dwell      = "14d"                    # erst 14 Tage nach Ankunft
target         = "Finanzen/Rechnungen/{doc.date:%Y}/{doc.date:%Y-%m-%d}_{doc.sender|Unbekannt}_{doc.number|{name}}.{ext}"

[[rules]]
name            = "vertraege"
glob            = "Scans/**"
category        = "vertrag"
category_source = "rules"                 # nur Schlüsselwort-Treffer, keine LLM-Kategorie
target          = "Verträge/{doc.sender|Unbekannt}/{doc.date:%Y-%m-%d}_{doc.title|{name}}.{ext}"

[[rules]]
name   = "urlaubsfotos"
glob   = "Eingang/**"
ext    = ["jpg", "jpeg", "heic"]
fields = { "exif.has_gps" = "true" }
fields_regex = { "exif.country" = '^(?!DE$)' }  # nur Auslandsfotos
target = "Fotos/{exif.date:%Y}/{exif.country}-{exif.city}/{exif.date:%Y-%m-%d}_{name}.{ext}"

[[rules]]
name       = "telekom"
glob       = "Downloads/**"
ext        = ["pdf"]
text_regex = '(?i)telekom deutschland gmbh'
target     = "Finanzen/Telekom/"
```

## Kategorien

### Eingebauter Startsatz
`rechnung`, `mahnung`, `angebot`, `auftragsbestaetigung`, `quittung`, `vertrag`, `kontoauszug`, `gehaltsabrechnung`, `steuer`, `versicherung`, `medizin`, `bescheinigung`, `foto`, `screenshot`. Jede Kategorie hat DE/EN-Schlüsselwörter (mit Gewicht), Regex-Muster (z. B. IBAN, Rechnungsnummer, „Zahlbar bis“, Steuer-ID), Ausschlusswörter und die Endungen, für die sie gilt. `foto` und `screenshot` stützen sich auf Metadaten (Kamera-EXIF bzw. typische Bildschirmauflösung und Dateinamen wie `Screenshot 2026-…`) statt auf Text.

### Eigene Kategorien
Datei `categories.toml` neben der `config.toml` (Pfad über `[classify] categories_file` änderbar):
```toml
disable_builtin = ["medizin"]          # eingebaute Kategorien abschalten

[[categories]]
name             = "rechnung"          # gleicher Name ersetzt die eingebaute Kategorie vollständig
keywords         = { "rechnung" = 3, "invoice" = 3, "rechnungsnummer" = 4, "zahlbar bis" = 2, "ust-id" = 1 }
patterns         = { '(?i)rechnungs?-?nr\.?\s*[:#]?\s*[\w-]+' = 4 }
exclude_keywords = ["angebot", "kostenvoranschlag"]
ext              = ["pdf", "docx", "jpg", "png"]

[[categories]]
name     = "kita"
keywords = { "kita" = 3, "betreuungsvertrag" = 4, "elternbeitrag" = 3 }
ext      = ["pdf"]

[[senders]]                            # bekannte Absender für {doc.sender}
name  = "Telekom"
match = ["Telekom Deutschland GmbH", "telekom.de"]
```

### Zuordnung und Konfidenz
- Für jede Datei wird je Kategorie ein Punktwert aus gewichteten Schlüsselwort- und Mustertreffern berechnet; Ausschlusswörter setzen ihn auf 0. Konfidenz 0–1 ergibt sich aus der absoluten Stärke des besten Werts **und** dem Abstand zur zweitbesten Kategorie. Die genaue Formel legt der Implementierungsplan fest und dokumentiert sie im README.
- Eigenschaften: deterministisch (gleicher Inhalt + gleiche Definitionen = gleiche Kategorie und Konfidenz), erklärbar (Cache und Bericht nennen die ausschlaggebenden Treffer, z. B. „rechnungsnummer, zahlbar bis, IBAN“), monoton (ein zusätzlicher Treffer für Kategorie A senkt deren Punktwert nie).
- Trifft keine Kategorie sicher und ist das LLM eingeschaltet, schlägt das LLM eine Kategorie aus der Liste der bekannten Namen vor (oder `unbekannt`). Seine Konfidenz ist `min(LLM-Angabe, [llm] max_confidence)` (Default 0,85). Quelle wird als `llm` mit Modellname vermerkt.
- Ergebnis je Datei: Kategorie (oder keine), Konfidenz, zweitbeste Kategorie mit Konfidenz, Quelle (`rules` | `llm`), Treffer.

## Extrahierte Felder und Platzhalter

| Platzhalter / Feld | Quelle (Reihenfolge) |
|---|---|
| `{category}` | Zuordnung oben (nur wenn Konfidenz ≥ Schwelle der Regel, sonst Regel greift nicht) |
| `{doc.date[:fmt]}` | Datum aus dem Text (Muster wie „Rechnungsdatum“, „Datum:“, Briefkopf) → Dokument-Metadaten (PDF `CreationDate`, Office `dcterms:created`) → LLM |
| `{doc.sender}` | `[[senders]]`-Treffer → LLM |
| `{doc.number}` | Muster (Rechnungs-/Vertrags-/Kundennummer) → LLM |
| `{doc.amount}` | Muster (Gesamtbetrag, Endbetrag) → LLM; Format `123,45` |
| `{doc.title}` | LLM-Namensvorschlag → Metadaten-Titel, sofern kein Müll („Microsoft Word - Dokument1“, „Untitled“, Dateiname) |
| `{doc.author}`, `{doc.pages}` | Metadaten |
| `{exif.camera}` | EXIF `Make` + `Model` (bereinigt, z. B. „Apple iPhone 15“) |
| `{exif.lat}`, `{exif.lon}`, `exif.has_gps` | EXIF GPS |
| `{exif.city}`, `{exif.country}` | Offline-Auflösung der GPS-Koordinaten: nächste Stadt > 15.000 Einwohner aus eingebetteter GeoNames-Liste, Land als ISO-Code (`DE`, `PT`) |
| `{video.date[:fmt]}` | MP4/MOV `mvhd` creation_time (UTC → lokale Zeit) |

Regeln:
- **Ersatzwert:** `{feld|Ersatz}` setzt `Ersatz` ein, wenn das Feld fehlt; der Ersatz darf selbst ein Platzhalter sein (`{doc.number|{name}}`). Fehlt ein Feld ohne Ersatz, wird die Datei als `skipped: missing-field:<feld>` übersprungen.
- Eingesetzte Werte werden bereinigt: ungültige Windows-Zeichen ersetzt, Leerraum normalisiert, Länge begrenzt (`doc.title` 60, andere Felder 40 Zeichen), danach greift die Normalisierung aus Phase 4.
- Feldnamen in `fields` / `fields_regex` sind dieselben wie die Platzhalter (ohne Klammern); Vergleich bei `fields` exakt und ohne Groß-/Kleinschreibung.
- Unbekannte Platzhalter oder Felder sind ein Fehler beim Prüfen der Regeldatei (wie in Phase 4).

## Neue Regel-Bedingungen

| Bedingung | Bedeutung |
|---|---|
| `category` | Name oder Liste; passt, wenn die Datei eine dieser Kategorien mit Konfidenz ≥ `min_confidence` hat |
| `min_confidence` | Schwelle für `category`, Default `[classify] min_confidence` (0,8) |
| `category_source` | `any` (Default) oder `rules` (LLM-Kategorien zählen nicht) |
| `fields` / `fields_regex` | Tabelle Feld → Wert bzw. Regex |
| `text_regex` | Regex auf den extrahierten Text (Textlayer, Office-Text, OCR-Text) |
| `min_dwell` | Mindestzeit seit Ankunft im Ordner (`14d`, `2m` wie in der Config) |

- Auswertungsreihenfolge: erst die billigen Bedingungen aus Phase 4, dann die Bedingungen aus dem Cache, zuletzt `text_regex`.
- `text_regex` braucht den Volltext: `plan rules` extrahiert ihn für die verbleibenden Kandidaten neu (Textlayer und Office sind schnell). OCR-Text kommt aus dem verschlüsselten OCR-Cache, OCR läuft nur bei fehlendem oder veraltetem Eintrag. Der Text lebt nur während des Laufs im Speicher.
- **Ankunftszeit für `min_dwell`:** das spätere von (a) Erstelldatum der Datei (NTFS-Erstellzeit; bei Download und Kopie = Ankunft) und (b) `first_seen` im Index. `first_seen` wird gesetzt, wenn eine Datei in einem Scan einer Wurzel zum ersten Mal auftaucht; Dateien, die schon beim ersten Scan einer Wurzel da waren, haben kein `first_seen` (nur (a) zählt). Damit gelten im Ordner verschobene oder umbenannte Dateien konservativ als neu angekommen.

## Bericht: Abschnitt „Inhalte“
- Tabelle je Kategorie: Dateien, Größe, Anteil, davon per LLM.
- Quellen: Text, Metadaten, OCR, LLM; nicht lesbar (verschlüsselt, beschädigt, zu groß, nicht unterstützt); nicht analysiert (Cloud-only, nicht klassifiziert).
- „Zum Prüfen“: Dateien mit bester Konfidenz unter der Default-Schwelle, mit Top-2-Kategorien samt Konfidenz und ausschlaggebenden Treffern, sortiert nach Ordner. Begrenzt auf 200 Einträge im HTML, vollständig in CSV/JSON.
- Fotos: Verteilung nach Land/Stadt und Kamera (Top 10).
- Ohne gelaufenes `classify` erscheint der Abschnitt mit Hinweis „noch nicht klassifiziert“.

## Index (Erweiterung)
- Neue Tabelle für den Inhalts-Cache je Datei: Größe, mtime, Extraktor-Version, Status (`ok`, `unreadable:<grund>`, `unsupported`, `too-large`), Kategorie, Konfidenz, zweitbeste Kategorie mit Konfidenz, Quelle, Treffer, Felder (JSON), LLM-Modell (falls genutzt), Fingerabdruck der Kategorie-Definitionen, Zeitpunkt. **Kein Volltext.**
- Eigene Tabelle für den **OCR-Text-Cache** (Entscheidung nach Review, 2026-10-04): OCR kostet 1–2 s je Seite, und ohne Cache würde jede Änderung an `categories.toml` und jedes `text_regex` auf Scans erneut OCR auslösen. Gespeichert wird nur OCR-Text, verschlüsselt mit Windows-DPAPI (`CryptProtectData`, an das Benutzerkonto gebunden), mit Größe, mtime, OCR-Version und Sprachen. Gültig solange diese gleich sind. Abschaltbar mit `[classify] cache_ocr_text = false`. Text aus Textlayer und Office wird nicht gespeichert.
- Neue Spalte `first_seen` je Datei, die Re-Scans überlebt.
- Gültigkeit: solange Größe, mtime und Extraktor-Version gleich sind. Ändert sich der Fingerabdruck der Kategorie-Definitionen, muss neu zugeordnet werden; dazu wird der Text neu gelesen (Textlayer/Office direkt, OCR-Text aus dem OCR-Cache; die Zusammenfassung nennt das: „Kategorien geändert: N Dateien neu zugeordnet“). LLM-Ergebnisse werden nur bei geändertem Modell oder `--force` neu angefragt.
- `classify --clear [<pfad> | --profile X]` löscht Inhalts- und OCR-Text-Cache.
- Zum Abstimmen der Kategorien an einer Auswahl: `classify --only <glob>` (relativ zur Wurzel).

## Config (Erweiterung)
```toml
[classify]
categories_file = "categories.toml"   # relativ zum Config-Ordner
min_confidence  = 0.8
max_file_size   = "100MB"             # größere Dateien: skipped too-large
ocr             = true
ocr_languages   = ["de", "en"]        # Windows-OCR-Sprachpakete
ocr_max_pages   = 5                   # je PDF ohne Textlayer
cache_ocr_text  = true                # OCR-Text DPAPI-verschlüsselt cachen
max_text_chars  = 200000              # Obergrenze je Datei für Analyse
threads         = 0                   # 0 = halbe Kernanzahl

[llm]
enabled        = false
endpoint       = "http://127.0.0.1:11434"   # nur Loopback erlaubt
model          = "qwen2.5:7b"               # Beispiel; README empfiehlt Modelle
timeout        = "60s"
max_input_chars = 6000                       # Textauszug je Anfrage
tasks          = ["category", "fields", "title"]
max_confidence = 0.85
```
Profile können `[classify]`- und `[llm]`-Werte überschreiben (Phase-5-Mechanismus).

## Acceptance Criteria
- [ ] Given ein indizierter Ordner, when `classify` läuft, then ändert sich im Ordner kein Byte und keine mtime (nur lesender Zugriff)
- [ ] Given kein Index für den Pfad, when `classify` läuft, then bricht es mit Hinweis auf `scan` ab
- [ ] Given ein zweiter `classify` ohne Dateiänderungen, when er läuft, then wird keine Datei erneut gelesen und der Lauf dauert bei 10.000 Dateien wenige Sekunden
- [ ] Given eine geänderte `categories.toml`, when `classify` läuft, then werden die betroffenen Dateien neu zugeordnet und die Zusammenfassung nennt das; gescannte PDFs werden dabei **nicht** erneut per OCR gelesen (OCR-Text-Cache)
- [ ] Given der OCR-Text-Cache, when man die Index-Datei roh liest, then ist kein OCR-Text im Klartext enthalten; mit `cache_ocr_text = false` entsteht kein Eintrag
- [ ] Given der synthetische Testkorpus (generierte PDFs mit Textlayer, gescannte PDFs ohne Textlayer, docx/xlsx/pptx, JPEG/HEIC mit und ohne EXIF/GPS, MP4/MOV), when `classify --no-llm` läuft, then stimmen Kategorie und Felder mit der erwarteten Liste überein
- [ ] Given ein PDF ohne Textlayer und installiertes OCR-Sprachpaket, when `classify` läuft, then wird höchstens `ocr_max_pages` Seiten OCR gelesen und die Kategorie aus dem OCR-Text bestimmt
- [ ] Given ein Foto mit GPS-Koordinaten in Lissabon, when `classify` läuft, then sind `exif.city = Lisboa` (bzw. GeoNames-Name) und `exif.country = PT` gesetzt, ohne Netzwerkzugriff
- [ ] Given eine MP4-/MOV-Datei mit `mvhd`-Zeit, when eine Regel `{video.date:%Y}` nutzt, then wird das Aufnahmejahr eingesetzt
- [ ] Given eine Regel mit `category = "rechnung"` und eine Datei mit Konfidenz unter `min_confidence`, when `plan rules` läuft, then gibt es keine Aktion, die Datei steht als `skipped: low-confidence` im Plan und im Bericht unter „Zum Prüfen“ mit Top-2-Kategorien
- [ ] Given `category_source = "rules"` und eine nur per LLM zugeordnete Datei, when `plan rules` läuft, then greift die Regel nicht
- [ ] Given ein Ziel mit `{doc.sender|Unbekannt}` und eine Datei ohne Absender, when `plan rules` läuft, then wird `Unbekannt` eingesetzt; ohne Ersatzwert erscheint `skipped: missing-field:doc.sender`
- [ ] Given eine Regel mit unbekanntem Platzhalter, Feld oder Kategorienamen, when die Regeldatei geprüft wird, then bricht es vor der Planung mit Datei, Zeile und Regelname ab
- [ ] Given `min_dwell = "14d"` und eine vor 3 Tagen heruntergeladene Datei mit mtime von vor 2 Jahren, when `plan rules` läuft, then wird sie nicht geplant
- [ ] Given eine Datei, die schon beim ersten Scan der Wurzel vorhanden war und deren Erstellzeit älter als `min_dwell` ist, when `plan rules` läuft, then gilt `min_dwell` als erfüllt
- [ ] Given `text_regex`, when `plan rules` läuft, then wird Text nur für Kandidaten extrahiert, die alle anderen Bedingungen erfüllen, und nicht gespeichert
- [ ] Given eine Cloud-only-Datei, when `classify` oder `plan rules` läuft, then wird sie nicht gelesen (kein Download ausgelöst) und erscheint als `skipped: cloud-placeholder`
- [ ] Given ein verschlüsseltes oder beschädigtes PDF/Office-Dokument, when `classify` läuft, then gilt es als `unreadable` mit Grund, wird gecacht und der Lauf geht weiter
- [ ] Given `[llm] enabled = true` und Ollama nicht erreichbar, when `classify` läuft, then läuft die regelbasierte Klassifikation vollständig durch und die Zusammenfassung warnt einmal
- [ ] Given ein LLM-Endpoint, der nicht auf Loopback zeigt, when die Config geladen wird, then ist das ein Fehler mit Hinweis
- [ ] Given eine ungültige oder nicht schemakonforme LLM-Antwort, when `classify` läuft, then wird sie verworfen und die Datei behält das regelbasierte Ergebnis
- [ ] Given das LLM schlägt eine Kategorie außerhalb der bekannten Namen vor, when das Ergebnis verarbeitet wird, then wird es als `unbekannt` behandelt
- [ ] Given ein Profil mit `classify = true`, when `run --profile X` läuft, then läuft `classify` zwischen `scan` und Bericht, und die Benachrichtigung nennt neue Dateien „zum Prüfen“
- [ ] Given `plan rules` → `apply` → `scan` → `classify` → `plan rules` mit denselben Regeln, when der zweite Plan entsteht, then enthält er keine Aktionen (Idempotenz)
- [ ] Given `classify --clear`, when es läuft, then ist der Inhalts-Cache der Wurzel leer und der nächste `classify` analysiert alles neu
- [ ] Given ein Plan aus Phase 2–5, when `apply`/`undo` laufen, then funktionieren sie unverändert
- [ ] Manueller Test auf dem echten Downloads-/Scans-Ordner (einmal ohne, einmal mit LLM): ≥ 90 % korrekte Kategorie bei Stichprobe, 0 falsche Aktionen im Plan, Ordner nach `apply` bis auf „Zum Prüfen“ leer, `undo` stellt den Ausgangszustand her; dokumentiert in dieser Spec

## Edge Cases & Error States
| Scenario | Expected Behavior |
|----------|-------------------|
| Cloud-only-Platzhalter (`RECALL_ON_DATA_ACCESS`/`OFFLINE`) | Nie öffnen, auch nicht für Metadaten; `skipped: cloud-placeholder`, nicht gecacht |
| Verschlüsseltes PDF / passwortgeschütztes Office-Dokument | `unreadable: encrypted`, gecacht; nur unverschlüsselte Metadaten werden genutzt, falls lesbar |
| Beschädigte oder abgeschnittene Datei, Parser-Panic | `unreadable: corrupt`, gecacht; Panic wird je Datei abgefangen, der Lauf geht weiter |
| Datei größer als `max_file_size` | `too-large`, gecacht; EXIF/`mvhd` werden trotzdem gelesen (nur Kopfbereich) |
| Riesiger Text (z. B. 2.000-Seiten-PDF) | Analyse auf `max_text_chars` begrenzt |
| PDF ohne Textlayer, OCR abgeschaltet | Kategorie nur aus Metadaten/Dateiname; meist „Zum Prüfen“ |
| OCR-Sprachpaket fehlt | Einmaliger Hinweis mit Anleitung (Windows-Einstellungen → Sprache), betroffene Dateien `unreadable: no-ocr-language`, nicht dauerhaft gecacht |
| PDF mit Mischung aus Text- und Bildseiten | Textlayer genügt, wenn er eine Mindestmenge Text liefert; sonst OCR |
| Datei ist gesperrt (offen in Word) | `unreadable: locked`, nicht gecacht, nächster Lauf versucht es erneut |
| Datei ändert sich während der Analyse | Größe/mtime nach dem Lesen erneut prüfen; bei Abweichung verwerfen, `stale` |
| Zwei Kategorien gleich stark | Konfidenz niedrig (Abstand 0) → „Zum Prüfen“ |
| Text in anderer Sprache als DE/EN | Schlüsselwörter greifen selten → „Zum Prüfen“ oder LLM |
| GPS-Koordinaten 0/0 oder ungültig | Gilt als „kein GPS“ |
| GPS auf offener See / fern jeder Stadt | Nächste Stadt nur innerhalb von 50 km, sonst nur `exif.country` (falls bestimmbar) bzw. Feld fehlt |
| HEIC ohne lesbares EXIF | Wie „kein EXIF“ in Phase 4 |
| Video ohne `mvhd`-Zeit oder Zeit 0 (1904-01-01) | Feld fehlt |
| Ollama nicht erreichbar / Modell nicht installiert / Timeout | Warnung (einmal je Lauf), Fortsetzung ohne LLM; nicht als Fehler gecacht |
| LLM antwortet mit ungültigem JSON, unbekannter Kategorie oder unplausiblem Datum | Verworfen bzw. `unbekannt`; Feld fehlt |
| LLM-Titel enthält ungültige Zeichen oder ist zu lang | Bereinigung und Kürzung wie bei allen Feldern |
| `categories.toml` fehlerhaft (Syntax, ungültige Regex, doppelter Name) | Fehler mit Datei und Zeile vor jeder Analyse |
| Eigene Kategorie gleichen Namens wie eingebaute | Ersetzt die eingebaute vollständig (kein Zusammenführen) |
| Regel nutzt `category` mit Name, der nirgends definiert ist | Fehler beim Prüfen der Regeldatei |
| `classify` parallel zu `scan` derselben Wurzel | Scan-Sperre aus Phase 5 gilt auch für `classify`; der zweite Befehl bricht mit Hinweis ab |
| Abbruch mit Ctrl+C | Bisherige Ergebnisse bleiben im Cache, Index konsistent, Exit-Code wie bei abgebrochenem Scan |
| Index wurde nach `classify` neu gescannt, Datei unverändert | Cache bleibt gültig (Schlüssel Pfad + Größe + mtime) |
| Datei wurde verschoben/umbenannt (gleicher Inhalt) | Neuer Pfad → neu analysiert (kein Inhalts-Hash als Schlüssel in 6a) |

## Technical Constraints
- **Performance:** Textlayer/Office/EXIF/Video zielen auf ≥ 20 Dateien/s auf SSD; OCR ca. 1–2 s je Seite. Begrenzte Parallelität (`threads`, Default halbe Kernanzahl), Prozesspriorität „unter normal“ während `classify`, damit der Rechner bedienbar bleibt. Unveränderte Dateien kosten nur einen Cache-Abgleich. Nur der nötige Teil einer Datei wird gelesen (EXIF-/`mvhd`-Kopf, Office-ZIP-Einträge).
- **Security / Datenschutz:**
  - Kein Netzwerkzugriff außer zum konfigurierten LLM-Endpoint, und der muss auf Loopback zeigen (`127.0.0.1`, `::1`, `localhost`); andere Hosts sind ein Config-Fehler. Keine Telemetrie, keine Online-Geocodierung.
  - Kein unverschlüsselter Text auf Platte; der Inhalts-Cache enthält nur Felder (Datum, Absender, Betrag, Nummer, Titel, Ort). OCR-Text liegt nur DPAPI-verschlüsselt im OCR-Cache (nur das eigene Benutzerkonto kann ihn entschlüsseln; abschaltbar). Das README weist darauf hin, dass Felder und OCR-Cache sensibel sein können, und nennt `classify --clear`.
  - An das LLM geht nur ein gekürzter Textauszug (`max_input_chars`) plus Dateiname; Antworten werden gegen ein JSON-Schema geprüft und nie als Befehl interpretiert (Schutz gegen Prompt-Injection aus Dokumentinhalt: das LLM kann höchstens Kategorie/Felder/Titel liefern, die dieselben Prüfungen durchlaufen wie alle anderen Werte).
  - Parser laufen gegen nicht vertrauenswürdige Dateien: Panics je Datei abfangen, Größen- und Tiefengrenzen beim Entpacken von Office-ZIPs (Zip-Bomben), kein Ausführen von Makros oder eingebetteten Inhalten.
- **Compliance:** GeoNames-Daten stehen unter CC BY 4.0 → Namensnennung im README und in `--version`/About-Ausgabe. Lizenzen aller neuen Crates müssen mit MIT/Apache-2.0 verträglich sein.
- **Plattform:** OCR über `Windows.Media.Ocr`, PDF-Seitenrendering für OCR über `Windows.Data.Pdf` (WinRT, ab Windows 10, keine Zusatzinstallation). Text- und Metadaten-Extraktion in reinem Rust, ohne externe Programme.
- **Kompatibilität:** Plan-Format bleibt `version: 2` (neue `skipped`-Gründe sind Strings). Ältere Indizes werden per Schema-Migration erweitert.

## Dependencies
- Phase 1: Index, Scan, Cloud-only-Erkennung, Bericht (HTML/CSV/JSON)
- Phase 2/3: Plan, Apply, Undo, geschützte Pfade, OneDrive-Obergrenzen
- Phase 4: Regel-Engine (Bedingungen, Platzhalter, Normalisierung, EXIF-Cache)
- Phase 5: Profile, `run`, Scan-Sperre, Benachrichtigung (umgesetzt, manueller Test offen)
- Windows-APIs: `Windows.Media.Ocr`, `Windows.Data.Pdf` (WinRT), OCR-Sprachpakete DE/EN
- Optional: Ollama (lokal installiert, vom Nutzer betrieben)
- Daten: GeoNames `cities15000` (eingebettet, ca. 1–2 MB komprimiert)
- Neue Crates (Auswahl im Implementierungsplan): PDF-Text/Metadaten, ZIP/XML für Office, HTTP-Client für Loopback, WinRT-Bindings

## Open Questions
- [ ] Welche PDF-Bibliothek liefert in reinem Rust zuverlässig Text aus realen Rechnungen/Scans (Spike im Implementierungsplan, Vergleich an 20 echten PDFs)?
- [ ] Reicht die Qualität der Windows-OCR für deutsche Scans mit kleinem Druck, oder braucht es Vorverarbeitung (Rendering-Auflösung, Graustufen)?
- [ ] Welches lokale Modell empfiehlt das README als Default (Größe vs. Qualität auf typischer Hardware ohne GPU)?
- [ ] Exakte Formel für die Konfidenz und Kalibrierung der Default-Schwelle 0,8 am Testkorpus
- [ ] Soll ein Inhalts-Hash (aus Phase 1, falls vorhanden) als zweiter Cache-Schlüssel dienen, damit verschobene Dateien nicht neu analysiert werden? (In 6a: nein, ggf. später)
