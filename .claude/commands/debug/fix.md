# Debug Fix — Execute Fix Plan

Apply a bug fix by executing a fix plan.

## Arguments
$ARGUMENTS should be a path to a fix plan file, e.g. `docs/bugs/my-bug-fix-plan.md`.
If no path is given, list available fix plan files in `docs/bugs/` and ask the user to choose.

## Process

1. **Read the fix plan.** Confirm you understand every change before touching any file.

2. **Apply the fix** — follow the plan precisely:
   - Make only the changes described in the plan.
   - Do not refactor surrounding code.
   - Do not add features or unrelated improvements.

3. **Verify:**
   - Run existing tests if a test suite exists.
   - Apply the reproduction steps from the linked analysis to confirm the bug is resolved.
   - Check for regressions in related functionality.

4. **Document:** Add a short note at the top of the fix-plan file: `status: fixed` and `fixed_at: <date>`.

Report to the user: what was fixed, test results, and any remaining concerns.
