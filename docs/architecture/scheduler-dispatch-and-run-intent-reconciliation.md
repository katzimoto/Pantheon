# Scheduler Dispatch and Run-Intent Reconciliation

## Status

Draft design — Pantheon scheduler handoff specification.

## Purpose

This document defines the final boundary of Pantheon scheduling: how a scheduler-selected, admitted, and transactionally reserved execution configuration becomes a durable Run intent and is then reconciled into actual backend execution.

The core rule is:

> **The Scheduler does not launch executors. Its responsibility ends when it atomically creates a durable Run intent, associated ExecutionBinding, ResourceReservations, BudgetHolds, and transitions the Task to Active. The Run Controller owns all execution after that handoff.**

This keeps scheduling, execution control, and backend implementation cleanly separated.

See also:

- `docs/architecture/task-lifecycle.md`
- `docs/architecture/execution-fabric.md`
- `docs/architecture/execution-offer-routing-and-admission-handshake.md`
- `docs/architecture/scheduler-reservations-ownership-and-leases.md`
- `docs/architecture/scheduler-resource-ledger-and-admission.md`

## Architectural boundary

```text
Scheduler
   │
   │ atomic commitment
   ▼
ExecutionBinding
ResourceReservations
BudgetHolds
Run intent
Task → Active
   │
   └──────── SCHEDULER ENDS HERE
                │
════════════════╪══════════════════
                │
          RUN CONTROLLER
                │
                ▼
        prepare environment
                │
                ▼
        ensure execution
                │
                ▼
         observe/reconcile
                │
       ┌────────┼─────────┐
       ▼        ▼         ▼
    running   exited    unknown
```

## 1. Task becomes Active when Run intent commits

Pantheon must not wait for a backend process or session to report RUNNING before transferring ownership from the Scheduler to a Run.

The Task transition is:

```text
Ready
  ↓
atomic Run-intent transaction
  ↓
Active
```

The same transaction creates the Run and records it as the Task's active owner.

`Active` means:

> A nonterminal Run is currently responsible for progressing this Task.

It does not mean that an external executor is necessarily consuming CPU or responding to messages at that instant.

### v1 invariant

```text
Task.phase == Active
        ⇒
exactly one nonterminal Run owns responsibility
for progressing that Task
```

Speculative duplicate Runs are out of scope for v1.

## 2. Atomic scheduler handoff

The final scheduling transaction is conceptually:

```text
BEGIN WRITE TRANSACTION

1. verify SchedulingClaim ownership
2. verify Task is still Ready
3. verify Goal revision is current
4. verify Graph revision is current
5. verify Policy revision is current
6. verify selected ExecutionOffer is still valid
7. verify all effective resource claims still fit
8. verify all BudgetHolds still fit
9. create ResourceReservations
10. create BudgetHolds
11. create immutable ExecutionBinding
12. create Run intent
13. Task Ready → Active
14. set Task.activeRunId
15. consume SchedulingClaim

COMMIT
```

No network calls or external executor operations occur inside this transaction.

After commit:

- the Scheduler no longer owns the Task;
- the SchedulingClaim no longer exists;
- fairness accounting records successful scheduler service;
- the Run Controller owns execution reconciliation.

## 3. Run desired state and observed execution are separate

A Run expresses desired execution state independently from what the backend is currently observed doing.

Conceptually:

```yaml
run:
  id: run_123
  binding: binding_456

  desired:
    execution: running

  status:
    observedExecution: unknown
```

The full Run schema is defined by the Run subsystem, not here.

The important invariant is:

```text
desired execution != observed execution
```

The Run Controller continually reconciles the external world toward the desired Run state.

## 4. Use ensure-execution semantics, not naive launch semantics

The Execution Fabric should avoid an imperative contract whose semantics are merely:

```text
launch something now
```

Instead, the backend contract should provide idempotent ensure semantics:

```text
ensureExecution(binding, launchKey, attachment?)
```

Meaning:

> Ensure that the execution represented by this immutable ExecutionBinding and LaunchKey exists exactly once, or return the strongest observation that can be established about it.

The backend implementation may internally:

- discover an existing native execution;
- attach or reattach to it;
- recover adapter-private state;
- create a new native execution if absence is established;
- inspect current native execution state.

