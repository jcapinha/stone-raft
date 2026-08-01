---
name: pre-pr-sanity-check
description: Quick pre-PR sweep for stale TODOs, inconsistent file/module comments, and obvious cleanup in changed files. Use before opening a pull request, when the user asks for a pre-PR sanity check, comment consistency review, or TODO cleanup before pre-commit hooks exist.
---

# Pre-PR sanity check

Lightweight review before the user opens a PR. Report findings first; only fix when the user asks or the change is trivial (e.g. removing a TODO that is clearly done).

This is not a linter, security audit, or full code review.

## Defaults

- Base branch: `main` (user may override)
- Focus on files changed on the current branch: `git diff main...HEAD`
- Include only source and config the team cares about (typically `*.py`, `*.yaml`, `*.yml`, `Dockerfile`, `*.md` in the change set). Skip lockfiles and generated artifacts unless the user asks otherwise.

## Workflow

### 1. Resolve scope

Run in parallel:

```bash
git status
git rev-parse --abbrev-ref HEAD
git diff main...HEAD --name-only
git diff main...HEAD --stat
```

If the branch has no commits ahead of base, say so and stop.

### 2. TODO / FIXME sweep

In changed files, search for `TODO`, `FIXME`, `HACK`, `XXX`, `WIP`.

For each hit:

- **Still valid?** Keep it if the work is genuinely unfinished in this PR.
- **Stale?** Flag if the referenced work is already done, the comment duplicates another file, or the note no longer matches the code.
- **Actionable?** Prefer `TODO(owner): short reason` or link to a ticket when the repo uses that pattern; flag vague or orphaned notes.

Also run a quick repo-wide search for the same TODO text in unchanged files. Duplicate TODOs across files are worth calling out.

### 3. Comment and file-header consistency

Before judging changed files, infer the **dominant pattern** in the repo:

1. Sample 5–10 modules at the repo root and in the same package as the changed files.
2. Note whether file purpose is declared via:
   - module docstring (`"""..."""` on line 1), or
   - `# --- Title ---` banner plus a `# This file/module is used to...` line, or
   - no header at all.

**Check changed files only:**

- New or heavily edited modules should follow the dominant pattern, not introduce a third style.
- Banner + docstring on the same file is redundant; pick one.
- "This file is used to..." should describe *purpose*, not restate the filename or every function name.
- Inline comments should explain non-obvious logic (business rules, external API quirks, magic numbers). Flag comments that narrate what the code already says (`# increment i`).
- Duplicate comments copied into multiple files (same constant explained in two places) are a consistency issue; suggest one canonical location.

Do not rewrite the whole repo. Only flag inconsistencies in **changed files**, or obvious copy-paste drift the PR introduced.

### 4. Quick hygiene (changed files only)

Skim for obvious pre-PR noise:

- `print()` / debug logging left behind
- Large blocks of commented-out code
- Temporary test overrides called out in comments (e.g. "paste values here for quick test") still present when the PR claims production readiness
- `.env`, keys, or credentials accidentally in the diff (critical: tell the user not to commit)

Skip deep style debates (naming, typing, architecture). Those belong in pre-commit or review.

### 5. Report

Return this structure:

```markdown
## Pre-PR sanity check

**Branch:** `<branch>` vs `<base>` · **Files reviewed:** N changed

### Summary
<one sentence: ready / minor cleanup recommended / fix before PR>

### Stale or questionable TODOs
- `path:line` — <why it looks stale or what to do>

### Comment consistency
- `path` — <what pattern the repo uses vs what this file does>

### Quick hygiene
- `path:line` — <issue, or "none found">

### Suggested fixes (optional)
1. <smallest high-value fix, if any>
```

Use severity informally:

- **Fix before PR** — stale TODO for work this PR completes, secrets in diff, misleading file headers on new modules
- **Nice to fix** — duplicate TODO, mixed header styles in files you touched, noisy debug comments
- **Ignore for now** — pre-existing inconsistency in untouched files

### 6. After the report

- Do **not** open a PR, commit, or push unless the user asks.
- Offer to apply trivial fixes (remove stale TODOs, align one file header) only after showing the report.
- If the user wants this automated later, suggest pre-commit hooks (`ruff`, custom TODO checker) but do not set them up unless asked.

## What to skip

- Full test runs (unless the user asks)
- Formatting/lint passes (pre-commit territory)
- Refactoring unrelated files for consistency
- Reviewing every file in the repo

## Example finding

```markdown
### Stale or questionable TODOs
- `hr/settings.py:4` — TODO asks for Secret Manager wiring; `utils/secrets.py` already implements `get_required()`. Remove or narrow to remaining env vars.

### Comment consistency
- Repo mostly uses `# --- Title ---` + purpose line (`hr/parse.py`, `utils/bigquery.py`).
- `hr/job.py` uses a module docstring only. Fine if intentional; otherwise align with the banner pattern used elsewhere in `hr/`.
```
