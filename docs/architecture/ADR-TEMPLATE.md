[English](./ADR-TEMPLATE.md) | [한국어](./ADR-TEMPLATE.ko.md)

# ADR-XXX: <Concise Title>

**Status**: Draft | Proposed | Accepted | Superseded | Deprecated
**Date**: YYYY-MM-DD
**Scope**: `<crate-or-module-path>`, `<secondary-scope>`, ...
**Supersedes**: (optional) ADR-NNN, or `none`
**Superseded by**: (optional, only when this ADR is superseded) ADR-MMM
**Related**: (optional) ADR-NNN, ADR-MMM
**Implementation**: (optional) public implementation record or code path that carried out this ADR

---

## Context

What is the architectural problem this ADR addresses? Focus on **why** — the forces, constraints, incidents, or requirements that motivated the decision. Keep this section factual and reference prior ADRs or design docs rather than re-explaining them.

## Decision

The single architectural decision (or a small cluster of tightly-related decisions) that this ADR locks in. Use numbered sub-sections for multi-part decisions:

### 1. <First part of the decision>

The rule, with a short example or snippet if it clarifies.

```rust
// Example or canonical form (optional)
```

**Rationale**: why this choice over alternatives.

### 2. <Second part, if any>

...

## Consequences

### Positive

- Benefit 1
- Benefit 2

### Negative

- Cost / tradeoff 1
- Cost / tradeoff 2

### Neutral

- Observable effect that is neither clearly positive nor negative.

## Alternatives Considered

**A. <Alternative name>.** Why rejected.
**B. <Alternative name>.** Why rejected.

## Known Follow-ups (optional)

Items that are out of scope for this ADR but become relevant once it lands. Number them so subsequent iterations can link back:

1. **<Follow-up title>** — what, why, rough effort. Link to a design doc if one exists.

## Related Docs

- `docs/...` — supporting specifications or guides
- `docs/architecture/ADR-NNN-*.md` — related architectural decisions

---

## How to use this template

1. Copy `ADR-TEMPLATE.md` + `ADR-TEMPLATE.ko.md` to `ADR-XXX-<kebab-title>.md` + `.ko.md` where `XXX` is the next unused three-digit number (see `docs/architecture/README.md` for the registry).
2. Fill all required header fields. `Status` starts at `Draft` or `Proposed`; promote to `Accepted` only after review and (if applicable) implementation.
3. Write `Context` first so reviewers understand *why* before *what*.
4. The `Decision` section is the load-bearing content. If you find yourself writing multi-page implementation detail, move it to a companion implementation record and link to it via `**Implementation**:` when that record is public.
5. `Consequences` is required. `Alternatives Considered` is strongly encouraged — at least 2 alternatives.
6. After writing, verify the ADR appears in `docs/architecture/README.md` registry.
7. When the Decision changes materially, do not silently edit: either create a new ADR that `Supersedes` this one, or append an explicit `## Update YYYY-MM-DD` section.

## Naming conventions

- **Filename**: `ADR-XXX-<kebab-case-title>.md`. Korean companion: `ADR-XXX-<kebab-case-title>.ko.md`.
- **Title**: concise (≤ 60 chars). Full sentences in Context / Decision, not the title.
- **Status keyword**: single word when possible. Promotion history (`Accepted (promoted from Proposed YYYY-MM-DD; <reason>)`) is acceptable.
- **Date**: the date the ADR was first authored. Does NOT change on subsequent revisions; use `## Update YYYY-MM-DD` sub-sections for material changes.
