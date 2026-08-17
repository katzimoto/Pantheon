# Event and Observability Model

## Status

Canonical Pantheon event, audit, tracing, metrics, and diagnostics specification.

## Purpose

This document defines how Pantheon records durable domain facts, security/audit history, operational traces, metrics, and diagnostic logs without turning telemetry into a second source of mutable control-plane truth.

The central rule is:

> **Pantheon state tables are authoritative current state. The append-only Event Journal records durable historical facts and provides a transactional outbox, but Pantheon is not event-sourced.**

See also:

- `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`
- `docs/architecture/tasks/task-lifecycle.md`
- `docs/architecture/execution/run-and-attempt.md`
- `docs/architecture/persistence-and-recovery/recovery-policy.md`
- `docs/architecture/operations/budget-usage-and-rate-limits.md`
- `docs/architecture/artifacts-and-workspaces/artifact-model.md`
- `docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md`

## Signal model

Pantheon distinguishes five concepts.

```text
DOMAIN EVENT
  durable point-in-time fact about Pantheon state/decisions

AUDIT EVENT
  durable security/control-plane fact requiring audit treatment

TRACE
  duration-bearing operation used for performance/causal diagnostics

METRIC
  aggregate operational measurement

DIAGNOSTIC LOG
  human/debug text that is not authoritative state
```

A Domain Event may also be audit-relevant. Traces, metrics and diagnostic logs may be sampled or dropped according to telemetry policy; durable Domain/Audit Events are not sampled.

## Durable Event Journal

The Event Journal is append-only SQLite state. Events are inserted and never modified in place to rewrite history.

Conceptual columns:

```text
event_id
journal_epoch
sequence
event_type
occurred_at
recorded_at
source
subject
subject_revision
actor
command_id
causation_event_id
correlation_id
trace_id
span_id
payload
```

The Event Journal is not used to reconstruct ordinary controller state by replaying from genesis. Current Goal, Task, Run, Attempt, Reservation, Budget, Workspace and other resource tables remain authoritative for restart/recovery.

## Transactional event rule

Every authoritative SQLite mutation that has durable semantic meaning commits its corresponding Event in the same SQLite transaction.

```text
BEGIN WRITE TRANSACTION

mutate authoritative state
insert corresponding Event row

COMMIT
```

Examples include:

- Goal revision creation;
- Task lifecycle transition;
- SchedulingClaim creation/consumption;
- ExecutionBinding commitment;
- Run/Attempt creation or terminal transition;
- ResourceReservation release;
- Budget debit/hold extension/settlement;
- capability Grant creation/revocation;
- Artifact/Candidate metadata commitment;
- Acceptance verdict;
- RecoveryDecision;
- Workspace/Integration state transition.

No telemetry/export network operation occurs inside an authoritative state transaction.

The durable Event row therefore also acts as the transactional outbox record.

## Export semantics

External export happens after commit and is at-least-once.

```text
SQLite Event Journal
        ↓
Exporter
        ↓
OpenTelemetry / future external sinks
```

If Pantheon crashes after a sink accepts an Event but before the export cursor is persisted, the same Event may be delivered again.

Every Event therefore has an immutable globally unique `eventId`. Delivery retries reuse the same ID. Consumers must tolerate duplicates.

Exporter state is separate mutable state, for example:

```yaml
export:
  sink: otel://default
  journalEpoch: epoch_...
  lastSequence: 48192
```

Event existence and Event delivery are different facts.

## Journal epoch and sequence

Each durable journal history has a random `JournalEpoch`.

Normal daemon restart preserves the epoch.

A disaster recovery operation that restores an older database snapshot and thereby branches historical journal continuity creates a new JournalEpoch before new Events are emitted.

The durable local ordering cursor is:

```text
(journalEpoch, sequence)
```

`sequence` is monotonically increasing within a journal history. It is ordering metadata, not Event identity.

This prevents an older restored database from producing unrelated Events that appear to extend an already-exported sequence range.

## Occurrence time versus record time

Events record both:

```text
occurredAt
  when the occurrence is believed to have happened

recordedAt
  when Pantheon durably recorded the Event
```

For local transactional state changes these may be nearly identical. Reconciled external observations may have an occurrence time derived from external evidence and a later record time.

Timestamps are never used as the sole ordering authority; journal sequence provides local durable record order.

## Canonical Event envelope

Conceptually:

```yaml
apiVersion: pantheon.events/v1
kind: Event

metadata:
  id: evt_01K...
  journal:
    epoch: epoch_...
    sequence: 48192
  type: pantheon.task.phase.changed.v1
  occurredAt: ...
  recordedAt: ...

source:
  component: controller://task
  daemonIncarnation: daemon_...

subject:
  ref: task_123
  revision: 18

actor:
  kind: controller
  ref: controller://scheduler

causality:
  commandId: cmd_891
  causationEventId: evt_...
  correlationId: corr_...

trace:
  traceId: ...
  spanId: ...

data:
  previousPhase: Ready
  phase: Active
  activeRun: run_456
```

