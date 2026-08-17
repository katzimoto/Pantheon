# TaskGraph Dependencies

## Status

Canonical Pantheon TaskGraph/dependency specification; `after_terminal` and advanced conditional joins are post-v1.

## Purpose

TaskGraph expresses durable semantic work relationships. Task specs remain immutable; graph edges/bindings are revisioned controller state.

## V1 dependency kind

V1 implements one readiness prerequisite:

```text
requires_success
```

Meaning downstream Task cannot become logically runnable until the upstream Task has terminal `Succeeded` and any required output bindings are valid.

`after_terminal` is architecture-reserved but implementation-deferred because v1 has no clear workflow requiring work to begin after an upstream failure/cancellation regardless of outcome. Such recovery/replanning is expressed through Recovery/Planner rather than a second default readiness rule.

## Inputs and dependencies are separate

Task input bindings explicitly name accepted immutable outputs/Artifacts. Dependency edge expresses readiness relation.

A required input binding from upstream Task implies a corresponding `requires_success` prerequisite in v1; Pantheon materialization validates/adds the needed relationship rather than allowing a consumer to run before its required output can exist.

Not every success dependency must necessarily bind an output; it may represent ordering/semantic prerequisite.

## Graph revision

Graph is revisioned. Mutations are atomic and preserve history. Task specs are not edited merely because graph changes.

Conceptual temporal edge representation:

```text
upstream_task
downstream_task
kind
created_graph_revision
removed_graph_revision NULL
```

Active at revision R when created <= R and removed is null or > R.

## Atomic GraphPatch

Planner/spawn controller proposes mutations; Graph Controller validates and transactionally:

```text
check Goal/Graph revision fence
validate Tasks/refs
validate edge/input compatibility
validate no cycles
create/remove temporal edges
create input bindings
increment Graph revision
append Events
```

Crash cannot leave an input binding without its required prerequisite or a half-applied graph patch.

## Cycle safety

V1 TaskGraph readiness dependencies must remain acyclic. Cycle detection occurs inside/around the authoritative GraphPatch commit with revision/CAS revalidation.

Blocking spawn join state is a separate durable relation but also cannot create an orchestration deadlock by keeping the parent Run alive; the parent yields before Waiting.

## Failure propagation

If an upstream `requires_success` Task reaches terminal Failed/Cancelled/Superseded without a compatible replacement satisfying the dependency, downstream remains non-runnable. Recovery/Planner decides whether to:

```text
retry/replace upstream
replan graph
supersede downstream
fail enclosing Task/Goal
request human
```

Graph edge itself does not silently convert failure into success.

## Supersession

Superseded Tasks remain history. GraphPatch may replace an edge/input producer with a new immutable Task; old edge becomes historical via removed revision rather than deletion.

Active Task supersession follows Task Finalizing semantics before terminal Superseded.

## Dynamic spawn

V1 blocking spawn transaction creates:

```text
child Task
spawn provenance
blocking Join
required graph relationship/input expectation
Graph revision
parent Run terminalTarget=Yielded
```

Join satisfaction later binds accepted child Artifact and returns Waiting parent Task Ready. It does not directly create a Run.

## Core invariants

1. V1 readiness dependency is `requires_success` only.
2. Required upstream output binding implies success prerequisite.
3. TaskGraph mutations are revisioned/atomic and never rewrite TaskSpec history.
4. Active dependency graph is acyclic.
5. Upstream failure feeds Recovery/Planner; dependency is not silently satisfied.
6. `after_terminal`, quorum/any/conditional expressions and advanced graph gates are deferred.
