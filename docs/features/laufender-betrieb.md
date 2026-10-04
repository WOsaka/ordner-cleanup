---
title: "Laufender Betrieb: Verlauf, Health-Score, geplante Läufe, Profile, Vorlagen (Phase 5)"
status: implemented    # draft | approved | implemented
created: 2026-10-04
updated: 2026-10-04
---

# Feature: Laufender Betrieb (Phase 5)

## Problem Statement
Jeder `scan` ist bisher eine Momentaufnahme: Der Index überschreibt den alten Stand. Ob ein Ordner wächst, vermüllt oder sich nach dem Aufräumen wieder füllt, merkt man erst, wenn es zu spät ist. Jeder Lauf muss von Hand mit Pfad und Optionen gestartet werden, und ob eine Ordnerstruktur noch einer gewollten Zielstruktur entspricht, ist nirgends sichtbar.

Betroffen ist zunächst der Entwickler selbst, später Kollegen. Hauptziel von Phase 5 ist der **Überblick über die Zeit**: regelmäßige, automatische Läufe mit Verlauf und einer verständlichen Kennzahl, ohne das Grundprinzip „Plan → Review → Apply“ aufzuweichen (siehe [`../roadmap.md`](../roadmap.md)).

## Goals
- [ ] Jeder vollständige `scan` schreibt eine Momentaufnahme der Kennzahlen in einen dauerhaften Verlauf (`history.db`)
- [ ] Health-Score 0–100 je Wurzel und je Ordner der ersten Ebene, mit erklärten Abzügen
- [ ] `report` zeigt Veränderung zum letzten Lauf und Trends; `history` zeigt den Verlauf im Terminal
- [ ] Ein Befehl richtet einen geplanten Lauf ein (`schedule add`); danach laufen Scan, Bericht und optional Pläne ohne Eingriff
- [ ] Eine Windows-Benachrichtigung meldet relevante Veränderungen, ein Klick öffnet den Bericht
- [ ] Profile bündeln Wurzel, Regeldatei, Pläne, Vorlage und Config-Abweichungen; `--profile` ersetzt Pfad und Optionen
- [ ] Vorlagen (PARA, Johnny.Decimal, eigene) beschreiben eine Zielstruktur; der Soll/Ist-Abgleich erscheint in Bericht und Score

## Out of Scope
- Watch-Mode (Dauerprozess mit Dateisystem-Überwachung)
- Jede Form von Auto-Apply: geplante Läufe erzeugen höchstens **Pläne**, `apply` bleibt manuell
- Umziehen in eine Vorlage oder Anlegen fehlender Vorlagen-Ordner (Abgleich nur anzeigen; Verschieben über die Regeln aus Phase 4)
- Versand von Berichten (E-Mail, Cloud), Dashboards, GUI
- Inhaltsanalyse, KI-Klassifikation (Phase 6)
- Automatisches Löschen von Verlaufsdaten
- Mehrere Benutzer bzw. zentrale Auswertung über mehrere Rechner
- Netzlaufwerke (SMB), Linux/macOS

## User Journey

### Verlauf und Health-Score
1. Der Nutzer scannt wie gewohnt (`scan <pfad>` bzw. `scan --profile X`).
2. Am Ende eines **vollständigen** Scans berechnet das System die Kennzahlen aus dem Index und schreibt eine Momentaufnahme in `history.db`, je Wurzel und je Ordner der ersten Ebene. Die Scan-Ausgabe nennt den Health-Score und die Veränderung zum letzten Lauf („Health-Score 72 (−5 seit 27.09.)“).
3. `report` enthält einen neuen Abschnitt „Verlauf“: Tabelle „jetzt / letzter Lauf / Veränderung“ und je Kennzahl eine kleine Trendgrafik (inline-SVG), dazu Score mit den drei größten Abzügen.
4. `history [<pfad> | --profile X] [--limit N]` zeigt den Verlauf im Terminal (eine Zeile je Momentaufnahme).

### Geplante Läufe
1. Der Nutzer legt ein Profil an (siehe unten) und richtet einen Lauf ein:
   `ordner-cleanup schedule add --profile downloads --weekly MO 09:00` (alternativ `--daily 09:00`).
