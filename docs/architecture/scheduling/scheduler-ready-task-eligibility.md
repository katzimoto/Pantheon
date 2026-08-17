# Scheduler Ready-Task Eligibility

## Status

Canonical Pantheon logical scheduling-eligibility specification.

## Purpose

This subsystem answers only:

> Which Ready Tasks are semantically eligible for Scheduler service and therefore belong to a continuing scheduler-eligibility interval?

It does **not** decide whether an otherwise eligible Task may be selected this instant because of temporary scheduler suppression, nor does it decide backend/resource/budget capacity. Selection suppression belongs to Scheduler ordering/control state; capacity belongs to later Agent Resolution, Execution Fabric, Resource/Budget/Rate feasibility.

## Eligibility predicate

`SchedulingEligible` is the durable per-Task semantic condition whose current `True` interval is aged by `task_scheduling_state.eligible_since` in the fairness contract. A Task is `SchedulingEligible` only when all hard logical gates pass:

```text
Task.phase == Ready
Task status revision current
no nonterminal Run owns Task
Goal nonterminal and current revision reconciled enough for dispatch
TaskGraph revision/current activation valid
Task dependencies/gates satisfied
current hard Task/Goal security or configuration state does not semantically fence dispatch
```

Failure of one of these semantic gates ends the current eligibility interval. When the semantic condition later becomes true again, a new eligibility interval begins according to `docs/architecture/scheduling/scheduler-task-ordering-and-fairness.md`.

Temporary scheduler controls and retry timing do **not** change this predicate merely because they suppress immediate selection.

## Selection-time suppression

A Task may remain:

```text
Task.phase == Ready
SchedulingEligible == True
eligible_since != NULL
```

while Scheduler is temporarily forbidden from selecting or committing it.

Selection-time suppression includes at least:

```text
scheduler_state.dispatch_mode == PAUSED
recovery safety barrier not open
active ConfigurationRevision not yet published/usable for a scheduling cycle
task_scheduling_state.next_attempt_at > now
an active SchedulingClaim already owns the current scheduling attempt
```

These gates are evaluated before/while selecting work, but they do not end the Task's semantic eligibility interval and therefore do not reset `eligible_since`.

A hard current security/configuration rule that makes the Task itself no longer valid for dispatch is different: that is a semantic eligibility failure and ends the interval. Temporary controller readiness or operator pause is not.

## ConfigurationRevision

Scheduler captures one immutable `configRevision` for an entire candidate-resolution/commit cycle. Do not use an ambiguous generic `policyRevision` as the scheduling fence.

The captured revision determines the exact active registries/policies used for:

```text
Agent Resolution
route policy
execution profiles
Sandbox profiles
relevant authorization ceiling compilation
```

Immediately before T3 Run-intent commit, Scheduler rechecks that the active ConfigurationRevision is still the captured one. If activation advanced, abort/re-evaluate rather than commit a mixed-revision Binding.

Temporary absence/unavailability of a published usable ConfigurationRevision suppresses selection; it does not by itself erase an otherwise continuing `SchedulingEligible` interval. If the newly active configuration instead introduces a hard Task/Goal dispatch fence, normal semantic eligibility reconciliation ends that interval.

## Dispatch control

Dispatch control is explicit operator/control-plane state. Pausing dispatch prevents **new Scheduler Run commits**. It does not cancel current Runs, imply external execution stopped, mutate Task lifecycle, or make an otherwise semantically eligible Ready Task lose its `eligible_since` waiting age.

Goal Finalizing/terminal state is different: it is a semantic Goal condition and fences new Runs under that Goal through the eligibility predicate.

## Ownership gate

Ready implies zero nonterminal responsible Runs. Scheduler additionally checks persistence ownership/partial uniqueness before T3.

If inconsistent state says `Ready + live Run`, the Task is not dispatchable; create/reconcile a RecoveryFinding rather than scheduling around it.

## `next_attempt_at` / temporary backoff

Recovery or a failed scheduling attempt may attach durable scheduler backoff in `task_scheduling_state.next_attempt_at` (the temporary `notBefore` concept used by older wording).

Until it elapses, the Task remains Ready and, if all semantic gates still pass, remains `SchedulingEligible=True` with the same `eligible_since`; it is simply suppressed from the current selection cycle. This is not a separate Task phase and does not start a new eligibility interval.

Relevant authoritative wakeups may clear/advance the suppression earlier as defined by the fairness contract.

## What is not eligibility

The following do **not** make a logical Ready Task semantically ineligible in this stage:

```text
installation dispatch is paused
temporary scheduler next_attempt_at/backoff has not elapsed
recovery/configuration controller readiness temporarily suppresses selection
all compatible backends busy
Sandbox slot unavailable
memory/CPU capacity unavailable
BudgetHold temporarily unavailable
provider rate-limited
no current ExecutionOffer due temporary health
```

The first group is Scheduler selection suppression. The remaining cases are feasibility/availability outcomes after eligibility. In both cases the Task may remain Ready and `SchedulingEligible`, preserving its current waiting-age interval while it cannot presently be selected or admitted.

Hard permanent incompatibility discovered later may feed structured Recovery/Goal planning, and a current hard security/configuration fence may make the Task semantically ineligible, but neither condition is silently encoded as Pending.

## Queue

In-memory scheduling queue/index is disposable optimization. SQLite Task/Goal/Graph/scheduler/config state is authority. Daemon restart rebuilds the scheduling view from durable state without resetting a continuing eligibility interval merely because process memory, dispatch permission or temporary backoff changed.

## SchedulingClaim

Before expensive resolution, Scheduler may acquire the durable short-lived SchedulingClaim described by S5. The claim binds expected Task/Goal/Graph/config revisions and prevents competing scheduler cycles from both reaching T3 for the same Task.

Claim ownership suppresses competing selection of the same Task; it does not make that Task semantically ineligible. Claim expiry coordinates Scheduler attempts only and never proves anything about external execution.

## Core invariants

1. `SchedulingEligible` is durable logical Task/Goal/Graph/security/config semantic state, not temporary selection suppression and not resource/backend availability.
2. Ready Task has zero nonterminal Runs.
3. Goal/Graph semantic reconciliation and hard current security/configuration fences may end eligibility; operator pause, recovery/configuration readiness and scheduler backoff do not end a continuing eligibility interval.
4. Scheduler cycle uses one captured ConfigurationRevision and aborts if it changes before authoritative commit.
5. Queue is cache; SQLite is truth.
6. Resource/Budget/offer scarcity leaves Task Ready and may leave it `SchedulingEligible=True` for later reconsideration.
7. Temporary selection suppression never resets `eligible_since`; only a semantic `SchedulingEligible True -> False` transition ends that waiting-age interval.
