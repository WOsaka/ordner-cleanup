# Feature Implementation Plan

Generate a detailed, developer-ready implementation plan from a feature spec.

## Arguments
$ARGUMENTS should be a path to a feature spec file, e.g. `docs/features/my-feature.md`.
If no path is given, list available specs in `docs/features/` and ask the user to choose.

## Process

1. **Read the spec** at the given path. If `status` is not `approved` and not `draft`, warn the user.

2. **Analyze the codebase** — explore relevant files, entry points, and existing patterns that the feature will touch or extend.

3. **Generate the plan** covering:
   - List of files to create or modify (with purpose)
   - Data model changes (schema, types, interfaces)
   - API / interface changes
   - Step-by-step implementation sequence (ordered, with dependencies noted)
   - Test strategy (unit, integration, E2E)
   - Risk and open questions

4. **Write the plan** to `docs/implementation-plans/<same-kebab-name>.md` using the template at `.claude/templates/implementation-plan.md`.

   Set frontmatter:
   - `feature_spec: <relative path to spec>`
   - `status: pending-approval`

5. Tell the user the file path and that the plan requires approval (status must be `approved`) before `/feature:code` will proceed.
