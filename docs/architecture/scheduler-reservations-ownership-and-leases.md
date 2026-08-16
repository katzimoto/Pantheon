# Scheduler Reservations, Ownership, and Leases

## Status

Draft design — Pantheon scheduler subsystem specification.

## Purpose

This document defines the durable ownership layer that turns a successful routing/admission decision into crash-safe execution intent without double-dispatch, accidental capacity reuse, concurrent budget overspend, or stale-controller mutation.

The central rule is:

> **Lease expiry may transfer control authority, but it never proves that external execution stopped. Resource capacity and unsettled budget headroom are released only after reconciliation establishes that release is safe.**

See also:

- `docs/architecture/run-and-attempt.md`
- `docs/architecture/budget-usage-and-rate-limits.md`
- `docs/architecture/execution-offer-routing-and-admission-handshake.md`
- `docs/architecture/global-recovery-and-crash-reconciliation.md`

## Ownership primitives

Pantheon distinguishes three resource/control concepts plus a separate accounting hold.

### SchedulingClaim

A short-lived scheduler coordination claim meaning one scheduler attempt currently owns the right to try scheduling a Task.

Properties:

- created before execution commitment;
- no external executor exists because of the claim alone;
- may safely expire;
- bound to Task/Goal/Graph/policy revisions;
- at most one active SchedulingClaim per Task.

If it expires before an ExecutionBinding/Run intent is committed, the Task becomes claimable again.

### ResourceReservation

A durable commitment of reversible resource capacity to a holder.

Properties:

- created atomically with initial BudgetHolds, ExecutionBinding, and Run intent;
- counts against allocatable capacity until explicitly released;
- does not auto-expire because time passes;
- unknown execution state retains the reservation;
- release is idempotent.

### BudgetHold

A durable reservation of future spending authority, not resource capacity.

Properties:

- prevents concurrent work from spending the same remaining budget;
- actual usage converts held quantity into consumed quantity;
- only unused headroom returns when safely settled;
- may be extended later by the Budget Controller;
- is governed by `budget-usage-and-rate-limits.md`.

### ControlLease

A renewable controller-ownership record meaning this Pantheon controller incarnation currently has authority to reconcile/command a Run.

Conceptually:

```yaml
controlLease:
  run: run_123
  holder: daemon-incarnation://...
  ownershipEpoch: 18
  leaseToken: <fresh-random-token>
  renewedAt: ...
  validUntil: ...
```

Properties:

- may expire;
- expiry permits adoption by another controller/reconciler;
- expiry never releases ResourceReservations or settles BudgetHolds;
- `ownershipEpoch` is monotonic within durable Run history;
- every acquisition/adoption rotates `leaseToken` to a fresh unpredictable value;
- authoritative mutating operations must verify the current lease identity.

The required fencing check is conceptually:

```text
command.runId == current.runId
AND
command.ownershipEpoch == current.ownershipEpoch
AND
command.leaseToken == current.leaseToken
```

Epoch alone is insufficient after restoring an older database snapshot because a numeric epoch can be reintroduced/reused. The fresh lease token prevents stale controllers/callbacks from becoming valid after restart or restore.

Adapters should propagate the fencing identity to native execution controls where practical. Backend inability to enforce it internally does not weaken Pantheon's authoritative-state checks.

For v1, Pantheon remains a single local daemon with SQLite and an OS-backed installation lock, so no distributed lease service is required.

## ResourceReservation lifecycle

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

- `HELD` — durable commitment exists; external facility not yet confirmed in use.
- `ACTIVE` — associated facility is confirmed in use.
- `RELEASING` — release/shutdown desired and reconciliation is in progress.
- `UNCERTAIN` — Pantheon cannot establish whether associated external work still exists.
- `RELEASED` — reservation no longer counts against capacity.

`HELD`, `ACTIVE`, `RELEASING`, and `UNCERTAIN` all remain capacity-accounted. Only `RELEASED` returns capacity.

BudgetHold has distinct held/consumed/settled accounting and does not reuse this lifecycle.

## Unknown execution fails closed

Communication loss, daemon restart, timeout, or backend uncertainty do not imply executor death.

If Pantheon cannot prove that the current Attempt stopped:

```text
Attempt execution = UNKNOWN
ResourceReservations = retained / UNCERTAIN
unused BudgetHold headroom = retained/fenced
Usage already observed = remains consumed
```

Pantheon never frees resource capacity or spending headroom merely because a liveness timeout elapsed.

## Holder scopes

v1 ResourceReservations support:

### Run-scoped

Examples:

