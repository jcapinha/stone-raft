# DEEPENING.md

How to safely deepen a cluster of shallow or over-specific modules, given their dependency type.

This file is used in Step 4 of SKILL.md — after the developer has reviewed the architecture report and chosen a candidate to act on.

---

## Step 1 — Classify the candidate's dependencies

The dependency type determines how the deepened module is tested and how the seam is structured.

**Type A — Pure computation / in-memory**
No I/O, no external state. Pure functions, data transformations, algorithms.
→ Always deepenable. Merge the modules and test through the new interface directly. No adapter needed.

**Type B — Local stand-in available**
Dependencies that have lightweight local substitutes (e.g. an in-memory dict instead of a real cache, `sqlite3` in-memory instead of a real DB).
→ Deepenable if the stand-in is practical. The deepened module is tested with the stand-in. The seam is internal — don't expose it through the module's external interface.

**Type C — Internal network boundary**
Your own services across a process or network boundary (internal APIs, microservices).
→ Define a port (abstract interface) at the seam. The deep module owns the logic; the transport is injected as an adapter. Test with a fake adapter.

**Type D — Third-party services**
External dependencies you don't control (APIs, cloud services, etc.).
→ The deepened module takes the external dependency as an injected port. Tests provide a mock adapter. Don't introduce a port unless at least two adapters are justified (typically production + test) — a single-adapter seam is just indirection.

---

## Step 2 — Design the new interface

Before writing code, describe the proposed interface:

- What does a caller need to pass in?
- What does the caller get back?
- What error modes must the caller handle?
- What invariants must the caller respect?

The interface should be **simpler than the sum of the interfaces it replaces**. If it isn't, the deepening is not worth doing.

Write a short Python pseudocode sketch — not a full implementation, just enough to make the interface concrete.

---

## Step 3 — Merge and implement

- Combine the shallow/specific modules into the new deeper module.
- Move all shared logic into the implementation; expose only the new, simpler interface.
- Do not expose internal seams through the external interface just because tests use them.
- Prefer generic parameters over task-specific ones. A function that takes `filters: dict` is more reusable than one that takes `user_id: int, status: str`.

---

## Step 4 — Update tests

- Write new tests at the deepened module's interface. Assert on observable outcomes, not internal state.
- Old tests on the shallow modules become waste once tests at the new interface exist — delete them.
- Internal seams (used only by the module's own tests) stay private.

---

## Step 5 — Update callers

- Update all callers to use the new interface.
- Do not leave compatibility shims unless there is a strong reason. The goal is fewer, simpler call sites.

---

## Seam rules

- **One adapter = hypothetical seam.** Don't formalise it yet.
- **Two adapters = real seam.** Introduce the port.
- A single-adapter seam is indirection without benefit — avoid it.
- Internal seams (used only inside the module's implementation) do not belong on the external interface.
