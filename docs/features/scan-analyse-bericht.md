---
title: "Scan & Analyse-Bericht (Phase 1)"
status: approved       # draft | approved | implemented
created: 2026-10-02
updated: 2026-10-02
---

# Feature: Scan & Analyse-Bericht (Phase 1)

## Problem Statement
Auf dem eigenen Windows-PC fehlt der Überblick: wo der Speicherplatz verbraucht wird, welche Dateien doppelt oder veraltet sind und welche Dateien Probleme verursachen (lange Pfade, ungültige Zeichen, Sync-Konflikte). Ohne diese Analyse ist gezieltes Aufräumen nicht möglich.

Betroffen ist zunächst nur der Entwickler selbst. Später sollen Kollegen das Tool nutzen, daher muss es als einzelne `.exe` verteilbar sein.

Phase 1 ist die **rein lesende** Grundlage für alle späteren Phasen (Aufräumen, Neuorganisation, siehe [`../roadmap.md`](../roadmap.md)). Die Gesamtziele des Projekts sind **Platz sparen** und **Ordnung schaffen**.

## Goals
- [ ] `scan` erfasst einen Ordnerbaum in einem lokalen SQLite-Index, inkrementell bei wiederholten Läufen
- [ ] `report` erzeugt aus dem Index eine Terminal-Zusammenfassung, eine HTML-Datei ohne externe Abhängigkeiten sowie JSON- und CSV-Exporte
- [ ] Der Bericht beantwortet: Wo ist der Platz hin? Was ist doppelt? Was ist alt? Was ist kaputt oder problematisch?
- [ ] Es wird kein einziges Byte im gescannten Bereich verändert, und OneDrive-Cloud-Dateien werden nie heruntergeladen
- [ ] Performance-Ziele (siehe Technical Constraints) werden eingehalten

## Out of Scope
- Alle schreibenden Aktionen: Löschen, Verschieben, Umbenennen, Undo (ab Phase 2/3)
- Regel-Engine, Watch-Mode, geplante Läufe
- Inhaltsähnlichkeit (Fuzzy-Hashing, Bild-pHash), Inhaltsanalyse, KI-Klassifikation
- Duplikate ganzer Ordner
- Health-Score
- GUI (es gibt nur CLI und HTML-Bericht)
- Netzlaufwerke (SMB) und SharePoint-spezifische Behandlung
- Anonymisierung von Berichten
- Linux/macOS
- Herunterladen von OneDrive-Platzhaltern (kein `--hydrate`)

## User Journey
1. Der Nutzer startet `ordner-cleanup scan "C:\Users\<user>\Dokumente"`.
2. Das System läuft den Baum ab und zeigt dabei einen Fortschritt (Dateien/Ordner, Bytes, aktueller Pfad, Fehlerzähler).
   - Ausgeschlossene Pfade werden nicht betreten.
   - Pfade mit „Nur Summe“ werden nur aufsummiert.
   - Symlinks/Junctions werden erfasst, aber nicht betreten.
   - OneDrive-Cloud-Platzhalter werden nur über Metadaten erfasst.
3. Das System schreibt den Index nach `%LOCALAPPDATA%\ordner-cleanup\index.db`. Bei einem erneuten Scan desselben Pfads werden Dateien mit unveränderter Größe und unverändertem Änderungsdatum nicht neu verarbeitet, und gelöschte Dateien werden aus dem Index entfernt.
4. Das System ermittelt Duplikat-Kandidaten in drei Stufen: Gruppierung nach Größe, dann Teil-Hash (Anfang und Ende der Datei), dann Voll-Hash. Hashes werden im Index gecacht.
5. Am Ende des Scans gibt das System eine kurze Zusammenfassung aus (Dauer, Anzahl, Gesamtgröße, Fehler).
6. Der Nutzer startet `ordner-cleanup report [<pfad>] [--out <ordner>] [--format html,json,csv]`.
7. Das System gibt eine Terminal-Zusammenfassung mit den wichtigsten Kennzahlen aus, schreibt die Berichtsdateien in den Zielordner (Default: aktuelles Verzeichnis, Unterordner `ordner-cleanup-report-<timestamp>`) und nennt die Pfade.
8. Der Nutzer öffnet die HTML-Datei im Browser. Sie enthält sortierbare und filterbare Tabellen sowie einen aufklappbaren Größenbaum.

