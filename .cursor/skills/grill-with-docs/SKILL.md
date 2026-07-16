---
name: grill-with-docs
description: >-
  Relentless design interview that also maintains living project context.
  Use when the user invokes /grill-with-docs, wants to grill a plan while
  updating CONTEXT.md and REJECTED.md, or stress-tests stone-raft decisions
  with documentation.
disable-model-invocation: true
---

# grill-with-docs

Interview the user relentlessly about every aspect of the plan until you reach a shared understanding. Walk down each branch of the design tree, resolving dependencies between decisions one-by-one. For each question, provide your recommended answer.

Ask the questions one at a time. Prefer the AskQuestion widget when it is available; otherwise use numbered options in text. Asking multiple questions at once is bewildering.

If a fact can be answered by exploring the codebase, explore the codebase instead. Decisions belong to the user — put each one to them and wait for an answer.

Do not enact a build plan until the user confirms shared understanding.

## Teaching tone

The author is learning Rust and systems/audio programming from a Python and data-pipelines background, without a traditional engineering background.

- Use plain language. Define audio or systems terms the first time you use them.
- Prefer concrete analogies to Python or data pipelines when helpful (for example: a Rust crate is like a library you would `pip install`).
- Still give a recommended answer each time, and explain the trade-off without assuming engineering jargon.
- Do not skip hard decisions; teach while deciding.

## Living docs

This skill maintains two files at the repo root:

- `CONTEXT.md` — current truth (Project, Language, Decisions)
- `REJECTED.md` — closed doors so agents do not re-suggest abandoned options

### At session start

Read `CONTEXT.md` and `REJECTED.md` before asking anything. Treat them as current truth. Never re-propose items listed in `REJECTED.md` unless the user explicitly reopens that door.

### While grilling

Update docs as each decision resolves — do not batch everything until the end.

**`CONTEXT.md`**

- Three sections only: Project, Language, Decisions.
- **Language**: domain terms for this synth project. Keep definitions tight (one or two sentences). When multiple words mean the same thing, pick one and list the others under `_Avoid_`. No Rust crate names or implementation chatter here.
- **Decisions**: current choices only. Each entry is 2–4 lines: what was chosen, why, and any hard constraint. Edit in place when a decision changes. Do not leave “was X, now Y” history in this file.

**`REJECTED.md`**

- Short bullets for rejected or reversed options.
- When the user reverses a decision: update the Decisions section in `CONTEXT.md` in place, then add or adjust a closed-door bullet here (title, what was abandoned, why it stays closed).
- Keep it skim-small. Remove bullets that are obsolete because the whole area was deleted.
- Do not narrate dates or “6 days ago.” Git is the archive.

### After shared understanding

Stop. Summarize only if useful. Do not start implementing unless the user asks.
