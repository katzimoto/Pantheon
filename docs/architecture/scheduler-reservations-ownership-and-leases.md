# Scheduler Reservations, Ownership, and Leases

## Status

Draft design — Pantheon scheduler subsystem specification.

## Purpose

This document defines the durable ownership layer that turns a successful routing/admission decision into crash-safe execution intent without double-dispatch, accidental capacity reuse, or concurrent budget overspend.

The central rule is:

> **Lease expiry may transfer control authority, but it never proves that external execution stopped. Resource capacity and unsettled budget headroom are released only after reconciliation establishes that release is safe.**

See also:

- `docs/architecture/run-and-attempt.md`
- `docs/architecture/budget-usage-and-rate-limits.md`
- `docs/architecture/execution-offer-routing-and-admission-handshake.md`

## Ownership primitives

Pantheon distinguishes three resource/control concepts plus a separate accounting hold.

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

A durable commitment of reversible resource capacity to a holder.

Properties:

- created atomically with initial BudgetHolds, ExecutionBinding, and Run intent;
- counts against allocatable capacity until explicitly released;
- does not auto-expire simply because time passes;
- unknown execution state retains the reservation;
- release is idempotent.

### BudgetHold

A durable reservation of *future spending authority*, not resource capacity.

Properties:

- prevents concurrent work from consuming the same remaining budget;
- actual usage converts held quantity into consumed quantity;
- only unused headroom returns when the hold is safely settled;
- may be extended later by the Budget Controller;
- is governed by `budget-usage-and-rate-limits.md`, not by Resource Ledger arithmetic.

### ControlLease

A renewable controller-ownership record meaning:

> this Pantheon controller incarnation currently has authority to reconcile/command this Run.

Properties:

- may expire;
- expiry allows control to transfer to another controller/reconciler;
- expiry never releases ResourceReservations or settles BudgetHolds;
- carries a monotonic ownership epoch to reject stale controller actions.

For v1, Pantheon remains a single local daemon with SQLite, so no external distributed lease service is required. The conceptual ControlLease/epoch model remains explicit for restart/recovery semantics and future multi-controller operation.

## Reservation lifecycle

Recommended v1 ResourceReservation states:

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

- `HELD` — durable reservation committed; associated external facility is not yet confirmed in use;
- `ACTIVE` — associated facility is confirmed in use;
- `RELEASING` — shutdown/release is desired and reconciliation is in progress;
- `UNCERTAIN` — Pantheon cannot establish whether associated external work still exists;
- `RELEASED` — reservation no longer counts against capacity.

`HELD`, `ACTIVE`, `RELEASING`, and `UNCERTAIN` all remain capacity-accounted.

Only `RELEASED` returns capacity to the Resource Ledger.

BudgetHold has different accounting semantics and does not reuse this lifecycle as a substitute for its own held/consumed/settled accounting.

## Unknown execution fails closed

Communication loss, daemon restart, timeout, or backend uncertainty do not imply executor death.

If Pantheon cannot prove that a Run's current Attempt stopped:

```text
Attempt execution = uncertain
ResourceReservations = retained / UNCERTAIN
unused BudgetHold headroom = retained/fenced
Usage already observed = remains consumed
```

Pantheon must never free execution capacity or spending headroom merely because a liveness timeout elapsed.

## Reservation holder scopes

Reservation lifetime is not necessarily identical to Run lifetime.

v1 supports two ResourceReservation holder scopes.

### Run-scoped reservation

Examples:

- backend execution capacity;
- execution-time host resources;
- temporary sandbox capacity.

Released after the Run no longer uses them and safe release is confirmed.

A Run-scoped reservation can span multiple sequential Attempts under the same immutable ExecutionBinding.

### Task-scoped reservation

Examples:

- Task worktree/workspace that must survive multiple Runs or evaluation phases.

```text
Task
 ├── Task-owned workspace reservation
 ├── Run 1 execution reservations
 │    ├── Attempt 1
 │    └── Attempt 2
 └── Run 2 execution reservations
```

Additional ResourceReservation holder scopes are deferred until demonstrated need exists.

BudgetHold holder scope is separately generic because control-plane intelligence such as planning or evaluation may also consume finite budget outside worker Runs.

## Atomic execution commitment

ResourceReservations, **initial** BudgetHolds, immutable ExecutionBinding, Run intent, Task handoff, and SchedulingClaim consumption form one durable commitment boundary.

Conceptual SQLite transaction:

```text
BEGIN WRITE TRANSACTION

1. Verify SchedulingClaim ownership/currentness.
2. Verify Task is still Ready and scheduler-eligible.
3. Verify Goal/Graph/policy revisions.
4. Verify selected Logical Agent is still eligible.
5. Verify selected ExecutionOffer request hash, offer hash, expiry, and backend descriptor revision.
6. Rebuild/revalidate EffectiveResourceClaimSet.
7. Verify all resources still fit.
8. Verify required metering/enforcement compatibility still holds.
9. Verify every applicable initial BudgetHold still fits current BudgetAccount state.
10. Create ResourceReservations.
11. Create initial BudgetHolds.
12. Create immutable ExecutionBinding.
13. Create Run intent and transfer Task to Active.
14. Consume SchedulingClaim.

COMMIT
```

