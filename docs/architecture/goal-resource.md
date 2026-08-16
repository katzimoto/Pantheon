# Goal Resource

## Status

Draft design — Pantheon goal subsystem specification.

## Purpose

A Pantheon `Goal` is the durable, revisioned contract for the user's desired outcome. It survives multiple Tasks, TaskGraph revisions, agents, models, replans, process restarts, and context resets.

A Goal is not executable. Tasks and Runs are the mechanisms used to satisfy it.

## Core hierarchy

```text
USER REQUEST
    ↓
GOAL
Durable user outcome contract
    ↓
TASKGRAPH
Current work structure
    ↓
TASK
Bounded outcome
    ↓
RUN
Resolved execution
    ↓
ATTEMPT
One concrete try
    ↓
ARTIFACT
Produced result
    ↓
EVIDENCE
Verification
```

## Foundational principles

1. **Goal is not a prompt or session.** The original request is preserved as provenance; the Goal is a normalized durable contract.
2. **Goal has stable identity and explicit revisions.** User intent can legitimately evolve over time.
3. **Tasks remain immutable.** Each Task records which Goal revision caused it to be materialized.
4. **Goal describes outcome, not execution strategy.** It never binds to a model, provider, agent, queue or session.
5. **Constraints, preferences and acceptance are distinct.** Constraints are mandatory, preferences are optimization hints, acceptance determines success.
6. **Task success does not imply Goal success.** Goal acceptance is evaluated independently against the final accepted deliverables.
7. **Goal acceptance is evidence-based.** It reuses the same Acceptance Engine and Evidence model used for Tasks.
8. **Replanning reconciles revisions rather than rewriting history.** Completed and running Task specs are not silently mutated.
9. **No nested Goal tree in v1.** One Goal owns one evolving TaskGraph.
10. **Spawned Tasks stay under the same Goal.** A detached child is detached from its parent Task lifetime, not from the Goal.
11. **Tasks cannot create new Goals to escape policy, budget or decomposition ceilings.** Goal creation is a separate privileged operation.

## Goal versus source request

The raw user request is immutable provenance. The Goal is a normalized contract derived from it.

```text
User request
    ↓
source/provenance
    ↓
Goal revision 1
```

Conceptual provenance:

```yaml
provenance:
  createdBy:
    type: user
  sources:
    - ref: message://conversation/123/msg/981
  createdAt: ...
```

Every later user clarification/change is also preserved as an immutable event/source.

## Revision model

Goals are revisioned rather than immutable.

```text
Goal id: goal_123
revision 1
    ↓ user clarification
revision 2
    ↓ requirement change
revision 3
```

The Goal ID remains stable. The contract changes only through explicit revision creation.

Every planner proposal must bind to the Goal revision it observed:

```yaml
goal:
  id: goal_123
  revision: 4
basedOn:
  graphRevision: 28
```

If the current Goal revision has advanced, the proposal is stale and cannot be committed without revalidation/replanning.

## Task provenance and Goal revisions

Every materialized Task records the Goal revision that caused it to exist:

```yaml
provenance:
  goal:
    id: goal_123
    revision: 4
```

When the Goal changes, Pantheon can distinguish work that remains valid from work that must be reviewed or superseded.

## Semantic structure

A Goal contains six semantic areas:

```text
OBJECTIVE
DELIVERABLES
INPUTS
CONSTRAINTS
PREFERENCES
ACCEPTANCE
```

### Objective

The ultimate user-level outcome.

```yaml
objective: >
  Add provider-independent agent authorization to Pantheon
  with deterministic policy enforcement and sandbox-aware
  execution.
```

The objective must not prescribe agent/model/provider or orchestration steps.

### Inputs

Durable references relevant to the Goal:

```yaml
inputs:
  - name: repository
    ref: repo://Pantheon
```

### Deliverables

Top-level outputs that represent the user's final result, not every intermediate Task artifact.

```yaml
deliverables:
  - name: implementation
    kind: code.changeset
    required: true
  - name: architecture
    kind: architecture.document
    required: true
```

Deliverables are slots. The evolving TaskGraph binds accepted Task outputs to them. Replanning may change which Task produces a deliverable without changing the Goal contract.

### Constraints

Requirements that must remain true while pursuing the Goal.

```yaml
constraints:
  - id: provider-independent
    statement: >
      Logical agents must not be bound to one provider or concrete model.
  - id: deterministic-authority
    statement: >
      Scheduling, permissions, Task state and acceptance remain owned
      by the Pantheon control plane.
```

Constraints create ceilings for descendants. They may tighten authority but do not grant permission.

### Preferences

