# Feature Code — Implement from Plan

Implement a feature by executing an approved implementation plan.

## Arguments
$ARGUMENTS should be a path to an implementation plan file, e.g. `docs/implementation-plans/my-feature.md`.
If no path is given, list available plans in `docs/implementation-plans/` and ask the user to choose.

## Pre-flight checks

1. Read the implementation plan. Check frontmatter `status` field.
   - If `status` is NOT `approved`: **stop** and ask the user to set `status: approved` in the plan file manually before continuing.

2. Read the linked feature spec (`feature_spec` frontmatter field). Confirm you understand the acceptance criteria.

## Implementation

Execute the plan step by step:
- Follow the implementation sequence in the plan exactly.
- After each major step, verify it works before proceeding.
- Do not add features, abstractions, or error handling beyond what the plan and spec require.
- Run tests if a test suite exists.

## Post-implementation

Update the plan frontmatter: `status: implemented`.

Tell the user what was implemented and any open items.