### Bericht: Inhalte
| Abschnitt | Inhalt |
|---|---|
| Übersicht | Gescannte Wurzel, Scan-Zeitpunkt, Anzahl Dateien/Ordner, Gesamtgröße, Anteil „Nur Summe“, Anzahl Cloud-Platzhalter, Fehleranzahl |
| Größenbaum | Aufklappbarer Ordnerbaum mit kumulierter Größe und Dateianzahl, sortiert nach Größe |
| Top-Listen | Größte Dateien, größte Ordner (Top N, Default 50) |
| Dateitypen | Anzahl und Größe pro Endung und pro Kategorie (Dokumente, Bilder, Video, Audio, Archive, Code, Ausführbar, Sonstige) |
| Alter | Altersklassen nach Änderungsdatum: < 1 Monat, < 6 Monate, < 1 Jahr, < 3 Jahre, > 3 Jahre. Liste „alter“ Dateien (Default > 1 Jahr, per `--old-after` änderbar) |
| Exakte Duplikate | Gruppen identischer Dateien (Voll-Hash) mit Pfaden, Größe und verschwendetem Platz (Größe × (Anzahl − 1)), sortiert nach Verschwendung |
| Wahrscheinliche Duplikate | Gruppen mit Cloud-Platzhalter-Beteiligung: gleicher Name (case-insensitiv) und gleiche Größe, deutlich als „nicht verifiziert“ markiert |
| Ähnliche Dateien / Versionen | Gruppen im selben Ordner, deren normalisierter Name übereinstimmt (siehe Namensheuristik) |
| Struktur | Leere Ordner, Ordner mit nur einer Datei, Verschachtelung über Schwellwert (Default Tiefe > 8 relativ zur Wurzel), Riesenordner (Default > 1000 direkte Einträge) |
| Problemdateien | Pfade > 260 Zeichen; für OneDrive/SharePoint ungültige Zeichen oder Namen (`" * : < > ? / \ \|`, reservierte Namen wie `CON`, `NUL`, `COM1`, führende/abschließende Leerzeichen, abschließender Punkt); 0-Byte-Dateien; Office-Lockdateien `~$*`; Temp-Dateien (`*.tmp`, `~*.tmp`); abgebrochene Downloads (`*.crdownload`, `*.part`, `*.partial`); OneDrive-Konfliktdateien (`<name>-<PCNAME>.<ext>`, `<name>-<PCNAME>-<n>.<ext>`); Systemmüll (`Thumbs.db`, `desktop.ini`, `.DS_Store`) |
| Fehler | Liste aller Pfade, die nicht gelesen werden konnten, mit Grund |

### Namensheuristik „ähnliche Dateien“
Der Dateiname ohne Endung wird normalisiert, danach werden Dateien mit gleichem normalisiertem Namen und gleicher Endung **im selben Ordner** gruppiert. Normalisierung:
- Kleinschreibung, Trimmen, `_`, `-` und Leerzeichen vereinheitlichen
- Entfernen von Kopie-Suffixen: `- Kopie`, `- Kopie (n)`, `- Copy`, `Copy of`, `Kopie von`, ` (n)`
- Entfernen von Versionsmarkern: `v<n>`, `version <n>`, `rev<n>`, `final`, `endgültig`, `neu`, `new`, `alt`, `old`
- Entfernen von Datumsangaben am Anfang oder Ende (`YYYY-MM-DD`, `YYYYMMDD`, `DD.MM.YYYY`)
- Eine Gruppe muss mindestens zwei Dateien enthalten. Exakte Duplikate werden zusätzlich als solche gekennzeichnet.