Not every optional field is present for every Event.

## Event type registry

Event types are stable, low-cardinality and versioned.

Good:

```text
pantheon.task.phase.changed.v1
pantheon.run.created.v1
pantheon.attempt.observation.changed.v1
pantheon.recovery.decision.created.v1
```

Never embed dynamic IDs into type names.

A registry defines for each type:

- semantic definition;
- payload schema/version;
- canonical producer(s);
- default severity/presentation;
- privacy classification;
- audit classification;
- retention class.

Compatible optional payload additions may remain in the same major Event type version. Incompatible semantic/payload changes create a new versioned type.

## Source, subject and actor

### Source

The Pantheon component that emitted/observed the fact.

### Subject

The primary resource the Event describes. There is normally one primary subject; related resources belong in structured payload fields.

### Actor

The principal whose authorized action caused the occurrence when meaningful.

For example, an Agent may request a Git action while the Git Broker is the source that performs and records it:

```yaml
source:
  component: broker://git

actor:
  kind: agent
  ref: agent://coder
```

Autonomous reconciliation may use the controller as both source and actor. Human approvals identify the human principal as actor.

## Commands versus Events

Commands express intent; Events record occurrences.

```text
COMMAND
Cancel Task 12

EVENT
Task cancellation requested

EVENT
Attempt termination confirmed

EVENT
Task became Cancelled
```

Every mutating external/API command receives a durable idempotency `commandId`. Events caused by that command reference it.

A command ID does not replace Event identity.

## Causation, correlation and tracing

Pantheon distinguishes:

```text
causationEventId
  direct durable historical cause

correlationId
  durable grouping across a wider Pantheon operation/workflow

traceId/spanId
  transient distributed observability context
```

Trace context is never used for authorization, idempotency, lifecycle identity, ownership or fencing.

Pantheon uses standard W3C/OpenTelemetry trace context where supported rather than defining a custom distributed tracing protocol.

Do not create one trace for an entire long-lived Goal. Trace bounded operations such as:

- API command handling;
- Planner invocation;
- Agent resolution;
- scheduler selection/routing;
- Run reconciliation;
- backend inspect/ensure/terminate;
- Artifact sealing;
- Acceptance evaluator execution;
- Integration operation.

Durable Event causality links important history across many short traces.

## Authoritative event production

Workers and backend adapters cannot manufacture authoritative Pantheon lifecycle/audit Events.

Agents invoke canonical APIs such as `task.submit_result`; Pantheon validates the request, commits authoritative state and emits the corresponding Event.

Backend adapters translate private implementation state into normalized observations. Controller-owned state transitions then emit Pantheon Events.

## Authorization data

Security-relevant Events may reference authorization records:

```yaml
data:
  authorization:
    decision: authz_712
    configRevision: cfgrev_43
    authzPolicyDigest: sha256:...
    grant: grant_42
```

`configRevision` identifies the active configuration snapshot used by the authorization decision; `authzPolicyDigest` identifies the exact immutable AuthorizationComponent content within that revision. Pantheon does not use an ambiguous generic `policyHash` for authorization audit evidence.

Never place bearer capability tickets, secret values, credentials or API keys in the Event Journal.

Event/Audit records identify the authorization evidence used; they do not become authorization channels themselves.

## Large and sensitive content

Durable Event payloads are metadata-first and bounded.

Do not inline:

- source trees;
- patches;
- model prompts/completions;
- PTY transcripts;
- full test logs;
- core dumps;
- large diagnostic payloads.

Large durable content is sealed as an Artifact and referenced by ArtifactRef.

Raw model prompts/outputs are not stored in durable Events by default. If explicit transcript capture is enabled, transcript content becomes a restricted Artifact with its own permission and retention policy.

Private model reasoning/chain-of-thought is never an ordinary observability payload.

## Diagnostic logs

Diagnostic logs are non-authoritative operational text.

Examples:

```text
DEBUG Run controller polling backend
WARN backend status call exceeded threshold
ERROR Git worktree inventory command failed
```

Loss of diagnostic logs does not corrupt Pantheon control-plane state.

When a diagnostic condition becomes semantically meaningful, the owning controller changes authoritative status/conditions and emits a durable Event for that state transition.

## No-op reconciliation

Repeated reconciliation that discovers no meaningful state change does not emit a durable Event.

```text
RUNNING → inspect → still RUNNING
```

may produce a span/debug log but no Event Journal row.

Durable Events are emitted for meaningful observations, decisions, state transitions, external-side-effect establishment and security/audit actions.

## Events as wakeups, not authority

Events may wake controllers, schedulers, exporters or learning pipelines.

Every consumer that makes a control decision re-reads current authoritative state before acting.

```text
Event says Task became Ready
        ↓
Scheduler wakes
        ↓
Scheduler re-reads current Task/Goal/Graph/policy
```

Event replay never directly replays external side effects.

## OpenTelemetry export

