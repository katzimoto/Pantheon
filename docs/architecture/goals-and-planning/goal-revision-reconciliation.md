# Goal Revision and Reconciliation

## Status

Canonical Pantheon Goal revision/reconciliation specification.

## Purpose

A Goal revision changes desired user outcome state; it does not mutate running Tasks directly.

> **Goal revision is desired state. Reconciliation decides which existing work remains valid, which requires revalidation, which must be superseded through normal finalization, and what new work must be planned.**

See also:

- `docs/architecture/goals-and-planning/goal-resource.md`
- `docs/architecture/goals-and-planning/goal-lifecycle-and-completion-controller.md`
- `docs/architecture/tasks/task-lifecycle.md`
- `docs/architecture/goals-and-planning/planner-and-task-decomposition.md`
- `docs/architecture/evaluation-and-acceptance/evaluation-and-evaluator-registry.md`

## Immutable revisions

Goal ID is stable; each semantic change creates a new immutable GoalRevision. Revision creation records provenance and advances `goals.current_revision` through optimistic CAS.

Terminal Goals do not reopen. Normal semantic revisions are allowed only while Goal lifecycle permits them; once Goal Finalizing/terminal, changed requirements become a new Goal.

## Goal acceptance pinning at revision commit

Goal acceptance semantics are part of the immutable GoalRevision, not something first decided when completion happens later.

If the proposed GoalRevision contains acceptance criteria with logical evaluator refs, Goal revision creation resolves those refs against the active trusted Evaluator Registry before the revision becomes authoritative. The committed GoalRevision pins:

```text
acceptance contract digest
criterion set
logical evaluator refs
exact immutable EvaluatorVersion digests
ConfigurationRevision used for evaluator resolution
evaluatorRegistryDigest
```

Conceptually the authoritative revision transaction performs:

```text
prepare/canonicalize proposed GoalRevision
resolve permitted logical evaluator refs
validate exact EvaluatorVersions
        ↓
BEGIN IMMEDIATE
re-read expected current Goal revision
re-read active ConfigurationRevision / trusted evaluator registry identity
verify proposed acceptance resolution still matches that revision
insert immutable GoalRevision with pinned acceptance contract
advance goals.current_revision by expected-revision CAS
create GoalReconciliation obligation
append Events
COMMIT
```

No evaluator process/backend execution occurs inside this transaction.

A registry change after commit never silently substitutes a new evaluator into the old GoalRevision. If a pinned version later becomes unavailable or forbidden by current hard/security policy, Goal evaluation blocks/reconciles; a deliberate newer GoalRevision may pin a different permitted version.

This semantic pinning does not freeze old authorization or hard-policy authority. Current policy is still checked when EvaluationOperations are admitted/executed.

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

Pinned evaluator identity is likewise semantic history, not execution permission. A previously pinned evaluator can be blocked by current hard/security policy without being silently rewritten to another evaluator version.

## Preferences versus constraints/acceptance

Preference-only Goal revision usually does not disrupt existing valid work. It affects future planning/routing decisions.

Constraint or acceptance changes may require:

- revalidation of existing Tasks/results;
- supersession/new Tasks;
- invalidation of a current GoalCompletionCandidate;
- new Goal-level evaluation work.

Changing Goal acceptance criteria or intentionally moving one of their EvaluatorVersions is itself a semantic GoalRevision. The reconciler does not mutate the old revision's pinned evaluator set.

The reconciler does not infer that every textual Goal change invalidates all Tasks.

## GoalCompletionCandidate invalidation

A GoalCompletionCandidate freezes exact Goal/Graph revision, deliverable bindings and the GoalRevision's pinned acceptance contract. If a newer Goal revision commits while Goal is Evaluating:

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
- define/replace trusted evaluator implementations dynamically;
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
3. GoalRevision commit resolves permitted logical Goal evaluator refs and pins exact immutable EvaluatorVersions plus evaluator-resolution provenance.
4. Registry evolution never silently changes an existing GoalRevision acceptance contract; changing pinned evaluator semantics requires a newer GoalRevision.
5. Current hard/security policy still gates evaluator execution and is not frozen by semantic evaluator pinning.
6. Planner/Graph/Scheduler/Completion actions are fenced by observed Goal revision.
7. Active/Evaluating Tasks classified SUPERSEDE pass through `Finalizing/terminalTarget=Superseded`; no direct terminalization around live Runs.
8. Terminal Tasks remain terminal.
9. Preference changes are generally non-disruptive; constraints/acceptance may cause revalidation/supersession/new work.
10. GoalCompletionCandidate/Evidence are revision-bound and stale after a semantic Goal revision.
11. Security tightening follows current Configuration/authorization authority and is not delayed by semantic Goal reconciliation.
