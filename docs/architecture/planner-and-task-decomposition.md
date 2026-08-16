# Planner and Task Decomposition

## Status

Canonical Pantheon Planner architecture; progressive autonomous planning is post-v1.

## Purpose

Planner converts a Goal revision into a proposed bounded TaskGraph/GraphPatch. It supplies semantic decomposition, not control-plane authority.

> **Planner proposes structure; Pantheon validates/materializes it. Planner never assigns concrete execution backends/models, grants permissions, creates Runs or directly mutates lifecycle state.**

## Inputs

Planner invocation receives a bounded immutable snapshot including:

```text
Goal revision
current TaskGraph revision/summary
reconciliation/trigger reason
relevant accepted Artifacts/Evidence
hard decomposition/security/budget ceilings
Planner ContextPolicy/Agent snapshot where applicable
```

It does not rely on hidden chain-of-thought from previous planners/workers.

## Output

Planner returns a structured proposal/GraphPatch describing bounded Tasks, dependencies/input bindings and rationale/provenance.

Pantheon validates:

```text
Goal/Graph revision fence
Task schemas/types
boundedness
cycle/dependency legality
security ceiling inheritance
output/input compatibility
policy/decomposition limits
idempotency
```

Only then are Tasks/edges materialized transactionally.

## V1 planning modes

V1 supports two practical modes:

```text
DIRECT
  Goal is already one bounded Task; Planner proposes one Task/minimal graph.

SHALLOW
  Planner proposes a small useful DAG of bounded Tasks up front.
```

`PROGRESSIVE` autonomous long-horizon decomposition/continuous graph optimization is architecture-reserved but implementation-deferred. Runtime discovery in v1 is handled by the explicitly bounded blocking `task.spawn` protocol rather than a general self-expanding Planner loop.

## Minimum useful decomposition

Planner should create the smallest TaskGraph that exposes real independence/dependency/verification value.

Avoid one Task per trivial implementation step and avoid a single giant Task that hides separable outcomes required for acceptance.

Task is a bounded outcome, not an instruction-by-instruction transcript segment.

## Task requirements

Planner specifies semantic Task fields such as:

```text
type/objective
inputs/output slots
competency requirements
scope/effect constraints
acceptance/evaluator refs
```

Planner does not specify:

```text
concrete provider/model/backend
physical executor slot
actual ResourceReservation
BudgetHold
Run/Attempt
secret credential material
```

## Replanning triggers

Replanning is event/state-driven, not periodic model polling. Triggers can include:

```text
Goal revision reconciliation
unrecoverable Task failure
Acceptance evidence requiring different work
Join/child failure making plan impossible
structured discovery requiring replacement work
operator request
```

Planner proposes a **patch** against current immutable history. Running/completed TaskSpecs are never edited in place.

## Supersession

If changed Goal/plan makes an existing Task obsolete, Planner may propose a replacement relationship, but controller applies Task supersession through `Finalizing/terminalTarget=Superseded`; Planner never terminalizes a live Task directly.

## Planning budget/resource

Planner execution is a control operation/approved planning mechanism and remains bounded. Planning does not receive unlimited recursive budget merely because it can create more Tasks.

V1 may use a configured Planner Logical Agent/backend path, but the semantic Planner proposal remains provider-neutral and validated before materialization.

## Context and snapshots

Persist Planner proposal/decision summaries and structured rationale/provenance required for audit. Pantheon does not store/require hidden model chain-of-thought.

Large/reference inputs use Artifact/Context Builder mechanisms rather than embedding the entire project into the planning prompt.

## Dynamic spawn relationship

Normal workers use `task.spawn` for one bounded blocking child where required. Planner/coordinator may receive stricter `task.graph.propose` authority for multi-node changes.

In v1, runtime worker spawn is blocking/yielding only. Joined/detached/semantic-dedup/autonomous graph optimization are deferred.

## Failure

Planner execution failure is not Goal failure. Recovery may retry/reroute planning, request human input or eventually fail the Goal only when no permitted path remains.

A malformed/stale Planner proposal is rejected without partially mutating Graph state.

## Core invariants

1. Planner proposes; controller validates/materializes transactionally.
2. Planner never chooses concrete backend/model, creates Run/Attempt or grants authority.
3. V1 planning is DIRECT or SHALLOW; progressive autonomous planning is deferred.
4. Replanning is event-driven and patch-based; immutable Task history is preserved.
5. Runtime dynamic discovery uses bounded blocking spawn rather than unrestricted self-expanding planning.
6. Planner output is structured/auditable without storing hidden chain-of-thought.
