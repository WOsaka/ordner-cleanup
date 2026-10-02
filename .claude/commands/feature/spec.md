# Feature Spec — Interview & Document

You are conducting a structured feature specification interview. Your goal is to produce a complete, unambiguous feature spec that a developer (or you) can implement without further clarification.

## Arguments
$ARGUMENTS may contain an initial feature name or brief description. Use it as a starting point.

## Process

### Phase 1: Interview
Ask the user these questions — one group at a time, not all at once. Wait for answers before continuing.

**Round 1 — Core problem:**
1. What problem does this feature solve? Who is affected?
2. What does success look like? How would you measure it?
3. What is explicitly out of scope?

**Round 2 — User experience:**
4. Walk me through the user journey step by step.
5. What are edge cases or error states the user might encounter?
6. Are there existing patterns in the product this should follow?

**Round 3 — Technical & constraints:**
7. Are there performance, security, or compliance requirements?
8. What are the dependencies (other features, services, third-party systems)?
9. What is the deadline or priority?

### Phase 2: Refinement
After all answers: summarize your understanding back to the user. Ask: "Is there anything missing or incorrect?" Iterate until the user confirms.

### Phase 3: Generate Spec File
Write the spec to `docs/features/<kebab-case-feature-name>.md` using the template at `.claude/templates/feature-spec.md`.

Set `status: draft` in the frontmatter.

Tell the user the file path when done.
