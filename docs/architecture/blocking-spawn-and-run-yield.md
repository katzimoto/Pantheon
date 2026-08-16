# Blocking Spawn and Run Yield

## Status

Draft design — authoritative correction for blocking dynamic Task spawn semantics.

## Purpose

Pantheon supports dynamic discovery of bounded child work. A blocking child creates a durable wait condition: the parent Task cannot make useful semantic progress until the child result is available.

The central rule is:

> **A blocking spawn suspends the Task, not a live executor. The current Run yields execution responsibility, releases Run-scoped capacity, and the Task resumes later through a new Run after the child join is satisfied.**

This document resolves the previously unspecified relationship between `Task Waiting`, Run ownership, Run-scoped reservations, dynamic child joins, and continuation after child completion.

See also:

- `docs/architecture/task-spawn-and-dynamic-graphs.md`
- `docs/architecture/task-lifecycle.md`
- `docs/architecture/run-and-attempt.md`
- `docs/architecture/scheduler-resource-ledger-and-admission.md`
- `docs/architecture/scheduler-reservations-ownership-and-leases.md`
- `docs/architecture/agent-control-channel.md`

## 1. Pantheon does not suspend a live Run in v1

Pantheon v1 does not preserve a backend session, PTY, process, model conversation, backend concurrency slot, Run-scoped memory/CPU reservation, or Run BudgetHold while waiting on a blocking child.

Doing so would require a second execution model for suspend/resume, idle accounting, provider-specific session expiration, recovery of dormant sessions, and deadlock avoidance.

Instead:

```text
Parent Task Active
        ↓
blocking spawn accepted
        ↓
Parent Run Finalizing
terminalTarget = Yielded
        ↓
stop/reconcile Attempt
settle Run accounting
release Run-scoped capacity
        ↓
Parent Run Yielded
        ↓
Parent Task Waiting
```

The Task remains durable; the executor does not.

## 2. `Yielded` is a terminal Run outcome

Run terminal outcomes become:

```text
Completed
Failed
Cancelled
Yielded
```

Meanings:

```text
Completed
= Run produced and durably submitted one Candidate.

Failed
= execution strategy ended without a usable Candidate and no further Attempt under the Binding is allowed.

Cancelled
= Run execution responsibility was intentionally revoked.

Yielded
= Run intentionally returned execution responsibility because the Task requires a durable blocking condition before semantic work can continue.
```

`Yielded` is not failure.

A yielded Run:

- has no Candidate;
- does not consume retry/recovery quota merely because it yielded;
- does not create failure evidence by virtue of yielding;
- is never reopened;
- retains immutable historical provenance like every other terminal Run.

Core invariant:

```text
Run Completed ⇒ exactly one Candidate
Run Yielded   ⇒ zero Candidates
```

## 3. Task/Run ownership invariants

Pantheon v1 enforces:

```text
Task Active
⇒ exactly one nonterminal Run owns responsibility for progressing it
```

```text
Task Waiting
⇒ zero nonterminal Runs
```

```text
Task Ready
⇒ zero nonterminal Runs
```

Therefore the Task must not enter `Waiting` while its yielding Run is still nonterminal.

## 4. Blocking spawn transaction

The initial blocking-spawn transaction atomically establishes both the child work and the parent's intent to yield.

Conceptually:

```text
BEGIN IMMEDIATE

validate Agent Control request and idempotency
validate task.spawn authorization
validate child Task proposal and ancestry limits
validate graph/join legality

materialize Child Task
record immutable spawn provenance
create blocking Join
increment TaskGraph revision

Parent Run:
  Active → Finalizing
  terminalTarget = Yielded
  desiredExecution = stopped

record Events

COMMIT
```

The parent Task remains `Active` during this transaction.

This prevents two unsafe partial states:

```text
parent stops but child was never materialized
```

and:

```text
child exists but the old Run continues producing unrelated semantic work after Pantheon accepted the blocking yield
```

## 5. Finalization before `Task Waiting`

After the blocking-spawn transaction, the Run Controller performs ordinary safe finalization:

```text
Run Finalizing / target Yielded
        ↓
stop new semantic Agent Control operations
        ↓
terminate/reconcile current Attempt
        ↓
settle actual usage
        ↓
settle/release Run BudgetHold
        ↓
release Run-scoped ResourceReservations
        ↓
revoke AgentControlSession
        ↓
checkpoint Task Workspace
```

