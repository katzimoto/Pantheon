# Scheduler Reservations, Ownership, and Leases

## Status

Draft design — Pantheon scheduler subsystem specification.

## Purpose

This document defines the durable ownership layer that turns a successful routing/admission decision into crash-safe execution intent without double-dispatch or accidental capacity reuse.

The central rule is:

> **Lease expiry may transfer control authority, but it never proves that external execution stopped. Resource capacity is released only after reconciliation establishes that release is safe.**

## Ownership primitives

Pantheon distinguishes three different concepts.

### SchedulingClaim

A short-lived scheduler coordination claim meaning:

> one scheduler attempt currently owns the right to try scheduling this Task.

Properties:

- created before execution commitment;
- no external executor exists because of the claim alone;
- may safely expire;
- bound to Task/Goal/Graph/policy revisions;
- at most one active SchedulingClaim per Task.

If a SchedulingClaim expires before an ExecutionBinding/Run intent is committed, the Task becomes claimable again.

### ResourceReservation

A durable commitment of resource capacity to a holder.

Properties:

- created atomically with ExecutionBinding, BudgetHold, and Run intent;
- counts against allocatable capacity until explicitly released;
- does not auto-expire simply because time passes;
- unknown execution state retains the reservation;
- release is idempotent.

### ControlLease

A renewable controller-ownership record meaning:

> this Pantheon controller incarnation currently has authority to reconcile/command this Run.

Properties:

- may expire;
- expiry allows control to transfer to another controller/reconciler;
- expiry never releases ResourceReservations;
- carries a monotonic ownership epoch to reject stale controller actions.

For v1, Pantheon remains a single local daemon with SQLite, so no external distributed lease service is required. The conceptual ControlLease/epoch model is retained so restart/recovery semantics remain explicit and future multi-controller operation is possible.

## Reservation lifecycle

Recommended v1 reservation states:

```text
HELD
  ↓
ACTIVE
  ↓
RELEASING
  ↓
RELEASED

Exceptional:
UNCERTAIN
```

Semantics:

- `HELD` — durable reservation committed; external execution is not yet confirmed;
- `ACTIVE` — associated facility is confirmed in use;
- `RELEASING` — shutdown/release is desired and reconciliation is in progress;
- `UNCERTAIN` — Pantheon cannot establish whether associated external work still exists;
- `RELEASED` — reservation no longer counts against capacity.

`HELD`, `ACTIVE`, `RELEASING`, and `UNCERTAIN` all remain capacity-accounted.

Only `RELEASED` returns capacity to the Resource Ledger.

## Unknown execution fails closed

Communication loss, daemon restart, timeout, or backend uncertainty do not imply executor death.

If Pantheon cannot prove that a Run's external execution stopped:

```text
Run execution = uncertain
ResourceReservations = retained
Budget state = fenced/reconciled conservatively
```

Pantheon must never free execution capacity merely because a liveness timeout elapsed.

## Reservation holder scopes

Reservation lifetime is not necessarily identical to Run lifetime.

v1 supports two holder scopes:

### Run-scoped reservation

Examples:

- backend execution capacity;
- execution-time host resources;
- temporary sandbox capacity.

Released after the Run no longer uses them and safe release is confirmed.

### Task-scoped reservation

Examples:

- Task worktree/workspace that must survive multiple Runs or evaluation phases.

A Task may therefore retain its workspace while replacing execution Runs:

```text
Task
 ├── Task-owned workspace reservation
 ├── Run 1 execution reservations
 └── Run 2 execution reservations
```

Additional holder scopes are deferred until a demonstrated need exists.

## Atomic execution commitment

ResourceReservations, BudgetHolds, immutable ExecutionBinding, Run intent, and SchedulingClaim consumption form one durable commitment boundary.

Conceptual SQLite transaction:

```text
BEGIN WRITE TRANSACTION

1. Verify SchedulingClaim ownership/currentness.
2. Verify Task is still Ready and scheduler-eligible.
3. Verify Goal/Graph/policy revisions.
4. Verify selected ExecutionOffer request hash, offer hash, expiry, and backend descriptor revision.
5. Rebuild/revalidate effective ResourceClaimSet.
6. Verify all resources still fit.
7. Verify all BudgetHolds fit.
8. Create ResourceReservations.
9. Create BudgetHolds.
10. Create immutable ExecutionBinding.
11. Create Run intent.
12. Consume SchedulingClaim.

COMMIT
```

Failure at any step rolls back the entire transaction.

There are no network/backend calls inside this transaction.

## External execution begins after durable intent

Correct ordering:

```text
DB commit
  ↓
Run intent durable
  ↓
Run Controller observes intent
  ↓
ExecutorBackend launch/reconcile
```

Pantheon must never start an external executor first and hope to persist it afterward.

