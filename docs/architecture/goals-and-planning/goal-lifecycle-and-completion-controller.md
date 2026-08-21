# Goal Lifecycle and Completion Controller

## Status

Canonical Pantheon Goal lifecycle and completion specification.

## Purpose

A Goal is Pantheon’s durable top-level user-outcome contract. This document defines who owns Goal status, when Pantheon may believe a Goal is complete, how Goal acceptance is evaluated, and what must be finalized before a Goal becomes terminal.

The central rule is:

> **Goal completion has two boundaries: first prove the user outcome against an immutable completion snapshot; then finalize all Goal-owned execution obligations before declaring the Goal terminal.**

## Goal Completion Controller

The Goal Completion Controller owns Goal phase, completion readiness, GoalCompletionCandidate creation/currentness, Goal-level evaluation coordination, terminalTarget, and Goal finalization.

It does not schedule Tasks, create Runs, perform planning, or execute evaluators directly. Evaluation Controller may produce Evidence/AcceptanceResult for the exact GoalCompletionCandidate subject, but it never owns Goal lifecycle transitions.

## Goal phases

Nonterminal:

```text
Planning
Active
Evaluating
Finalizing
```

Terminal:

```text
Succeeded
Failed
Cancelled
```

### Planning

The Goal exists but Pantheon has not yet established its first coherent TaskGraph. Initial planning/recovery occurs here.

Later replanning does not move the Goal back to Planning; the Goal remains Active with reconciliation/planning conditions.

### Active

Pantheon is still pursuing the current Goal revision. Tasks may be in any nonterminal or terminal phase. Goal Active does not imply an executor is currently running.

### Evaluating

The Goal Completion Controller has frozen one immutable GoalCompletionCandidate and Pantheon is evaluating Goal acceptance against that exact snapshot.

### Finalizing

Pantheon has selected a terminal target and has fenced creation of new Goal work while it drains/reconciles subordinate obligations and seals final output ownership.

## Terminal targets

Goal uses the same finalizer-style pattern as Task:

```yaml
phase: Finalizing
terminalTarget:
  outcome: Succeeded
  reason: GoalAcceptanceSatisfied
```

or Failed/Cancelled with an explicit reason.

Terminal Goals never reopen.

## Completion is not “all Tasks terminal”

Goal success is determined from required Goal deliverables and Goal acceptance, not by counting terminal Tasks.

Required deliverable slots are the structural completion roots.

Only accepted immutable Task outputs may bind Goal deliverables. A binding records at least the Goal revision, deliverable name, Artifact digest/ref, producing Task/Candidate, and Graph revision.

Raw Run output, live workspace state, unaccepted Candidates, or Agent claims cannot satisfy a Goal deliverable.

## Required work

For v1, required Task work is the dependency closure supporting the Tasks bound to required deliverables. Avoid a second independent `goalBlocking` flag.

Work that does not contribute to a required deliverable or explicit Goal acceptance criterion is auxiliary/follow-up work and does not intrinsically prevent completion.

## Completion readiness

A Goal may produce a completion candidate only when all required deliverable slots have valid accepted bindings and the current Goal revision has been reconciled to the current authoritative TaskGraph.

Conceptually:

```text
CompletionReady =
  all required deliverables bound
  AND current Goal revision reconciled
  AND current authoritative Graph revision compatible
  AND no unresolved reconciliation requires new required work
```

## GoalCompletionCandidate

When completion readiness holds, the Goal Completion Controller creates an immutable content-addressed GoalCompletionCandidate freezing at least:

```text
Goal ID + revision
TaskGraph revision
required deliverable bindings
producer Candidate digests
Goal acceptance-contract digest
exact pinned EvaluatorVersions from that GoalRevision
ConfigurationRevision used for evaluator resolution
evaluatorRegistryDigest
```

The GoalCompletionCandidate carries forward the GoalRevision's already-pinned acceptance semantics; candidate creation never resolves logical evaluator refs against the then-current registry.

Creating the current candidate is atomic with `Goal Active -> Evaluating`.

If the Goal revision advances while Evaluating, the old completion candidate remains immutable history but becomes stale for terminalizing the Goal. Pending evaluation is stopped where safe and the Goal returns to Active reconciliation.

## Goal acceptance

Goal acceptance reuses the same EvaluationRound/Evidence architecture as Task acceptance, with a different concrete immutable subject type.

For a Goal with explicit criteria, Goal Completion Controller creates/reuses an EvaluationRound whose concrete subject is exactly the current `GoalCompletionCandidate`. That Round contains no Task-Candidate ownership and uses the exact EvaluatorVersions already pinned by the GoalRevision acceptance contract.

