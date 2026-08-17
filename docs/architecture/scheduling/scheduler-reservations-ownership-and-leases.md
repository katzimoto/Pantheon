# Scheduler Reservations, Ownership, and Leases

## Status

Canonical Pantheon scheduler ownership/reservation specification.

## Purpose

This subsystem converts a routing/admission decision into crash-safe execution intent without double dispatch, duplicate resource ownership, concurrent budget overspend or stale-controller mutation.

The central rule is:

> **Lease expiry may transfer control authority, but it never proves external execution stopped. Capacity and unsettled budget headroom are released only when reconciliation establishes that release is safe.**

See also:

- `docs/architecture/scheduling/scheduler-resource-ledger-and-admission.md`
- `docs/architecture/execution/execution-offer-routing-and-admission-handshake.md`
- `docs/architecture/execution/run-and-attempt.md`
- `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`
- `docs/architecture/operations/budget-usage-and-rate-limits.md`
- `docs/architecture/goals-and-planning/planner-and-task-decomposition.md`

## Ownership primitives

Pantheon separates:

```text
SchedulingClaim
ResourceReservation
ControlLease
BudgetHold
```

A lease is control authority, not resource ownership. A reservation is capacity authority, not observed utilization. A BudgetHold is spending authority/headroom, not factual usage.

## SchedulingClaim

A SchedulingClaim is a short-lived durable coordination claim on a Ready Task while one scheduling cycle resolves Agents/offers/admission.

It binds at least:

- Task and Task status revision;
- Goal revision;
- Graph revision;
- captured ConfigurationRevision;
- scheduler/daemon incarnation;
- acquisition/expiry time.

Claim expiry permits another scheduler cycle to try; it does not imply anything about an already-created Run because the atomic scheduler commit consumes/removes the claim when a Run is created.

## ResourceReservation

A ResourceReservation is durable ownership of reservable capacity. Reservation lifecycle:

```text
HELD -> ACTIVE -> RELEASING -> RELEASED
             \-> UNCERTAIN
```

Reservations do not auto-expire because a daemon lease expired. `UNCERTAIN` capacity remains charged until reconciliation/explicit audited resolution makes reuse safe.

## Holder scopes

V1 supports explicit holder scopes rather than one generic stringly-typed owner:

```text
Task
Run
control-operation
```

Typical Task-scoped resources:

- Task Workspace/disk reservation;
- other durable Task-owned capacity intentionally surviving Runs.

Typical Run-scoped resources:

- global/Goal Run concurrency;
- backend concurrency;
- Run Sandbox/container/VM slot;
- Run-specific CPU/memory/executor capacity.

Explicitly accounted controller work uses `control-operation` scope. In v1 this includes at least:

```text
EvaluationOperation
PlanningOperation
```

The concrete holder relationship remains relational. `control-operation` is the accounting scope, not permission to store only one unconstrained opaque owner string.

## Critical rule: Task-scoped reservations are reused, not re-created

A new Run for a Task must not reserve another copy of capacity the Task already owns.

Admission constructs an effective claim set in two stages:

```text
DESIRED EFFECTIVE CLAIMS
  = Task/Agent/Offer/Workspace/Sandbox/policy requirements

INCREMENTAL CLAIMS FOR THIS SCHEDULER COMMIT
  = desired claims
    - compatible ACTIVE/HELD Task-scoped reservations already owned by this Task
```

For each Task-scoped key, Pantheon verifies that the existing reservation still has the correct resource identity/quantity/revision semantics. If more quantity is required, Admission requests only the positive delta or performs an explicit resize transaction; it never blindly creates a second Task reservation.

A Task requeue/new Run therefore preserves one Workspace reservation rather than leaking one reservation per Run.

Persistence should enforce this with a partial/conditional uniqueness invariant equivalent to:

```text
at most one non-released Task-scoped reservation
per (task_id, resource_key)
```

where the resource model requires one logical reservation per key. Controller logic still validates quantity/compatibility.

## Run-scoped reservations are fresh per Run

A new Run receives fresh Run-scoped reservations because backend/sandbox/concurrency requirements may differ across Bindings. Released/old Run reservations are historical and are not mutated into ownership for another Run.

## Control-operation reservations

A PlanningOperation/EvaluationOperation that needs reservable capacity acquires it under its own durable control-operation identity before crossing the relevant external contact boundary.

The operation, not an ephemeral adapter request, owns that capacity until reconciliation/finalization proves release safe.

For an external PlanningOperation:

```text
PlanningOperation intent
        ↓
required control-operation Reservations/Holds committed
        ↓
PlanningAttempt created
        ↓
contact marker committed
        ↓
external Planner call
```

An UNKNOWN PlanningAttempt keeps the operation's relevant reservation `UNCERTAIN`/charged. Pantheon does not release/reassign that capacity merely because a Planner response was lost or a controller process restarted.

A local deterministic planning path that performs no external/resource-bearing work does not create artificial Reservations merely to fit this model.

## Reservation assessment

Resource Ledger admission is whole-set and revision-aware. It assesses incremental claims against current allocatable capacity and current non-released reservations.

Result remains:

```text
ADMITTABLE
TEMP_UNAVAILABLE
UNSATISFIABLE
```

Admission never infers availability from observed utilization alone.

## ControlLease

Controller ownership uses a fencing identity that survives daemon restart/snapshot hazards:

```text
monotonic control epoch
+
fresh unpredictable leaseToken
+
daemon incarnation ID
```

A callback/operation is current only when all required fencing values match durable current ownership. An old controller with a reused/replayed epoch but stale token/incarnation cannot mutate current state.

