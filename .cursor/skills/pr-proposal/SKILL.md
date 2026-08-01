---
name: pr-proposal
description: Proposes pull request title and description by analyzing all commits on the current branch since it diverged from the base branch. Use when the user wants a PR title/description draft, is opening a PR manually, asks what to put in their PR, or mentions commits on the current branch.
---

# PR title and description proposal

Draft a PR title and body for the user to paste when they create the PR manually. Do not run `gh pr create`, push, or open the PR unless the user explicitly asks.

## Defaults

- Base branch: `main` (user may override, e.g. `develop`)
- Compare full branch history since divergence, not only the latest commit

## Workflow

### 1. Resolve branches

Run in parallel:

```bash
git status
git branch -vv
git rev-parse --abbrev-ref HEAD
```

Confirm the current branch and whether it tracks a remote. If the user named a base branch, use it; otherwise use `main`.

Verify the base exists locally:

```bash
git rev-parse --verify main
```

If missing, try `git fetch origin main` once, then re-check.

### 2. Collect branch history

Run:

```bash
git log main..HEAD --oneline
git log main..HEAD --format='%h %s%n%b---'
git diff main...HEAD --stat
```

Optional when the diff is large or commits are vague:

```bash
git diff main...HEAD
```

Rules:

- Use `main..HEAD` for commits and `main...HEAD` for the cumulative diff (merge-base aware).
- Read every commit in `main..HEAD`, not only the tip.
- If there are no commits ahead of base, say so and stop; do not invent a proposal.

### 3. Analyze

From commits and diff:

- Group changes into one coherent story (feature, fix, refactor, chore, docs).
- Prefer one primary intent for the title; mention secondary scope in the summary bullets.
- Note breaking changes, migrations, config/env changes, and areas that need manual testing.
- Match tone and scope to recent PRs/commits in the repo when visible.

### 4. Propose output

Return exactly this structure:

```markdown
## Proposed PR title

<single line, imperative, ≤72 chars when possible>

## Proposed PR description

## Summary
- <bullet: what changed and why>
- <bullet: secondary change or impact, if any>

Title guidance:

- Imperative mood: "Add HR ingestion job", not "Added…"
- No trailing period; no ticket prefix unless the repo consistently uses one

Description guidance:

- Summary: 1–3 bullets, outcomes and rationale, not a commit list
- Do not paste raw `git log` unless the user asks for a changelog section

### 5. Offer variants (only when useful)

If the branch mixes unrelated concerns or the right title is ambiguous, add:

```markdown
## Alternatives
- **Title:** … — when …
```

Keep to at most two alternatives.

## Edge cases

| Situation | Action |
|-----------|--------|
| Branch equals base (no commits) | Report no divergence; no proposal |
| Many small commits | Synthesize theme; do not enumerate every SHA |
| Merge commits on branch | Focus on net change; ignore merge commit noise |
| User on `main` | Warn; ask which feature branch to analyze |
| Dirty working tree | Mention uncommitted files do not affect the proposal unless they ask to include them |

## What not to do

- Do not create commits, push, or open the PR
- Do not update git config
- Do not summarize only the latest commit when multiple commits exist on the branch
