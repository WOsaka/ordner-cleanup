# ordner-cleanup — Claude Code Instructions

> TODO: Abschnitt „Tech-Stack & Befehle“ ausfüllen, sobald der Stack feststeht.

## Projekt

Strukturierung und Bereinigung von Ordnersystemen auf dem PC (Analyse, Aufräumen, Neuorganisation von Verzeichnissen). Details und Scope werden per `/feature:spec` festgelegt.

## Tech-Stack & Befehle

- Sprache/Framework: <…>
- Tests: `<Befehl>`
- Lint/Format: `<Befehl>`
- Start lokal: `<Befehl>`

## Projektstruktur

```
.claude/
  commands/         # Custom Slash Commands (Skills)
    feature/        # Feature-Workflow
    debug/          # Debugging-Workflow
  templates/        # MD-Vorlagen für die Skills
docs/
  features/               # Feature-Specs (.md)
  implementation-plans/   # Implementierungspläne (.md)
  bugs/                   # Bug-Analysen und Fix-Pläne (.md)
```

## Workflow

### Feature-Entwicklung
1. `/feature:spec` — Interview + Feature-Spec → `docs/features/<name>.md`
2. `/feature:implementation-plan` — Plan aus Spec → `docs/implementation-plans/<name>.md`
3. `/feature:code` — Umsetzung aus Plan (prüft Plan-Freigabe)

### Bugfixing
1. `/debug:analysis` — Bug analysieren → `docs/bugs/<name>-analysis.md`
2. `/debug:fix-plan` — Fix-Plan → `docs/bugs/<name>-fix-plan.md`
3. `/debug:fix` — Fix ausführen

## Konventionen

- Dateinamen in kebab-case: `docs/features/<name>.md`, `docs/implementation-plans/<name>.md`, `docs/bugs/<name>-analysis.md`, `<name>-fix-plan.md`
- Implementierungspläne brauchen `status: approved` im Frontmatter (manuell setzen), bevor `/feature:code` startet.

## Git-Arbeitsweise

- Claude arbeitet **immer auf `dev`**: vor Änderungen `git switch dev`, dort committen und pushen. Keine Feature-/Docs-Branches, außer der User verlangt es ausdrücklich.
- `main` ist der Release-Branch: nur per PR `dev → main` (der User merged). Nie direkt auf `main` committen oder pushen.
- Commit-Messages kurz, ohne `Co-Authored-By`-/Session-Footer.

## Superpowers Plugin

Das `superpowers`-Plugin ist in `.claude/settings.json` vorkonfiguriert (beim ersten Start Marketplace-Vertrauen bestätigen). Der Workflow oben hat Vorrang (kein `brainstorming` statt `/feature:spec`, kein `systematic-debugging` statt `/debug:analysis`). Ergänzend nutzen:
- `test-driven-development` bei der Umsetzung in `/feature:code`
- `verification-before-completion` vor `status: implemented` / `status: fixed`
- `requesting-code-review` / `receiving-code-review` vor dem Merge
- `using-git-worktrees` für isolierte Workspaces
