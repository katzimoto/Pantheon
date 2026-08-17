# Goal Resource

## Status

Canonical Pantheon Goal semantic-resource specification.

## Purpose

A `Goal` is the durable revisioned contract for the user's desired outcome. It survives TaskGraph changes, multiple Tasks/Runs/Attempts, replanning, daemon restart and context resets.

> **Goal describes the desired outcome, not an execution strategy. Task success does not imply Goal success.**

Lifecycle/finalization authority is defined by `docs/architecture/goals-and-planning/goal-lifecycle-and-completion-controller.md`; revision impact is defined by `docs/architecture/goals-and-planning/goal-revision-reconciliation.md`.

## Hierarchy

```text
USER REQUEST
  ↓
GOAL
  ↓
TASKGRAPH
  ↓
TASK
  ↓
RUN
  ↓
ATTEMPT
  ↓
ARTIFACT / CANDIDATE / EVIDENCE
```

Goal is not executable and never binds a provider/model/backend/session.

## Identity and revisions

Goal ID is stable. Semantic change creates a new immutable GoalRevision:

```text
goal_123 rev 1
  ↓ clarification/change
goal_123 rev 2
```

Original request/clarifications remain provenance. Tasks bind the Goal revision that caused their materialization. Planner/Graph/Scheduler/Completion work is fenced by the revision it observed.

Terminal Goals never reopen; later independent/follow-up work is a new Goal with optional provenance link.

## Semantic areas

A Goal contract contains:

```text
OBJECTIVE
INPUTS
DELIVERABLES
CONSTRAINTS
PREFERENCES
ACCEPTANCE
```

### Objective

Human-level desired outcome without prescribing concrete provider/model/orchestration steps.

### Inputs

Durable references relevant to the Goal, e.g. repository or Artifact refs.

### Deliverables

Named top-level output slots representing user-visible final results, not every intermediate Task Artifact.

Example:

```yaml
deliverables:
  - name: implementation
    kind: code.changeset
    required: true
  - name: architecture
    kind: architecture.document
    required: true
```

The evolving TaskGraph binds accepted immutable Task outputs to deliverable slots. Replanning may change which Task produces a slot without rewriting old Task history.

### Constraints

Mandatory requirements/ceilings. Descendants may tighten but cannot turn Goal constraints into new authority.

### Preferences

Optimization hints that may be traded off when constraints/capabilities require it. Preference is not a hard constraint.

### Acceptance

Goal acceptance uses the same Evaluation/Evidence architecture as Task acceptance, applied to an immutable `GoalCompletionCandidate`.

A Goal acceptance contract may contain trusted logical evaluator refs. When a GoalRevision becomes authoritative, Pantheon resolves those refs against the active trusted Evaluator Registry and pins the exact immutable `EvaluatorVersion` digests plus evaluator-resolution provenance into that immutable GoalRevision acceptance contract. Evaluation later consumes those pinned versions; it does not resolve whatever registry version happens to be current at Goal completion time.

Conceptually the immutable acceptance portion of a GoalRevision carries:

```text
acceptance contract digest
criterion IDs/statements/severity
logical evaluator refs
exact EvaluatorVersion digests
ConfigurationRevision used for resolution
evaluatorRegistryDigest
```

A later registry publication never silently changes an existing GoalRevision. If a pinned evaluator version becomes unavailable or forbidden under current hard/security policy, Pantheon does not substitute a newer version; Goal evaluation becomes blocked/reconciliation-required and a deliberate semantic GoalRevision may pin a replacement.

Pinning acceptance semantics never freezes obsolete security authority. Every EvaluationOperation still rechecks current hard policy/current authorization before execution.

A Goal may have zero explicit evaluator criteria; in that case accepted required deliverable bindings provide structural acceptance according to the Goal lifecycle contract.

## Lifecycle

Canonical Goal phases:

```text
Planning
Active
Evaluating
Finalizing
Succeeded
Failed
Cancelled
```

Initial Goal starts Planning until the first coherent TaskGraph exists. Later replanning is an Active condition rather than a phase regression.

Goal Completion Controller owns phase, completion readiness, GoalCompletionCandidate, Goal-level EvaluationRound, terminalTarget and finalization.

### Active

Pantheon is pursuing the current Goal revision. Any mix of Task states/replanning may exist; Goal Active does not imply an executor is currently running.

### Evaluating