Pantheon core does not distinguish provider-specific launch, recovery, attach, or reconnect mechanisms.

## 5. LaunchKey remains stable for the entire Run

Each Run has an immutable LaunchKey created before execution is attempted.

Every reconciliation attempt uses the same key.

Conceptually:

```text
Run run_123
   │
LaunchKey launch_ABC
   │
ExecutorBackend executor://A
   │
opaque native execution
```

Backend behavior must be idempotent with respect to the LaunchKey.

If native infrastructure does not offer such a primitive, the backend adapter is responsible for maintaining enough durable private state to provide equivalent semantics.

Transport retries, daemon restart, backend-plugin restart, or reconnect attempts do not create a new Run and do not create a new LaunchKey.

## 6. Backend attachment state is opaque

Once native execution exists, a backend may need implementation-specific state to recover or reattach later.

Pantheon may persist an opaque versioned attachment such as:

```yaml
backendAttachment:
  backend: executor://A
  schemaVersion: 3
  opaqueState: ...
```

Pantheon core stores and returns this state to the owning backend but never interprets it.

Concrete provider session IDs, process details, runtime handles, or transport state therefore remain below the Execution Fabric boundary.

## 7. Run Controller owns every post-binding action

After the scheduler transaction commits, the Run Controller owns all execution-side reconciliation.

Its conceptual loop is:

```text
read desired Run state
        ↓
read persisted observation / attachment
        ↓
inspect / reconcile ExecutorBackend
        ↓
compare desired vs observed
        ↓
perform only the necessary action
        ↓
persist new observation
```

The Scheduler must not later:

- relaunch the backend;
- switch backend;
- release Run reservations;
- select another offer;
- retry the execution itself.

Those concerns belong to Run reconciliation and later retry/escalation policy.

## 8. Preparation occurs after durable Run intent

External preparation may include:

- worktree materialization;
- sandbox or container preparation;
- logical Agent / Genome compilation;
- skill materialization;
- secret-handle preparation;
- policy compilation;
- PTY or session infrastructure;
- logging and event plumbing.

These are side effects and therefore happen only after Run intent is durable.

Preparation controllers must be idempotent.

Conceptually:

```text
Run intent
    ↓
Preparation Controllers
    │
    ├─ workspace prepared
    ├─ sandbox prepared
    ├─ context compiled
    └─ authorization environment ready
    ↓
LaunchReady = True
    ↓
ExecutorBackend.ensureExecution(...)
```

## 9. LaunchReady is a derived condition, not a lifecycle phase

Preparation detail should not explode Run or Task phases.

Use conditions such as:

```yaml
conditions:
  - type: WorkspaceReady
    status: "True"

  - type: SandboxReady
    status: "True"

  - type: PolicyReady
    status: "True"

  - type: LaunchReady
    status: "True"
```

This follows Pantheon's general lifecycle design: few high-level phases with rich controller-owned conditions.

## 10. Current authority is checked before external side effects

ExecutionBinding records the exact routing and policy snapshot used when the Run was created, but it does not grant indefinite future authority.

Before each consequential external action, the Run Controller must recheck current enclosing authority, including where applicable:

- desired Run state still permits execution;
- current Goal revision/reconciliation does not forbid continuation;
- current hard policy permits the action;
- required ResourceReservations remain valid;
- capability grants/tickets remain valid where relevant;
- current ownership epoch is valid.

If authority has been revoked since the Run was created, Pantheon must not launch or continue performing newly forbidden side effects merely because the immutable Binding was once valid.

The Binding remains immutable audit history.

## 11. Minimal normalized execution observations

Pantheon core should understand only a small normalized set of external execution observations:

```text
ABSENT
STARTING
RUNNING
EXITED
UNKNOWN
```

Backend adapters translate their implementation-specific states into these observations.

### ABSENT

The backend can positively establish that no execution exists for the Run's LaunchKey.

If desired state is `running`, creation or recreation may be safe.

### STARTING

Execution is known to exist but is not yet ready for normal interaction.

Execution capacity remains reserved and active.

### RUNNING

Execution is known to exist and is capable of performing assigned work.

### EXITED

The backend can positively establish that execution terminated.

This is not Task success. Semantic completion still requires a candidate result and the Acceptance subsystem.

### UNKNOWN

Pantheon cannot establish whether execution exists or what its current state is.