Lease expiry permits control takeover after durable fencing rotation; it **never** proves a process/session stopped.

Control-operation lifecycle ownership does not reuse a Run ControlLease unless that concrete controller contract explicitly defines one. Evaluation/Planning reconciliation instead uses their durable operation/attempt identities and current controller transaction authority.

## Atomic scheduler Run-intent commit

The Scheduler ends at one authoritative transaction. Conceptually T3:

```text
BEGIN IMMEDIATE

re-read/revalidate:
  Task Ready + expected revision
  Goal/Graph current and reconciled
  SchedulingClaim current
  captured ConfigurationRevision still active
  selected Agent/offer still valid
  current resource descriptors
  existing Task-scoped reservation ownership
  incremental resource fit
  budget headroom
  policy/dispatch fences

create/activate only required incremental Reservations
create initial BudgetHolds
create immutable ExecutionBinding
create immutable Run intent/status
Task Ready -> Active
consume SchedulingClaim
charge fairness service point
append Events

COMMIT
```

No backend/process/Git/network call occurs inside this transaction.

Task-scoped reservations that already existed are referenced/revalidated, not duplicated.

## Initial BudgetHold

Scheduler commits an initial bounded tranche of spend authority, not the entire possible Run budget. Later extensions are separate atomic budget transactions and never mutate the ExecutionBinding.

BudgetHold availability is checked against all applicable overlapping accounts/periods.

Control operations such as Planning/Evaluation acquire their own bounded Holds through their controller transaction paths; they do not borrow a Task Run's BudgetHold merely because the work ultimately serves that Goal.

## Ownership after scheduler commit

After T3:

```text
Task Active
Run durable
Binding immutable
Reservations durable
initial Holds durable
```

Scheduler responsibility ends. Run Controller owns preparation, Sandbox/Context readiness, Attempt creation, launch/reconciliation and Run finalization.

Planner control-operation execution is separate from T3. A PlanningOperation cannot create a scheduled Run directly; its resulting PlanningRecord/GraphPatch must first pass Graph Controller validation/materialization.

## Attempt LaunchKey boundary

LaunchKey does not belong to the scheduler or Run-intent transaction. It belongs to an Attempt created after preparation is LaunchReady.

The Run Controller creates Attempt + LaunchKey + AgentControlSession durably, then separately commits the pre-launch contact marker before crossing the backend launch-call boundary, as defined in `docs/architecture/execution/run-and-attempt.md`.

PlanningAttempt is not a Run Attempt and does not receive a LaunchKey. It uses the provider-neutral PlanningAttempt identity/contact marker defined by the Planner architecture.

## Release rules

Run-scoped reservations enter RELEASING/RELEASED only when the external obligation they protect is known safe to release or an explicit audited administrative resolution has accepted/fenced the risk.

Examples:

```text
Attempt definitively exited
Sandbox safely released
Run finalization completed
```

UNKNOWN execution/sandbox state retains relevant capacity as `UNCERTAIN`.

Control-operation reservations use the same safety rule: a PlanningOperation/EvaluationOperation releases reserved capacity only when its external attempt/contact and any owned external resource are reconciled/finalized sufficiently to prove reuse safe.

Task-scoped reservations normally survive individual Run terminalization. They are released by Task/Workspace finalization, explicit recovery/reset or other Task-owned lifecycle rules.

## Blocking yield

A blocking spawn moves the current Run toward `terminalTarget=Yielded`. During Run finalization:

- Run-scoped execution/backend/Sandbox reservations are safely released;
- Run BudgetHold is settled;
- Task-scoped Workspace reservation is retained.

Only after those obligations are safe does the transaction commit `Run -> Yielded` and `Task Active -> Waiting`.

This prevents parent Runs from holding the concurrency capacity required by their children.

## Capacity shrink

If published allocatable capacity decreases below current reservations, existing reservations remain authoritative/charged. Pantheon blocks incompatible new admission; it does not silently revoke live capacity from current holders.

## UNKNOWN and administrative resolution

UNKNOWN is an observation state, not a timeout-based proof of absence. Normal recovery keeps reservations/holds fenced.

Pantheon additionally provides an explicit operator-only force-resolution path for permanently unrecoverable UNKNOWN obligations. Force resolution must:

- identify the exact Attempt/LaunchKey/Sandbox/control-operation external obligation;
- fence/tombstone that lineage so later callbacks cannot reacquire authority;
- record actor, reason, evidence and acknowledged risk;
- decide reservation/hold handling explicitly;
- never fabricate factual Usage/Charge merely to make accounting balance.

Force resolution is exceptional administrative acceptance of uncertainty, not automatic retry/release after a timer.

## Invariants

1. Lease expiry transfers controller authority only; it never proves execution stopped.
2. ResourceReservation and BudgetHold are independent ledgers.
3. Existing compatible Task-scoped reservations are subtracted before incremental Run admission; they are not re-created per Run.
4. At most one live logical Task-scoped reservation exists per `(task, resource key)` where the resource model is singular.
5. New Runs receive fresh Run-scoped reservations.
6. T3 atomically commits Binding + required Reservations + initial Holds + Run + Task Active.
7. LaunchKey belongs to normal Run Attempt after preparation, not Scheduler or PlanningAttempt.
8. PlanningOperation/EvaluationOperation may own explicit control-operation Reservations/Holds; UNKNOWN external control work retains/fences them until safe release.
9. UNKNOWN obligations retain/fence capacity until reconciled or explicitly force-resolved.
10. Task Workspace capacity survives ordinary Run retry/requeue/yield unless Task-owned policy says otherwise.
