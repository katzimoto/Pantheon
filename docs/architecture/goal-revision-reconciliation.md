# Goal Revision and Reconciliation

## Status

Canonical Pantheon Goal revision/reconciliation specification.

## Purpose

A Goal revision changes desired user outcome state; it does not mutate running Tasks directly.

> **Goal revision is desired state. Reconciliation decides which existing work remains valid, which requires revalidation, which must be superseded through normal finalization, and what new work must be planned.**

See also:

- `goal-resource.md`
- `goal-lifecycle-and-completion-controller.md`
- `task-lifecycle.md`
- `planner-and-task-decomposition.md`

## Immutable revisions

Goal ID is stable; each semantic change creates a new immutable GoalRevision. Revision creation records provenance and advances `goals.current_revision` through optimistic CAS.

Terminal Goals do not reopen. Normal semantic revisions are allowed only while Goal lifecycle permits them; once Goal Finalizing/terminal, changed requirements become a new Goal.

## Revision fence

Planner proposals, Graph patches, Scheduler decisions, completion candidates and reconciliation work bind the Goal revision they observed.

Before authoritative commit, Pantheon rechecks that the expected current Goal revision is still valid. Stale work is re-evaluated rather than blindly applied.

## Reconciliation classifications

Each existing Task/work item is classified relative to the new revision as one of:

```text
STILL_VALID
REVALIDATE
SUPERSEDE
NEW_WORK
```

Classification is controller/planner-owned structured state, not a model's direct lifecycle mutation.

### STILL_VALID

The Task contract/result remains compatible with the revised Goal. Running/accepted work continues normally.

### REVALIDATE

Existing immutable work may remain useful but one or more compatibility/acceptance assumptions need re-evaluation. Revalidation never rewrites the TaskSpec.

### SUPERSEDE

The old Task is no longer authoritative for the revised Goal. The Task is not deleted or rewritten; it moves through normal Task finalization.

### NEW_WORK

Additional immutable Tasks/edges/bindings are materialized through a validated GraphPatch.

## Critical supersession rule

Goal reconciliation **must never terminalize an Active/Evaluating Task as `Superseded` while its responsible Run is nonterminal.**

Correct path:

```text
Goal revision commits
  ↓
reconciler classifies Task SUPERSEDE
  ↓
Task -> Finalizing / terminalTarget=Superseded
  ↓
responsible Run desiredExecution=stopped
Run/Attempt reconciled toward safe terminal state
Reservations/Holds/finalizers settled as required
  ↓
Task -> Superseded
```

If Run/Attempt termination is UNKNOWN, Task remains Finalizing and the unresolved obligation stays fenced. No terminal Task is manufactured around a live/unknown Run.

This preserves:

```text
Task terminal => no nonterminal responsible Run
```

and avoids false RecoveryFindings/quarantine caused by inconsistent lifecycle state.

## Security tightening

Security authority is special: a new active configuration/hard-policy tightening can immediately deny future brokered operations independent of Goal semantic reconciliation. If the current Sandbox cannot physically enforce the new ceiling, Run is stopped/finalized.

Goal reconciliation does not weaken current security merely because an older Goal revision allowed broader behavior.

## Preferences versus constraints/acceptance

Preference-only Goal revision usually does not disrupt existing valid work. It affects future planning/routing decisions.

Constraint or acceptance changes may require:

- revalidation of existing Tasks/results;
- supersession/new Tasks;
- invalidation of a current GoalCompletionCandidate;
- new Goal-level evaluation work.

The reconciler does not infer that every textual Goal change invalidates all Tasks.

## GoalCompletionCandidate invalidation

A GoalCompletionCandidate freezes exact Goal/Graph revision and deliverable bindings. If a newer Goal revision commits while Goal is Evaluating:

- old completion candidate remains immutable history;
- its pending evaluation may be stopped when safe;
- its Evidence cannot satisfy the new revision;
- Goal returns to Active reconciliation according to the Goal lifecycle contract.

## Task immutability

A running/completed TaskSpec is never edited to match the new Goal. If contract change is necessary, Planner materializes a replacement/superseding Task with provenance linking it to the old work.

Outputs from old Tasks may still be reused/bound where the new Task/Goal contract explicitly accepts them.

## Planner responsibility

Reconciler records deterministic impact state and asks Planner for a bounded GraphPatch only where structural/semantic decomposition changes are required.

Planner may not:

- mutate old Task specs/status directly;
- assign concrete backend/model;
- bypass supersession finalization;
- create a scheduled Run directly.

## Reconciliation durability

Goal revision transaction creates the immutable revision/current pointer plus a durable reconciliation obligation/Event. Reconciliation is restart-safe and revision-bound.

Conceptually:

```text
GoalRevision R+1 committed
  ↓
GoalReconciliation(goal,R+1,state=PENDING)
  ↓
impact classification / GraphPatch
  ↓
Task finalization/new work
  ↓
reconciliation COMPLETE
```

Scheduler/Goal completion readiness requires the current Goal revision to be reconciled according to the relevant fence rules.

## Cancellation/terminal Goal interaction

If Goal cancellation/finalization wins while revision reconciliation is in progress, enclosing Goal terminal authority fences new planning/materialization. Existing reconciliation work becomes stale and Tasks follow Goal cancellation/finalization semantics.

## Core invariants

1. Goal changes create immutable revisions; history is never rewritten.
2. Goal revision is desired state, not direct running-state mutation.
3. Planner/Graph/Scheduler/Completion actions are fenced by observed Goal revision.
4. Active/Evaluating Tasks classified SUPERSEDE pass through `Finalizing/terminalTarget=Superseded`; no direct terminalization around live Runs.
5. Terminal Tasks remain terminal.
6. Preference changes are generally non-disruptive; constraints/acceptance may cause revalidation/supersession/new work.
7. GoalCompletionCandidate/Evidence are revision-bound and stale after a semantic Goal revision.
8. Security tightening follows current Configuration/authorization authority and is not delayed by semantic Goal reconciliation.