Optimization preferences that may be traded off when necessary.

```yaml
preferences:
  - id: local-first
    statement: >
      Prefer local execution when quality and capability requirements permit it.
```

A preference such as `local-first` is distinct from a hard constraint such as `local-only`.

### Acceptance

Goal acceptance reuses the Task Acceptance/Evidence architecture.

```yaml
acceptance:
  strategy: all
  criteria:
    - id: provider-routing
      statement: >
        A logical coding agent can execute through multiple compatible
        harnesses without changing its identity.
      evaluator:
        ref: check://pantheon/provider-routing
      severity: required
```

Task success never automatically implies Goal success.

## Goal completion candidate

When Pantheon believes the Goal may be satisfied, it freezes an immutable completion snapshot:

```yaml
goalCompletionCandidate:
  goal:
    id: goal_123
    revision: 7
  graph:
    id: graph_123
    revision: 42
  deliverables:
    implementation:
      ref: artifact://changeset-822
      digest: sha256:...
    architecture:
      ref: artifact://doc-192
      digest: sha256:...
  evidence:
    - evidence://...
```

Goal acceptance is evaluated against this exact snapshot. If a deliverable changes, previous acceptance evidence becomes stale.

## Long-running continuity

Pantheon does not rely on one persistent conversation to remember a Goal. Durable state is reconstructed from:

```text
Goal revisions
TaskGraph revisions
Tasks
Artifacts
Evidence
Events
assumptions / unknowns
acceptance state
```

Fresh planners and workers receive a context snapshot built from current Pantheon state.

## No nested Goals in v1

Avoid a `Goal → SubGoal → SubGoal` hierarchy. Work decomposition belongs in the TaskGraph. If a bounded outcome contributes to a Goal, it is normally a Task. If the user wants a separately managed outcome, create a new Goal.

## Spawn behavior

All dynamically spawned descendants remain under the same Goal by default:

```text
Goal G
  └── Task A
       └── Task B
            └── Task C
```

A detached Task is Goal-owned and may outlive the parent Task, but it is not an orphan.

Normal workers may request `task.spawn`; planner agents may propose graph patches. They do not receive `goal.create` authority by default.

## Goal revision impact

A Goal revision does not directly rewrite the TaskGraph. It triggers reconciliation/impact analysis. Existing work is classified conceptually as:

```text
still-valid
needs-review
must-supersede
new-work-needed
```

The planner then proposes a graph patch that preserves historical Tasks and adds/supersedes work as required.

## Same Goal versus new Goal

Use the same Goal when the user is still pursuing essentially the same real-world outcome and is clarifying/changing requirements.

Create a new Goal when the new outcome is independently manageable and semantically separate.

Terminal Goals do not reopen; later additional work becomes a new Goal, optionally linked to the previous one.

## Proposed conceptual manifest

```yaml
apiVersion: pantheon/v1alpha1
kind: Goal

metadata:
  id: goal_01K...
  name: pantheon-agent-orchestrator
  revision: 4
  labels:
    project: pantheon

provenance:
  createdBy:
    type: user
  sources:
    - ref: message://...

spec:
  objective: >
    Build a local-first heterogeneous multi-agent orchestrator
    with a deterministic control plane.

  inputs:
    - name: repository
      ref: repo://Pantheon

  deliverables:
    - name: implementation
      kind: code.changeset
      required: true
    - name: architecture
      kind: architecture.document
      required: true

  constraints:
    - id: provider-independent
      statement: >
        Logical agents must not be bound to one provider or concrete model.

    - id: deterministic-authority
      statement: >
        Scheduling, permissions, Task state and acceptance remain owned
        by the Pantheon control plane.

  preferences:
    - id: local-first
      statement: >
        Prefer local execution when quality and capability requirements permit it.

  acceptance:
    strategy: all
    criteria:
      - id: provider-routing
        statement: >
          A logical coding agent can execute through multiple compatible
          harnesses without changing its identity.
        evaluator:
          ref: check://pantheon/provider-routing
        severity: required
```

## Key decisions

1. Goal is a durable user-outcome contract, not a prompt/session.
2. Goal identity is stable and the contract is explicitly revisioned.
3. Tasks are immutable and bind to the Goal revision that created them.
4. Constraints, preferences and acceptance are separate concepts.
5. Task success never automatically implies Goal success.
6. Goal completion is evaluated against an immutable completion snapshot.
7. Replanning reconciles new Goal revisions rather than rewriting history.
8. No nested Goal hierarchy in v1.
9. Spawned Tasks remain under the same Goal.
10. Tasks cannot create Goals to bypass ceilings or policy.
