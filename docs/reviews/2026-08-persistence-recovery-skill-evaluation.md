# Evaluation: Persistence and Recovery Transaction Review Skill

## Status

**Not canonical.** This is the evaluation record for Issue #42
("Implement the Pantheon persistence and recovery transaction-review skill").
It exercises the skill created by that mission against representative slices of
Issues #16, #17, and #18, records the trigger/non-trigger evaluations and the
fresh-context with/without comparison the mission's Evidence section asked for,
and is evidence that the skill selects relevant contract sections, identifies
applicable invariant families, and demands mission-appropriate evidence. It
states nothing about what Pantheon is; `docs/architecture/` remains the source
of truth.

## Method

Three representative mission slices — one from each of #16, #17, and #18 — and
one non-trigger scenario were run against the skill's procedure
(`.agents/skills/persistence-and-recovery-transaction-review/SKILL.md`). For
each trigger slice the record shows: the invariant families the skill's step 1
identifies, the contract anchors step 2 selects, the evidence shapes step 3
requires, and the confirmation (step 4) that no persistence semantics were
invented. A fresh-context with/without comparison was then run for the #17
slice: two independent agent contexts received the same mission prompt, one
with the skill body and one without, and their family identification and
evidence demands were compared.

## Trigger evaluation 1 — Issue #16 slice (store kernel, migrations, RestoreGeneration)

Slice: connection policy + initial ordered migrations + minimal `system_state`
with installation identity / RestoreGeneration, fail-closed on bad migration
state.

- **Families identified (skill step 1):**
  - *State-dependent authoritative write and transaction mode* — migrations
    are authoritative transactions; a failed migration must not leave the
    database claiming a schema version it did not fully apply.
  - *Restore-generation* — creation of one fresh unpredictable value that an
    ordinary reopen preserves exactly (fencing is not active in #16, but this
    is where the anchor lives).
  - *Cardinality and uniqueness enforcement* — STRICT authoritative tables and
    schema constraints appropriate to their invariants, checksummed
    `schema_migrations`.
- **Anchors selected (skill step 2):**
  `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`
  "SQLite operating rules", "Physical database", "Migrations / backup",
  "Disaster-restore authority fence (T0)";
  `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`
  "25. SQLite operational requirements", "26. Database integrity checks".
  Nothing else from the two contracts is retrieved for this slice.
- **Evidence demanded (skill step 3), matched to #16's acceptance criteria:**
  a test proving fresh init → migrations apply → RestoreGeneration = G →
  close → reopen → schema valid and RestoreGeneration still exactly G; a test
  asserting the full connection operating policy (WAL, `synchronous=FULL`,
  `foreign_keys=ON`, `trusted_schema=OFF`, bounded busy timeout, no
  shared-cache); a negative migration fixture proving a failed migration does
  not advance authoritative migration state or leave partially committed
  schema; a test that an unsupported/inconsistent migration state fails closed
  with a typed store-level error.
- **Nothing invented (skill step 4):** no Goal/Task/Run/scheduler/command/
  Event Journal tables, no ORM, no async runtime, no backup/restore workflow —
  all of which #16's scope and constraints explicitly exclude.

## Trigger evaluation 2 — Issue #17 slice (revisioned CAS transaction)

Slice: the reusable revision/CAS primitive plus the serialized authoritative
writer boundary. This is the scenario used for the A/B comparison below; the
skill's procedure yields:

- **Families identified:**
  - *State-dependent authoritative write and transaction mode* — the
    transaction primitive and the serialized writer / read-only separation.
  - *Revision/CAS predicates with exact affected-row checks* — mutation of a
    revisioned mutable row.
  - *Typed stale/conflict outcomes* — callers must distinguish stale/conflict
    from other failure.
  - The remaining five families (holder-safety FK, cardinality/uniqueness,
    command identity, restore-generation fencing, launch-contact certainty)
    are explicitly judged **not touched**: the mechanism defines no holder
    relationships, adds no production uniqueness constraint, and no command,
    restore, or external-effect boundary is introduced.
- **Anchors selected:**
  `sqlite-persistence-and-transactions.md` "SQLite operating rules",
  "Immutable documents versus mutable status", "Commands" (the typed-outcome
  model), "Named transaction families";
  `global-recovery-and-crash-reconciliation.md` "25. SQLite operational
  requirements".
- **Evidence demanded:** a concurrent race where two mutations based on the
  same observed revision have exactly one winner, the loser receives the typed
  stale result, and the winner's revision increments exactly once; a test that
  observes the write mode actually used rather than asserting a literal
  string; an injected-error rollback test leaving no partial authoritative
  state; a missing-row test producing the typed stale/missing result; a
  read-only path test proving mutation is refused.