Failure at any step rolls back the entire transaction.

There are no network/backend calls inside this transaction.

The initial Attempt is not required to exist in this scheduler transaction. After Run preparation is complete and `LaunchReady=True`, the Run Controller durably creates an Attempt with its immutable LaunchKey before contacting the backend.

## Initial holds are tranches, not whole-parent budgets

The scheduler commitment must not reserve an entire Goal/project budget for one Run.

Instead it creates a policy-sized initial tranche:

```text
Goal token budget:
  limit = 2,000,000
  consumed = 620,000
  held = 180,000

new Run initial hold = 100,000
```

This protects concurrency while still preventing multiple Runs from independently spending the same remaining headroom.

Initial hold sizing may use normalized offer usage estimates, policy ceilings, Task class history, or conservative configured defaults. The estimate is not itself factual Usage.

## BudgetHold extension is a later atomic accounting operation

A Run may legitimately need more spending authority without changing its ExecutionBinding.

When held headroom is low, the Run Controller may request an extension from the Budget Controller.

Conceptually:

```text
Run hold = 100k
consumed = 92k
      ↓
request +50k
      ↓
BEGIN WRITE TRANSACTION
  verify Run current/authorized
  verify BudgetAccount period/revision
  verify all applicable budgets have headroom
  increase held quantity atomically
COMMIT
```

No model/worker may directly mutate a BudgetHold.

A successful extension does **not** create a new Run and does not mutate the immutable ExecutionBinding. It changes runtime accounting authority under the existing Run.

If extension is denied, the later Failure/Retry/Escalation subsystem decides whether to stop, ask for approval, reroute, return partial work, or fail.

## External execution begins after durable intent

Correct ordering:

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

Pantheon must never start an external executor first and hope to persist either its Run or Attempt afterward.

## Idempotent Attempt LaunchKey

Every Attempt receives one immutable `LaunchKey` before external execution is attempted.

All retries, reconnects, adapter restarts, daemon restarts, and reconciliation operations for that same Attempt use the same LaunchKey.

Required semantic contract:

```text
ensureExecution(launchKey = X)
first invocation  → create/attach execution E
retry invocation  → return/attach execution E
```

Never:

```text
retry invocation → create independent execution F
```

If native infrastructure has no idempotency primitive, the adapter must provide equivalent semantics using backend-private durable state where possible.

A fresh execution after prior execution is definitively terminated creates a new Attempt with a new LaunchKey. The Run/Binding may remain the same if retry policy intentionally retries the same strategy.

## Backend execution identity

After ensure/reconciliation, the backend may return an opaque execution reference or attachment state.

```text
Run
  ↓
Attempt
  ↓
LaunchKey
  ↓
ExecutorBackend
  ↓
opaque backend execution reference
```

Concrete provider/runtime identifiers remain adapter-private or audit/accounting metadata. Core scheduling logic does not interpret them.

## Control ownership and epoch fencing

Each Run carries a monotonic ownership epoch so stale controllers cannot later mutate authoritative state.

Example:

```text
epoch 14 → controller A
epoch 15 → controller B
```

A mutating controller operation is accepted only if its epoch matches current Run ownership.

The epoch is an authority/fencing mechanism, not proof that external work stopped.

## Usage conversion under BudgetHold

Factual Usage is normally recorded at Attempt/operation granularity.

When an accepted UsageRecord implies a charge against Pantheon-authoritative budgets, accounting atomically converts held headroom into consumed quantity across all applicable BudgetAccounts.

Example:

```text
Run token hold = 100k
new UsageRecord = 12k tokens

held headroom decreases by 12k
consumed increases by 12k
```

If actual usage exceeds remaining held headroom because the execution path is guarded or observational rather than hard-enforced, Pantheon records the **actual** Usage/Charge and marks the affected BudgetAccount/hold overdrawn as appropriate. It never truncates factual usage to the configured limit.

The overdraw changes future execution authority; it does not rewrite history.

## Idempotent accounting

Reconciliation/event replay must not double-charge Usage.

Usage ingestion therefore relies on stable operation identity or equivalent monotonic checkpoints as defined in `budget-usage-and-rate-limits.md`.

Resource release, BudgetHold extension, usage conversion, and hold settlement are all idempotent operations.

## Cancellation and termination

Cancellation changes desired state first.

Correct flow:

```text
termination desired
  ↓
backend terminate/reconcile current Attempt
  ↓
confirmed stopped
  ↓
release appropriate Run-scoped ResourceReservations
  ↓
settle BudgetHold when all attributable usage is reconciled
```

Pantheon must not release capacity or unused budget merely because cancellation was requested.

If termination outcome remains unknown:

