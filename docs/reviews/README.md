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
| `docs/reviews/archive/2026-08-second-architecture-review.txt` | Byte-preserved raw snapshot of the second adversarial review from its historical branch. Its one Critical and four High follow-ups were subsequently dispositioned. |
| `docs/reviews/archive/2026-08-final-adversarial-architecture-review.txt` | Byte-preserved raw snapshot of the final adversarial review from its historical branch. Its two remaining blockers were subsequently dispositioned. |
| `docs/reviews/2026-08-adversarial-review-closure.md` | Historical closure note mapping the second/final review blockers to the canonical contracts that subsequently absorbed them. Not architecture. |
| `docs/reviews/2026-08-rust-agent-skill-research.md` | Research/decision result ranking Rust-specific agent-skill candidates (Issue #22): which are Pantheon-specific enough to add, defer, or reject, and why. Not a skill catalog; skills are implemented by later missions inside the mechanism Issue #21 establishes. |
| `docs/reviews/2026-08-persistence-recovery-skill-evaluation.md` | Evaluation record for the `persistence-and-recovery-transaction-review` skill (Issue #42): trigger/non-trigger runs against slices of Issues #16/#17/#18 and a fresh-context with/without comparison. Evidence, not a skill or an authority. |
| `docs/reviews/2026-08-accountable-orchestration-research.md` | Research/decision record for Issue #100: lessons from SMOG and primary evidence on multi-agent scaling, evaluation, inquiry state, context provenance, human steering and governed learning. Records retained/rejected post-MVP movements; only decisions separately written into canonical contracts are authoritative. |

The two files under `docs/reviews/archive/` are raw historical snapshots rather
than live Markdown documentation. They intentionally retain the old paths,
line references, verdict wording and unresolved-state statements from the
repository snapshots they reviewed. Keeping them as `.txt` prevents historical
references from masquerading as current documentation references while
preserving the source reports exactly. Read
`docs/reviews/2026-08-adversarial-review-closure.md` for their later
disposition, and `docs/architecture/` for the current contract.

The first review-resolution ledger and second-review instructions predate the
documentation reorganization; their references to architecture files were
updated to the current paths, but their content is otherwise historical.

## Adding to this area

Name new live review documents `<YYYY-MM>-<subject>.md` and open them with a
`## Status` section saying what the document is and that it is not canonical.
Add a row to the table above. Raw immutable snapshots whose references should
not be treated as current may be stored under `docs/reviews/archive/` as text
and accompanied by a live disposition/closure note.

Superseded architecture material also belongs here rather than in
`docs/architecture/`, with a `## Status` line naming what superseded it.
