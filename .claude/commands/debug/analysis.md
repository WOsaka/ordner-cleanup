# Debug Analysis

Systematically analyze a bug and document findings.

## Arguments
$ARGUMENTS may contain a bug description, error message, or file path.

## Process

### 1. Gather context
Ask the user:
- What is the observed behavior?
- What is the expected behavior?
- How can it be reproduced? (steps, environment, data)
- When did it start? Any recent changes?

### 2. Investigate
- Search the codebase for relevant code paths.
- Read error logs, stack traces, or test output the user provides.
- Identify the likely root cause(s) — rank them by probability.
- Identify contributing factors (config, data, race conditions, etc.).

### 3. Document findings
Write analysis to `docs/bugs/<kebab-case-name>-analysis.md` with:
- Bug summary
- Root cause hypothesis (ranked)
- Evidence supporting each hypothesis
- Affected code locations (file:line references)
- Reproduction steps

Tell the user the file path and recommended next step: `/debug:fix-plan`.