2. Das System legt in der Windows-Aufgabenplanung eine Aufgabe für den aktuellen Benutzer an (keine Adminrechte, kein gespeichertes Passwort, nur bei angemeldetem Benutzer, nur am Netzteil, Priorität „unter normal“, verpasste Läufe werden nachgeholt).
3. Zum Zeitpunkt startet die Aufgabe `ordner-cleanup run --profile downloads`: `scan` → Momentaufnahme → `report` → die im Profil genannten `plan …`-Befehle. **Nie `apply`.**
4. Der Bericht liegt unter `<data>\reports\<profil>\<JJJJMMTT-HHMMSS>.html`, zusätzlich als `latest.html`; Pläne unter `<data>\plans\<profil>\`. Ältere Berichte über der Aufbewahrungsgrenze (Default 12) werden entfernt.
5. Bei relevanter Veränderung erscheint eine Windows-Benachrichtigung, z. B. „Downloads: Health-Score 58 (−7), 2 Pläne bereit“. Ein Klick öffnet `latest.html`. Pläne prüft und führt der Nutzer wie gewohnt mit `apply` aus.
6. `schedule list` zeigt Aufgaben mit nächstem/letztem Lauf und Ergebnis; `schedule remove --profile X` entfernt sie.

### Profile
1. In der `config.toml`:
   ```toml
   [profiles.downloads]
   root = 'C:\Users\Oskar\Downloads'
   rules_file = 'downloads.rules.toml'   # optional, relativ zum Config-Ordner
   plans = ["rules", "junk"]             # optional: was `run` zusätzlich plant
   template = "para"                     # optional: eingebaut oder Pfad zu einer Vorlage
   exclude = ["*.iso"]                   # optional: Abweichungen von der globalen Config
   ```
2. `--profile <name>` funktioniert bei `scan`, `report`, `plan …`, `run` und `history` und ersetzt Pfad und die im Profil gesetzten Optionen. Explizite CLI-Angaben haben Vorrang vor dem Profil, das Profil vor der globalen Config.
3. `profiles` listet alle Profile mit Wurzel, letztem Lauf und Score.

### Vorlagen
1. Eingebaut: `para` (1 Projekte, 2 Bereiche, 3 Ressourcen, 4 Archiv) und `johnny-decimal` (Bereiche `10-19 …`, Kategorien `11 …`, IDs `11.01 …`). Eigene Vorlagen als TOML-Datei.
2. Eine Vorlage beschreibt je Ebene: Pflicht-Ordner, erlaubte Ordner (Namen oder Regex), ob Dateien auf dieser Ebene erlaubt sind, und eine maximale Tiefe. `_Archiv` und `.ordner-cleanup` sind immer erlaubt.
3. Ist für eine Wurzel (über das Profil oder `--template`) eine Vorlage gesetzt, zeigt `report` den Abschnitt „Soll/Ist“: fehlende Pflicht-Ordner, unerwartete Ordner, Dateien auf nicht vorgesehenen Ebenen, verletzte Namensmuster, zu tiefe Verschachtelung. Der Teilwert „Strukturtreue“ geht in den Health-Score ein.

## Kennzahlen und Health-Score

### Kennzahlen je Momentaufnahme (Wurzel und jeder Ordner der ersten Ebene)
Größe, Dateien, Ordner, Cloud-only-Anteil (Dateien/Bytes), Müll (Anzahl/Bytes nach den `junk`-Kategorien aus Phase 3), Duplikate (Gruppen, verschwendete Bytes), Problemdateien (Anzahl je Art), leere Ordner, Versionsgruppen, alte Daten (Anteil älter als `old_after`, außerhalb von `_Archiv`; **nur Kennzahl, kein Abzug**), Vorlagen-Abweichungen (falls Vorlage gesetzt), Health-Score samt Teilwerten. Dazu: Zeitpunkt, Scan-Status, Tool-Version, Config-Fingerabdruck.

### Health-Score
- Gewichteter Durchschnitt aus Teilwerten 0–100: **Müll**, **Duplikate**, **Problemdateien**, **Struktur** (leere, zu tiefe und übervolle Ordner, Ein-Datei-Ordner), **Versionswildwuchs**, mit Vorlage zusätzlich **Strukturtreue**. Nicht anwendbare Teilwerte (z. B. ohne Vorlage) fallen aus der Gewichtung.
- Default-Gewichte (Config, `[health.weights]`): Müll 20, Duplikate 25, Problemdateien 20, Struktur 20, Versionen 15, Strukturtreue 20. Gewicht 0 schaltet einen Teilwert ab.
- Teilwerte sind relativ zur Ordnergröße (Anteile bzw. Häufigkeit je 1.000 Dateien), damit große und kleine Ordner vergleichbar sind. Die genauen Formeln und Schwellen legt der Implementierungsplan fest und dokumentiert sie im README.
- Eigenschaften: deterministisch (derselbe Index und dieselbe Config ergeben denselben Score), monoton (mehr Müll, Duplikate, Probleme oder Abweichungen senken einen Teilwert nie weniger als weniger davon), erklärbar (der Bericht nennt die drei größten Abzüge mit Zahl, z. B. „−12: 4,1 GB Duplikate (8 % der Daten)“).

## Benachrichtigung
- Nur bei geplanten Läufen (`run`), nicht bei manuellen Befehlen.
- Erscheint, wenn mindestens eins zutrifft (Schwellen per Config `[notify]`): Score fällt um ≥ 5 Punkte gegenüber dem letzten vergleichbaren Lauf, Score < 60, Lauf gescheitert bzw. Wurzel nicht erreichbar, `run` hat mindestens einen nicht-leeren Plan erzeugt.
- Sonst kein Toast. Klick öffnet den Bericht (bzw. bei Fehlern das Lauf-Protokoll).

## Acceptance Criteria
- [ ] Given ein vollständiger `scan`, when er endet, then steht eine Momentaufnahme je Wurzel und Ordner der ersten Ebene in `history.db`, und die Ausgabe nennt Score und Veränderung
- [ ] Given ein abgebrochener oder unvollständiger Scan, when er endet, then entsteht keine Momentaufnahme für den Trend (höchstens eine als unvollständig markierte, die nicht verglichen wird)
- [ ] Given `--reset-index` oder ein neues Index-Schema, when danach gescannt wird, then ist der Verlauf vollständig erhalten
- [ ] Given zweimal derselbe Index und dieselbe Config, when der Score berechnet wird, then ist er identisch
- [ ] Given zusätzlicher Müll bzw. zusätzliche Duplikate bei sonst gleichem Ordner, when der Score berechnet wird, then ist er nicht höher als vorher
- [ ] Given ein Bericht mit Verlauf, when er geöffnet wird, then zeigt er Veränderung zum letzten Lauf, Trendgrafiken ohne externe Skripte und die drei größten Abzüge
- [ ] Given eine geänderte Config (z. B. neue Excludes) zwischen zwei Läufen, when der Bericht entsteht, then ist der Vergleich als „eingeschränkt“ markiert
- [ ] Given alte Daten außerhalb von `_Archiv`, when der Score berechnet wird, then senken sie ihn nicht; sie erscheinen nur als Kennzahl
- [ ] Given `schedule add --profile X --weekly MO 09:00`, when ausgeführt, then existiert eine Aufgabe für den aktuellen Benutzer ohne Adminrechte, nur am Netzteil, mit Nachholen verpasster Läufe; `schedule list` zeigt sie, `schedule remove` entfernt sie
- [ ] Given ein geplanter Lauf, when `run` läuft, then entstehen Momentaufnahme, Bericht unter `reports\<profil>\` samt `latest.html` und die im Profil genannten Pläne, und im Ordner ändert sich kein Byte (außer den Tool-Dateien)
- [ ] Given mehr als 12 Berichte eines Profils, when `run` endet, then bleiben die jüngsten 12 und `latest.html`
- [ ] Given ein Score-Abfall um ≥ 5, Score < 60, ein gescheiterter Lauf oder ein nicht-leerer Plan, when `run` endet, then erscheint eine Benachrichtigung, deren Klick den Bericht öffnet; sonst erscheint keine
- [ ] Given ein Profil, when `scan --profile X`, `plan rules --profile X` oder `report --profile X` läuft, then werden Wurzel, Regeldatei und Config-Abweichungen des Profils verwendet; explizite CLI-Angaben haben Vorrang
- [ ] Given ein unbekanntes Profil oder ein Profil mit ungültigem Schlüssel, when ein Befehl es nutzt bzw. die Config geladen wird, then gibt es einen klaren Fehler mit Liste der Profile bzw. dem Schlüssel
- [ ] Given die Vorlage `para` und eine Wurzel mit fehlendem Pflicht-Ordner, unerwartetem Ordner und Dateien auf oberster Ebene, when `report` läuft, then listet „Soll/Ist“ genau diese Abweichungen und der Teilwert „Strukturtreue“ sinkt
- [ ] Given eine eigene Vorlage mit ungültiger Regex oder unbekanntem Schlüssel, when sie geladen wird, then gibt es einen Fehler mit Datei und Zeile
- [ ] Given eine Profil-Wurzel, die nicht erreichbar ist, when `run` läuft, then endet es ohne Momentaufnahme mit Protokolleintrag, Benachrichtigung „nicht erreichbar“ und Exit-Code ≠ 0; der nächste Bericht nennt den ausgefallenen Lauf
- [ ] Manueller Test: geplante Aufgabe über mindestens zwei echte Läufe (inkl. Benachrichtigung und Klick), Profil mit Vorlage auf einem echten Ordner, dokumentiert

## Edge Cases & Error States
| Scenario | Expected Behavior |
|----------|-------------------|
| `history.db` fehlt | Wird beim ersten Schreiben angelegt |
| `history.db` beschädigt oder neueres Schema | Bericht/Scan laufen weiter ohne Verlauf, mit Warnung; nichts wird überschrieben |
| Index gesperrt (manueller Scan läuft), wenn `run` startet | `run` überspringt den Lauf, protokolliert ihn, Benachrichtigung nur bei wiederholtem Ausfall |
| Rechner war zum geplanten Zeitpunkt aus | Lauf wird nachgeholt, sobald der Rechner läuft und am Netzteil ist |
| Wechsel auf Akku während des Laufs | Aufgabenplanung beendet den Lauf; Scan gilt als abgebrochen, keine Momentaufnahme |
| exe verschoben oder neue Version an anderem Ort | `schedule list` meldet „Aufgabe zeigt auf fehlendes Programm“; `schedule add` für dasselbe Profil ersetzt die Aufgabe |
| `schedule add` für ein Profil, das schon eine Aufgabe hat | Aufgabe wird ersetzt (Meldung), nie doppelt angelegt |
| Benachrichtigungen systemweit deaktiviert | Kein Fehler; Ergebnis steht im Bericht und im Lauf-Protokoll |
| Ordner der ersten Ebene umbenannt | Alter Name endet im Verlauf, neuer beginnt; kein Versuch, Umbenennungen zu erkennen |
| Ordner der ersten Ebene sehr viele (> 200) | Momentaufnahme je Ordner nur für die größten 200, Rest als „Sonstige“ zusammengefasst |
| Erste Momentaufnahme einer Wurzel | Kein Vergleich, Bericht zeigt „erster Lauf“ |
| Uhrzeit/Zeitzone geändert, Momentaufnahmen nicht chronologisch | Sortierung nach gespeicherter UTC-Zeit; Veränderung immer zum zuletzt **geschriebenen** vergleichbaren Lauf |
| Cloud-only-Dateien | Zählen in den Kennzahlen (Größe aus den Metadaten), werden nie gelesen; Duplikate nur über lokale Dateien wie in Phase 1 |
| Profil verweist auf fehlende Regeldatei | `run` erzeugt Scan und Bericht, überspringt `plan rules` mit Fehler im Protokoll und Benachrichtigung |
| Vorlage passt gar nicht (Strukturtreue nahe 0) | Score sinkt entsprechend; Bericht zeigt höchstens die ersten 100 Abweichungen je Art plus Anzahl |
| Zwei Profile mit derselben Wurzel | Erlaubt; Verlauf gehört zur Wurzel, Bericht und Pläne zum Profil |

## Technical Constraints
- Performance: Kennzahlen und Momentaufnahme verlängern `scan` um höchstens 5 %; Score und Verlauf für 100.000 Dateien in unter 2 Sekunden; Bericht mit 52 Momentaufnahmen nicht spürbar langsamer als heute. Geplante Läufe mit Priorität „unter normal“.
- Security: Aufgabe läuft als aktueller Benutzer, ohne Adminrechte, ohne gespeichertes Passwort, nur bei angemeldetem Benutzer. `run` ruft nie `apply` auf. Profil-Wurzeln werden wie bei `scan` geprüft (Netzlaufwerke nur mit `force` im Profil). Regex in Vorlagen über `regex` (lineare Laufzeit).
- Compliance: Alles bleibt lokal, kein Netzwerk, keine Telemetrie. Der Verlauf enthält nur Wurzel, Namen der Ordner der ersten Ebene und Zahlen.
- Datenhaltung: `history.db` (SQLite) im Datenordner neben dem Index, eigenes Schema mit eigener Version und Migration; unabhängig von `--reset-index`. Verlauf wird nie automatisch gelöscht; Berichte je Profil: die letzten 12 (Config `reports_keep`).
- Rückwärtskompatibilität: Befehle ohne `--profile` verhalten sich wie bisher; bestehende Configs bleiben gültig.

## Dependencies
- Phase 1: Scan, Index, Problemdateien, Duplikate, Bericht ([`scan-analyse-bericht.md`](scan-analyse-bericht.md))
- Phase 3: Müll-Kategorien, leere Ordner, Versionsgruppen ([`aufraeumaktionen.md`](aufraeumaktionen.md))
- Phase 4: `plan rules`, `rules_file` ([`regel-engine-umbenennen.md`](regel-engine-umbenennen.md)); **Umsetzung von Phase 5 beginnt erst nach Abschluss von Phase 4**
- Windows-Aufgabenplanung über `schtasks.exe /Create /XML` (Alternative: COM `ITaskService`; Wahl im Implementierungsplan)
- Windows-Benachrichtigung (WinRT-Toast mit Aktivierung über Datei-URI); Crate (z. B. `tauri-winrt-notification`) oder direkt `windows`, Wahl im Implementierungsplan

## Open Questions
- [x] Automatik: nur Plan + Bericht, `apply` bleibt manuell
- [x] Umfang: geplante Läufe + Bericht, Verlauf + Health-Score, Profile + Vorlagen; Watch-Mode später
- [x] Speicherort Verlauf: eigene `history.db`
- [x] Benachrichtigung: Windows-Toast bei Score −5, Score < 60, Fehler oder neuen Plänen
- [x] Alte Daten: nur Kennzahl, kein Abzug im Score
- [x] Vorlagen: nur anzeigen, keine Aktionen
- [x] Akku: nur am Netzteil
- [x] Aufbewahrung: Verlauf ewig, Berichte die letzten 12 je Profil
- [x] Reihenfolge der Umsetzung: Verlauf + Score → `run`/`schedule`/Benachrichtigung → Profile → Vorlagen (Vorlagen bei Bedarf als Phase 5b abtrennbar)
- [x] Formeln und Schwellen der Teilwerte: festgelegt im Implementierungsplan, dokumentiert im README (`METRICS_VERSION = 1`)

## Präzisierungen bei der Umsetzung
- **Reihenfolge:** Profile kamen vor `run`/`schedule`, weil `run --profile` sie voraussetzt (Verlauf und Score → Profile → `run` → Benachrichtigung → `schedule` → Vorlagen).
- **Aufbewahrung:** Wie die Berichte werden auch die Pläne eines Profils nur für die letzten `reports_keep` Läufe aufbewahrt (`plans\<profil>\<Zeitstempel>-<art>.json`).
- **Berichtsdatei:** `run` schreibt nur die HTML-Datei (`<Zeitstempel>.html`, dazu `latest.html`); JSON und CSV gibt es weiter über `report`.
- **Lauf-Protokoll:** `runs\<profil>.jsonl` kennt zusätzlich den Status `partial` (Scan mit Warnungen oder gescheiterter Plan, Exit-Code 2).
- **Formeln:** siehe README, Abschnitt „Verlauf und Health-Score“.

## Manueller Test (2026-10-04)

Profil `downloads` (Wurzel Downloads, `template = "para"`), Aufgabe per `schedule add --profile downloads --daily 16:35`, Release-Build.

| Prüfung | Ergebnis |
|---|---|
| Aufgabe startet zur Zeit und führt `run` aus | bestanden (`schedule list`: „ok, Score 47“) |
| Toast erscheint | bestanden |
| Klick auf den Toast öffnet den Bericht | bestanden |
| `schedule list`, `schedule remove` | bestanden, Aufgabe entfernt |

Nicht geprüft:
- **Kein Konsolenfenster beim Start durch die Aufgabenplanung:** nicht ausdrücklich beobachtet; belegt nur durch den PE-Subsystem-Test.
- **Akku-Verhalten** und **Nachholen nach Ausschalten.**
- **Zweiter Lauf** (Verlauf mit Veränderung zum Vortag).