- backend execution capacity;
- execution-time host resources;
- temporary sandbox capacity.

A Run-scoped reservation may span multiple sequential Attempts under the same immutable ExecutionBinding.

### Task-scoped

Examples:

- Task worktree/workspace retained across multiple Runs and evaluation/finalization.

```text
Task
 ├── Task-owned workspace reservation
 ├── Run 1 execution reservations
 │    ├── Attempt 1
 │    └── Attempt 2
 └── Run 2 execution reservations
```

BudgetHold holder scope is separately generic because planning/evaluation and other control-plane work may consume finite budget outside worker Runs.

## Atomic execution commitment

ResourceReservations, **initial** BudgetHolds, immutable ExecutionBinding, Run intent, Task handoff, and SchedulingClaim consumption form one transaction.

```text
BEGIN WRITE TRANSACTION

1. Verify SchedulingClaim ownership/currentness.
2. Verify Task is still Ready and scheduler-eligible.
3. Verify Goal/Graph/policy revisions.
4. Verify selected Logical Agent is still eligible.
5. Verify selected ExecutionOffer hash/expiry/backend descriptor revision.
6. Rebuild/revalidate EffectiveResourceClaimSet.
7. Verify all resource claims still fit.
8. Verify required metering/enforcement compatibility.
9. Verify every applicable initial BudgetHold still fits.
10. Create ResourceReservations.
11. Create initial BudgetHolds.
12. Create immutable ExecutionBinding.
13. Create Run intent and transfer Task to Active.
14. Consume SchedulingClaim.

COMMIT
```

Failure rolls back the whole transaction. No network/backend calls occur inside it.

The initial Attempt is created later, after preparation reaches `LaunchReady=True`, and is durably assigned its LaunchKey before backend execution is contacted.

## Initial BudgetHolds are tranches

The scheduler must not reserve an entire parent Goal/project budget for one Run.

A policy-sized initial tranche preserves concurrency while preventing multiple Runs from spending the same remaining headroom.

Hold sizing may use normalized offer estimates, configured ceilings, Task-class history, or conservative defaults. Estimates are not factual Usage.

## BudgetHold extension

A Run may request additional spending authority without changing its immutable ExecutionBinding.

Conceptually:

```text
BEGIN WRITE TRANSACTION
  verify Run and current control authority
  verify BudgetAccount period/revision
  verify all applicable budgets have headroom
  increase held quantity atomically
COMMIT
```

No worker/model may directly mutate a BudgetHold. Denial is handed to Recovery Policy rather than being hard-coded into the Run Controller.

## External execution begins after durable intent

```text
DB Run commitment
  ↓
Run intent durable
  ↓
Run Controller prepares Run
  ↓
Attempt + LaunchKey durable
  ↓
ExecutorBackend ensure/reconcile
```

Pantheon never starts an external executor first and hopes to persist Run/Attempt identity afterward.

## Attempt LaunchKey

Every Attempt receives one immutable LaunchKey before its first external execution side effect.

All retries, reconnects, adapter restarts, daemon restarts, and reconciliation operations for that same Attempt reuse the same LaunchKey.

```text
ensureExecution(launchKey = X)
first invocation  → create/attach execution E
retry invocation  → return/attach execution E
```

A retry of the same Attempt must never intentionally create independent execution F.

A fresh execution after definitive termination creates a **new Attempt with a new LaunchKey**. The Run and immutable Binding may remain the same if Recovery Policy intentionally retries the same execution strategy.

## Backend execution identity

```text
Run
  ↓
Attempt
  ↓
LaunchKey
  ↓
ExecutorBackend
  ↓
opaque backend execution reference/attachment
```

Concrete provider/runtime identifiers remain adapter-private or audit/accounting metadata. Core scheduling logic does not interpret them.

## Control ownership and fencing

Ownership transfer example:

```text
epoch 14 / token A → controller incarnation A
epoch 15 / token B → controller incarnation B
```

A stale controller with epoch 14 cannot mutate epoch 15 state. A stale callback with a coincidentally reused numeric epoch after database restore also fails because its old random lease token differs.

On normal daemon restart or restore, the recovering controller rotates the lease token before issuing external commands.

Lease expiry or token rotation is never evidence that external work stopped.

## Usage conversion under BudgetHold

Factual Usage is recorded at Attempt/operation granularity. When accepted Usage implies a charge against Pantheon-authoritative budgets, accounting atomically converts held headroom into consumed quantity across all applicable BudgetAccounts.

If actual usage exceeds remaining held headroom on a guarded/observational path, Pantheon records the true Usage/Charge and marks the relevant budget/hold overdrawn. It never truncates reality to the configured limit.