Required deliverables are structurally bound/current reconciliation is complete and Pantheon has frozen one immutable current GoalCompletionCandidate.

### Finalizing

A terminal target (`Succeeded|Failed|Cancelled`) is durable; new planning/spawn/Scheduler Run creation for the Goal is fenced while all Goal-owned obligations are safely quiesced.

### Terminal

Terminal Goals never reopen.

## Deliverable binding

Only accepted immutable Task outputs may satisfy a Goal deliverable.

Binding records at least:

```text
Goal + revision
deliverable slot
Artifact digest
producer Task
producer Candidate digest
Graph revision
```

Raw Run output, mutable Workspace state or Agent narration cannot bind a deliverable.

## GoalCompletionCandidate

When structural completion is currently valid, Goal Completion Controller freezes a content-addressed snapshot:

```yaml
goalCompletionCandidate:
  goal:
    id: goal_123
    revision: 7
  graph:
    revision: 42
  deliverables:
    implementation:
      artifact: artifact://sha256/...
      producerCandidate: candidate://sha256/...
  acceptanceContract:
    digest: sha256:...
    evaluatorVersions:
      release-check: sha256:E1
  configRevision: cfgrev_...
  evaluatorRegistryDigest: sha256:ER
```

The candidate carries forward the GoalRevision's already-frozen acceptance contract/evaluator-resolution provenance. It does not re-resolve logical evaluator refs.

Candidate is immutable. New Goal revision makes the old completion candidate/evidence stale for current completion.

## Completion is not "all Tasks terminal"

Required deliverables plus Goal acceptance determine Goal success. Auxiliary work need not become a required output merely because it exists.

During successful Goal Finalizing, residual non-required Tasks are cancelled/finalized so the Goal does not terminalize while new/active execution remains.

## Terminal quiescence

`Succeeded` requires both:

```text
accepted GoalCompletionCandidate
+
no unresolved Goal-owned execution/control obligations
```

UNKNOWN descendant Attempt/Sandbox/resource/budget obligations keep Goal Finalizing until reconciled or explicitly force-resolved.

## Failure hierarchy

```text
Attempt failure != Run failure != Task failure != Goal failure
```

Failed Task may be replaced/replanned. Goal fails only when policy/controller determines no permitted path remains to satisfy the current Goal contract and finalization completes.

## Cancellation

Goal cancellation sets `Finalizing/terminalTarget=Cancelled`, fences new work, propagates cancellation to nonterminal Goal Tasks and reaches terminal Cancelled only after obligations are safely finalized.

## Goal revision while Evaluating

A new semantic GoalRevision invalidates the current GoalCompletionCandidate as current authority, stops/reconciles pending evaluation where safe and returns the Goal to Active reconciliation. Historical Candidate/Evidence remain immutable.

Normal semantic revisions are rejected once Goal Finalizing has selected a terminal target.

## No nested Goals in v1

One Goal owns one evolving TaskGraph. Bounded decomposition is TaskGraph work. Workers cannot create Goals to escape policy/budget/depth ceilings.

Dynamic child Tasks stay under the same Goal.

## Retention

Accepted final Goal deliverable Artifacts become retention roots before Goal reaches Succeeded so Artifact GC cannot delete the user's final result.

Goal success does not imply Git merge/push/deployment unless the Goal contract explicitly requires that external state/deliverable; integration remains separately authorized.

## Core invariants

1. Goal is a stable revisioned user-outcome contract, never provider/model/session strategy.
2. Tasks are immutable and record their creating Goal revision.
3. Objective/inputs/deliverables/constraints/preferences/acceptance are distinct.
4. Goal phases are Planning/Active/Evaluating/Finalizing/terminal and owned by Goal Completion Controller.
5. Task success does not imply Goal success; all-Tasks-terminal is not the completion predicate.
6. Only accepted immutable Task outputs bind deliverables.
7. GoalRevision pins exact trusted EvaluatorVersions for Goal acceptance when that semantic contract becomes authoritative; later registry movement does not silently alter it.
8. GoalCompletionCandidate freezes exact Goal/Graph revision, deliverables and the already-pinned Goal acceptance contract before Goal evaluation.
9. Goal revision invalidates stale completion evidence rather than rewriting history.
10. Terminal Goal requires accepted outcome plus quiescent/fenced subordinate obligations.
11. Current hard/security policy is rechecked at evaluation time and is not frozen merely because evaluator semantics were pinned.
12. No nested Goals/workers creating Goals in v1.