Only when the external execution lineage is safely terminal/reconciled and Run-scoped release preconditions are satisfied may Pantheon commit:

```text
Run → Yielded
Task Active → Waiting
Workspace → Frozen
```

If Attempt termination becomes `UNKNOWN`:

- Run remains `Finalizing`;
- Task remains `Active`;
- reservations remain charged/UNCERTAIN;
- Budget state remains conservative;
- no replacement Run may be created.

`UNKNOWN` never permits yield completion merely to make progress.

## 6. Resource semantics

Yield releases Run-scoped execution resources, including as applicable:

```text
backend concurrency
resource://limit/global/runs
resource://limit/goal/<goal>/runs
Run sandbox/process slots
Run CPU/memory reservations
Run-scoped BudgetHold unused headroom
AgentControlSession
```

Task-scoped durable state remains, including:

```text
Task Workspace
Task-scoped WorkspaceReservation
Task immutable specification
Task input/output bindings
spawn provenance
Join state
existing immutable Artifacts
```

The distinction is:

```text
execution capacity → released
semantic Task state → retained
```

Task-scoped reservations must not be recreated or double-counted when a continuation Run is later scheduled.

## 7. Budget settlement

A yielding Run settles budget exactly like another concluded execution lineage.

Example:

```text
initial Hold = 80k normalized tokens
actual usage = 31k
```

At safe settlement:

```text
31k → consumed factual usage
49k → released unused headroom
```

Yield never refunds actual consumption.

The continuation Run receives a new initial BudgetHold through normal scheduling/admission.

## 8. Workspace checkpoint and freeze

Before the parent Run becomes `Yielded`, Pantheon records an immutable `WorkspaceRevision` representing the exact Task-owned workspace state that continuation will inherit.

Conceptually:

```yaml
yield:
  run: run_17
  workspaceRevision: workspace-rev_82
  join: join_44
```

While the Task is `Waiting`, the Task workspace is retained but frozen against worker mutation.

No executor owns the Task during this period.

## 9. Join owns the wait condition

The durable reason for waiting belongs to TaskGraph/join state, not to a process, model session, Run, or in-memory future.

Conceptually:

```yaml
join:
  id: join_44
  parentTask: task_123
  strategy: all
  requirements:
    - childTask: task_456
      output: findings
  state: PENDING
```

For v1, blocking joins use `all` semantics.

## 10. Child outputs are immutable bindings

A child result enters the parent through accepted immutable Artifact bindings.

```text
Child Task
   ↓
Candidate
   ↓
Acceptance PASS
   ↓
ArtifactRef
   ↓
parent Join/input binding
```

The parent does not consume child stdout, hidden conversation state, or mutable child workspace as authoritative input.

## 11. Join satisfaction returns Task to `Ready`

When every required blocking-child output is accepted and bound, the Join Controller may atomically perform:

```text
BEGIN IMMEDIATE

verify parent Task == Waiting
verify parent has no nonterminal Run
verify required child outputs are accepted/current
bind required ArtifactRefs
mark Join SATISFIED
Task Waiting → Ready
Workspace remains durable for next Run
record Events

COMMIT
```

The Join Controller never creates a Run directly.

The Scheduler remains the sole authority that creates a new scheduled Run.

## 12. Continuation is a new Run

A yielded Run is never resumed.

After child completion, semantic context has changed because new accepted inputs exist. Therefore continuation requires:

```text
Task Ready
    ↓
Agent Resolution
    ↓
new ExecutionRequest
    ↓
new Agent + ExecutionOffer selection
    ↓
new ExecutionBinding
    ↓
new Run
```

Even if the same Agent/backend is selected again, the new Run is distinct because its immutable execution context includes the newly resolved child inputs.

## 13. ContinuationContext is not RecoveryContext

Yield is normal orchestration, not failure.

Pantheon should distinguish a continuation snapshot/context from recovery evidence.

Conceptually:

```yaml
continuation:
  reason: blocking-child-completed
  priorRun: run_17
  join: join_44
  workspaceRevision: workspace-rev_82
  resolvedInputs:
    findings: artifact://sha256/...
```

The Context Builder uses this alongside the immutable Task/Goal/Agent/policy inputs when constructing the next Run snapshot.