This ensures a daemon crash cannot erase knowledge that execution was intended/committed.

## Idempotent launch key

There is still a crash window after sending a launch request but before recording its acknowledgement.

Every Run therefore receives an immutable `LaunchKey` before external launch.

All retries of backend launch for the same Run use the same key.

Required semantic contract:

```text
launch(launchKey = X)
first invocation  → create/attach execution E
retry invocation  → return/attach execution E
```

Never:

```text
retry invocation → create independent execution F
```

If the native backend has no idempotency primitive, its adapter must provide the behavior using backend-private durable state where possible.

The LaunchKey is distinct from the backend's opaque execution/session identifier.

## Backend execution identity

After launch/reconciliation, the backend may return an opaque execution reference.

Pantheon treats it as adapter-owned identity:

```text
Run
  ↓
LaunchKey
  ↓
ExecutorBackend
  ↓
opaque backend execution reference
```

Concrete provider/runtime session identifiers remain adapter-private or audit metadata. Core scheduling logic does not interpret them.

## Control ownership and epoch fencing

A controller that loses/relinquishes ownership must not later wake up and mutate current Run state.

Each Run therefore carries a monotonic ownership epoch.

Example:

```text
epoch 14 → controller A
epoch 15 → controller B
```

Controller commands/events that mutate authoritative state are accepted only if their epoch matches the current Run ownership epoch.

Conceptually:

```text
if command.ownershipEpoch != run.currentOwnershipEpoch:
    reject stale controller action
```

The epoch is an authority/fencing mechanism, not a proof that external work stopped.

## Cancellation and termination

Cancellation changes desired state first.

Correct flow:

```text
termination desired
  ↓
backend terminate/reconcile
  ↓
confirmed stopped
  ↓
release appropriate Run-scoped reservations
```

Pantheon must not release capacity merely because cancellation was requested.

If termination result is unknown:

```text
reservation → UNCERTAIN
```

and continues counting against capacity.

## Budget Holds versus ResourceReservations

BudgetHolds and ResourceReservations share transaction/reconciliation machinery but not accounting semantics.

Resource reservation:

```text
reserve 12 units
Run ends safely
→ all 12 return to allocatable capacity
```

Budget hold:

```text
hold 100k tokens
Run consumes 63k
Run ends
→ 63k remains consumed
→ unused 37k returns to available budget
```

Therefore BudgetHold is not modeled as a ResourceReservation.

## Crash recovery

On daemon restart:

```text
load nonterminal Runs
load associated Reservations/BudgetHolds
for each Run:
    reconcile with ExecutorBackend
```

Possible outcomes:

### Confirmed running

- reservations remain ACTIVE;
- controller ownership/epoch is adopted/refreshed;
- Run continues reconciliation.

### Confirmed stopped

- reconcile terminal/attempt outcome;
- release appropriate Run-scoped reservations when safe;
- settle BudgetHolds.

### Unknown

- reservations become/remain UNCERTAIN;
- capacity stays charged;
- do not launch replacement execution until higher-level recovery policy permits it.

All durable reservation state lives in SQLite, never only in scheduler memory.

## Capacity shrinkage

If a resource owner publishes lower allocatable capacity below already-reserved quantity:

```text
reserved > allocatable
```

Pantheon marks the resource oversubscribed/degraded and blocks new admission.

Existing reservations are not automatically preempted or released.

Recovery/resource policy decides any later intervention.

## v1 non-goals

Defer:

- external consensus/lease service;
- multi-daemon active-active scheduling;
- automatic resource preemption;
- speculative duplicate Runs;
- arbitrary reservation holder scopes;
- releasing capacity based solely on heartbeat timeout.

## Key decisions

1. SchedulingClaims are short-lived and may safely expire before execution commitment.
2. ResourceReservations are durable and never auto-expire solely because time passed.
3. ControlLease expiry transfers reconciliation authority; it never proves an executor stopped.
4. Unknown execution state retains its reservations and fails closed.
5. v1 supports Run-scoped and Task-scoped ResourceReservations.
6. Reservation lifecycle is `HELD`, `ACTIVE`, `RELEASING`, `UNCERTAIN`, `RELEASED`; only `RELEASED` stops counting against capacity.
7. ResourceReservations, BudgetHolds, ExecutionBinding, Run intent, and SchedulingClaim consumption commit atomically.
8. External execution begins only after durable intent exists.
9. Every Run has an immutable LaunchKey; backend launch is idempotent with respect to it.
10. Run ownership uses a monotonic epoch to reject stale controller operations.
11. v1 remains single-daemon/SQLite and requires no external distributed lease system.
12. Cancellation does not free capacity until termination is confirmed.
13. Budget Holds and Resource Reservations share transactional machinery but have distinct accounting semantics.
14. All release/reconciliation operations are idempotent.
