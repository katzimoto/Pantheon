# Task Lifecycle and State Machine

## Status

Draft design — Pantheon task subsystem specification.

## Purpose

Pantheon separates the immutable Task contract from mutable execution state. The Task specification describes the required outcome; `TaskStatus` describes the controller-observed lifecycle of that Task.

The lifecycle must remain durable across worker failure, provider failure, Pantheon restarts, dynamic child Tasks, acceptance evaluation, cancellation, supersession, and finalization.

## Foundational principles

1. **TaskSpec is immutable; TaskStatus is controller-owned mutable state.**
2. **Task phase is intentionally small.** Operational detail belongs in conditions, reasons, Run state, and Attempt state.
3. **Only Pantheon transitions Task phase.** Workers submit commands/events but do not write status directly.
4. **Attempt failure is not Run failure; Run failure is not Task failure.**
5. **Waiting is durable and consumes no executor slot.**
6. **Terminal Tasks never reopen.** Further work requires a new or superseding Task.
7. **Acceptance and finalization are distinct.** A Task cannot transition directly from Active to Succeeded.
8. **State transitions are atomic, versioned, idempotent, and auditable.**

## v1 phases

Non-terminal:

```text
Pending
Ready
Active
Waiting
Evaluating
Finalizing
```

Terminal:

```text
Succeeded
Failed
Cancelled
Superseded
```

## Phase semantics

### Pending

The Task exists but is not yet eligible for scheduling.

Typical reasons:

- dependencies are unsatisfied;
- graph gate is unsatisfied;
- Task has not yet been activated.

Resource scarcity is not normally a Pending condition. If a Task is logically runnable but no suitable executor is currently available, the Task remains Ready and the scheduler waits.

### Ready

The Task is eligible for execution.

Typical readiness requirements:

- required dependencies are satisfied;
- policy and scope are valid;
- the Task is not terminal;
- graph activation conditions are satisfied.

The scheduler may create a Run for a Ready Task.

Queueing is Run state, not Task state.

### Active

A Run is currently responsible for making progress toward the Task outcome.

`Active` does not guarantee that a specific OS process is alive at every instant. Provider sessions, leases, heartbeats, process IDs and worker health belong to Run/Attempt state.

If a worker disappears, Pantheon reconciles the Run before deciding whether to return the Task to Ready or move toward terminal failure.

### Waiting

The Task remains live but cannot currently make useful progress and should not consume an executor slot.

Typical reasons:

```text
ChildJoin
ExternalEvent
HumanInput
RetryBackoff
ManualPause
```

Waiting is durable state, not an in-memory future or a worker polling loop.

For a blocking spawned child:

```text
Task A: Active
  ↓ spawn blocking Task B
Task A: Waiting / ChildJoin
Task B: Pending → Ready → Active
  ↓ Task B completes
Task A: Active or Ready
```

Whether the original Run can resume or a new Run must be created is a Run/harness concern.

### Evaluating

A worker has submitted a candidate result and Pantheon is evaluating the Task acceptance contract.

The candidate and relevant artifacts are frozen/bound by digest for authoritative evaluation.

Evaluation may include deterministic checks, policy evaluation, independent review, model rubrics, or human criteria.

A rejected candidate does not necessarily fail the Task. Recovery policy may return the Task to Ready for another Run.

### Finalizing

Pantheon has selected a terminal target and is completing idempotent finalization before entering that terminal phase.

Typical finalization work:

- seal artifacts;
- persist audit state;
- close/release provider sessions;
- release sandbox/worktree resources;
- notify joins/dependents;
- update graph state;
- perform separately authorized integration actions.

Finalization must be restart-safe and idempotent.

A generic terminal target avoids separate transient phases such as Cancelling, Failing, Superseding and Completing.

Example:

```yaml
phase: Finalizing
terminalTarget:
  outcome: Succeeded
  reason: AcceptanceSatisfied
```

or:

```yaml
phase: Finalizing
terminalTarget:
  outcome: Superseded
  reason: ReplacedByNewTask
  task: task_837
```

## Terminal phases

### Succeeded

The acceptance contract was satisfied and finalization completed.

### Failed

Pantheon determined that no permitted recovery path remains or that the Task cannot/should not continue toward success.

Examples:

- recovery exhausted;
- unrecoverable policy failure;
- required dependency permanently failed;
- acceptance cannot be satisfied under the current Task contract.

A single failed Attempt or Run does not automatically imply Task failure.

### Cancelled

An authority intentionally stopped the Task.

Examples:

- user cancellation;
- Goal cancellation;
- cancellation propagated from an attached parent;
- policy-driven cancellation.

Cancellation is distinct from failure.

### Superseded

The Task is no longer authoritative because a newer Task replaced it.

Supersession preserves historical truth when requirements/design change rather than misclassifying the old Task as Failed or Cancelled.