UNKNOWN is fail-closed:

- the Run remains the owner;
- associated execution reservations remain charged / uncertain;
- Pantheon must not start a duplicate replacement execution merely because observation is unavailable.

## 12. ABSENT and UNKNOWN are fundamentally different

This distinction must be preserved everywhere.

```text
ABSENT
= evidence that execution does not exist

UNKNOWN
= insufficient evidence either way
```

Only ABSENT can justify safe creation/recreation under the same Run when desired state remains running.

UNKNOWN triggers requery, reattachment, backend repair, or later recovery policy, but never speculative duplicate execution in v1.

## 13. ensureExecution returns structured certainty

A backend operation should expose more than generic success/error.

Conceptually, useful outcomes include:

```text
ESTABLISHED
  observed = STARTING or RUNNING

TERMINAL
  observed = EXITED

DEFINITIVE_FAILURE
  backend proves execution was not created
  and no execution-side effect remains

UNKNOWN_OUTCOME
  execution may or may not exist
```

A missing executable discovered before any start side effect is an example of `DEFINITIVE_FAILURE`.

A transport failure occurring during a native start operation is an example of `UNKNOWN_OUTCOME` unless the adapter can positively determine the final state.

## 14. Reconciliation retries stay within the same Run

The following do not by themselves create a new Run:

- transport error;
- timeout while checking status;
- backend adapter restart;
- daemon restart;
- reconnect attempt;
- lost event stream;
- reattachment attempt.

Pantheon continues reconciling:

```text
same Run
same ExecutionBinding
same LaunchKey
```

A new Run is created only after the current Run is formally concluded and higher-level retry/escalation policy selects another execution strategy.

## 15. Backend events are hints, not lifecycle authority

Backends may provide event streams, PTY EOF, callbacks, process watchers, or other notifications.

Such events should trigger reconciliation:

```text
backend event
    ↓
enqueue Run reconciliation
    ↓
inspect/reconcile current state
    ↓
persist authoritative Pantheon observation
```

Events themselves must not directly mutate authoritative Run or Task lifecycle state.

Periodic reconciliation remains a safety net.

## 16. Event handling must tolerate duplication and reordering

Backend events may arrive late, more than once, or out of order.

For example:

```text
RUNNING
EXITED
```

may be delivered as:

```text
EXITED
RUNNING
```

Pantheon must not resurrect terminated execution based on a stale event.

Events are therefore hints; authoritative state follows reconciliation plus Pantheon's monotonic lifecycle invariants.

## 17. Observation states are not a mandatory linear state machine

Pantheon may first observe an execution only after it has already terminated.

Therefore this is valid:

```text
ABSENT / UNKNOWN → EXITED
```

without Pantheon ever observing STARTING or RUNNING.

The normalized values are observations, not required sequential phases.

## 18. Persist observation before consequential follow-up

When the Run Controller establishes a meaningful state transition, it first persists the new observation and only then performs derived actions.

Example:

```text
observe EXITED
      ↓
persist EXITED observation
      ↓
reconcile Run outcome
      ↓
release eligible Run-scoped resources
      ↓
Task/retry/acceptance controllers react
```

Pantheon must not release resources, trigger retries, or perform integration before recording the state that justifies those actions.

## 19. Backend exit is not semantic result submission

External execution termination and Task completion are separate facts.

```text
backend execution exited
        !=
Task candidate submitted
```

A worker normally submits structured output through Pantheon (`task.submit_result` or equivalent) before or during executor shutdown.

If execution exits without a valid candidate result, the Run may have failed even though the backend process itself terminated normally.

Task success still requires the Acceptance subsystem to pass the candidate result.

## 20. Candidate evaluation may outlive execution

A valid candidate may be submitted before the external executor terminates.

Then:

```text
candidate submitted
      ↓
executor exits
      ↓
Task Evaluating
```

Run-scoped execution resources may be released once exit is confirmed, while Task-scoped resources such as a worktree may remain reserved for evaluation, review, or finalization.

This preserves the distinction between Run-scoped and Task-scoped reservations.

## 21. Scheduler fairness is charged at Run-intent commit

A Goal receives scheduler service once the final Run-intent transaction successfully commits.

Fairness accounting does not wait for the backend to report RUNNING.

At commitment:

- execution/budget capacity has been successfully allocated;
- the Task has transferred to Active;
- a durable Run owns responsibility.

Subsequent launch failure is a Run/execution outcome, not evidence that the Goal was never scheduled.

## 22. SchedulingClaim is consumed at handoff

The SchedulingClaim is a pre-Run coordination primitive only.

Once Run intent commits:

```text
SchedulingClaim → consumed
Task → Active
Run → owner
```

The claim does not survive until backend launch completes.

The Task cannot re-enter scheduler consideration unless a later lifecycle decision returns it to Ready.

## 23. Daemon restart recovers Runs directly

On daemon restart:

```text
load all nonterminal Runs
        ↓
acquire/adopt ownership epoch
        ↓
read desired execution state
        ↓
load BackendAttachment if any
        ↓
ExecutorBackend inspect/ensure/reconcile
        ↓
persist observation
```

Active Tasks are not returned to the scheduler merely because the daemon restarted.

The Run Controller owns recovery of committed execution.

## 24. Future dangling-execution detection

A backend may eventually need to enumerate executions tagged or identifiable as Pantheon-owned so reconciliation can detect native execution that exists without a corresponding current Run record.

Conceptually:

```text
backend inventory
      ↓
Pantheon-owned native executions
      ↓
compare with durable Runs
      ↓
managed / dangling / unknown ownership
```

This is deferred from v1 core dispatch semantics.

If a dangling execution is detected, Pantheon should quarantine/report and reconcile conservatively before destructive cleanup. It must not blindly kill opaque or uncertain work.

## Final scheduler boundary

```text
TaskGraph
    ↓
Ready
    ↓
S1 Eligibility
    ↓
Goal fairness / Task ordering
    ↓
SchedulingClaim
    ↓
ExecutionRequest
    ↓
Execution Fabric offers
    ↓
Routing
    ↓
Admission
    ↓
Atomic commitment
    ├─ ResourceReservations
    ├─ BudgetHolds
    ├─ ExecutionBinding
    ├─ Run intent
    └─ Task → Active
            │
════════════╪════════════════════
 Scheduler  │        Run control
   DONE     │
            ▼
      Run Controller
            ↓
       preparation
            ↓
      ensureExecution
            ↓
       reconciliation
```

## v1 scope

Include:

- atomic Task Ready → Active + Run creation handoff;
- exactly one nonterminal Run owner per Active Task;
- desired-vs-observed execution state;
- immutable LaunchKey;
- idempotent ensure-execution semantics;
- opaque backend attachment state;
- idempotent preparation controllers;
- LaunchReady and preparation conditions;
- current-authority rechecks before external side effects;
- normalized observations: ABSENT, STARTING, RUNNING, EXITED, UNKNOWN;
- structured definitive-vs-unknown backend outcomes;
- reconciliation retries within the same Run;
- backend events as hints;
- restart recovery through the Run Controller.

Defer:

- speculative duplicate Runs;
- generic dangling-execution reaping;
- distributed Run controllers;
- complex suspension/migration semantics;
- backend-specific lifecycle states in core;
- full Run/Attempt lifecycle and failure taxonomy, defined separately.

## Key decisions

1. **The Scheduler ends when the atomic Run-intent transaction commits.**
2. **Task Ready → Active occurs in that transaction, before external execution necessarily exists.**
3. **Exactly one nonterminal Run owns an Active Task in v1.**
4. **Desired Run state and observed backend execution state are separate.**
5. **Backend execution uses idempotent ensure-execution semantics and an immutable LaunchKey.**
6. **Backend reattachment state is opaque to Pantheon core.**
7. **Run Controller owns every execution-side action after the binding commits.**
8. **Preparation is post-commit, idempotent, and expressed through conditions rather than lifecycle phase explosion.**
9. **Current authority is rechecked before consequential external actions.**
10. **ABSENT and UNKNOWN are different; UNKNOWN never authorizes duplicate execution.**
11. **Transport/recovery retries remain within the same Run.**
12. **Backend events trigger reconciliation but do not authoritatively mutate lifecycle state.**
13. **Executor exit does not imply Task success.**
14. **Goal fairness is charged at successful Run-intent commitment.**
15. **Daemon restart recovers committed Runs directly through the Run Controller, not through the Scheduler.**
