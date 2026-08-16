# Scheduler: Ready-Task Eligibility

## Status

Draft design — Pantheon scheduler subsystem specification.

## Purpose

Pantheon distinguishes a Task being logically `Ready` from being currently eligible for scheduler consideration. TaskGraph readiness answers whether work may logically begin; scheduler eligibility answers whether the controller is currently permitted to consider dispatching it.

Scheduler eligibility is derived controller-owned status, not a TaskSpec field and not a Task lifecycle phase.

## Core flow

```text
TaskGraph Ready
      ↓
Scheduler Eligible
      ↓
Scheduler Candidate
      ↓
Admission / resources
      ↓
Run
```

`Ready` remains a semantic/lifecycle state. Eligibility, admission, ordering, routing and resource capacity are separate layers.

## Foundational principles

1. **Ready is logical, not physical.** A Ready Task has satisfied graph/input prerequisites but may still be temporarily ineligible for scheduling.
2. **Eligibility is derived state.** It may change as Goal revisions, policy, administrative suspension or recovery windows change.
3. **Eligibility does not include resource capacity, provider availability, priority ranking or model selection.** Those belong to later scheduler/router stages.
4. **Unknown eligibility fails closed.** Pantheon must not dispatch when current eligibility cannot be established.
5. **The candidate queue is disposable.** SQLite/task status is source of truth; in-memory scheduler indexes can be rebuilt after restart.
6. **Blocked eligibility is event-driven where possible, with periodic reconciliation as a safety net.**
7. **Selection uses an atomic per-Task scheduling claim.** At most one active claim may exist for a Task.
8. **Claims are revision-bound.** A stale Goal/Graph/Task/policy snapshot cannot proceed to dispatch.

## Eligibility condition

Conceptual Task status:

```yaml
status:
  phase: Ready

  conditions:
    - type: SchedulingEligible
      status: "False"
      reason: GoalRevisionFence
      message: >
        Goal revision 8 has not yet been reconciled.
      observedTaskVersion: 17
      observedGoalRevision: 8
      observedGraphRevision: 42
```

Allowed condition states are `True`, `False`, and `Unknown`.

`Unknown` is non-dispatchable and should normally indicate reconciliation or state-observation is incomplete.

## v1 eligibility gates

A Ready Task is scheduler-eligible only when all required gates pass:

```text
Task lifecycle gate
Goal revision gate
Goal compatibility gate
Graph revision gate
Dispatch-control gate
Ownership/claim gate
Time/backoff gate
Policy fence gate
```

Conceptually:

```rust
eligible(task) =
    task.phase == Ready
    && goal_is_current(task)
    && task_is_goal_compatible(task)
    && graph_state_is_current(task)
    && !dispatch_suspended(task)
    && !task_has_active_owner(task)
    && retry_window_open(task)
    && scheduler_policy_allows_consideration(task)
```

### Lifecycle gate

The Task must currently be `Ready`.

### Goal revision gate

An unreconciled Goal revision may establish a revision fence. Ready work based on stale Goal state must not dispatch until reconciliation determines it remains valid.

### Goal compatibility gate

The Task must be compatible with the current Goal revision. Its immutable provenance may reference an older Goal revision while a current `GoalCompatible` condition proves it remains valid for the latest revision.

### Graph revision gate

Readiness/graph-derived conditions must reflect the current TaskGraph revision. Stale readiness is re-evaluated before scheduler eligibility is granted.

### Dispatch-control gate

Administrative suspension at global, project or Goal level can make otherwise Ready Tasks temporarily ineligible.

### Ownership gate

A Task with an existing active scheduling claim or active Run ownership cannot be claimed again unless a recovery policy deliberately permits replacement.

### Time/backoff gate

A durable `notBefore`/retry-backoff deadline can temporarily make a Ready Task ineligible. Once the timestamp is reached it is reconsidered.

### Policy fence gate

Current hard policy may prevent work from entering scheduling consideration. Authorization of concrete actions remains a separate subsystem.

## Explicit non-gates

The following do not belong to scheduler eligibility:

```text
CPU availability
RAM/unified-memory availability
container capacity
provider quota/capacity
Claude/OpenCode/local-Qwen selection
Agent/model choice
priority/fairness ordering
physical concurrency
```

A Task can remain `SchedulingEligible=True` while it cannot currently be admitted because capacity is unavailable.

## Blockers

Eligibility failures should expose structured blockers rather than a bare boolean.

Conceptual example:

```yaml
scheduling:
  eligible: false

  blockers:
    - type: GoalRevisionFence
      goalRevision: 8
      recheckOn:
        - goal.reconciled

    - type: NotBefore
      until: 2026-08-16T18:00:00+03:00
      recheckAt: 2026-08-16T18:00:00+03:00
```

Blockers should identify the event or time that may make them false. This allows targeted re-evaluation rather than constant polling.

## Event-driven eligibility reconciliation

```text
Task → Ready
     ↓
Eligibility Reconciler
     ├── eligible → Candidate Queue
     └── blocked  → Blocker Registry
                        ↓
                 relevant event/time
                        ↓
                 reconsider Task
```

Examples:

```text
goal.reconciled
scheduler.resumed
policy.revised
retry.not_before_reached
task.claim.released
```

Pantheon should also periodically scan non-terminal Ready Tasks as a recovery/safety mechanism so lost events cannot permanently strand work.

## Candidate queue durability

The candidate queue is an in-memory scheduler index/cache only.

On restart:

```text
SQLite TaskStatus
    ↓
find phase=Ready
    ↓
recompute eligibility
    ↓
rebuild candidate queue
```

A separate durable queue is not required for v1 merely to remember eligible Tasks.

## Dynamic eligibility

Eligibility can be revoked and later restored without changing TaskSpec:

```text
SchedulingEligible=True
      ↓ Goal/policy revision
SchedulingEligible=False
      ↓ reconciliation
SchedulingEligible=True
```

This is expected behavior for long-running Goals.

## Scheduling claims

Eligibility alone does not protect against duplicate scheduler selection. Pantheon therefore creates an atomic per-Task `SchedulingClaim` before admission.

Conceptual transaction:

```text
BEGIN

recheck:
  task phase == Ready
  eligibility snapshot still current
  no active claim exists

insert SchedulingClaim

COMMIT
```

Only one contender may win. Others skip the Task.

A SchedulingClaim is not a Run. It only means the scheduler currently owns the right to attempt admission for that Task.

If admission fails, the claim is released and the Task remains Ready/eligible.

## Claim snapshot

Claims bind to the state used when they were created:

```yaml
claim:
  task: task_123

  observed:
    taskStatusVersion: 17
    goalRevision: 8
    graphRevision: 42
    policyRevision: 11
```

Before admission/dispatch, Pantheon revalidates the snapshot. If current revisions differ materially, the claim is stale and must be discarded/recomputed.

## Boundary with later scheduler stages

```text
S1 Eligibility
  Is this Ready Task currently allowed to enter scheduler consideration?

S2 Admission
  Can an execution configuration be admitted with current capacity?

S3 Reservations
  How is admitted capacity durably reserved and leased?

S4 Ordering/Fairness
  Which eligible candidate should be considered first?

S5 Concurrency Domains
  Which simultaneous-capacity limits apply?

S6 Dispatch
  How is an admitted/claimed Task converted into a durable Run intent safely?
```

## v1 decisions

1. `Ready` remains a TaskGraph/lifecycle state, not a dispatch guarantee.
2. `SchedulingEligible` is derived controller-owned status with `True`, `False`, or `Unknown`.
3. Eligibility only covers current-state/administrative fences, never provider/resource capacity or ranking.
4. Blockers expose reasons and targeted recheck events/times.
5. Scheduler queues are disposable indexes; SQLite remains source of truth.
6. Unknown eligibility fails closed.
7. Eligibility is dynamically revocable as Goal/policy state changes.
8. Selection creates an atomic per-Task SchedulingClaim.
9. Claims bind to Task/Goal/Graph/policy revisions and stale claims cannot dispatch.