- **Nothing invented:** no distributed-lock abstraction, no ORM, no
  transaction DSL, no production Goal/Task/Run schema; test-only revisioned
  fixture tables are used as #17 permits.

## Trigger evaluation 3 — Issue #18 slice (command / Event Journal kernel)

Slice: durable command identity `(commandEpoch, commandId)`, idempotent replay,
and one atomic transaction containing state mutation + command outcome +
Event append.

- **Families identified:**
  - *Command identity and idempotent replay* — the command/Event Journal
    boundary is the core of this slice.
  - *Restore-generation fencing* — `commandEpoch` must equal the current
    RestoreGeneration and stale epochs fail closed before row lookup.
  - *State-dependent authoritative write and transaction mode* — the single
    atomic commit of mutation + command + Event, and journal sequencing.
- **Anchors selected:**
  `sqlite-persistence-and-transactions.md` "Commands", "Event Journal",
  "Named transaction families", "Disaster-restore authority fence (T0)";
  `global-recovery-and-crash-reconciliation.md` "27. Backups and disaster
  recovery" (Operator command identity after restore).
- **Evidence demanded, matched to #18's acceptance criteria:** new command
  `(epoch G, id C, hash H)` commits fixture mutation + command outcome +
  Event; retry `(G, C, H)` returns the prior outcome without re-executing the
  mutation body; `(G, C, different hash)` fails closed as a typed conflict;
  `(old generation, new id)` is rejected as stale-command-epoch before
  command-row lookup; failure before COMMIT leaves no fixture mutation, no
  completed command, and no Event; journal sequencing is durable and monotonic
  within the current epoch and survives reopen.
- **Nothing invented:** no HTTP routes, no CLI/Operator wire types, no
  Goal/Task/Run event catalogs, no Event streaming/SSE or pruning, no secret
  persistence, no disaster-restore workflow.

## Non-trigger evaluation

Three scenarios were checked against the skill's trigger conditions and family
table. None of the four trigger conditions or eight families applies, so the
high-context procedure must not load:

1. A `pantheon-cli` change that reformats Run status output and adds a unit
   test for the formatting — CLI presentation work, no persistence touched.
2. A pure `pantheon-core` change adding a `GoalPhase` variant used only by
   domain logic — no persistence involvement.
3. A read-only query addition against an existing schema table with no
   invariant change — reads existing schema, mutates nothing.

Each is also a concrete demonstration of the mission's requirement that
"ordinary unrelated Rust/store-read work does not load this high-context
procedure unnecessarily."

## Fresh-context with/without comparison (Issue #17 slice)

Two independent agent contexts received the same mission prompt for the #17
slice, with repository access. Context **B** additionally received the skill
body; context **A** did not. Both were instructed to produce the invariant
families, the test evidence shapes, and the contract sections read.

| | Context A (no skill) | Context B (with skill) |
|---|---|---|
| Contracts retrieved | Read both canonical contracts in full (~3,300 lines) plus `implementation.md`; reported reading essentially every section of both | Read only the anchors the family table names: "SQLite operating rules", "Immutable documents versus mutable status", "Commands", "Named transaction families", "Core invariants" (trailing), recovery "25", plus `implementation.md` |
| Family identification | Named seven families; several overlap and one ("schema limited to what the mechanism requires") is a restatement of the mission criterion rather than a contract invariant; did not explicitly rule families in or out | Named the three touched families and explicitly judged the other five **not touched**, citing skill step 1; no family restated a mission criterion as a contract rule |
| Contract anchoring | Cited sections, but after loading the whole documents | Cited the same core sections with line ranges, tied each family to exactly one anchor |
| Evidence demanded | Concurrent same-revision race, injected rollback, read-only separation, typed stale result — correct and detailed | The same core evidence shapes, tied directly to the anchors, plus the explicit "observe the mode actually used, not the literal string" check |

**Comparison:** both contexts produced the high-value evidence shapes the
mission's acceptance criteria require (concurrent same-revision/CAS race,
injected mid-transaction failure with rollback proof, typed stale/conflict
outcome). The with-skill context achieved this with a fraction of the
retrieval cost (six targeted sections instead of two whole contracts), was
more precise about which families apply (explicit touched/not-touched
judgement instead of a seven-item list that included a criterion
restatement), and anchored every evidence demand to a contract section. The
difference is exactly the progressive-disclosure and retrieval-precision
value Issue #22's research predicted for this candidate.

## Outcome

The skill selects the relevant contract sections, identifies the applicable
invariant families, and asks for mission-appropriate
failure/concurrency/replay/recovery evidence for all three representative
MVP slices, and does not load for ordinary unrelated Rust or read-only work.
No persistence semantics outside the missions and contracts were introduced in
any evaluation.