## 14. Hidden model session state is never required for correctness

Pantheon correctness must not depend on preserving the old model conversation while a child runs.

The durable continuation inputs are:

- immutable Task contract;
- current compatible Goal/TaskGraph revision;
- prior yielded Run reference;
- WorkspaceRevision;
- Join/provenance records;
- accepted child ArtifactRefs;
- structured continuation summary;
- current policy and Agent inputs.

A backend-specific preserved conversation may later be an optional optimization, but it cannot be required to resume the Task.

This keeps blocking spawn provider-neutral.

## 15. Child failure

Child failure does not intrinsically fail the parent Task.

If an `all` blocking join can no longer be satisfied, the Join becomes `IMPOSSIBLE` and structured evidence is passed to Recovery/Planning policy.

Possible outcomes include:

- retry/requeue child work under existing policy;
- materialize alternative work;
- replan;
- request human intervention;
- fail or supersede the parent Task.

The parent requires no live Run while these decisions occur.

## 16. Parent cancellation while Waiting

If a Waiting parent Task is cancelled:

```text
Task Waiting
    ↓
Finalizing / terminalTarget=Cancelled
    ↓
Cancelled
```

Attached blocking descendants receive cancellation according to TaskGraph lifetime policy.

There is no parent executor to terminate because `Task Waiting ⇒ zero nonterminal Runs`.

Task-scoped workspace/resource cleanup follows normal finalization rules.

## 17. Goal revision while Waiting

Goal reconciliation evaluates a Waiting Task as durable control-plane state.

Possible results remain:

```text
STILL_VALID
REVALIDATE
SUPERSEDE
NEW_WORK
```

Supersession/cancellation operates on the Task and attached descendants; no dormant backend execution must be revived.

## 18. Agent Control behavior after yield intent

Once the blocking-spawn transaction commits `Run.phase = Finalizing` and `terminalTarget = Yielded`, the current Attempt loses authority to begin new semantic work.

The Agent Control Gateway must reject new operations such as:

```text
task.spawn
task.graph.propose
task.submit_result
new semantic artifact sealing
new consequential action.invoke
```

unless an operation is explicitly classified as finalization/telemetry-safe.

This prevents the worker from continuing semantic mutation after Pantheon has accepted the yield.

## 19. Idempotency

Blocking spawn retains the existing graph-level duplicate-prevention key:

```text
(parentRun, spawnIdempotencyKey)
```

The Agent Control channel additionally provides:

```text
(attemptId, requestId)
```

Retrying a lost request therefore returns the already-materialized child/join result rather than producing additional children.

## 20. v1 spawn-mode scope

V1 implements:

```text
blocking
```

with the yield protocol defined here.

The architecture reserves but implementation-defers:

```text
joined
detached
```

`joined` requires explicit later join-point semantics while a Run continues producing independent work.

`detached` adds Goal-owned lifetime semantics that are not required for the first implementation.

The existing TaskGraph vocabulary may retain those mode names as reserved/post-v1 concepts, but v1 policy/schema must reject them as unsupported where execution semantics would otherwise be ambiguous.

## Core invariants

1. **Blocking spawn suspends the Task, not a live executor.**
2. **A blocking spawn always drives the current Run toward terminal `Yielded`.**
3. **`Yielded` is a non-failure terminal Run outcome and has no Candidate.**
4. **`Task Active ⇒ exactly one nonterminal Run`.**
5. **`Task Waiting ⇒ zero nonterminal Runs`.**
6. **`Task Ready ⇒ zero nonterminal Runs`.**
7. **Task does not enter Waiting until the yielding Run is safely terminal.**
8. **UNKNOWN Attempt termination blocks yield completion and never authorizes a replacement Run.**
9. **Yield releases Run-scoped capacity but preserves Task-scoped state.**
10. **Workspace is checkpointed/frozen while the Task waits.**
11. **The durable Join, not a Run/session, owns the wait condition.**
12. **Child outputs enter the parent only through accepted immutable Artifact bindings.**
13. **Join satisfaction returns the Task to Ready; only the Scheduler may create the continuation Run.**
14. **Continuation always uses a new Run and immutable execution snapshot.**
15. **Yield is orchestration continuation, not Recovery, and consumes no retry quota.**
16. **V1 supports blocking spawn only; joined/detached execution semantics are deferred.**
