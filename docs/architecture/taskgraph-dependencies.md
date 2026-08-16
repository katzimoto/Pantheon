# TaskGraph Dependencies and Dataflow

## Status

Draft design — Pantheon task subsystem specification.

## Purpose

`TaskGraph` owns relationships among immutable `Task` contracts. It determines when work is logically ready, how named outputs satisfy downstream inputs, how runtime joins are represented, and how dynamic graph mutations are validated and versioned.

The graph is declarative. It reports dependency/readiness truth; it does not choose executors, perform recovery, or own Goal acceptance.

## Foundational principles

1. **Task defines work; TaskGraph defines relationships.**
2. **Ordering, dataflow, spawn provenance, and runtime joins are distinct relationship classes.**
3. **Dependency satisfaction is explicit.** v1 uses `requires_success` and `after_terminal` rather than vague hard/soft labels.
4. **Required data bindings create dependencies naturally.** Duplicate dependency configuration is avoided.
5. **Failure effects are local to actual dependencies.** No graph-wide fail-fast by default.
6. **Graph mutation is controller-owned, transactional, revisioned, and cycle-checked.**
7. **Readiness is logical eligibility, not resource availability.**

## Relationship classes

Pantheon defines four relationship classes:

```text
PREREQUISITE
Controls whether a Task may initially become Ready.

BINDING
Connects a named upstream output to a named downstream input.

SPAWN
Records immutable creation provenance.

JOIN
Controls waiting after a Task has already started.
```

These relationships must never be inferred from one another.

## Prerequisites

v1 supports two prerequisite conditions.

### `requires_success`

The downstream Task may become Ready only when the upstream Task reaches `Succeeded`.

```text
A --requires_success--> B
```

### `after_terminal`

The downstream Task waits until the upstream Task reaches any terminal phase (`Succeeded`, `Failed`, `Cancelled`, or `Superseded`). The upstream Task does not need to succeed.

```text
A --after_terminal--> B
```

This is useful for reporting, cleanup, diagnostics, and other order-only work.

Pantheon v1 deliberately defers boolean trigger expressions such as `one_failed`, quorum, threshold, and arbitrary upstream predicates. Conditional remediation should normally be materialized dynamically when the relevant condition actually occurs.

## Dataflow and input/output bindings

Control ordering and dataflow are separate concepts.

A Task may reference an already-existing input directly:

```yaml
inputs:
  - name: repository
    ref: repo://Pantheon
```

Or declare a required typed input slot:

```yaml
inputs:
  - name: architecture
    expects:
      kind: research.report
    required: true
```

`TaskGraph` resolves slots through bindings:

```yaml
bindings:
  - from:
      task: task_research
      output: findings
    to:
      task: task_implement
      input: architecture
```

A required binding is satisfied only when:

1. the upstream Task succeeded;
2. the named upstream output exists;
3. the produced Artifact is valid;
4. the output kind is compatible with the downstream input contract.

Therefore a required binding creates its own success dependency and should not require a duplicate explicit prerequisite.

Bindings always reference named outputs and named inputs. `whatever the Task returned` is not a valid graph contract.

## Type validation

The graph validator checks compatibility between output and input kinds before committing a graph revision.

Example:

```text
research.report -> research.report  valid
code.changeset  -> research.report  invalid
```

Future versions may support explicit converters/adapters, but v1 requires direct compatibility.

## Readiness

A Pending Task becomes Ready when all logical readiness conditions are true:

```text
all prerequisite gates satisfied
AND
all required input bindings resolvable
AND
graph activation permits the Task
```

Conceptually:

```text
ready(T) =
    prerequisites_satisfied(T)
    && required_inputs_resolved(T)
    && graph_active(T)
```

Executor/model availability, provider quota, CPU/RAM, and agent availability do not participate in readiness. Those are scheduler concerns.

## Dependency impossibility

If `A --requires_success--> B` and A reaches a non-success terminal state, B remains `Pending`; the graph reconciler reports that its dependency is currently impossible under the active graph revision.

Example condition:

```yaml
phase: Pending
conditions:
  - type: DependenciesSatisfied
    status: "False"
    reason: UpstreamFailed
  - type: DependenciesImpossible
    status: "True"
    reason: RequiredUpstreamTerminalFailure
```

The graph engine does not automatically mark B `Failed`. Recovery/planning policy may retry or supersede A, replan the graph, cancel the Goal, or ultimately terminalize B.

## Fan-out and fan-in

Fan-out requires no special primitive:

```text
          B
         /
A ------ C
         \
          D
```

When A satisfies each prerequisite, B/C/D independently become Ready. The scheduler decides concurrency.

v1 fan-in is all-of only:

```text
B --\
C ----> E
D --/
```

E becomes Ready only when every required prerequisite and binding is satisfied.

`ANY`, quorum, threshold, and boolean fan-in are deferred.

## Failure locality

Pantheon does not make TaskGraph globally fail-fast by default.

A failure blocks only work whose prerequisites or required bindings depend on that failure. Independent branches remain eligible to continue.

Goal-level strategies such as fail-fast, best-effort, or critical-path policies belong to the Goal/recovery layer, not TaskGraph semantics.

## Goal success

TaskGraph never infers that a Goal succeeded merely because leaf Tasks succeeded. A Goal may have its own acceptance contract.

```text
TaskGraph terminal shape != Goal acceptance
```

## Graph structure

Prefer explicit collections over a generic edge bag:

```yaml
apiVersion: pantheon/v1alpha1
kind: TaskGraph

metadata:
  id: graph_01K...

spec:
  goal:
    ref: goal_123

  tasks:
    - task: task_research
    - task: task_implement
    - task: task_test

  prerequisites:
    - upstream: task_implement
      downstream: task_test
      condition: requires_success

  bindings:
    - from:
        task: task_research
        output: findings
      to:
        task: task_implement
        input: architecture
```

Using `upstream`/`downstream` for prerequisites avoids ambiguity about edge direction.

Spawn provenance and runtime joins are maintained as separate graph state defined by the dynamic-spawn subsystem.

## Dynamic graph mutation

Only the Pantheon controller may commit graph mutations. Planner/worker agents propose changes.

A mutation transaction validates and atomically applies:

1. referenced Tasks/resources exist;
2. input/output names exist;
3. output/input kinds are compatible;
4. dependency/dataflow cycles are absent;
5. runtime wait/join deadlock cycles are absent;
6. security/task limits remain valid;
7. graph revision is current;
8. mutation obeys runtime monotonicity rules.

Then the graph revision increments and a durable event is appended.

## Two cycle classes

### Scheduling/dataflow cycle

```text
A requires B
B requires C
C requires A
```

No Task can become Ready. The mutation is rejected.

Cycle detection must include prerequisite edges plus required binding dependencies.

### Runtime join deadlock

```text
A waits for B
B waits for C
C waits for A
```

All active Tasks are waiting. The join mutation is rejected.

Spawn/provenance edges are excluded from both cycle calculations because provenance does not imply ordering.

## Runtime mutation monotonicity

Before execution begins, a planner may freely construct a valid graph.

After execution begins, mutations should be mostly monotonic. v1 permits adding Tasks, prerequisites, bindings, spawn provenance, and runtime joins. Existing prerequisites/bindings for materialized or executing work should not be casually removed to bypass previously established constraints.

If requirements materially change, supersede the affected Task and create a corrected Task/relationship rather than mutating history in place.

## Revisions and observed state

`TaskGraph` carries a monotonically increasing revision.

```text
graph revision 17 -> 18 -> 19
```

Task conditions record the graph revision they observed. If `DependenciesSatisfied=True` was computed against revision 17 while the graph is now revision 19, the reconciler treats the condition as stale and recomputes it.

## Reconciliation

Graph events never imperatively start downstream work.

```text
upstream Task terminal/event
        ↓
Graph Reconciler
        ↓
recompute prerequisite/binding conditions
        ↓
Pending -> Ready when satisfied
        ↓
Scheduler observes Ready
```

This keeps scheduling separate from graph semantics and makes crash recovery deterministic.

## v1 scope

Include:

- `requires_success` prerequisites;
- `after_terminal` prerequisites;
- all-of prerequisite aggregation;
- named output-to-input bindings;
- basic artifact-kind compatibility checks;
- multiple roots;
- arbitrary fan-out;
- all-of fan-in;
- dynamic Task addition;
- revisioned transactional mutations;
- dependency/dataflow cycle detection;
- runtime join deadlock detection;
- localized failure effects;
- no graph-wide fail-fast by default.

Defer:

- ANY/quorum/threshold dependencies;
- arbitrary boolean dependency expressions;
- failure-trigger expressions;
- mapped/batch Tasks;
- semantic graph optimization;
- automatic edge removal;
- implicit Goal success from graph leaves.

## Key invariants

1. **Dataflow and ordering are separate abstractions.**
2. **Required data bindings imply the necessary success dependency.**
3. **Spawn provenance, prerequisites, bindings, and joins are independent.**
4. **Failure propagates only through actual dependency requirements.**
5. **TaskGraph reports readiness truth; recovery remains a separate control-plane concern.**
6. **Only the controller commits graph mutations.**
7. **All graph mutations are revisioned, auditable, and cycle-safe.**
