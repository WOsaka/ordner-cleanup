# Dev-Template

Template-Repo für agentenbasierte Entwicklung mit Claude Code (Spec → Plan → Code, Analyse → Fix-Plan → Fix).

## Neues Projekt starten

1. Template kopieren bzw. als GitHub-„Template repository“ markieren und daraus ein neues Repo erzeugen.
2. Arbeitsbranch anlegen: `git switch -c dev` und mit `git push -u origin dev` pushen. Das Template liefert nur `main`, gearbeitet wird laut `CLAUDE.md` immer auf `dev`.
3. `CLAUDE.md` anpassen: Projektname, Beschreibung, Tech-Stack, Test-/Lint-Befehle.
4. `.gitignore` um stack-spezifische Einträge ergänzen.
5. Mit `/feature:spec` das erste Feature beschreiben.

Ablauf und Konventionen: siehe `CLAUDE.md`.
