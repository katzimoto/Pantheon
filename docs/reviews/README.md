# Pantheon Reviews

## Status

**Not canonical.** Nothing in this directory is an implementation requirement.

This area holds architecture reviews, review ledgers and review process
material. It records what reviewers found and how findings were
dispositioned. It does not state what Pantheon is.

## Authority relationship

Canonical architecture lives under `docs/architecture/`. The relationship is
one-directional:

```text
review finding
  → accepted decision
    → written into a canonical contract under docs/architecture/
      → the contract is the source of truth
```

Consequences:

- A review may diagnose a problem correctly and still propose a resolution
  that Pantheon did not adopt. Some resolutions recorded here deliberately
  differ from the reviewer's suggested mechanism.
- Where a review conflicts with a canonical contract, **the canonical
  contract wins**. Do not implement from a review.
- A finding that has been incorporated is fully represented by the canonical
  document. Read the contract, not the ledger, for the current rule.
- Do not copy review conclusions into architecture documents. A conclusion
  becomes architecture only through a deliberate decision to change the
  contract.

Use these documents to understand *why* a contract says what it says, or to
check whether a concern was already raised. Do not use them to determine what
the system should do.

## Contents

| Document | What it is |
|---|---|
| `docs/reviews/2026-08-architecture-review-resolution.md` | Ledger of how the Critical/High findings from the first adversarial review were dispositioned, with pointers to the canonical documents that now carry each decision. Historical record. |
| `docs/reviews/2026-08-second-architecture-review-instructions.md` | The brief written for a second adversarial review: scope, decisions to treat as current at the time of writing, and required output format. Process material, not architecture. |
| `docs/reviews/2026-08-rust-agent-skill-research.md` | Research/decision result ranking Rust-specific agent-skill candidates (Issue #22): which are Pantheon-specific enough to add, defer, or reject, and why. Not a skill catalog; skills are implemented by later missions inside the mechanism Issue #21 establishes. |

Both documents predate the documentation reorganization; their references to
architecture files were updated to the current paths, but their content is
otherwise unchanged and reflects the state of the project when they were
written.

## Adding to this area

Name new documents `<YYYY-MM>-<subject>.md` and open them with a `## Status`
section saying what the document is and that it is not canonical. Add a row to
the table above.

Superseded architecture material also belongs here rather than in
`docs/architecture/`, with a `## Status` line naming what superseded it.
