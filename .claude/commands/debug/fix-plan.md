# Debug Fix Plan

Generate a targeted fix plan from a bug analysis.

## Arguments
$ARGUMENTS should be a path to a bug analysis file, e.g. `docs/bugs/my-bug-analysis.md`.
If no path is given, list available analysis files in `docs/bugs/` and ask the user to choose.

## Process

1. **Read the analysis file.** Identify the primary root cause to address.

2. **Explore affected code** — read the specific files and line ranges referenced in the analysis.

3. **Design the fix:**
   - What exactly changes, and why?
   - Are there safer alternatives? Note tradeoffs briefly.
   - What is the minimal change that fixes the root cause without side effects?
   - What regression risks exist?

4. **Write the fix plan** to `docs/bugs/<same-base-name>-fix-plan.md`:
   - Exact files and lines to change
   - The change description (before/after logic, not pseudocode)
   - Test cases to add or modify
   - Rollback approach if the fix causes regressions

Tell the user the file path. Next step: `/debug:fix`.
