---
name: improve-codebase-architecture
description: Find deepening opportunities in a Python codebase. Use when you want to improve architecture, find refactoring opportunities, consolidate over-fragmented or over-specific modules, or make the codebase more readable and AI-navigable.
---

# improve-codebase-architecture

Surface architectural friction and propose deepening opportunities — refactors that turn shallow, overly-specific modules into deep, reusable ones.

The aim is **reusability and AI-navigability**: fewer files, broader modules, generic functions, and small interfaces hiding large implementations.

Use the vocabulary defined in [LANGUAGE.md](./LANGUAGE.md) exactly in every suggestion. Consistent language is the point — don't drift into "component," "service," "API," or "boundary."

---

## Key principles

- **Deletion test**: imagine deleting the module. If complexity vanishes, it was a pass-through. If complexity reappears across N callers, it was earning its keep.
- **The interface is the test surface.** One adapter = hypothetical seam. Two adapters = real seam.
- **Prefer fewer, broader modules.** A file that does one tiny specific thing is a smell. Favour generic, reusable functions over task-specific ones.
- **Don't create new files unless the abstraction is genuinely independent.**

Full vocabulary definitions are in [LANGUAGE.md](./LANGUAGE.md).

---

## This skill is informed by the project's domain model

- Read `CONTEXT.md` at the repo root first (domain language, module ownership).
- Read any ADRs in `docs/adr/` that touch the area you're analysing. Do not re-litigate decisions already recorded there unless the friction is severe enough to warrant reopening — and flag it explicitly if so.
- The domain language gives names to good seams; ADRs record decisions the skill should not re-litigate.

---

## Process

### Step 1 — Explore the codebase organically

Walk the codebase. Don't follow rigid heuristics — explore and note where you experience friction:

- Where does understanding one concept require bouncing between many small files?
- Where are modules shallow — interface nearly as complex as the implementation?
- Where are functions or classes too specific to one use case, not reusable across others?
- Where is logic duplicated across multiple narrow modules that could be one generic one?
- Where do callers have to know too much about internals?

Note every friction point as a **candidate**.

### Step 2 — Score and filter candidates

For each candidate, assess:

- **Depth** — how much behaviour is hidden behind the interface? Shallow = bad.
- **Specificity** — is this module/function written for one exact task, or is it generic? Overly specific = bad.
- **Fragmentation** — could this and N sibling modules be one broader module without losing clarity?
- **Reuse** — could callers in other parts of the codebase benefit from this if it were more generic?

Discard candidates where the refactor would add indirection without adding depth or reusability.

### Step 3 — Produce a markdown report

Write the findings to `docs/architecture-review.md`. Do **not** create GitHub issues, PRs, or HTML reports. The developer reviews and acts on the markdown report themselves.

Structure the report as follows:

---

#### Architecture Review — `{date}`

**Summary**
One paragraph: overall state of the codebase architecture, main patterns of friction observed.

---

**Candidates**

For each candidate, write a card:

```
### [Module/file name] — [one-line description of the problem]

**Problem**
What friction this causes. Why it's shallow or too specific.

**Deepening opportunity**
What a deeper, more generic version would look like.
Describe the proposed interface (what callers would see) vs what would be hidden.

**Before / After sketch**
Short illustrative Python pseudocode showing the current shape vs the proposed shape.
Keep it brief — this is a sketch, not a full implementation.

**Recommendation strength**: Strong | Worth exploring | Speculative

**ADR conflict** (if any): note here if this contradicts an existing ADR and why it may be worth reopening.
```

End the report with:

```
## Top recommendation
Which candidate to tackle first, and why.
```

---

### Step 4 — Implementing a chosen candidate

When the developer chooses a candidate to act on, follow [DEEPENING.md](./DEEPENING.md) for how to safely merge and deepen that cluster of modules given its dependency type.

Do not implement anything until the developer has reviewed the report and explicitly chosen a candidate.

---

## Output rules

- All output goes to `docs/architecture-review.md` as plain markdown.
- No GitHub issues, no PRs, no HTML.
- The developer commits and reviews changes themselves — do not stage, commit, or push anything.
- Use `CONTEXT.md` vocabulary for domain concepts, `LANGUAGE.md` vocabulary for architecture concepts.
