---
name: persistence-and-recovery-transaction-review
description: Review or implement Pantheon's authoritative SQLite persistence and recovery work — authoritative `pantheon-store` mutations, schema/table families that carry authoritative or revisioned state, command/idempotency/Event Journal transactions, recovery/reconciliation paths whose correctness depends on persisted evidence, and reads that issue more than one statement whose answers must agree with each other. Use to identify the invariant families a change touches, retrieve only the relevant sections of the canonical persistence/recovery contracts, and demand high-value concurrency, failure, replay, and recovery evidence. Do not use for a single-statement read whose answer nothing else must agree with, pure `pantheon-core` domain work, CLI presentation work, or unrelated Rust changes.
metadata:
  pantheon-authority: procedural-guidance-only
---

# Pantheon persistence and recovery transaction review

This skill operationalizes the canonical persistence and recovery contracts:
`docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`
and `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`,
plus the `pantheon-store` crate boundary in `docs/development/implementation.md`.
It is procedural guidance only: it tells you which invariant families to look
for, which sections of the contracts to read, and what evidence to demand. It
never restates a rule the contracts already state and never replaces them. If
this skill's text and the current canonical contracts disagree, the contracts
win and this skill is defective — report the drift rather than following the
skill.

## When this applies

Use when a change — implementation, self-review, or review input — touches any
of:

- an authoritative `pantheon-store` write: a mutation of a mutable
  authoritative or revisioned row, or the transaction that carries it;
- schema for a table or table family that stores authoritative or revisioned
  state, including a migration, an index, or a constraint;
- a command, idempotency, or Event Journal transaction boundary;
- a recovery or reconciliation path whose correctness depends on persisted
  evidence (startup inventory, restore, crash reconciliation, fencing);
- a read that issues more than one statement whose answers must agree with
  each other — a list and the cursor it corresponds to, a resource
  representation and the validator it is served under, a fence and the rows it
  fences.

## Procedure

1. **Identify the invariant families the change touches before writing or
   reviewing anything.** Walk the family table below and check off only the
   families that apply to this specific change. If the change touches none of
   them, stop: the change is a non-trigger (see Non-triggers) and does not
   need this procedure.

2. **For each touched family, read its contract anchor before judging the
   change.** The anchor is the section of the canonical contract that owns the
   rule. Read the current text there, not a remembered summary. Do not load or
   copy the two contracts wholesale; retrieve only the named sections.

3. **For each touched family, require evidence at the same altitude as the
   risk.** The table states the high-value evidence shapes that actually prove
   the property. A passing unit suite is not evidence of a concurrency,
   recovery, or atomicity property; name the concrete failing case the test
   exercises and prove the outcome of that case.

4. **When the change adds a fence, check it at the identity level, not only
   the payload level.** Verifying that each stored payload matches its own
   recorded digest proves the payloads are intact; it does not prove the
   record is the one it claims to be. Ask what a swap of the whole record for
   another internally consistent one would do, and require the fence to catch
   that too.

5. **Keep transaction and schema design owned by the driving mission and the
   canonical architecture.** If the mission does not specify a table, state
   machine, recovery policy, or schema rule, do not invent one to satisfy this
   checklist.

6. **Keep this review distinct from independent review.** Applying this
   checklist to your own change is implementation self-review. It is not an
   independent review, does not mechanically grade a pull request, and does not
   claim semantic acceptance on its own — see
   `docs/development/change-lifecycle.md` and `pantheon-independent-review`.

7. **Finish through `./scripts/verify.sh`.** The canonical gate is
   `./scripts/verify.sh`, run and interpreted per
   `pantheon-change-verification`. Do not introduce a second verification
   command, a transaction test runner, an ORM, a database abstraction, or a
   persistence framework to satisfy this procedure.

## Invariant families

For each touched family: read the anchor section, establish the listed
concern, and require the listed evidence shape.

