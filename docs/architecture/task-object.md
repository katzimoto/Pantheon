# Task Object

## Status

Draft design — Pantheon task subsystem specification.

## Purpose

A Pantheon `Task` is an immutable, provider-independent contract describing one bounded outcome. It defines what must be accomplished, what inputs are relevant, what outputs are expected, how success is judged, and what boundaries constrain the work.

A Task does **not** define who performs the work or how it is executed.

## Core resource distinctions

```text
GOAL
What the user ultimately wants
    ↓
TASK
One bounded outcome that must be produced
    ↓
RUN
A resolved execution of that Task
    ↓
ATTEMPT
One concrete try by one executor
    ↓
ARTIFACT
Evidence or result produced by execution
```

The Task remains unchanged when execution moves from a local model to OpenCode or Claude Code.

## Foundational principles

1. **Outcome, not trajectory.** A Task describes the desired result rather than prescribing implementation steps.
2. **Immutable after materialization.** Running work always references a stable Task spec hash.
3. **Provider independent.** Agent, model, harness, session, quota and runtime details belong to Run/Attempt state.
4. **Bounded and verifiable.** A valid Task should represent one meaningful outcome that can be independently understood and evaluated.
5. **Typed inputs and outputs.** Large content is referenced through resources/artifacts rather than embedded into the Task.
6. **Acceptance is separate from production.** Producing an output does not itself prove the Task succeeded.
7. **Task scope is a ceiling.** A Task may narrow the authority available to execution but may not widen enclosing policy.
8. **Graph relationships are external.** Dependencies, joins and dynamic spawn relationships belong to TaskGraph state, not the Task spec.

## Proposed shape

```yaml
apiVersion: pantheon/v1alpha1
kind: Task

metadata:
  id: task_01K...
  name: fix-checkout-timeout
  labels:
    component: checkout
    domain: software

spec:
  type: code.debug

  objective: >
    Identify the cause of the checkout timeout and implement
    the smallest safe fix.

  inputs:
    - name: repository
      ref: repo://whiskyshop

    - name: incident
      ref: artifact://checkout-timeout-report

  requirements:
    capabilities:
      - code-analysis
      - code-editing
      - test-execution

  scope:
    resources:
      include:
        - workspace://src/checkout/**
        - workspace://tests/checkout/**

    effects:
      permit:
        - filesystem.read
        - filesystem.write
        - process.spawn
        - git.commit

      forbid:
        - git.push
        - service.production.mutate

  outputs:
    - name: changeset
      kind: code.changeset
      required: true

    - name: diagnosis
      kind: report
      required: true

  acceptance:
    criteria:
      - id: checkout-works
        statement: Checkout integration tests pass.

      - id: payment-regression
        statement: Existing payment tests continue to pass.

      - id: root-cause
        statement: The result identifies the root cause of the timeout.
```

Field names are still draft. The conceptual boundaries are the important part.

## Objective

`spec.objective` is the semantic center of a Task.

Good:

```yaml
objective: Fix the checkout timeout.
```

Bad:

```yaml
objective: Open src/checkout.ts, change line 72, use Claude, then run npm test.
```

The second form mixes desired outcome, implementation procedure and executor choice. Pantheon must preserve room for an agent to discover that the initial implementation assumption was wrong.

A Task must also be more bounded than a broad Goal. `Improve the website` is a Goal; `Reduce checkout failures caused by the payment callback timeout` can be a Task.

## Inputs

Inputs are semantic references to relevant resources, not inline context blobs and not authorization grants.

```yaml
inputs:
  - name: repository
    ref: repo://Pantheon
  - name: requirements
    ref: artifact://feature-spec-123
```

A Context Builder resolves these references into executor-specific context later.

Input relevance does not imply write permission.

## Outputs

Expected outputs are typed contracts, not the actual produced artifacts.

Example kinds:

```text
code.changeset
report
research.report
security.findings
test.results
design.document
artifact.file
decision
diagnosis
patch
```

After execution, a Run binds output names to concrete ArtifactRefs.

```text
TaskSpec.outputs        expected contract
TaskRun.outputs         actual ArtifactRefs
```

## Acceptance

Acceptance criteria define what success means. They are distinct from output existence.

A code changeset may exist while tests fail. A report may exist while failing to answer the requested question.

Executor-declared completion therefore transitions a run only to a candidate-complete state; Pantheon-owned acceptance logic determines whether the Task is actually satisfied.

Acceptance mechanisms are specified separately in the Acceptance & Completion Contracts design.

## Requirements

Requirements describe execution capabilities the Task needs without naming a concrete executor.

Examples:

```yaml
requirements:
  capabilities:
    - vision
    - browser
    - code-analysis
  locality: local-only
```

The router later resolves an executor that satisfies these constraints.

## Scope as least-privilege envelope

Task scope narrows the maximum authority needed for this Task.

Effective authority is the intersection of enclosing policy and Task scope.

```text
System Policy
    ∩
User Policy
    ∩
Project Policy
    ∩
Agent Policy
    ∩
Task Envelope
    ∩
Temporary Grants
```

A Task may narrow authority but cannot broaden it.

## Task types

Task types are namespaced and used for agent discovery and defaults/policy selection.

Examples:

```text
code.debug
code.implement
code.refactor
code.review
research.web
research.codebase
research.literature
security.ctf
security.audit
security.reverse-engineering
ops.deploy
ops.diagnose
design.architecture
```

Task type must not dictate a concrete agent/model/harness.

## Immutability

A Task becomes immutable when materialized for execution. Requirement changes produce a new revision or superseding Task rather than mutating the contract underneath active Runs.

Each Run records a `taskSpecHash` so execution and evaluation remain reproducible.

Results never mutate the Task definition.

## IDs and names

Task IDs should be opaque, durable identifiers such as UUIDv7/ULID-style IDs.

Human-readable names are metadata and may evolve independently.

## What a Task must not contain

```text
Agent assignment
Model
Provider/harness
Credentials or API keys
Session IDs
Runtime status
Retry counters consumed
Token/quota usage
Worktree path
PID/process state
Dependency edges
Mutable child-task IDs
Raw result bodies
Arbitrary executable hooks
Agent memory
Private reasoning traces
```

## Resource relationships

```text
Goal
  ↓
TaskGraph
  ├── Task
  │     └── Run
  │          ├── Attempt
  │          └── Artifact refs
  └── Task
```

TaskGraph owns dependency/order relationships. Dynamic Task creation and lineage are described in `task-spawn-and-dynamic-graphs.md`.

## Core invariant

> Task = immutable statement of required outcome.
>
> Run = resolved execution of that statement.
>
> Attempt = one worker trying to satisfy it.
>
> Artifact = evidence/result produced by execution.
>
> TaskGraph = relationships among Tasks.
