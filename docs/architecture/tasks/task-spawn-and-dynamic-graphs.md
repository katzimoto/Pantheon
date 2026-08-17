# Task Spawning and Dynamic Graphs

## Status

Canonical Pantheon dynamic Task-spawn specification.

## Purpose

Running Agents may discover bounded additional outcomes, but only Pantheon materializes Tasks and mutates TaskGraph state.

> **Agents propose work; Pantheon materializes it. Workers request outcomes, never unmanaged agents/processes.**

See also:

- `docs/architecture/execution/agent-control-channel.md`
- `docs/architecture/tasks/blocking-spawn-and-run-yield.md`
- `docs/architecture/tasks/taskgraph-dependencies.md`
- `docs/architecture/tasks/task-lifecycle.md`

## Authority

Normal v1 worker capability:

```text
task.spawn
```

Reserved post-v1 worker/coordinator vocabulary:

```text
task.graph.propose
```

No v1 `AgentControlSession` is authorized to invoke `task.graph.propose`. Multi-node structural graph proposals in v1 enter through the PlanningOperation/PlanningRecord/GraphPatch path defined by the Planner architecture, not through a worker control verb.

The worker does not choose child Agent, provider/model/backend, Sandbox or physical concurrency.

Spawn request arrives through Attempt-authenticated Agent Control. Pantheon derives parent Attempt/Run/Task/Goal from the session; request bodies cannot impersonate another parent.

## V1 spawn mode

V1 implements **blocking spawn only**.

Architecture reserves future joined/detached relationships and `task.graph.propose`, but they are implementation-deferred because they require additional lifetime/join-point/authority semantics. They are not part of the v1 behavior contract.

Blocking means the parent cannot usefully continue until the accepted child result exists.

Pantheon does **not** keep the parent executor Run suspended. It uses the yield protocol:

```text
parent Run Active
  ↓ blocking spawn accepted/materialized
Run -> Finalizing / terminalTarget=Yielded
Task stays Active while old execution closes
  ↓
Run Yielded
Task -> Waiting
  ↓ child accepted/join satisfied
Task -> Ready
  ↓ Scheduler creates a new Run with ContinuationContext
```

This releases Run-scoped concurrency/resources while preserving the Task Workspace.

## Spawn proposal

Conceptual semantic payload contains only bounded child outcome information, for example:

```yaml
requestId: req_...
idempotencyKey: refresh-token-behavior
proposal:
  type: research.codebase
  objective: Determine how refresh tokens are invalidated.
  inputs:
    - ref: repo://project
  outputs:
    - name: findings
      kind: research.report
reason:
  code: missing-information
  explanation: ...
relationship:
  mode: blocking
```

Server derives parent provenance. Child inputs are explicit refs; parent prompts/history/secrets are not automatically copied.

## Validation

Before materialization Pantheon checks at least:

1. current Attempt/Run/Task authority and `task.spawn` permission;
2. Task schema and accepted Task type;
3. Goal still permits new work;
4. depth/fan-out/descendant limits;
5. security ceiling inheritance;
6. graph integrity/cycle safety;
7. exact idempotency;
8. bounded Task outcome/output contract;
9. budget/resource policy for descendant creation;
10. legality of blocking relationship under current Task/Goal state.

After the blocking-spawn transaction commits, the old Agent loses authority to start additional semantic work; Run finalization stops the Attempt.

## Idempotency

Two layers apply:

```text
(attempt_id, requestId)
  Agent Control request idempotency

(parentRun, idempotencyKey)
  spawn semantic/materialization idempotency
```

Retrying a lost response returns the same child Task rather than creating duplicates. Same idempotency identity with different proposal hash fails closed.

Semantic duplicate detection is deferred.

## Atomic materialization and yield intent

Blocking spawn is one authoritative Graph/Run-intent transaction:

```text
BEGIN IMMEDIATE

revalidate current parent Agent/Run/Task/Goal/revisions
validate spawn/idempotency
create immutable child Task
record immutable spawn provenance
create child input bindings
create blocking Join
create required graph edges
increment Graph revision

parent Run:
  Active -> Finalizing
  terminalTarget = Yielded
  desiredExecution = stopped

append Events
COMMIT
```

Task remains Active until Run Controller safely finalizes/yields. This prevents a committed child with no parent wait intent, or a parent stopping without a committed child.

## Join

V1 join strategy is only:

```text
all
```

Join state belongs to durable TaskGraph/controller state, not to a model process.

When the child Task succeeds, the required accepted child output Artifact is bound into the parent join/ContinuationContext. Parent never scrapes child stdout/conversation/workspace.

Once all required outputs are accepted and the parent is Waiting with zero nonterminal Runs:

```text
Join -> SATISFIED
Task Waiting -> Ready
```

Scheduler, not Join Controller, creates the later Run.

## Child failure

Child failure does not intrinsically fail the parent. If an `all` Join becomes impossible under current child state, Join/Recovery logic receives structured evidence and may retry/replan/request human/fail parent according to policy.

## Security inheritance

Descendants inherit ceilings, never privileges:

```text
built-in/system hard policy
∩ user/project configuration
∩ Goal/Task restrictions
∩ parent ancestry ceiling
∩ child Task restrictions
```

Spawn cannot manufacture broader filesystem/network/secret/delegation authority. Child receives no secret material and no implicit parent credential.

## Context inheritance

Child receives explicit input refs/Artifacts plus its own ContextPlan. Parent provider conversation, hidden reasoning, unrelated memory and secrets are not copied.

Parent continuation later receives accepted child Artifact bindings through `ContinuationContext`, not by resuming the old provider session.

## Spawn reasons

Initial structured provenance reasons may include:

```text
discovered-dependency
missing-information
specialist-required
validation-required
remediation
replan
```

Human explanation is metadata, never graph authority by itself.

## Limits

Hierarchical ceilings may include:

```text
maxDirectChildren
maxDepth
maxDescendants
maxConcurrentDescendants
```

Task may tighten but not raise enclosing ceilings.

## Cancellation

Blocking child is attached to the parent Task/Goal lifetime. Parent cancellation propagates cancellation to attached child according to Task finalization semantics.

Because parent Waiting owns no Run, cancellation does not require reviving/terminating a dormant parent executor.

Future detached lifetime semantics are explicitly deferred.

## Provenance

Child records immutable creation provenance such as:

```text
createdBy Attempt/Run
parent Task
Goal
spawn reason
Graph revision
proposal hash
```

Provenance does not imply that every parent-child relation is a success dependency; graph edges/join state are explicit.

## Core invariants

1. Agents propose bounded Tasks; Pantheon alone materializes graph state.
2. Parent identity is server-derived from Attempt Agent Control, never caller-asserted.
3. Spawn is exact-idempotent and graph mutation is transactional.
4. V1 dynamic spawn is blocking only and always yields the old Run rather than suspending it.
5. V1 exposes `task.spawn` as the runtime worker graph-discovery operation; `task.graph.propose` is reserved post-v1 vocabulary and has no v1 Agent Control authority.
6. Multi-node structural graph planning in v1 enters through PlanningOperation -> PlanningRecord -> GraphPatch, not through a worker control verb.
7. Task enters Waiting only after yielded Run is safely terminal and Run-scoped capacity is released.
8. Child result enters parent continuation only as accepted immutable Artifact binding.
9. Join satisfaction returns Task Ready; only Scheduler creates the new Run.
10. Descendants inherit ceilings, never privileges or raw credentials.
11. Joined/detached/semantic dedup/quorum joins are post-v1.
