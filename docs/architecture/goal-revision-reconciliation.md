# Goal Revision and Reconciliation

## Status

Draft design — Pantheon goal reconciliation subsystem specification.

## Purpose

Changing a Goal changes desired state. It does not directly mutate running work.

Pantheon must preserve historical truth while reconciling Tasks, TaskGraph state, evidence, authorization, and active Runs toward the latest Goal revision.

## Foundational principles

1. **Goal revisions are append-only immutable revisions behind a stable Goal ID.**
2. **Goal revision is a controller command, not direct mutation.**
3. **Revision changes use optimistic concurrency and idempotency.**
4. **An impactful Goal revision establishes a revision fence so stale Ready work is not newly dispatched.**
5. **Harder constraints take effect immediately for future authorization decisions.**
6. **Existing work is classified and reconciled rather than globally discarded.**
7. **Terminal Task history never changes.** A Succeeded Task remains Succeeded even if its contribution becomes obsolete for a later Goal revision.
8. **Task evidence remains valid for the immutable Task; Goal-level evidence is revision-bound.**
9. **Replanning produces GraphPatch proposals rather than rewriting historical Task specs.**
10. **Desired reconciliation state is persisted before external Runs/processes are converged toward it.**

## Revision storage

Do not overwrite the prior Goal spec in place.

```text
Goal
  id: goal_123
  currentRevision: 8

GoalRevision
  r1
  r2
  ...
  r8
```

Each GoalRevision is immutable and links to its predecessor.

Conceptual command:

```yaml
command: goal.revise
commandId: cmd_01K...
goal: goal_123
expectedRevision: 7
changes:
  constraints:
    add:
      - id: local-only
        statement: >
          Project data must not leave the local machine.
source:
  ref: message://...
```

Validation occurs before revision materialization. If `expectedRevision` does not match the current revision, the update is rejected and must be reconsidered against the newer Goal.

## Revision fence

After an impactful revision is committed, Pantheon records that desired Goal state is newer than reconciled execution state.

```yaml
status:
  currentRevision: 8
  observedRevision: 7
  conditions:
    - type: Reconciled
      status: "False"
      reason: GoalRevisionChanged
```

Until reconciliation catches up, stale Ready Tasks from older revisions are not newly dispatched when their continued validity has not yet been established.

This fence does not imply indiscriminate cancellation of already-running work.

## Immediate security effect

A Goal revision that tightens security constraints affects future authorization immediately.

Example:

```text
Goal r7: network access allowed
Goal r8: local-only
```

The authorization controller must prevent conflicting future actions, revoke conflicting temporary grants/capability tickets where applicable, and mark affected Runs for reconciliation.

An old Run Manifest remains immutable evidence of how a Run started, but it does not freeze authority forever. Every privileged action remains subject to current enclosing policy at the Pantheon execution boundary.

## Impact classification

Existing work is classified against the new Goal revision:

```text
STILL_VALID
REVALIDATE
SUPERSEDE
NEW_WORK
```

### STILL_VALID

The Task remains useful exactly as defined. It may continue or remain schedulable.

### REVALIDATE

The Task or its output may remain useful, but its contribution to the new Goal must be reassessed.

### SUPERSEDE

The non-terminal Task is no longer the work Pantheon wants. Its immutable spec is preserved and replacement work is created.

### NEW_WORK

The new Goal revision requires work that is not yet represented in the TaskGraph.

## Phase-specific behavior

### Pending / Ready

- STILL_VALID: remain Pending/Ready.
- SUPERSEDE: transition through Finalizing to Superseded.

### Active

- STILL_VALID: continue.
- SUPERSEDE: request controlled Run cancellation, then terminalize the Task as Superseded and materialize replacement work.

### Waiting

Re-evaluate wait/join relationships. Obsolete children or dependencies must not leave a parent permanently waiting on irrelevant work.

### Evaluating

If the Task contract is still relevant, evaluation may finish. If the non-terminal Task is obsolete, it may be superseded while its produced Artifacts remain durable history.

### Terminal Tasks

Terminal Task state never changes.

A Task that previously reached `Succeeded` remains historically successful. What changes is its compatibility/contribution to the current Goal revision.

## Goal compatibility condition

Task provenance records the Goal revision under which the Task was created:

```yaml
provenance:
  createdUnderGoalRevision: 4
```

Current status may separately record whether that immutable Task remains compatible with the latest Goal:

```yaml
conditions:
  - type: GoalCompatible
    status: "True"
    observedGoalRevision: 9
    reason: ReconciledStillValid
```

This preserves both historical origin and current relevance.

## Evidence semantics

Task evidence is bound to the immutable Task/candidate and remains valid as evidence for that Task.

Goal-level evidence is bound to a specific Goal revision and completion snapshot. A Goal revision may therefore make previous Goal evidence stale without invalidating Task evidence.

```text
Task Evidence
artifact ABC satisfies immutable Task A
→ remains historically valid

Goal Evidence
Goal r7 satisfied by ABC + XYZ
→ stale after Goal r8 when relevant requirements changed
```

## Deterministic impact discovery

Changes to inputs, deliverables, constraints, or bindings can be used to discover structurally impacted Tasks through the TaskGraph/dataflow graph before semantic reasoning is invoked.

