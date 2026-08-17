# Task Object

## Status

Canonical Pantheon Task subsystem specification.

## Purpose

A Pantheon `Task` is an immutable, execution-independent contract describing one bounded outcome. It defines what must be accomplished, relevant inputs, expected outputs, acceptance criteria, semantic competencies required, and the maximum scope of work.

A Task does **not** define which Logical Agent performs the work or how/where that Agent executes.

See also:

- `docs/architecture/tasks/taskgraph-dependencies.md`
- `docs/architecture/tasks/task-lifecycle.md`
- `docs/architecture/evaluation-and-acceptance/task-acceptance-and-completion.md`
- `docs/architecture/agents-and-context/logical-agent-resolution.md`
- `docs/architecture/execution/run-and-attempt.md`

## Core hierarchy

```text
GOAL
What the user ultimately wants
    ↓
TASKGRAPH
Relationships between bounded outcomes
    ↓
TASK
One bounded outcome that must be produced
    ↓
RUN
One immutable resolved execution strategy
    ↓
ATTEMPT
One logical backend-execution lineage
    ↓
ARTIFACT / EVIDENCE
Produced outputs and proof
```

## Foundational principles

1. **Outcome, not trajectory.** Task describes the desired result rather than prescribing implementation steps.
2. **Immutable after materialization.** Active work always references a stable Task spec hash.
3. **Execution independent.** Logical Agent, backend, model/runtime details, session identity and routing belong below Task.
4. **Bounded and verifiable.** A valid Task represents one meaningful outcome that can be independently understood and evaluated.
5. **Typed inputs and outputs.** Large content is referenced through resources/artifacts rather than embedded into Task.
6. **Acceptance is separate from production.** Producing an output does not prove Task success.
7. **Task scope is a ceiling.** Task may narrow authority but never broaden enclosing policy.
8. **Graph relationships are external.** Dependencies, joins and spawn lineage belong to TaskGraph/runtime graph state.
9. **Semantic competencies are not execution features.** Task says what ability is needed; Agent resolution and Execution Fabric separately determine who can do it and how it can run.

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
    competencies:
      - code.analysis
      - code.debugging
      - code.editing
      - test.execution

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

Field names remain draft until schema freeze; the conceptual boundaries are normative.

## Objective

`spec.objective` is the semantic center of a Task.

Good:

```yaml
objective: Fix the checkout timeout.
```

Bad:

```yaml
objective: Open a specific file, use a specific executor, change line 72, then run one command.
```

The second form mixes desired outcome, assumed implementation procedure, and execution strategy. Pantheon must preserve room to discover that an initial implementation assumption is wrong.

A Task is more bounded than a Goal. `Improve the website` is a Goal; `Reduce checkout failures caused by the payment callback timeout` can be a Task.

## Inputs

Inputs are semantic references to relevant resources, not inline context blobs and not authorization grants.

```yaml
inputs:
  - name: repository
    ref: repo://Pantheon
  - name: requirements
    ref: artifact://feature-spec-123
```

A later Context Builder resolves these references into an immutable Run execution snapshot and backend-specific presentation.

Input relevance does not imply read or write authority.

## Outputs

Expected outputs are typed contracts, not produced artifacts themselves.

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

A candidate result binds output names to immutable ArtifactRefs.

```text
TaskSpec.outputs     expected contract
Candidate.outputs    actual ArtifactRefs
```

## Acceptance

Acceptance criteria define what success means and are separate from output existence.

A code changeset may exist while tests fail. A report may exist while failing to answer the required question.

A Run may submit one candidate result, but only Pantheon-owned acceptance logic may declare the Task satisfied.

## Requirements and competencies

Task requirements describe **semantic abilities** needed to achieve the outcome.

```yaml
requirements:
  competencies:
    - vision.analysis
    - web.research
    - code.analysis
```

A competency is not:

- a concrete Agent;
- an Agent Skill;
- a backend/provider/model;
- an Execution Feature;
- a tool/action permission;
- an authorization capability grant/ticket.

The Agent Resolver first uses Task type and competencies to determine which Logical Agents are eligible.

For each eligible Agent, Pantheon then constructs an Agent-specific `ExecutionRequest` whose execution requirements are evaluated by the Execution Fabric.

This produces the intended separation:

```text
Task competencies
      ↓
Logical Agent eligibility
      ↓
Agent-specific ExecutionRequest
      ↓
Execution Features / placement / isolation / resources
      ↓
ExecutorBackend offers
```

## Task type

Task types are namespaced and used for Agent discovery and policy/default selection.

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

Task type must not dictate a concrete Agent or backend.

## Scope as least-privilege envelope

Task scope narrows the maximum authority needed for this Task.

Effective authority remains the intersection of enclosing policy and Task scope:

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

Scope does not itself grant authority.

## Immutability

A Task becomes immutable when materialized for execution.

Requirement changes produce a new/superseding Task or appropriate graph/reconciliation action rather than mutating the contract beneath active Runs.

Every Run records/references the immutable Task spec hash used for its strategy.

Results never mutate Task definition.

## IDs and names

Task IDs are opaque durable identifiers. Human-readable names are metadata and must not be used as execution identity.

## What Task must not contain

```text
Logical Agent assignment
backend/provider/model/runtime assignment
credentials or API keys
session IDs / LaunchKeys
runtime status
consumed retry counters
token/quota usage
worktree path
PID/process state
dependency edges
mutable child-task IDs
raw result bodies
arbitrary executable hooks
Agent memory
private reasoning traces
```

## Resource relationships

```text
Goal
  ↓
TaskGraph
  ├── Task
  │     └── Run
  │          ├── Attempt
  │          └── candidate ArtifactRefs
  └── Task
```

TaskGraph owns dependency/order relationships. Dynamic Task creation and provenance are defined separately.

## Core invariants

> **Task = immutable statement of required bounded outcome.**
>
> **Competency = semantic ability the Task requires, not an execution or authorization mechanism.**
>
> **Logical Agent selection and backend routing are deliberately excluded from Task.**
>
> **Run = one immutable resolved strategy for pursuing the Task.**
>
> **Attempt = one logical backend-execution lineage under that Run.**
