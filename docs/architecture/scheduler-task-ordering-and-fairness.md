# Scheduler Task Ordering, Priority, and Fairness

## Status

Canonical Pantheon scheduler ordering, priority, and fairness specification.

## Purpose

This subsystem decides which scheduler-eligible Task receives the next scheduling claim. It does not choose an executor backend, model, provider, concrete resource footprint, or Run configuration.

The central rule is:

> **Pantheon schedules Goals fairly first, then selects a Task inside the chosen Goal.**

This prevents a Goal with many Ready Tasks from monopolizing the scheduler while keeping ordering deterministic and easy to audit.

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

Only user/operator or deterministic system scheduling policy may raise cross-Goal priority. Planner agents and worker agents may not set arbitrary global scheduling priority.

A scheduling-priority change does not create a semantic Goal revision, Task revision, or TaskGraph revision. It creates scheduler-policy state/revision only.

## Non-preemptive v1

Priority affects future dispatch order only.

A newly elevated Goal does not terminate already-running work from a lower-priority Goal. Existing admitted Runs continue unless separately cancelled by user, policy, Goal reconciliation, or recovery logic.

Pantheon v1 therefore implements non-preemptive priority.

## Goal fairness

Within the same effective priority class, select the Goal that was least recently **successfully served**.

Conceptually:

```text
Goal A lastSuccessfulDispatch = newest
Goal C lastSuccessfulDispatch = middle
Goal B lastSuccessfulDispatch = oldest

next Goal = B
```

A Goal that has never successfully dispatched is treated as least recently served and receives an early opportunity.

### Fairness is charged only on successful service

Selecting a Goal or attempting routing/admission does not count as service.

Fairness state advances only after Pantheon successfully obtains the reservation needed to create the Run intent.

If a Task cannot currently be executed, the Goal does not lose fair share merely because the scheduler tried it.

## Task ordering within a Goal

For v1, choose the oldest scheduler-eligible Task:

```text
eligibleSince ASC
TaskId ASC
```

`eligibleSince` is the time at which `SchedulingEligible` most recently transitioned to `True`, not Task creation time.

The stable Task ID is used as deterministic tie-breaker.

No v1 ordering based on:

- LLM-estimated importance;
- predicted Task duration;
- model difficulty;
- file count;
- speculative critical-path duration.

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

Backoff belongs to scheduler attempt state, not Task lifecycle.

A Task may remain:

```text
phase: Ready
SchedulingEligible: True
```

while scheduler state records conceptually:

```yaml
scheduler:
  nextAttemptAt: ...
  lastFailure:
    reason: temporarily-unavailable
```

This does not introduce Task phases such as `Queued`, `Retrying`, or `Backoff`.

Relevant events may cause early reconsideration before `nextAttemptAt`, for example a Resource Ledger revision that releases the resource that previously prevented admission.

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

Aging should be based on scheduler service waiting time, not Task creation time.

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

This updates scheduling metadata, for example:

```text
normal → foreground
```

It does not change the semantic Goal contract because urgency is scheduling policy, not desired outcome.

## Disposable queue

Persistent controller state is the source of truth. Any in-memory scheduler queue is disposable.

After restart Pantheon rebuilds the scheduling view from:

- scheduler-eligible Tasks;
- Goal ownership;
- priority policy;
- fairness state;
- scheduler attempt/backoff state;
- active scheduling claims.

## Conceptual selection algorithm

```text
eligible = Tasks where:
  phase == Ready
  SchedulingEligible == True
  nextAttemptAt <= now
  no active SchedulingClaim

group eligible by Goal

for each Goal:
  calculate effectivePriority
  read lastSuccessfulDispatch

priority = highest effectivePriority present
candidateGoals = Goals at that priority
goal = least recently successfully served candidateGoal

task = oldest eligible Task in Goal
       tie-break by stable TaskId

atomically acquire SchedulingClaim(task)
```

Then Pantheon proceeds to ExecutionRequest / offers / routing / admission / reservation.

On successful reservation/Run-intent creation:

```text
update Goal.lastSuccessfulDispatch
```

On temporary scheduling failure:

```text
release SchedulingClaim
record structured failure/backoff
continue scheduling other work
```

## v1 decisions

1. Ordering is backend/provider/model/resource agnostic.
2. Two-level scheduling: Goal first, Task second.
3. Priority is scheduler metadata on Goal ownership, not TaskSpec or semantic GoalSpec.
4. Use named deterministic priority classes.
5. Models cannot elevate cross-Goal scheduling priority.
6. Priority is non-preemptive in v1.
7. Fairness within a priority class is least-recently-successfully-served Goal.
8. Inside a Goal, oldest scheduler-eligible Task wins with deterministic Task ID tie-break.
9. Use `eligibleSince`, not `createdAt`.
10. Best-effort ordering avoids head-of-line blocking.
11. Temporary scheduling failure uses scheduler backoff plus event-driven wakeup without changing Task phase.
12. Bounded aging prevents indefinite starvation.
13. Fairness is charged only after successful reservation/dispatch.
14. No critical-path prediction or usage-weighted fair sharing in v1.
15. User/operator reprioritization changes scheduler policy, not semantic Goal/Task/Graph state.
