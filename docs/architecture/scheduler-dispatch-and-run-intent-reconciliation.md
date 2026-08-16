# Scheduler Dispatch and Run-Intent Reconciliation

## Status

Draft design — Pantheon scheduler handoff specification.

## Purpose

This document defines the final boundary of Pantheon scheduling: how a scheduler-selected, admitted, and transactionally reserved execution configuration becomes a durable Run intent and is then reconciled into concrete backend execution through Attempts.

The core rule is:

> **The Scheduler does not launch executors. Its responsibility ends when it atomically creates a durable Run intent, associated ExecutionBinding, ResourceReservations, BudgetHolds, and transitions the Task to Active. The Run Controller owns all execution after that handoff.**

This keeps scheduling, execution control, retry semantics, and backend implementation cleanly separated.

See also:

- `docs/architecture/task-lifecycle.md`
- `docs/architecture/execution-fabric.md`
- `docs/architecture/execution-offer-routing-and-admission-handshake.md`
- `docs/architecture/scheduler-reservations-ownership-and-leases.md`
- `docs/architecture/scheduler-resource-ledger-and-admission.md`
- `docs/architecture/run-and-attempt.md`

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
       create Attempt + LaunchKey
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
- the Run Controller owns preparation and execution reconciliation.

The initial Attempt is created later, after Run preparation reaches `LaunchReady=True`.

## 3. Run desired state and Attempt observation are separate

A Run expresses desired execution responsibility independently from what the current Attempt is observed doing.

Conceptually:

```yaml
run:
  id: run_123
  binding: binding_456

  desired:
    execution: running

  status:
    currentAttempt: attempt_1
```

and:

```yaml
attempt:
  id: attempt_1
  run: run_123
  launchKey: launch_ABC

  status:
    observedExecution: unknown
```

The important invariants are:

```text
Run owns immutable execution strategy / Binding
Attempt owns one concrete execution lineage / LaunchKey
```

and:

```text
desired Run execution != observed Attempt execution
```

## 4. Preparation occurs before Attempt creation

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
durably create Attempt + LaunchKey
    ↓
ExecutorBackend.ensureExecution(...)
```

If preparation fails before Attempt creation, there was no backend execution Attempt.

## 5. LaunchReady is a derived condition, not a lifecycle phase

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

## 6. Attempt is durable before backend side effects

Once `LaunchReady=True`, the Run Controller must create the Attempt and its immutable LaunchKey transactionally before contacting the backend.

Correct ordering:

```text
BEGIN
create Attempt
assign ordinal
assign immutable LaunchKey
set Run.currentAttemptId
COMMIT

       ↓

ExecutorBackend.ensureExecution(
    binding,
    attempt.launchKey,
    attachment?
)
```

If Pantheon crashes after Attempt commit but before the backend call, restart discovers the same nonterminal Attempt and invokes `ensureExecution` with the same LaunchKey.

Pantheon must never start backend execution first and persist the Attempt afterward.

## 7. Use ensure-execution semantics, not naive launch semantics

The Execution Fabric should avoid an imperative contract whose semantics are merely:

```text
launch something now
```

Instead, the backend contract should provide idempotent ensure semantics:

```text
ensureExecution(binding, launchKey, attachment?)
```

Meaning:

> Ensure that the execution lineage represented by this immutable ExecutionBinding and Attempt LaunchKey exists exactly once, or return the strongest observation that can be established about it.

The backend implementation may internally:

- discover an existing native execution;
- attach or reattach to it;
- recover adapter-private state;
- create a new native execution if absence is established and this Attempt has not yet established one;
- inspect current native execution state.

Pantheon core does not distinguish provider-specific launch, recovery, attach, or reconnect mechanisms.

## 8. LaunchKey remains stable for the entire Attempt

The LaunchKey belongs to the Attempt, not the Run.

Conceptually:

```text
Run run_123
  Binding binding_456
   │
   ├─ Attempt attempt_1
   │    LaunchKey launch_A
   │
   └─ Attempt attempt_2
        LaunchKey launch_B
```

Backend behavior must be idempotent with respect to the Attempt LaunchKey.

Transport retries, daemon restart, backend-plugin restart, status retries, and reattachment attempts for the same execution lineage use the same Attempt and same LaunchKey.

If the native execution for an Attempt is definitively terminated and retry policy intentionally requests a fresh execution under the same immutable Binding, Pantheon creates a new Attempt with a new LaunchKey.

A change to the Binding is not a new Attempt; it requires a new Run.

## 9. Backend attachment state is Attempt-scoped and opaque

Once native execution exists, a backend may need implementation-specific state to recover or reattach later.

Pantheon may persist an opaque versioned attachment associated with the Attempt:

```yaml
backendAttachment:
  attempt: attempt_1
  backend: executor://A
  schemaVersion: 3
  opaqueState: ...