Pantheon maps durable named Domain Events to OpenTelemetry log-based Events and carries trace/span correlation when available.

Traces and metrics are emitted through normal OpenTelemetry instrumentation.

OpenTelemetry is an external observability representation, not Pantheon state authority.

Exporter failure cannot block core state mutation after the Event Journal transaction commits.

## Metrics

Metrics are operational aggregates and never authoritative accounting/state.

Useful v1 metrics include:

```text
pantheon.tasks                      gauge by phase/type
pantheon.runs                       gauge by phase
pantheon.attempts.total             counter by outcome/failure-origin
pantheon.recovery.decisions.total   counter by action
pantheon.scheduler.dispatch.duration histogram
pantheon.run.reconcile.duration     histogram
pantheon.backend.operation.duration histogram by backend/operation
pantheon.resource.reserved          gauge by resource class
pantheon.budget.remaining           gauge where meaningful
pantheon.uncertain_obligations      gauge
pantheon.event_export.lag           gauge
pantheon.workspace.count            gauge by phase
```

High-cardinality object IDs such as Task/Run/Attempt/Event/Artifact IDs are not default metric labels.

Authoritative token/cost accounting remains in Usage/Charge/Budget records; metrics may only aggregate it.

## Audit durability

Durable Domain/Audit Events are recorded at 100% and are never sampled.

Tracing/debug telemetry may use sampling according to operator policy.

Audit-relevant families include at least:

- authorization decisions relevant to consequential actions;
- Grant creation/revocation;
- human approvals;
- terminal Task/Run lifecycle transitions;
- Budget consumption/overdraw;
- Git integration actions;
- recovery quarantine/fencing decisions;
- policy/Goal revisions with control-plane impact.

## Initial Event families

V1 should reserve namespaces including:

```text
pantheon.goal.*
pantheon.task.*
pantheon.graph.*
pantheon.scheduler.*
pantheon.execution.binding.*
pantheon.run.*
pantheon.attempt.*
pantheon.resource.*
pantheon.budget.*
pantheon.usage.*
pantheon.authorization.*
pantheon.grant.*
pantheon.artifact.*
pantheon.candidate.*
pantheon.acceptance.*
pantheon.recovery.*
pantheon.workspace.*
pantheon.integration.*
pantheon.system.*
```

Not every internal function requires a durable Event.

## Event retention

Event metadata and referenced payload retention are separate.

A historical Event may remain while a referenced Artifact payload is later garbage-collected according to Artifact retention policy. The Event continues to record the exact historical ArtifactRef.

Tamper-evident hash chains/signing are deferred from local v1. If stronger audit trust is later required, prefer signed journal segments, an external immutable audit sink or transparency-log style checkpointing over a purely local unauthenticated hash chain.

## Learning integration

Agent Genome learning consumes structured Events plus Acceptance Evidence, Usage and related immutable records rather than scraping free-form diagnostic logs or trusting worker self-report.

Useful learning dimensions include:

- Task type/competencies;
- selected Logical Agent/Binding;
- Attempt failures;
- Recovery decisions;
- Usage/Charge;
- Candidate/Artifact provenance;
- Acceptance evidence;
- human corrections/approvals.

## V1 invariants

1. Domain Events, Audit Events, Traces, Metrics and Diagnostic Logs are distinct signals.
2. Pantheon uses one append-only durable Event Journal in SQLite.
3. Current resource/state tables remain authoritative; Pantheon is not event-sourced.
4. Authoritative mutations and their durable Events commit in the same SQLite transaction.
5. No network/export operation occurs in an authoritative state transaction.
6. Event Journal doubles as a transactional outbox; export is asynchronous and at-least-once.
7. Event IDs are globally unique and stable across delivery retries.
8. Export cursors/state are mutable records separate from immutable Events.
9. JournalEpoch scopes local sequence ordering and changes when disaster recovery branches history.
10. Sequence is ordering metadata, not identity; timestamps do not replace it.
11. `occurredAt` and `recordedAt` are separate.
12. Event types are stable, low-cardinality and versioned through an Event Type Registry.
13. Agents/backends cannot manufacture authoritative lifecycle/audit Events.
14. Source, subject and actor are distinct fields.
15. Commands are intent; Events are facts; mutating commands carry idempotent command IDs.
16. Causation, correlation and distributed trace context remain distinct.
17. Trace context never carries authority.
18. Durable Event payloads are bounded and metadata-first; large/sensitive bodies become restricted Artifacts.
19. No-op reconciliation does not emit durable Events.
20. Event consumers always re-read authoritative current state before consequential actions.
21. Event replay never directly replays external side effects.
22. Metrics are aggregates, not authority, and high-cardinality object IDs are excluded from default labels.
23. Durable Domain/Audit Events are never sampled.
24. Telemetry sink failure cannot block Pantheon after the durable Event transaction commits.
25. Cryptographic tamper-evident journal mechanisms are deferred from v1 unless an external trust/compliance requirement appears.
