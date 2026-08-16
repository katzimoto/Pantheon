# Task Spawning and Dynamic Graphs

## Status

Draft design — Pantheon task subsystem specification.

## Purpose

Pantheon supports dynamic discovery of work: a running Task may determine that additional bounded outcomes are needed. Workers may propose new Tasks, but only the Pantheon controller may materialize Tasks or mutate TaskGraph state.

This preserves autonomous decomposition without surrendering deterministic control of scheduling, authorization, graph integrity or resource usage.

## Foundational principle

> Agents propose work; Pantheon materializes it.

A worker may say `I need this additional outcome`. It may not directly create scheduler state, choose the executor, grant authority or mutate graph edges.

## Three independent relationships

Pantheon must distinguish creation provenance, dependencies and joins.

```text
SPAWN / PROVENANCE
A ──spawn──> B
A caused B to exist.

DEPENDENCY
B ──dependency──> A
A cannot satisfy its contract without B.

JOIN
A ──waits-for──> B
A's current execution is waiting for a condition involving B.
```

These relationships must never be inferred from one another.

A spawned Task may be blocking, joined later, or detached. A dependency may have been created by the original planner rather than by runtime spawning.

## Spawn request

Task spawning is a controller command/event, not a user-authored durable Task resource.

Conceptual request:

```yaml
requestId: spawn_01K...

parent:
  task: task_123
  run: run_456

proposal:
  type: research.codebase
  objective: >
    Determine how refresh tokens are invalidated.

  inputs:
    - ref: repo://project

  outputs:
    - name: findings
      kind: research.report

reason:
  code: missing-information
  explanation: >
    The implementation depends on existing refresh-token
    invalidation behavior.

relationship:
  mode: blocking
```

Pantheon responds by accepting/materializing the proposal or rejecting it with a structured reason.

## Spawn validation

Before materialization the controller must check at least:

1. caller authorization (`task.spawn`);
2. Task schema validity;
3. requested Task type policy;
4. ancestry depth and fan-out limits;
5. descendant/concurrency budgets;
6. security-envelope inheritance;
7. graph integrity/cycle safety;
8. idempotency;
9. relevant resource/budget/quota policy;
10. whether the requested join/dependency relationship is legal.

The worker does not choose the child agent, model or harness.

## Idempotency

Spawn requests must be idempotent in v1.

A worker may retry a spawn request after a crash or lost response. Repeating the same `(parentRun, idempotencyKey)` returns the previously materialized Task rather than creating another Task.

```text
(parentRun, idempotencyKey)
          ↓
       task_839
```

Exact duplicate prevention is mandatory. Semantic duplicate detection is deferred.

## Transactional graph mutation

Materialization must be atomic.

Conceptually one transaction performs:

```text
validate proposal
    ↓
create immutable Task
    ↓
record provenance
    ↓
create requested graph relationships
    ↓
increment graph revision
    ↓
emit events
```

A crash must not leave a Task without its graph relationship or a parent waiting on a child that was never committed.

## Graph revisions

TaskGraphs are dynamic and revisioned.

```text
revision 17
   ↓ spawn accepted
revision 18
   ↓ another mutation
revision 19
```

Every mutation records what changed and why. Runs may record the graph revision relevant to their scheduling/creation context.

## Spawn relationship modes

### Blocking

The parent cannot make useful progress until the child result is available.

```text
Parent run
   ↓ spawn
Child
   ↓
Parent waits
   ↓ child completes
Parent resumes
```

### Joined

The parent may continue with independent work and later reaches a join point that requires the child result.

```text
Parent ──────────────┐
   └── Child         │
        ↓            │
     completes       │
                     ▼
                  join point
```

### Detached

The new Task belongs to the wider Goal but does not block the parent and survives the parent's normal completion.

Typical examples include documentation, follow-up hardening or non-blocking investigation.

The relationship mode belongs to graph state, not the child Task spec.

## Join semantics

Joins are extensible but v1 should support only `all`.

Future strategies may include:

```text
all
any
quorum
min-success
```

Child failure is an event/input to join/recovery logic and does not intrinsically imply parent failure.

## Lifetime and cancellation

Default v1 semantics:

```text
blocking/joined child -> attached to parent execution
detached child        -> owned by the Goal
```

If a parent is explicitly cancelled, attached descendants receive cancellation. Detached work continues unless the Goal itself is cancelled.

Failure and cancellation are distinct events. v1 may use conservative cancellation of attached children when a parent irrecoverably fails; later recovery policy may preserve useful descendants.