## Acceptance Criteria
- [ ] Given ein Ordnerbaum, when `scan` und danach `report` laufen, then stimmen Dateianzahl und Gesamtgröße mit einer unabhängigen Zählung überein (ohne ausgeschlossene Pfade)
- [ ] Given ein beliebiger Scan, when er läuft, then werden keine Dateien oder Ordner im gescannten Bereich erstellt, geändert, gelöscht oder in Attributen/Zeitstempeln verändert. Der Last-Access-Zeitstempel ist davon ausgenommen, soweit das OS ihn setzt.
- [ ] Given eine OneDrive-Datei, die nur in der Cloud liegt (Attribut `RECALL_ON_DATA_ACCESS` oder `RECALL_ON_OPEN` oder `OFFLINE`), when gescannt wird, then wird sie mit Metadaten erfasst, nie geöffnet, nie gehasht und bleibt cloud-only
- [ ] Given zwei Dateien mit identischem Inhalt an verschiedenen Orten, when gescannt wird, then erscheinen sie im Bericht als eine exakte Duplikatgruppe mit korrekt berechnetem verschwendetem Platz
- [ ] Given zwei Dateien gleicher Größe mit unterschiedlichem Inhalt, when gescannt wird, then erscheinen sie nicht als Duplikate
- [ ] Given Dateien, deren Größe einzigartig ist, when gescannt wird, then werden sie nicht gehasht
- [ ] Given ein zweiter Scan ohne Änderungen, when er läuft, then wird keine Datei neu gehasht, und die Laufzeit erfüllt das Re-Scan-Ziel
- [ ] Given eine Datei wurde zwischen zwei Scans geändert, gelöscht oder hinzugefügt, when erneut gescannt wird, then spiegelt der Index den neuen Zustand wider, und veraltete Hashes werden verworfen
- [ ] Given ein Symlink oder eine Junction, when gescannt wird, then erscheint er als Link-Eintrag (mit Ziel), wird aber nicht betreten. Die Größe des Ziels wird nicht mitgezählt.
- [ ] Given ein Pfad unter einem Default-Ausschluss, when gescannt wird, then wird er nicht betreten. Mit `--no-default-excludes` wird er gescannt.
- [ ] Given ein Pfad unter einem „Nur Summe“-Default (z. B. `node_modules`), when gescannt wird, then erscheinen im Bericht nur Gesamtgröße und Dateianzahl des Ordners, ohne Einzeldateien, Duplikate oder Problemprüfungen darin
- [ ] Given versteckte oder System-Dateien, when gescannt wird, then werden sie mitgezählt und im Bericht als versteckt bzw. System markiert
- [ ] Given ein Pfad länger als 260 Zeichen, when gescannt wird, then wird er korrekt gelesen (Extended-Length-Präfix `\\?\`) und als Problemdatei gelistet
- [ ] Given ein Ordner ohne Leserechte, when gescannt wird, then bricht der Scan nicht ab, und der Pfad erscheint mit Grund im Abschnitt Fehler
- [ ] Given Dateien wie `Angebot.docx`, `Angebot_v2.docx` und `Angebot - Kopie (3).docx` im selben Ordner, when der Bericht erzeugt wird, then bilden sie eine Gruppe „ähnliche Dateien“
- [ ] Given ein Scan, when `report --format html,json,csv` läuft, then entstehen eine einzelne HTML-Datei ohne Netzwerkreferenzen, eine JSON-Datei mit allen Abschnitten und je eine CSV-Datei pro Liste (Duplikate, ähnliche Dateien, Problemdateien, alte Dateien, größte Dateien, leere Ordner, Fehler)
- [ ] Given der Bericht, when er geöffnet wird, then enthält er keine Dateiinhalte, sondern nur Metadaten und Hashes
- [ ] Given das Tool läuft, when man den Netzwerkverkehr beobachtet, then gibt es keine ausgehenden Verbindungen
- [ ] Given `Strg+C` während des Scans, when der Abbruch erfolgt, then bleibt der Index konsistent (Transaktionen), und der nächste Scan setzt sauber auf

## Edge Cases & Error States
| Scenario | Expected Behavior |
|----------|-------------------|
| Zugriff verweigert (Ordner oder Datei) | Überspringen, als Fehler protokollieren, Scan läuft weiter |
| Datei ist gesperrt (z. B. offen in Office) beim Hashen | Metadaten erfassen, Hash als „nicht ermittelbar“ markieren, Fehler protokollieren; die Datei zählt nicht als verifiziertes Duplikat |
| Datei verschwindet oder ändert sich während des Scans | Eintrag verwerfen bzw. Hash verwerfen, wenn sich Größe oder Änderungsdatum zwischen Lesen und Hashen geändert haben; als Warnung protokollieren |
| OneDrive-Cloud-Platzhalter | Nur Metadaten; nie öffnen; nur in „wahrscheinliche Duplikate“ |
| OneDrive „Immer auf diesem Gerät behalten“ / lokal verfügbar | Wie normale Datei behandeln (hashen erlaubt) |
| Symlink/Junction-Zyklus | Kann nicht entstehen, da Links nicht betreten werden |
| Hardlinks (mehrere Pfade, gleiche Datei-ID) | Als Hardlink-Gruppe erkennen (Volume-Seriennummer + File-ID), Größe nur einmal zählen, nicht als Duplikat melden |
| Pfad > 260 Zeichen | Mit `\\?\`-Präfix lesen, als Problemdatei melden |
| Ungültiges UTF-16 im Dateinamen | Verlustfrei speichern (z. B. WTF-8/OsString), in der Anzeige mit Ersatzzeichen darstellen |
| Wurzelpfad existiert nicht oder ist eine Datei | Klare Fehlermeldung, Exit-Code ≠ 0 |
| Wurzelpfad ist ein Netzlaufwerk/UNC | Warnung „nicht unterstützt in Phase 1“; mit `--force` dennoch scannen |
| Gescannter Pfad liegt in einem ausgeschlossenen Bereich | Hinweis ausgeben; der explizit angegebene Wurzelpfad hat Vorrang vor Default-Ausschlüssen |
| Index-Datei beschädigt oder Schema veraltet | Klare Meldung, Angebot `--reset-index`; bei Schema-Version-Mismatch automatisch migrieren oder neu aufbauen |
| `report` ohne vorherigen Scan | Meldung „Kein Scan für <pfad> vorhanden – zuerst `scan` ausführen“, Exit-Code ≠ 0 |
| Mehrere gescannte Wurzeln im Index | `report` ohne Pfad listet die vorhandenen Wurzeln und verlangt eine Auswahl, wenn es mehr als eine gibt |
| Verschachtelte Wurzeln (zweiter Scan auf einem Unterordner) | Einträge werden nicht doppelt gespeichert; der Bericht bezieht sich auf die angefragte Wurzel |
| Zielordner für Bericht nicht beschreibbar | Klare Fehlermeldung, Exit-Code ≠ 0 |
| Sehr große Dateien (> 4 GB) | Korrekt behandeln (64-bit Größen), Teil-Hash begrenzt den Lese-Aufwand |
| Strg+C | Laufende Transaktion abschließen oder zurückrollen, Index konsistent lassen |

## Default-Ausschlüsse
Alle Regeln sind über die Config sowie über `--exclude <glob>`, `--summary-only <glob>` und `--no-default-excludes` überschreibbar.

| Stufe | Pfade/Muster |
|---|---|
| **Ausgeschlossen** (nicht betreten) | `C:\Windows`, `C:\Program Files`, `C:\Program Files (x86)`, `C:\ProgramData`, `$Recycle.Bin`, `System Volume Information`, `pagefile.sys`, `hiberfil.sys`, `swapfile.sys`, eigener Index-/Datenordner `%LOCALAPPDATA%\ordner-cleanup`, eigene Berichtsordner `ordner-cleanup-report-*` |
| **Nur Summe** (Größe + Anzahl, keine Details) | `.git`, `node_modules`, `target`, `.venv`, `venv`, `__pycache__`, `.gradle`, `bin`/`obj` (nur wenn im übergeordneten Ordner eine `*.csproj`/`*.sln` liegt), `%USERPROFILE%\AppData` |
| **Voll** | alles andere |

## CLI (Entwurf)
```
ordner-cleanup scan <pfad> [--exclude <glob>]... [--summary-only <glob>]... [--no-default-excludes] [--force] [--reset-index] [--threads <n>]
ordner-cleanup report [<pfad>] [--out <ordner>] [--format html,json,csv] [--old-after <dauer, z.B. 1y, 18m>] [--top <n>]
ordner-cleanup index list | index remove <pfad>
```
- Konfiguration: `%APPDATA%\ordner-cleanup\config.toml`; CLI-Flags haben Vorrang
- Index: `%LOCALAPPDATA%\ordner-cleanup\index.db` (SQLite)
- Exit-Codes: `0` OK, `1` Fehler, `2` OK mit Teilfehlern (z. B. Zugriff verweigert)

## Technical Constraints
- **Sprache/Plattform:** Rust; nur Windows 10/11 (NTFS). Plattformnahe Teile (Attribute, File-ID, Long Paths) hinter einer Abstraktion, damit die Logik ohne echtes Dateisystem testbar bleibt.
- **Verteilung:** eine einzelne statisch gelinkte `.exe`, ohne Runtime-Abhängigkeiten (SQLite eingebettet)
- **Performance (SSD, Richtwert):** Erstscan von 100k Dateien inklusive Hashing der Duplikat-Kandidaten < 2 min; Re-Scan ohne Änderungen < 15 s; Speicherbedarf < 500 MB
- **Hashing:** schneller nicht-kryptografischer Hash (z. B. xxHash3-128 oder BLAKE3); Teil-Hash über Anfang und Ende der Datei (je 64 KiB) plus Größe
- **Nur lesend:** Dateien werden ausschließlich mit Lesezugriff geöffnet, mit Share-Modus Read/Write/Delete, damit andere Programme nicht blockiert werden. Geschrieben wird nur in Index- und Berichtsordner.
- **Security/Datenschutz:** rein lokal; keine Netzwerkzugriffe, keine Telemetrie; Berichte enthalten Pfade und Metadaten, aber keine Dateiinhalte; die HTML-Datei lädt keine externen Ressourcen
- **Compliance:** Berichte können vertrauliche Datei- und Pfadnamen enthalten. Die Weitergabe liegt in der Verantwortung des Nutzers; ein Hinweis steht im Bericht.

## Dependencies
- Windows-API: Dateiattribute (Hidden, System, ReparsePoint, Offline, RecallOnOpen, RecallOnDataAccess), File-ID/Volume-Serial, Extended-Length-Pfade
- Eingebettetes SQLite
- Keine externen Dienste
- Grundlage für Phase 2/3: Index-Schema und JSON-Export sollen später von Plan/Apply wiederverwendet werden

## Open Questions
- [ ] Konkrete Crate-Auswahl (CLI-Parser, Walker, SQLite-Binding, Hash, HTML-Templating) wird im Implementierungsplan entschieden
- [ ] Muster für OneDrive-Konfliktdateien gegen reale Beispiele verifizieren (der Rechnername als Suffix kann variieren)
- [ ] Schwellwerte (Tiefe > 8, Riesenordner > 1000 Einträge, Top 50) nach ersten echten Scans ggf. anpassen