```

Pantheon core stores and returns this state to the owning backend but never interprets it.

Concrete provider session IDs, process details, runtime handles, or transport state therefore remain below the Execution Fabric boundary.

## 10. Run Controller owns every post-binding action

After the scheduler transaction commits, the Run Controller owns all execution-side reconciliation.

Its conceptual loop is:

```text
read desired Run state
        ↓
read preparation conditions
        ↓
read current Attempt / persisted attachment
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

- create/recreate backend execution;
- switch backend;
- release Run reservations;
- select another offer;
- create another Attempt;
- retry the execution itself.

Attempt creation after failure belongs to Run reconciliation plus the later retry/escalation policy.

## 11. Current authority is checked before external side effects

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

## 12. Minimal normalized Attempt execution observations

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

The backend can positively establish that no native execution exists for the current Attempt LaunchKey.

Before that Attempt has ever established execution, `ABSENT` can permit `ensureExecution` to create it.

After an established execution has definitively terminated, the Attempt is terminal; a deliberate fresh retry requires a new Attempt rather than recreating the old Attempt under its old LaunchKey.

### STARTING

Execution is known to exist but is not yet ready for normal interaction.

Execution capacity remains reserved and active.

### RUNNING

Execution is known to exist and is capable of performing assigned work.

### EXITED

The backend can positively establish that this Attempt's execution terminated.

This is not Task success and does not by itself determine whether the Run should retry. Semantic completion still requires a candidate result and the Acceptance subsystem.

### UNKNOWN

Pantheon cannot establish whether execution exists or what its current state is.

UNKNOWN is fail-closed:

- the Attempt remains nonterminal;
- the Run remains the Task owner;
- associated execution reservations remain charged / uncertain;
- Pantheon must not create a replacement Attempt merely because observation is unavailable.

## 13. ABSENT and UNKNOWN are fundamentally different

This distinction must be preserved everywhere.

```text
ABSENT
= evidence that this Attempt currently has no native execution

UNKNOWN
= insufficient evidence either way
```

UNKNOWN triggers requery, reattachment, backend repair, or later recovery policy, but never speculative duplicate execution in v1.

A new Attempt may only be created after the prior Attempt's execution lineage is established terminal and retry policy chooses a fresh incarnation.

## 14. ensureExecution returns structured certainty

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

## 15. Reconciliation is not retry

The following do not by themselves create a new Attempt or Run:

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
same Attempt
same LaunchKey
```

A **new Attempt** is a deliberate execution-layer retry under the same Binding after prior execution continuity is conclusively ended.

A **new Run** is required when higher-level retry/escalation changes the ExecutionBinding or supplies new semantic execution context such as acceptance-rejection feedback.

## 16. Backend events are hints, not lifecycle authority

Backends may provide event streams, PTY EOF, callbacks, process watchers, or other notifications.

Such events should trigger reconciliation:

```text
backend event
    ↓
enqueue Run/Attempt reconciliation
    ↓
inspect/reconcile current state
    ↓
persist authoritative Pantheon observation
```

Events themselves must not directly mutate authoritative Run, Attempt, or Task lifecycle state.

Periodic reconciliation remains a safety net.

## 17. Event handling must tolerate duplication and reordering

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

Pantheon must not resurrect a terminal Attempt based on a stale event.

Events are therefore hints; authoritative state follows reconciliation plus Pantheon's monotonic lifecycle invariants.

## 18. Observation states are not a mandatory linear state machine

Pantheon may first observe an execution only after it has already terminated.

Therefore an Attempt may be observed directly as:

```text
UNKNOWN → EXITED
```

without Pantheon ever observing STARTING or RUNNING.

The normalized values are observations, not required sequential phases.

## 19. Persist observation before consequential follow-up

When the Run Controller establishes a meaningful Attempt observation, it first persists that state and only then performs derived actions.

Example:

```text
observe EXITED
      ↓
persist Attempt EXITED observation
      ↓
record immutable termination/failure evidence
      ↓
retry/finalization policy reacts
      ↓
release resources only when their owner no longer requires them
```

Pantheon must not release resources, create a new Attempt, trigger a new Run, or perform integration before recording the state that justifies those actions.

## 20. Backend exit is not semantic result submission

External execution termination and Task completion are separate facts.

```text
backend Attempt exited
        !=
Task candidate submitted
```

A worker normally submits structured output through Pantheon (`task.submit_result` or equivalent) before or during executor shutdown.

If an Attempt exits without a valid candidate result, the Run may require retry or may fail according to later failure policy even if the native process itself terminated normally.

Task success still requires the Acceptance subsystem to pass the candidate result.

## 21. Candidate submission belongs to the Run

A Run produces at most one candidate result.

When a structurally valid candidate is durably submitted:

```text
candidate submitted
      ↓
Task Active → Evaluating
Run Active → Finalizing
Run desired execution → stopped
      ↓