Conceptually:

```text
GoalCompletionCandidate
        ↓
EvaluationRound(subject=GOAL_COMPLETION_CANDIDATE)
        ↓
EvaluationOperation / HumanEvaluationRequest
        ↓
Evidence
        ↓
AcceptanceResult
        ↓
Goal Completion Controller rechecks current Goal/candidate authority
```

V1 evaluator kinds remain `check`, `schema`, and `human`.

If the Goal has no explicit acceptance criteria, structurally valid required deliverable bindings are sufficient for Goal acceptance. Pantheon does not create fake EvaluationOperations/EvaluationAttempts merely to represent an empty criterion set.

## Accepted Goal candidate -> Finalizing

Acceptance PASS moves the Goal to Finalizing with `terminalTarget=Succeeded`; it never moves directly to Succeeded.

The acceptance-to-finalization transaction rechecks:

```text
Goal.phase == Evaluating
Goal.current_completion_candidate == EvaluationRound Goal subject
GoalRevision still current
EvaluationRound/AcceptanceResult applies to that exact candidate
all required criterion Evidence belongs to that Round and pinned EvaluatorVersions
no cancellation/new revision/finalization fence won
```

Then it atomically sets `Goal -> Finalizing / terminalTarget=Succeeded` and appends its Events. Evaluation Controller/evaluator never performs this lifecycle transition directly.

Entering Finalizing fences all new Goal work:

```text
no new Planner materialization
no task.spawn for the Goal
no new Scheduler Run commit for the Goal
no new ordinary EvaluationOperations
```

Existing obligations are reconciled toward closure.

## Successful finalization

Goal reaches Succeeded only after the accepted GoalCompletionCandidate exists and Goal-owned operational obligations are safely quiescent.

At minimum, finalization must ensure:

```text
no nonterminal Run under the Goal
no nonterminal Attempt under the Goal
no active EvaluationOperation under the Goal's current/stale evaluation rounds requiring closure
no unresolved reservations requiring active ownership
no unresolved BudgetHold requiring settlement
residual non-required Tasks finalized/cancelled
required deliverable retention pins established
```

UNKNOWN descendant execution keeps the Goal in Finalizing until reconciled or explicitly resolved under the Recovery architecture.

Residual auxiliary Tasks are cancelled with reason `GoalSatisfied`; they are not allowed to start after successful outcome acceptance.

## Cancellation

Goal cancellation is a desired-state transition, and it remains available in every nonterminal phase:

```text
Planning/Active/Evaluating
  -> Finalizing / terminalTarget=Cancelled

Finalizing / terminalTarget=Succeeded
Finalizing / terminalTarget=Failed
  -> Finalizing / terminalTarget=Cancelled    retarget

Finalizing / terminalTarget=Cancelled
  -> unchanged                                idempotent

Succeeded | Failed | Cancelled
  -> refused                                  conflict
```

The principle beneath the table: **Finalizing means the terminal outcome has not yet become immutable terminal history.** Cancellation can still win while finalization is running — including against a Goal whose accepted completion said `Succeeded`, or whose recovery policy had targeted `Failed` — by retargeting that same finalization to `Cancelled`. A retarget revises `terminal_target` through the normal authoritative CAS and nothing else: the Goal stays in `Finalizing`, drains the same obligations, and takes no shortcut around finalization. Once terminal state commits, history is immutable (invariant 18), and cancellation can no longer reopen or rewrite it.

Cancellation immediately fences creation of new Goal work, stops pending Goal evaluation where safe, and requests cancellation/finalization of all nonterminal Goal Tasks — whether it fenced the Goal itself or retargeted one already finalizing.

Completed Evidence/AcceptanceResult may remain immutable history but cannot cause Goal success after cancellation because Goal Completion Controller rechecks current Goal authority before applying it. The same recheck makes the opposite race safe: a PASS applying `Evaluating -> Finalizing/Succeeded` loses if cancellation won first.

Goal becomes Cancelled only when those obligations are safely finalized under the target now in force.

## Failure

Task failure does not automatically fail Goal.

The hierarchy is:

```text
Attempt failure != Run failure != Task failure != Goal failure
```

Planner/Recovery may replace work or find another path to a required deliverable. Goal fails only when control-plane policy determines that no permitted path remains to satisfy the current Goal contract.

Failure also passes through Finalizing.

## Revisions and lifecycle

