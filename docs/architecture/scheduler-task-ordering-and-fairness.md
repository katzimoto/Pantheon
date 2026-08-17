# Scheduler Task Ordering, Priority, and Fairness

## Status

Canonical Pantheon scheduler ordering, priority, and fairness specification.

## Purpose

This subsystem decides which scheduler-eligible Task receives the next scheduling claim. It does not choose an executor backend, model, provider, concrete resource footprint, or Run configuration.

The central rule is:

> **Pantheon schedules Goals fairly first, then selects a Task inside the chosen Goal. Scheduler queues are disposable, but every policy input that must survive restart is durable controller state.**

This prevents a Goal with many Ready Tasks from monopolizing the scheduler while keeping ordering deterministic and auditable across daemon restart.

## Boundaries

```text
Ready Task
   ↓
S1 Scheduling Eligibility
   ↓
S3 Ordering / Fairness
   ↓
Scheduling Claim
   ↓
Execution Fabric / Routing / Admission
```

S3 is deliberately independent of:

- concrete executor backends;
- providers, runtimes, models or harnesses;
- host memory/CPU/resource claims;
- token/cost budgets;
- ExecutionOffer details.

## Durable scheduler state

The in-memory queue is a cache. V1 persists the policy/ordering state needed to reconstruct the same scheduling semantics after restart.

Conceptually:

```text
scheduler_state
  singleton
  dispatch_mode             RUNNING|PAUSED
  next_service_sequence     monotonically increasing integer
  revision
  updated_at

goal_scheduling_state
  goal_id
  base_priority_class       foreground|normal|background
  last_served_sequence      nullable
  revision
  updated_at

task_scheduling_state
  task_id
  eligible_since            nullable
  next_attempt_at           nullable
  last_failure_code         nullable
  last_failure_detail       nullable bounded structured detail
  revision
  updated_at
```

`dispatch_mode` is durable operator desired state. It is **not** the recovery/configuration readiness gate. Effective Run dispatch additionally requires the global recovery barrier and all other T3 preconditions.

`next_service_sequence` and `last_served_sequence` provide restart-stable least-recently-successfully-served ordering without depending on wall-clock monotonicity. `eligible_since`/`next_attempt_at` remain timestamps because they represent waiting/backoff intervals.

## Two-level scheduling

Eligible Tasks are grouped by Goal:

```text
Goal A
├── A1
├── A2
└── A3

Goal B
├── B1
└── B2
```

Selection proceeds in three stages:

```text
1. choose effective priority class
2. choose Goal fairly within that class
3. choose oldest eligible Task within the Goal
```

## Scheduling priority

Priority is scheduling metadata associated with a Goal, not part of semantic `Goal.spec` and not part of TaskSpec.

Conceptual classes:

```text
foreground
normal
background
```

Named classes resolve to deterministic internal values. Concrete numeric values are implementation/configuration details.

The durable `goal_scheduling_state.base_priority_class` stores the current base class. Only user/operator or deterministic system scheduling policy may raise cross-Goal priority. Planner agents and worker agents may not set arbitrary global scheduling priority.

A scheduling-priority change does not create a semantic Goal revision, Task revision, or TaskGraph revision. It CAS-updates scheduler-policy state/revision and appends the corresponding Event.

## Non-preemptive v1

Priority affects future dispatch order only.

A newly elevated Goal does not terminate already-running work from a lower-priority Goal. Existing admitted Runs continue unless separately cancelled by user, policy, Goal reconciliation, or recovery logic.

Pantheon v1 therefore implements non-preemptive priority.

## Goal fairness

Within the same effective priority class, select the Goal that was least recently **successfully served**.

Fairness uses durable logical sequence numbers:

```text
Goal A lastServedSequence = 104
Goal C lastServedSequence = 101
Goal B lastServedSequence = 97

next Goal = B
```

A Goal with `last_served_sequence = NULL` has never successfully dispatched and is treated as least recently served. Stable Goal ID is the deterministic tie-breaker between equally unserved/equal-sequence Goals.

### Fairness is charged only on successful service

Selecting a Goal or attempting routing/admission does not count as service.

Fairness state advances only in the authoritative T3 transaction that successfully commits the Run intent for the selected Goal:

```text
goal.last_served_sequence = scheduler_state.next_service_sequence
scheduler_state.next_service_sequence += 1
```

If a Task cannot currently be executed, the Goal does not lose fair share merely because the scheduler tried it.

The sequence update and Run-intent commit are one transaction; a crash cannot leave fairness charged without the corresponding Run or a Run committed without its fairness charge.