## Idempotent accounting

Usage ingestion relies on stable source operation identity or monotonic checkpoints so replay cannot double-charge.

These operations are idempotent:

- ResourceReservation release;
- BudgetHold extension;
- Usage conversion;
- BudgetHold settlement.

## Cancellation and termination

Cancellation changes desired state first.

```text
termination desired
  ↓
backend terminate/reconcile current Attempt
  ↓
confirmed stopped
  ↓
release eligible Run-scoped ResourceReservations
  ↓
settle BudgetHold after attributable Usage is reconciled
```

If termination remains UNKNOWN, reservations stay UNCERTAIN and unused BudgetHold headroom remains fenced.

## BudgetHold settlement

Example:

```text
held allocation = 100k
actual consumed = 63k

63k remains consumed
37k unused headroom returns
```

A failed Attempt never refunds actual usage. A Run with multiple Attempts consumes from the Run's allocation unless policy extends it.

Settlement waits until all known/possible attributable usage is reconciled sufficiently to prove unused headroom is safe to release.

## Rate limits

Replenishing rate limits are temporary availability signals, not ResourceReservations and not BudgetHolds. `retry-after`/reset state may schedule a future wakeup but does not create durable spending capacity.

## Crash and restart recovery

On daemon restart:

```text
load nonterminal Runs
load Reservations / BudgetHolds
rotate/adopt ControlLease token + epoch
load current nonterminal Attempt
reconcile backend execution
reconcile Usage/Charge checkpoints
```

### Confirmed running

- current Attempt remains active;
- ResourceReservations remain ACTIVE;
- BudgetHold remains attributable;
- current ownership lease is used for subsequent actions.

### Confirmed stopped

- persist terminal Attempt observation/evidence first;
- reconcile final Usage/Charge;
- Recovery Policy decides finalization or another Attempt;
- release resources only when no longer required;
- settle BudgetHold only when safe.

### UNKNOWN

- current Attempt remains nonterminal;
- ResourceReservations remain/enter UNCERTAIN;
- unused BudgetHold headroom remains fenced;
- no replacement Attempt is created while continuity is unresolved.

Database restore uses the stronger recovery procedure in `global-recovery-and-crash-reconciliation.md`, including fresh lease-token rotation and external inventory/fencing before new dispatch.

## Capacity shrinkage

If allocatable resource capacity drops below already-reserved quantity, Pantheon marks the resource oversubscribed/degraded and blocks new admission. Existing reservations are not automatically preempted/released.

## v1 non-goals

Defer:

- distributed consensus/lease service;
- active-active multi-daemon scheduling;
- automatic resource preemption;
- speculative concurrent Attempts/duplicate Runs;
- arbitrary ResourceReservation holder scopes;
- releasing capacity/budget solely from heartbeat timeout;
- predictive ML tranche sizing.

## Key decisions

1. **SchedulingClaims are short-lived and may safely expire before execution commitment.**
2. **ResourceReservations are durable and never auto-expire solely because time passed.**
3. **BudgetHold is separate from ResourceReservation; usage converts held allowance to consumed allowance and only unused headroom returns.**
4. **ControlLease expiry transfers reconciliation authority; it never proves an executor stopped or settles budget.**
5. **ControlLease fencing uses Run ID + monotonic ownership epoch + fresh unpredictable lease token.**
6. **Lease tokens rotate on ownership adoption/restart/restore before external commands, preventing stale-controller validity even after old-database restore.**
7. **UNKNOWN execution retains ResourceReservations and unused budget headroom conservatively.**
8. **v1 supports Run-scoped and Task-scoped ResourceReservations.**
9. **Only RELEASED ResourceReservations stop counting against capacity.**
10. **ResourceReservations, initial BudgetHolds, ExecutionBinding, Run intent, Task handoff, and SchedulingClaim consumption commit atomically.**
11. **Initial BudgetHolds are policy-sized tranches, not whole parent budgets.**
12. **BudgetHold extensions are later atomic controller operations and do not mutate ExecutionBinding.**
13. **Actual Usage is never clamped to a configured limit.**
14. **Every Attempt has one immutable LaunchKey; a new Attempt gets a new key while reconciliation of the same lineage does not.**
15. **Cancellation does not free capacity or unused budget until termination/accounting settlement is safe.**
16. **Usage ingestion, reservation release, hold extension, and settlement are replay-safe/idempotent.**
17. **Rate limits remain temporary availability signals.**
18. **v1 remains single-daemon/SQLite and requires no external distributed lease system.**