```text
ResourceReservation → UNCERTAIN
unused BudgetHold headroom remains fenced
```

## BudgetHold settlement

At safe settlement:

```text
initial/extended held allocation = 100k
actual consumed = 63k

63k remains consumed
37k unused headroom returns to available budget
```

A failed Attempt does not refund actual usage.

A Run with multiple Attempts shares the Run's budget allocation unless policy extends it:

```text
Run hold = 150k
Attempt 1 uses 40k
Attempt 2 uses 60k
Run actual consumption = 100k
unused headroom = 50k
```

Settlement waits until Pantheon has reconciled all known/possible attributable usage for that holder. An `UNKNOWN` external execution cannot be treated as definitely done for accounting purposes.

## ResourceReservations versus BudgetHolds

They share transaction/reconciliation discipline but not accounting semantics.

```text
ResourceReservation
reserve 12 units
Run ends safely
→ all 12 return

BudgetHold
hold 100k
Run consumes 63k
Run ends safely
→ 63k remains consumed
→ unused 37k returns
```

BudgetHold is therefore not modeled as a ResourceReservation.

## Rate limits do not create durable holds

Replenishing rate limits are temporary backend availability signals, not ResourceReservations and not BudgetHolds.

A `retry-after` or known reset time can wake scheduling/reconciliation later, but Pantheon does not persistently reserve rate-limit capacity as if it were cumulative spend.

## Crash recovery

On daemon restart:

```text
load nonterminal Runs
load associated ResourceReservations / BudgetHolds
load current nonterminal Attempt, if any
reconcile backend execution
reconcile pending Usage/Charge checkpoints
```

Possible execution outcomes:

### Confirmed running

- current Attempt remains active;
- ResourceReservations remain ACTIVE;
- BudgetHold remains active and attributable Usage continues to settle against it;
- controller ownership/epoch is adopted/refreshed.

### Confirmed stopped

- persist terminal Attempt observation/evidence;
- reconcile final Usage/Charge information;
- higher-level policy may finalize Run or create another Attempt;
- release Run-scoped resources only when Run no longer needs them;
- settle BudgetHold only when Run/accounting is final enough to release unused headroom safely.

### Unknown

- current Attempt remains nonterminal;
- ResourceReservations become/remain UNCERTAIN;
- unused BudgetHold headroom remains fenced;
- no replacement Attempt is created while execution continuity is unresolved.

All durable reservation/accounting ownership state lives in SQLite, never only in scheduler memory.

## Capacity shrinkage

If a resource owner lowers allocatable capacity below already-reserved quantity:

```text
reserved > allocatable
```

Pantheon marks the resource oversubscribed/degraded and blocks new admission. Existing reservations are not automatically preempted or released.

Budget-period changes are handled by the Budget subsystem instead; they must not silently invalidate historical Usage/Charge records.

## v1 non-goals

Defer:

- external consensus/lease service;
- multi-daemon active-active scheduling;
- automatic resource preemption;
- speculative concurrent Attempts or duplicate Runs;
- arbitrary ResourceReservation holder scopes;
- releasing capacity/budget based solely on heartbeat timeout;
- predictive ML budget tranche sizing.

## Key decisions

1. **SchedulingClaims are short-lived and may safely expire before execution commitment.**
2. **ResourceReservations are durable and never auto-expire solely because time passed.**
3. **BudgetHold is separate from ResourceReservation: held spending authority converts to consumed usage and only unused headroom returns.**
4. **ControlLease expiry transfers reconciliation authority; it never proves an executor stopped or settles budget.**
5. **Unknown execution retains resource reservations and unused budget headroom conservatively.**
6. **v1 supports Run-scoped and Task-scoped ResourceReservations.**
7. **ResourceReservation lifecycle is HELD/ACTIVE/RELEASING/UNCERTAIN/RELEASED; only RELEASED stops counting against resource capacity.**
8. **ResourceReservations, initial BudgetHolds, ExecutionBinding, Run intent, Task handoff, and SchedulingClaim consumption commit atomically.**
9. **Initial BudgetHolds are policy-sized tranches, not entire parent budgets.**
10. **BudgetHold extensions are later atomic controller operations and do not mutate ExecutionBinding.**
11. **Actual Usage converts held allowance to consumed allowance and is never clamped to a configured limit.**
12. **Every Attempt has one immutable LaunchKey; backend ensure/reconciliation is idempotent with respect to it.**
13. **A new Attempt receives a new LaunchKey; reconnect/recovery of the same execution lineage does not.**
14. **Run ownership uses a monotonic epoch to reject stale controller operations.**
15. **Cancellation does not free capacity or unused budget until termination/accounting settlement is safe.**
16. **Usage ingestion, reservation release, hold extension, and settlement are idempotent/replay-safe.**
17. **Rate limits remain temporary availability signals, not durable reservations or BudgetHolds.**
18. **v1 remains single-daemon/SQLite and requires no external distributed lease system.**