Terminal phases are immutable. A terminal Task never returns to Ready, Active, or another non-terminal phase.

## Conditions

Detailed state is represented with orthogonal conditions rather than by proliferating top-level phases.

Conceptual condition:

```yaml
type: ChildJoinSatisfied
status: "False" # True | False | Unknown
reason: WaitingForTask
message: task_812 has not completed
lastTransitionAt: ...
observedTaskRevision: 3
observedGraphRevision: 28
```

Recommended condition status values:

```text
True
False
Unknown
```

`Unknown` is important during reconciliation when Pantheon does not yet know the truth of an external condition.

Useful condition types may include:

```text
DependenciesSatisfied
ChildJoinSatisfied
AcceptanceSatisfied
RunHealthy
Blocked
PolicySatisfied
```

`Blocked` is a condition, not a phase. The same applies to queueing and retrying: these are better represented by conditions/reasons or Run/Attempt state.

## Proposed TaskStatus shape

```yaml
status:
  phase: Waiting
  reason: ChildJoin
  message: Waiting for authentication research.

  version: 17
  observedTaskRevision: 3
  observedGraphRevision: 28

  activeRun:
    ref: run_938

  latestRun:
    ref: run_938

  terminalTarget: null

  conditions:
    - type: DependenciesSatisfied
      status: "True"
      reason: AllDependenciesCompleted
      lastTransitionAt: ...

    - type: ChildJoinSatisfied
      status: "False"
      reason: WaitingForTask
      message: task_812 has not completed
      lastTransitionAt: ...

    - type: AcceptanceSatisfied
      status: "Unknown"
      reason: NoCandidateSubmitted
```

## Normal transition path

```text
materialize
    ↓
Pending
    ↓ prerequisites satisfied
Ready
    ↓ Run created
Active
   ├─ needs durable wait ─→ Waiting ─→ Active/Ready
   └─ submit result ─────→ Evaluating
                              ↓ accepted
                          Finalizing
                              ↓
                          Succeeded
```

Candidate rejection or recoverable Run failure may return the Task to Ready.

Any non-terminal phase may move to Finalizing with a terminal target when cancellation, supersession or unrecoverable failure is selected.

## Transition authority

Workers emit semantic requests/events such as:

```text
task.submit_result
task.spawn.requested
task.wait.requested
task.cancel.requested
```

Pantheon validates these and performs the actual TaskStatus transition.

Workers never directly write `status.phase`.

## Atomic transitions and audit history

Pantheon should maintain both:

```text
materialized TaskStatus
+
append-only Task events
```

A phase transition occurs transactionally:

```text
BEGIN
read current state/version
validate transition
update TaskStatus + increment version
append TaskPhaseChanged event
COMMIT
```

Example event:

```yaml
event: TaskPhaseChanged
task: task_123
from: Active
to: Waiting
reason: ChildJoin
actor:
  type: controller
causedBy:
  event: spawn_892
transitionVersion: 18
```

## Compare-and-swap concurrency

Mutating commands include an expected TaskStatus version.

If two events race, only one transition may commit from a given version. The losing operation must re-read state and re-evaluate its action rather than blindly overwriting the winner.

This protects cases such as simultaneous user cancellation and acceptance success.

## Idempotent commands

All mutating controller commands should carry a stable `commandId`/idempotency key.

Retries of the same command must not duplicate:

- candidate submissions;
- evaluations;
- finalization;
- graph mutations;
- cancellation transitions.

## Durability and reconciliation

On controller restart Pantheon loads all non-terminal Tasks and reconciles them against durable graph state, Run state, evaluator state and external provider state.

Examples:

```text
Active + healthy Run lease
→ remain Active

Active + lost Run
→ recovery decision

Waiting / ChildJoin + join now satisfied
→ Active or Ready

Evaluating + evaluator incomplete
→ resume/restart evaluator

Finalizing + cleanup partially complete
→ continue idempotent finalization
```

Waiting relationships must never depend on an in-memory Rust future or a model process polling for completion.

## Failure hierarchy

Pantheon preserves three separate failure scopes:

```text
Attempt failure ≠ Run failure
Run failure     ≠ Task failure
```

A Task enters Failed only when the control plane determines that no valid recovery path remains or a terminal failure rule applies.

## v1 invariants

1. `Active → Succeeded` is illegal.
2. Success must pass through `Evaluating → Finalizing → Succeeded`.
3. Failed Attempts/Runs do not automatically fail the Task.
4. Waiting consumes no executor slot.
5. Terminal Tasks never reopen.
6. Dynamic child waits are durable graph relationships.
7. Only Pantheon transitions Task phase.
8. Transitions are atomic, versioned, idempotent and auditable.
9. Non-terminal state is reconciled after controller restart.