| Family | Touched when the change... | Contract anchor (read this section) | Establish / evidence |
|---|---|---|---|
| State-dependent authoritative write and transaction mode | starts an authoritative transaction whose decisions depend on read state, or runs inside one | `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` "SQLite operating rules" and "Named transaction families"; `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md` "25. SQLite operational requirements" | Confirm the transaction opens with the write mode the contract requires for a state-dependent authoritative write, revalidates its decision inside the transaction, and performs no external call inside it. Evidence: an injected mid-transaction failure proves full rollback, and a test asserts the required write mode is actually used. |
| Revision/CAS predicates with exact affected-row checks | updates a mutable authoritative row that carries a revision, or introduces such a table | `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` "Immutable documents versus mutable status" and "Named transaction families" | Establish how the mutation expresses the expected revision, how the affected-row count is checked, and how a zero-row outcome is treated. Evidence: two mutations based on the same observed revision — exactly one commits, the other fails deterministically. |
| Typed stale/conflict outcomes | a caller must distinguish a stale/conflict result from other failures: every revisioned mutation, command idempotency, CAS-based reconciliation | `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` "Immutable documents versus mutable status" and "Commands" | Establish that the stale/conflict outcome is distinguishable by type rather than a bare boolean or string, and that callers react to it correctly instead of retrying blindly. Evidence: the concurrent-race test asserts the loser receives the typed stale result and handles it. |
| Holder-safety foreign-key structure | adds or alters a reference between rows whose identity must not drift: Run/Task, Attempt/Run, Sandbox holder, ContextPlan attachment, EvaluationRound subject, finalization-obligation owner | `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` "Goal and Task", "Run status", "Attempt and launch-contact state", "Sandbox", "Context source snapshot and ContextPlan attachment", "Evaluation", "Explicit finalization obligations" | Establish whether identity is constrained through concrete composite foreign keys and XOR/FK checks rather than an opaque polymorphic holder field. Evidence: a schema-level test proves a cross-holder or wrong-holder reference cannot commit. |
| Cardinality and uniqueness enforcement | enforces a "one per" or "at most one nonterminal" rule, or adds an index or CHECK constraint | `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` "Goal and Task", "Run status", "Attempt and launch-contact state", "Planning", "Evaluation", "ResourceReservation" and "Invariant checker" | Establish which part of the rule is declared (partial unique index, CHECK) versus controller logic, and that both layers agree. Evidence: a test proves a second nonterminal row is rejected at the database layer, not only by the controller. |
| Command identity and idempotent replay | executes an authoritative mutation under durable command identity, or appends Event Journal rows | `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` "Commands", "Event Journal", "Named transaction families"; `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md` "27. Backups and disaster recovery" (Operator command identity after restore) | Establish the identity scope, same-identity same-hash versus same-identity different-hash behaviour, stale-epoch fail-closed before row lookup, and one atomic commit of state, command outcome, and events. Evidence: replay returns the prior outcome without re-execution, a different-hash conflict, a stale-epoch rejection, and a failure before commit that leaves no state, no completed command, and no event. |
| Restore-generation fencing | involves authority or idempotency a restored snapshot could rewind: commands, grants/tickets, broker operations, Agent Control sessions, the restore fence itself | `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` "Disaster-restore authority fence (T0)", "Commands", "Agent Control", "Grants and broker operation redemption"; `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md` "3. Installation identity, restore generation, and daemon incarnation" and "27. Backups and disaster recovery" | Establish what is generation-bound, where the current generation is compared, and that old-generation rows are fenced and never rewritten to current. Evidence: restore/recovery fencing cases — a consumed grant cannot redeem, a stale command epoch is rejected, an old-generation Agent Control session fails before request lookup. |
| Multi-statement read consistency | issues two or more reads whose answers must agree with each other, or changes which transaction a read runs in | `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` "SQLite operating rules"; `docs/architecture/operations/public-daemon-api-and-cli.md` "Gap-free list + Event watch" where a cursor is involved | Establish that the reads share **one explicit** read transaction. SQLite in autocommit gives each statement its own implicit transaction, so two statements in one helper can straddle a commit on the writer connection and return answers describing different states — and a comment claiming otherwise is not evidence. Evidence: a write committed between the two reads observed to be invisible to the second, plus a concurrency test asserting the derived invariant (every Event at or before the cursor has its row in the list; a fenced child is never shown beside an unfenced parent). |
| Launch-contact certainty | crosses an external-effect boundary: Attempt launch, Planning or Evaluation call, Sandbox, broker operation, or any restore-mode negative evidence | `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` "Attempt and launch-contact state", "Planning", "Evaluation", "Agent Control"; `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md` "9. External operation certainty" (including the restore-specific negative evidence rule) and "12. Execution and Sandbox recovery" | Establish the durable contact markers and their monotonicity, and that UNKNOWN contact authorizes no duplicate replacement work — including the restore rule that snapshot-only negative evidence is not post-snapshot proof of absence. Evidence: crash/reconciliation cases prove no overlapping replacement lineage is created while contact is ambiguous, and restore cases require fresh domain inspection before replacement work relies on a negative fact. |

## What this skill must not do

- It does not restate or copy the two persistence contracts; it points at their
  current sections. If the skill and the current contracts disagree, the
  contracts win and the skill is a defect to report, not a second authority.
- It does not decide tables, state machines, recovery policy, or schema rules
  for features the driving mission and the canonical architecture do not
  specify.
- It does not replace or add to `./scripts/verify.sh`, and it is not a
  transaction test runner, ORM, database abstraction, or persistence framework.
- It does not mechanically grade a pull request or claim semantic acceptance;
  that is independent review (`pantheon-independent-review`) from a distinct
  principal.
- It does not teach generic SQLite, generic Rust database handling, generic
  error-handling guidance, or speculative async/concurrency content.

## Non-triggers

- A **single-statement** read against existing schema whose answer nothing
  else must agree with. A read-only helper that issues more than one statement
  is not covered by this exemption — see the multi-statement read consistency
  family above.
- Pure `pantheon-core` domain work with no persistence involvement.
- CLI presentation work in `pantheon-cli`, or wire-format work in
  `pantheon-operator-protocol`.
- Unrelated Rust changes that do not touch persistence, transactions, schema,
  or recovery.