## Task ordering within a Goal

For v1, choose the oldest scheduler-eligible Task:

```text
eligibleSince ASC
TaskId ASC
```

`eligible_since` is durable scheduler state. It records when `SchedulingEligible` most recently transitioned from `False` to `True`, not Task creation time and not the latest scheduling-attempt time.

The stable Task ID is used as deterministic tie-breaker.

No v1 ordering based on:

- LLM-estimated importance;
- predicted Task duration;
- model difficulty;
- file count;
- speculative critical-path duration.

## Eligibility interval semantics

Scheduler eligibility and its age are distinct from Task lifecycle.

Rules:

```text
SchedulingEligible False -> True
  set eligible_since = now

SchedulingEligible remains True
  preserve eligible_since across routing/admission failures

SchedulingEligible True -> False
  current eligible interval ends
  eligible_since = NULL (or retained only as historical Event/provenance)

later False -> True
  set a new eligible_since
```

A temporary scheduler backoff does **not** make the Task semantically ineligible and does not reset `eligible_since`. It only suppresses consideration until `next_attempt_at` unless an authorized wakeup/reconciliation event clears or advances the backoff.

## Best-effort ordering

A temporarily unavailable older Task must not create head-of-line blocking.

Example:

```text
Goal A
  A1 oldest → cannot currently obtain an admissible execution
  A2        → runnable
```

Pantheon records scheduling-attempt backoff for A1, releases its scheduling claim, and continues considering A2 and other Goals.

Likewise, an unavailable foreground Goal must not force the entire scheduler idle when lower-priority work can run.

## Scheduling attempt backoff

Backoff belongs to durable scheduler attempt state, not Task lifecycle.

A Task may remain:

```text
phase: Ready
SchedulingEligible: True
```

while `task_scheduling_state` records conceptually:

```yaml
scheduler:
  eligibleSince: ...
  nextAttemptAt: ...
  lastFailure:
    code: temporarily-unavailable
    detail: ...
```

This does not introduce Task phases such as `Queued`, `Retrying`, or `Backoff`.

Relevant events may cause early reconsideration before `next_attempt_at`, for example a Resource Ledger revision that releases the resource that previously prevented admission. Such reconsideration CAS-updates/clears scheduler backoff state; Event delivery itself is not authoritative state.

On successful T3, stale temporary scheduling-failure/backoff state for the committed Task is cleared/normalized in the same transaction.

Periodic reconciliation remains a safety net.

## Bounded aging / starvation protection

Strict priority can starve lower classes if high-priority work arrives indefinitely.

Pantheon therefore derives an `effectivePriority` from:

```text
basePriorityClass
+
bounded waiting-age boost
```

The stored base class is not mutated by aging.

Aging is bounded. Conceptually:

```text
background → may age to normal
normal     → may age to foreground
foreground → remains foreground
```

Exact durations and boost policy are configuration, not architecture.

Aging is based on the durable current `eligible_since` interval, not Task creation time, not process uptime, and not the most recent failed scheduling attempt.

## Why fairness is Goal-scoped

Goals are the user-visible owners of work. Agents are execution roles, not tenants or scheduling owners.

Agent concurrency constraints belong in generic synthetic resources such as:

```text
resource://limit/agent/<agent>/runs
```

They do not belong in the fairness algorithm.

## Concurrency and fairness are separate

S3 determines ordering.

The generic Resource Ledger constrains how much work may run concurrently. Example synthetic resources:

```text
resource://limit/global/runs
resource://limit/goal/<goal>/runs
```

Thus a Goal with many Tasks cannot bypass configured concurrency limits even when it is selected repeatedly.

## No critical-path scheduling in v1

Critical-path scheduling would require useful Task-duration estimates. Agentic Task durations are not reliable enough initially to justify making the scheduler less predictable.

Pantheon may revisit critical-path hints after collecting historical telemetry on Task type, Agent, execution features, duration distributions, acceptance rates, and retries.

Such hints should remain bounded optimizations rather than semantic Task priority.

## Planner and worker authority

Planner agents and workers cannot elevate scheduler priority.

A spawned Task inherits the scheduling context of its Goal. Blocking child relationships do not grant the child global priority authority.

Future deterministic graph-derived within-Goal hints may be introduced without allowing models to manipulate global scheduling policy.

## User/operator reprioritization

The user/operator may explicitly request that Pantheon focus on a Goal.

This updates durable scheduling metadata, for example:

```text
normal → foreground
```

It does not change the semantic Goal contract because urgency is scheduling policy, not desired outcome.

