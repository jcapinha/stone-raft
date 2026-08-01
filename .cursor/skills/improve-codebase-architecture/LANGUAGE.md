# LANGUAGE.md

Shared vocabulary for every suggestion this skill makes. Use these terms exactly — don't substitute "component," "service," "API," or "boundary." Consistent language is the whole point.

---

**Module**
Anything with an interface and an implementation. Deliberately scale-agnostic — applies equally to a function, class, or package. Avoid: unit, component, service.

**Interface**
Everything a caller must know to use the module correctly. Includes the type signature, but also invariants, ordering constraints, error modes, required configuration, and performance characteristics. Avoid: API, signature (too narrow — those refer only to the type-level surface).

**Implementation**
What's inside a module — its body of code. Distinct from Adapter: a thing can be a small adapter with a large implementation (a database repository) or a large adapter with a small implementation (an in-memory fake).

**Depth**
The leverage a module provides at its interface: a lot of behaviour hidden behind a small, simple interface. Deep = high leverage. Shallow = interface nearly as complex as the implementation. The goal is always to increase depth.

**Seam**
Where an interface lives — a place where behaviour can be altered without editing internals. Use this term, not "boundary."

**Adapter**
A concrete thing satisfying an interface at a seam. Example: a real database connection and an in-memory fake are both adapters for the same storage interface.

**Leverage**
What callers gain from depth — they invoke a small interface and get a large amount of behaviour for free.

**Locality**
What maintainers gain from depth — a change to behaviour requires editing one place, not N scattered callers.

**Pass-through**
A module that adds no depth — its interface is as complex as its implementation, or it simply delegates to another module without hiding anything. A strong candidate for deletion or merger.

**Candidate**
A module (or cluster of modules) identified during exploration as a deepening opportunity.

**Deepening**
The act of merging shallow or over-specific modules into a broader, deeper one with a simpler interface and more hidden implementation.