current Attempt is terminated/reconciled
```

The candidate records both its producing Run and producing Attempt for provenance.

If Acceptance rejects the candidate, semantic retry normally creates a new Run with a new ExecutionRequest/Binding containing the rejection evidence rather than another Attempt under the old Binding.

## 22. Candidate evaluation may outlive execution

A valid candidate may be submitted before the external executor terminates.

Then:

```text
candidate submitted
      ↓
Attempt exits
      ↓
Task Evaluating
```

Run-scoped execution resources may be released once the Run is finalized and their use is confirmed ended, while Task-scoped resources such as a worktree may remain reserved for evaluation, review, or finalization.

This preserves the distinction between Run-scoped and Task-scoped reservations.

## 23. Scheduler fairness is charged at Run-intent commit

A Goal receives scheduler service once the final Run-intent transaction successfully commits.

Fairness accounting does not wait for Attempt creation or for the backend to report RUNNING.

At commitment:

- execution/budget capacity has been successfully allocated;
- the Task has transferred to Active;
- a durable Run owns responsibility.

Subsequent preparation or Attempt failure is a Run/execution outcome, not evidence that the Goal was never scheduled.

## 24. SchedulingClaim is consumed at handoff

The SchedulingClaim is a pre-Run coordination primitive only.

Once Run intent commits:

```text
SchedulingClaim → consumed
Task → Active
Run → owner
```

The claim does not survive until Attempt creation or backend launch completes.

The Task cannot re-enter scheduler consideration unless a later lifecycle decision returns it to Ready.

## 25. Daemon restart recovers Runs and Attempts directly

On daemon restart:

```text
load all nonterminal Runs
        ↓
acquire/adopt ownership epoch
        ↓
read desired Run state
        ↓
load current nonterminal Attempt, if any
        ↓
load Attempt BackendAttachment, if any
        ↓
ExecutorBackend inspect/ensure/reconcile same LaunchKey
        ↓
persist observation
```

Active Tasks are not returned to the scheduler merely because the daemon restarted.

The Run Controller owns recovery of committed execution.

If a Run is LaunchReady but no Attempt exists because the daemon crashed before Attempt creation, the controller may durably create the first Attempt and then proceed.

## 26. Future dangling-execution detection

A backend may eventually need to enumerate executions tagged or identifiable as Pantheon-owned so reconciliation can detect native execution that exists without a corresponding current Attempt record.

Conceptually:

```text
backend inventory
      ↓
Pantheon-owned native executions
      ↓
compare with durable Attempts
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
      Attempt + LaunchKey
            ↓
      ensureExecution
            ↓
       reconciliation
```

## v1 scope

Include:

- atomic Task Ready → Active + Run creation handoff;
- exactly one nonterminal Run owner per Active Task;
- immutable ExecutionBinding per Run;
- durable Attempt creation before backend side effects;
- immutable LaunchKey per Attempt;
- idempotent ensure-execution semantics;
- opaque Attempt backend attachment state;
- idempotent preparation controllers;
- LaunchReady and preparation conditions;
- current-authority rechecks before external side effects;
- normalized Attempt observations: ABSENT, STARTING, RUNNING, EXITED, UNKNOWN;
- structured definitive-vs-unknown backend outcomes;
- reconciliation within the same Attempt/LaunchKey;
- backend events as hints;
- restart recovery through the Run Controller.

Defer:

- speculative duplicate Runs or concurrent Attempts;
- generic dangling-execution reaping;
- distributed Run controllers;
- complex suspension/migration semantics;
- backend-specific lifecycle states in core;
- complete failure/retry classification policy, defined separately.

## Key decisions

1. **The Scheduler ends when the atomic Run-intent transaction commits.**
2. **Task Ready → Active occurs in that transaction, before external execution necessarily exists.**
3. **Exactly one nonterminal Run owns an Active Task in v1.**
4. **A Run owns one immutable ExecutionBinding; an Attempt owns one immutable LaunchKey.**
5. **Attempts are created durably after preparation and before any backend execution side effect.**
6. **Backend execution uses idempotent ensure-execution semantics keyed by the current Attempt LaunchKey.**
7. **Backend reattachment state is Attempt-scoped and opaque to Pantheon core.**
8. **Run Controller owns every execution-side action after the binding commits.**
9. **Preparation is post-commit, idempotent, and expressed through conditions rather than lifecycle phase explosion.**
10. **Current authority is rechecked before consequential external actions.**
11. **ABSENT and UNKNOWN are different; UNKNOWN never authorizes a duplicate or replacement Attempt.**
12. **Transport/recovery retries remain within the same Attempt and LaunchKey.**
13. **A deliberate fresh execution under the same Binding creates a new Attempt; a Binding change creates a new Run.**
14. **Backend events trigger reconciliation but do not authoritatively mutate lifecycle state.**
15. **Executor exit does not imply Task success.**
16. **A Run produces at most one candidate; acceptance rejection normally creates a new Run.**
17. **Goal fairness is charged at successful Run-intent commitment.**
18. **Daemon restart recovers committed Runs/Attempts directly through the Run Controller, not through the Scheduler.**