## Security-envelope inheritance

Descendants inherit ceilings, never privileges.

Maximum child authority is an intersection of all enclosing policy and ancestry envelopes.

```text
System policy
    ∩
User policy
    ∩
Project policy
    ∩
Goal policy
    ∩
Parent ancestry envelopes
    ∩
Child Task envelope
```

A child can narrow authority but cannot widen it through recursion.

Example:

```text
Parent: workspace://src/**
Child asks: workspace://**
Effective child ceiling: workspace://src/**
```

Privilege expansion requires an explicit higher-level grant; recursive Task creation cannot manufacture authority.

## Context inheritance

Children inherit constraints, not entire context.

A spawn proposal explicitly identifies required inputs/artifacts. The Context Builder decides how those references are represented for the child executor.

Parent prompts, full conversation history, unrelated memory and secrets are not automatically copied into descendants.

## Child results

Child outputs are returned through ArtifactRefs and structured completion metadata.

```yaml
childResult:
  task: task_812
  status: completed
  outputs:
    findings: artifact://research-812
```

The parent/context builder decides what portion of the artifact should enter active model context.

## Spawn outcomes, not agents

Normal workers should request outcomes:

```text
spawn_task(type, objective, inputs, outputs)
```

rather than workers:

```text
spawn_agent(agent, model)
```

Pantheon's router remains responsible for selecting the logical Agent and execution backend.

## Worker vs planner authority

Pantheon should distinguish two capabilities:

```text
task.spawn          propose one bounded new Task
task.graph.propose  propose a multi-node graph mutation
```

Normal specialist workers receive `task.spawn` when appropriate. Planner/coordinator agents may receive `task.graph.propose` under stricter policy.

This avoids turning every worker into an unrestricted planner.

## Spawn reasons

Each spawn proposal must include structured provenance about why new work is needed.

Initial reason codes:

```text
discovered-dependency
missing-information
specialist-required
validation-required
parallelizable-work
remediation
follow-up
replan
```

A human-readable explanation accompanies the code.

## Limits

Dynamic work must be bounded by hierarchical policy.

```text
maxDirectChildren
maxDepth
maxDescendants
maxConcurrentDescendants
```

These address distinct failure modes: immediate fan-out, recursive chains, exponential trees and local-resource saturation.

Limits primarily belong to system/user/project/agent/Goal policy. A Task may tighten them but cannot raise enclosing ceilings.

## Duplicate handling

### v1: exact duplicates

Use idempotency keys and parent-run identity.

### Later: semantic duplicates

Pantheon may detect likely equivalent Tasks and propose reuse/attachment, but semantic deduplication must not silently merge Tasks in v1.

## Provenance

Every dynamically materialized Task records immutable creation provenance, separate from its semantic spec.

Conceptually:

```yaml
provenance:
  createdBy:
    type: task-run
    id: run_456
  parentTask: task_123
  spawnReason:
    code: missing-information
  graphRevision: 18
  proposalHash: sha256:...
```

This says why the Task exists; it does not imply execution dependency.

## Learning and telemetry

Task spawning is a first-class outcome signal for Agent Genome learning.

Useful measurements include:

```text
spawn_count
spawn_depth
spawn_accept_rate
spawn_reject_rate
child_success_rate
child_output_consumed
duplicate_spawn_rate
child_cost
parent_improvement_after_child
```

This allows Pantheon to learn whether an agent usefully decomposes work or creates unnecessary coordination overhead.

## v1 scope

Include:

- `task.spawn`;
- idempotency;
- immutable spawn provenance;
- blocking/joined/detached relationships;
- `all` join strategy;
- transactional graph mutation;
- graph revisions;
- depth/fan-out/descendant/concurrency limits;
- security-envelope inheritance;
- explicit child input/artifact flow;
- attached-vs-detached cancellation semantics;
- structured spawn reasons.

Defer:

- semantic duplicate detection;
- batch/dynamic mapping;
- quorum/any/min-success joins;
- arbitrary conditional graph expressions;
- automatic graph optimization;
- cross-Goal Task reuse;
- automatic Task merging.

## Core invariants

1. **Agents propose work; Pantheon materializes it.**
2. **Spawn provenance, dependency and join relationships are independent.**
3. **Descendants inherit ceilings, never privileges.**
4. **Spawn is idempotent and graph mutation is transactional.**
5. **Workers request outcomes; Pantheon assigns workers and execution resources.**
