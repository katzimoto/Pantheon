# Scheduler Ready-Task Eligibility

## Status

Canonical Pantheon logical scheduling-eligibility specification.

## Purpose

This subsystem answers only:

> Which Tasks are logically eligible for the Scheduler to consider now?

It does **not** decide backend/resource/budget capacity. Those belong to later Agent Resolution, Execution Fabric, Resource/Budget/Rate feasibility.

## Eligibility predicate

A Task is Scheduler-eligible only when all hard logical gates pass:

```text
Task.phase == Ready
Task status revision current
no nonterminal Run owns Task
Goal nonterminal and current revision reconciled enough for dispatch
TaskGraph revision/current activation valid
Task dependencies/gates satisfied
system/Goal dispatch control allows new work
Task notBefore/backoff satisfied
current hard security/configuration does not fence dispatch
Scheduler has observed/published active ConfigurationRevision
```

Failure of any gate keeps the Task out of the scheduling candidate set until durable state changes.

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

## Dispatch control

Dispatch control is explicit operator/control-plane state. Pausing dispatch prevents **new Scheduler Run commits**. It does not cancel current Runs or imply external execution stopped.

Goal Finalizing/terminal state similarly fences new Runs under that Goal.

## Ownership gate

Ready implies zero nonterminal responsible Runs. Scheduler additionally checks persistence ownership/partial uniqueness before T3.

If inconsistent state says `Ready + live Run`, the Task is not dispatchable; create/reconcile a RecoveryFinding rather than scheduling around it.

## `notBefore` / backoff

Recovery/backoff may attach a durable `notBefore`. Until elapsed, the Task remains Ready but is not scheduler-eligible. This is not a separate Task phase.

## What is not eligibility

The following do **not** make a logical Ready Task ineligible in this stage:

```text
all compatible backends busy
Sandbox slot unavailable
memory/CPU capacity unavailable
BudgetHold temporarily unavailable
provider rate-limited
no current ExecutionOffer due temporary health
```

Those are feasibility/availability outcomes after eligibility. The Task remains Ready and can be reconsidered when capacity/health changes.

Hard permanent incompatibility discovered later may feed structured Recovery/Goal planning, but it is not silently encoded as Pending.

## Queue

In-memory scheduling queue/index is disposable optimization. SQLite Task/Goal/Graph/dispatch/config state is authority. Daemon restart rebuilds eligible work from durable state.

## SchedulingClaim

Before expensive resolution, Scheduler may acquire the durable short-lived SchedulingClaim described by S5. The claim binds expected Task/Goal/Graph/config revisions and prevents competing scheduler cycles from both reaching T3 for the same Task.

Claim expiry coordinates Scheduler attempts only; it never proves anything about external execution.

## Core invariants

1. Eligibility is logical control-plane state, not resource/backend availability.
2. Ready Task has zero nonterminal Runs.
3. Goal/Graph reconciliation and dispatch control fence eligibility.
4. Scheduler cycle uses one captured ConfigurationRevision and aborts if it changes before authoritative commit.
5. Queue is cache; SQLite is truth.
6. Resource/Budget/offer scarcity leaves Task Ready for later reconsideration.