## Dispatch desired state

Dispatch pause/resume is durable scheduler desired state:

```text
scheduler_state.dispatch_mode = RUNNING | PAUSED
```

`PAUSED` prevents new T3 Run-intent commits. It does not stop already-running Attempts, cancel Tasks, release Reservations, or modify Goal/Task semantic state.

Ordinary daemon restart preserves the desired mode. Pantheon never silently converts `PAUSED` to `RUNNING` merely because an in-memory queue was rebuilt.

Effective ability to dispatch is:

```text
scheduler_state.dispatch_mode == RUNNING
AND global recovery barrier open
AND active configuration published
AND normal T3 eligibility/admission preconditions hold
```

Recovery readiness is deliberately not persisted by rewriting `dispatch_mode`; it is a separate factual/controller gate.

## Disposable queue

Persistent controller state is the source of truth. Any in-memory scheduler queue is disposable.

After restart Pantheon rebuilds the scheduling view from:

- scheduler-eligible Tasks and durable `eligible_since`;
- Goal ownership;
- durable base priority policy;
- durable Goal fairness sequence state;
- durable Task attempt/backoff state;
- active scheduling claims;
- durable dispatch desired mode;
- current recovery/configuration/admission gates.

Queue reconstruction may change process-local data structures; it must not reset fairness, waiting age, backoff, priority, or operator pause intent.

## Conceptual selection algorithm

```text
require scheduler_state.dispatch_mode == RUNNING
require recovery/configuration gates permit dispatch

eligible = Tasks where:
  phase == Ready
  SchedulingEligible == True
  eligible_since IS NOT NULL
  (nextAttemptAt IS NULL OR nextAttemptAt <= now)
  no active SchedulingClaim

group eligible by Goal

for each Goal:
  calculate effectivePriority from basePriorityClass + bounded eligible waiting age
  read lastServedSequence

priority = highest effectivePriority present
candidateGoals = Goals at that priority
goal = least recently successfully served candidateGoal
       tie-break by stable GoalId

task = oldest eligible Task in Goal
       tie-break by stable TaskId

atomically acquire SchedulingClaim(task)
```

Then Pantheon proceeds to ExecutionRequest / offers / routing / admission / reservation.

On successful T3 Run-intent creation, in that same transaction:

```text
Goal.lastServedSequence = scheduler_state.nextServiceSequence
scheduler_state.nextServiceSequence += 1
clear/normalize Task scheduling backoff for the committed Run
```

On temporary scheduling failure:

```text
release SchedulingClaim
CAS-update structured failure/backoff
continue scheduling other work
```

## Crash/restart behavior

A crash before T3 may leave only a SchedulingClaim/backoff decision; claim expiry/reconciliation and durable scheduler state determine retry. Fairness is not charged.

A crash after T3 sees both the durable Run and its fairness-sequence update because they committed atomically.

On restart:

```text
load scheduler_state
load goal_scheduling_state
load task_scheduling_state
reconcile claims
rebuild disposable queue
respect PAUSED/RUNNING desired mode
wait for recovery/configuration gates
resume selection only if effectively permitted
```

No Event replay is required to reconstruct current fairness/backoff/priority/dispatch state.

## v1 decisions

1. Ordering is backend/provider/model/resource agnostic.
2. Two-level scheduling: Goal first, Task second.
3. Priority is durable scheduler metadata on Goal ownership, not TaskSpec or semantic GoalSpec.
4. Use named deterministic priority classes.
5. Models cannot elevate cross-Goal scheduling priority.
6. Priority is non-preemptive in v1.
7. Fairness within a priority class is least-recently-successfully-served Goal using durable logical service sequence numbers.
8. Fairness advances only in the successful T3 Run-intent transaction.
9. Inside a Goal, oldest scheduler-eligible Task wins with deterministic Task ID tie-break.
10. Use durable `eligible_since`, not `createdAt` or latest-attempt time.
11. Temporary scheduling failure uses durable scheduler backoff plus event-driven wakeup without changing Task phase or eligible waiting age.
12. Bounded aging prevents indefinite starvation.
13. User/operator reprioritization changes durable scheduler policy, not semantic Goal/Task/Graph state.
14. Dispatch pause/resume is durable desired state; ordinary restart never silently resumes a paused scheduler.
15. Recovery/configuration readiness is a separate effective-dispatch gate, not encoded by mutating operator desired mode.
16. In-memory queues are disposable and reconstructed exclusively from durable authoritative state.
17. No critical-path prediction or usage-weighted fair sharing in v1.