For example:

```text
changed Goal input
    ↓
Task A
    ↓
Task B
    ↓
Task C
```

The transitive consumers are candidates for REVALIDATE/SUPERSEDE. An LLM may help with semantic impact, but it is not needed to discover graph dependency structure.

## Preferences versus constraints

Preference changes normally affect future routing/scheduling decisions without cancelling useful existing Runs.

Hard constraint changes may immediately invalidate future actions and require active Run reconciliation.

This distinction is a primary reason Goal preferences and Goal constraints are separate concepts.

## Acceptance changes

Adding Goal acceptance requirements does not automatically invalidate implementation Tasks.

Example:

```text
r7 acceptance: tests pass
r8 acceptance: tests pass + security review
```

Existing implementation work may remain valid while the planner adds new validation/review Tasks and new Goal evidence is required.

## Scope expansion and reduction

### Expansion

Preserve accepted/useful existing work and add only newly required work.

### Reduction

- Pending/Ready obsolete Tasks: supersede.
- Active obsolete Tasks: cancel Run and supersede Task.
- Succeeded obsolete Tasks: remain Succeeded historically; mark their contribution obsolete for the new Goal.
- Shared infrastructure Tasks: preserve when still useful.

## Durable reconciliation record

Pantheon should persist reconciliation decisions for auditability, even if this is an internal controller record rather than a user-authored resource in v1.

Conceptual shape:

```yaml
GoalReconciliation:
  id: reconcile_01K...
  goal:
    id: goal_123
    fromRevision: 7
    toRevision: 8
  basedOn:
    graphRevision: 42
  taskImpact:
    task_100:
      action: keep
    task_101:
      action: supersede
      reason: violates-local-only-constraint
    task_102:
      action: revalidate
  graphPatch:
    ref: patch_728
```

This makes future inspection possible:

```text
r7 → r8
Added local-only constraint
7 Tasks kept
2 Tasks superseded
1 Run cancelled
3 Tasks added
```

## Controller boundary

Reconciliation flow:

```text
Goal revision
    ↓
Deterministic impact discovery
    ↓
Semantic planner/reviewer where needed
    ↓
GraphPatch proposal
    ↓
Validator
    ↓
Controller commit
```

The LLM planner may propose the adaptation. It cannot directly cancel Runs, supersede Tasks, or mutate graph state.

## Persist desired state before external convergence

Database/graph changes and desired Task terminalization should be committed first.

External actions such as terminating Claude Code, OpenCode, containers, or remote sessions are then reconciled toward that desired state.

This allows restart-safe convergence even when external systems cannot participate in a single transaction with SQLite.

## Rollover instead of stop-the-world

Goal reconciliation should reuse valid work and replace only obsolete work.

```text
old plan
 ├─ useful Task A ─────────────► keep
 ├─ obsolete Task B ──X
 └─ running Task C ─────X

new Goal
 ├─ reuse A
 ├─ replace B with D
 └─ replace C with E
```

Do not cancel and recreate an entire Goal/graph unless the Goal itself is fundamentally replaced.

## Crash recovery

`currentRevision` and `observedRevision` make reconciliation restart-safe.

```text
currentGoalRevision = 8
observedGoalRevision = 7
```

means reconciliation is incomplete. On restart, Pantheon resumes/recomputes reconciliation until:

```text
observedGoalRevision = 8
Reconciled = True
```

No in-memory-only reconciliation state is authoritative.

## End-to-end flow

```text
USER
 │
 │ goal.revise
 ▼
Goal r7 → r8
 │
 ▼
Revision Fence
 │
 ├─ stale dispatch paused
 └─ tightened constraints immediately enforced
 │
 ▼
Impact Analyzer
 │
 ├─ Goal diff
 ├─ dataflow/dependencies
 ├─ Task provenance
 ├─ active Runs
 └─ evidence
 │
 ▼
CLASSIFY
 │
 ├─ STILL_VALID
 ├─ REVALIDATE
 ├─ SUPERSEDE
 └─ NEW_WORK
 │
 ▼
Planner (if semantic adaptation required)
 │
 ▼
GraphPatch Proposal
 │
 ▼
Validator
 │
 ▼
Controller Commit
 │
 ├─ TaskGraph rN → rN+1
 └─ desired Run/Task changes persisted
 │
 ▼
External reconciliation
 │
 ▼
Goal observedRevision = currentRevision
Reconciled = True
```

## Key decisions

1. Goal revisions are immutable append-only revisions behind a stable Goal ID.
2. `goal.revise` uses idempotency and expected-revision concurrency control.
3. Impactful revisions establish a dispatch fence until new desired state is reconciled.
4. Tightened hard constraints affect authorization immediately.
5. Reconciliation reuses valid work rather than rebuilding the graph from scratch.
6. Non-terminal obsolete Tasks may be Superseded; terminal Task history never changes.
7. Succeeded Tasks may become obsolete contributors without ceasing to be Succeeded.
8. Task evidence remains Task-valid; Goal evidence is revision-bound.
9. Replanning uses GraphPatch proposals and preserves immutable history.
10. Persisted desired state drives crash-safe reconciliation of external executors.