Normal Goal revisions may be created while Planning, Active, or Evaluating. A revision during Evaluating invalidates the current completion candidate for terminalization and therefore invalidates its EvaluationRound/AcceptanceResult as current Goal authority, while retaining all immutable history.

Once Goal Finalizing begins, normal semantic revision is rejected. Cancellation remains available exactly as the Cancellation section defines: it retargets a finalization aimed elsewhere toward `Cancelled`, changes nothing when `Cancelled` is already the target, and is refused once the Goal is terminal. After terminalization, changed requirements become a new Goal, optionally related by provenance.

## Planning failure

Initial Planner failure is recoverable work and does not automatically fail the Goal. Goal remains Planning while Recovery may retry planning, change planning strategy, or request human input. Only an explicit terminal recovery decision moves the Goal toward Failed.

## Goal conditions

Useful conditions include:

```text
GraphInitialized
RevisionReconciled
RequiredDeliverablesBound
CompletionCandidateCurrent
AcceptanceSatisfied
ResidualTasksDrained
ExecutionQuiescent
BudgetSettled
DeliverablesPinned
```

Use conditions rather than proliferating Goal phases.

## Artifact retention

Accepted required Goal deliverables become retention roots owned by the Goal. Goal terminalization must not allow Artifact GC to remove the user’s final result under ordinary intermediate-task retention policy.

## Budgets

Goal terminalization creates no artificial refund. Historical UsageRecords/ChargeRecords remain factual. Finalization waits for outstanding Goal descendant/control-operation BudgetHolds to settle/reconcile. No new Goal spend authority is created after Finalizing begins.

## Integration boundary

Goal success does not imply Git merge/push/deployment unless the Goal contract explicitly requires integrated external state. Integration remains separately authorized and controller-owned.

## Persistence model

Conceptually:

```text
goals
  id
  current_revision
  phase
  terminal_target
  status_revision
  current_completion_candidate

goal_revisions
  immutable semantic content
  pinned Goal acceptance contract + EvaluatorVersions/evaluator-resolution provenance

goal_deliverable_bindings    immutable/history-preserving
goal_completion_candidates   immutable/content-addressed
```

Goal-level EvaluationRound ownership is represented by the Evaluation persistence model through a concrete FK to `goal_completion_candidates`, not by overloading `task_id` or an opaque generic subject string.

## Atomic completion transitions

Creation of the completion candidate rechecks current Goal/Graph revision and required deliverable bindings in one write transaction and atomically changes Active -> Evaluating.

For explicit criteria, EvaluationRound creation binds the concrete current GoalCompletionCandidate and its pinned acceptance contract. For zero criteria, structural acceptance can proceed without fake external evaluation state.

Acceptance-to-finalization rechecks that the GoalCompletionCandidate/revision/EvaluationRound/AcceptanceResult/Evidence are current and atomically changes Evaluating -> Finalizing with terminalTarget Succeeded.

Final terminalization rechecks all finalization obligations and atomically changes Finalizing -> terminal.

External cleanup/reconciliation never occurs inside the SQLite transaction.

## Core invariants

1. Goal Completion Controller exclusively owns Goal lifecycle/completion state.
2. Goal phases are Planning, Active, Evaluating, Finalizing, then Succeeded|Failed|Cancelled.
3. Goal success is not derived from all Tasks being terminal.
4. Required deliverables are structural completion roots.
5. Only accepted immutable Task outputs may bind Goal deliverables.
6. Completion readiness requires current Goal-revision/TaskGraph reconciliation.
7. GoalCompletionCandidate is immutable/content-addressed and carries the GoalRevision's already-pinned acceptance semantics.
8. Goal acceptance reuses EvaluationRound/Evidence through the concrete `GOAL_COMPLETION_CANDIDATE` subject type; it never overloads Task-Candidate ownership.
9. Goal acceptance PASS is applied only by Goal Completion Controller after current Goal/candidate authority is rechecked and goes to Finalizing, not directly Succeeded.
10. Zero-criterion Goal acceptance does not manufacture evaluator execution.
11. Finalizing fences all new Goal work.
12. Terminal Goal state requires subordinate execution/control obligations to be safely quiescent.
13. UNKNOWN external obligations block terminalization.
14. Task failure does not automatically imply Goal failure.
15. Goal has a durable terminalTarget.
16. Accepted Goal deliverables are Artifact-retention roots.
17. Goal success never implicitly performs Git integration/deployment.
18. Terminal Goals never reopen.
19. Cancellation is available in every nonterminal phase; while Finalizing it retargets the pending outcome to Cancelled rather than bypassing finalization, and a terminal Goal refuses it.
