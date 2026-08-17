# Public Daemon API and CLI

## Status

Canonical Pantheon operator-facing API/CLI specification.

## Purpose

> **`pantheond` is the only control-plane authority. CLI/UI/automation never open SQLite, mutate Workspaces/Git refs, provision Sandboxes, read secret material or call ExecutorBackends directly.**

This document defines the **Operator Control Surface**. Worker/Agent traffic uses the separate restricted Attempt-bound surface in `docs/architecture/execution/agent-control-channel.md`.

## Trust boundary

```text
human / CLI / UI / trusted automation
        ↓
Operator Control Surface
        ↓
pantheond controllers/persistence/brokers

untrusted Agent workload
        ↓
Agent Control Surface
        ↓
Attempt-scoped verbs only
```

The Operator socket is never exposed inside an untrusted Sandbox. Same-UID ownership alone is not an Agent/operator identity boundary.

## V1 transport

Operator Control v1 uses HTTP/1.1 + JSON over a local Unix-domain stream socket, conceptually:

```text
~/.pantheon/run/pantheon.sock
```

with restrictive parent-directory/socket permissions. V1 is single-user/local-only; TCP/remote listening is disabled by default until authenticated remote transport is deliberately designed.

The logical HTTP contract is transport-independent and may later be exposed through authenticated HTTPS without changing core resource semantics.

## API versioning

Base path:

```text
/api/v1
```

Public API version is distinct from Pantheon binary version, SQLite schema version, Event type version and Agent/config schema versions.

Compatible v1 evolution may add optional fields/endpoints/query parameters/extensible values. Meaning changes/removals require a new API version.

## OpenAPI

Pantheon maintains a machine-readable OpenAPI contract for the Operator HTTP surface. Transport-specific Unix-socket metadata may use a Pantheon extension; resource/HTTP semantics remain standard.

Generated clients/docs never become authority over the handwritten architecture/controller semantics.

## Semantic resources, not database CRUD

Public reads expose architectural resources such as:

```text
Goal
Task / TaskGraph
Run / Attempt
Artifact / Candidate / Evidence
Agent / Backend
Workspace / Sandbox
ResourceDescriptor / Reservation
Budget / Usage summary
Grant / Approval
Evaluation
RecoveryFinding / RecoveryDecision
ConfigurationRevision
IntegrationIntent
Event / Command
```

The API does not expose raw tables such as `run_status` or permit generic lifecycle field mutation.

Mutations are semantic commands, for example:

```text
revise/cancel Goal
cancel/retry Task
approve/deny Approval
adjust Budget configuration
apply/rollback configuration
request/cancel Integration
force-resolve exact UNKNOWN obligation
pause/resume dispatch
```

There is no generic `POST /runs`, `POST /attempts`, `PATCH phase`, or `DELETE reservation` escape hatch.

## Read endpoints

Representative v1 reads:

```text
GET /api/v1/system
GET /api/v1/health/live
GET /api/v1/health/ready

GET /api/v1/goals[/...]
GET /api/v1/tasks[/...]
GET /api/v1/runs[/...]
GET /api/v1/attempts[/...]

GET /api/v1/agents[/...]
GET /api/v1/backends[/...]

GET /api/v1/resources
GET /api/v1/reservations
GET /api/v1/budgets[/...]

GET /api/v1/workspaces[/...]
GET /api/v1/sandboxes[/...]

GET /api/v1/artifacts/{digest}
GET /api/v1/candidates/{digest}
GET /api/v1/evidence[/...]
GET /api/v1/evaluations[/...]

GET /api/v1/approvals[/...]
GET /api/v1/grants[/...]

GET /api/v1/recovery/findings[/...]
GET /api/v1/recovery/decisions[/...]

GET /api/v1/configuration
GET /api/v1/configuration/history

GET /api/v1/integrations[/...]
GET /api/v1/events
```

Responses are normalized public representations, not row dumps or backend-private attachments.

## Required operator mutation surface

### Goal

```text
POST /api/v1/goals
POST /api/v1/goals/{id}/revisions
POST /api/v1/goals/{id}/actions/cancel
```

### Task/recovery

```text
POST /api/v1/tasks/{id}/actions/cancel
POST /api/v1/tasks/{id}/recovery-overrides
```

CLI sugar `pantheon task retry` creates an explicit permitted recovery override/context; it does not directly create a Run.

### Approval/Grant

```text
POST /api/v1/approvals/{id}/actions/approve
POST /api/v1/approvals/{id}/actions/deny
```

Only Operator Control may perform these.

### Dispatch control

```text
GET  /api/v1/dispatch
POST /api/v1/dispatch/actions/pause
POST /api/v1/dispatch/actions/resume
```

Dispatch pause/resume mutates durable `scheduler_state.dispatch_mode` through the normal command/idempotency and revision-CAS path.

Conceptually `GET /api/v1/dispatch` exposes the distinction between durable desired state and current effective ability to commit new Runs:

```yaml
dispatch:
  desiredMode: RUNNING | PAUSED
  revision: ...
  effectiveCanDispatch: true | false
  blockedBy:
    - operator-pause
    - recovery
    - configuration
    - maintenance
```

`blockedBy` is a normalized set of current factual/controller gates, not a second desired-state field. `desiredMode=RUNNING` can therefore coexist with `effectiveCanDispatch=false` during startup recovery, while `desiredMode=PAUSED` remains paused even after recovery/configuration gates become healthy.

Pause fences **new Scheduler T3 Run-intent commits**. It does not cancel or stop already-committed Runs/Attempts, revoke existing execution authority, release resources, or pretend existing external work stopped.

Ordinary daemon restart preserves the durable desired mode. Pantheon never silently resumes operator-paused dispatch merely because process-local scheduler state was rebuilt.

Pause/resume responses report the durably committed desired mode; they do not promise that the recovery/configuration gates are currently open. Effective dispatchability is observed through `GET /api/v1/dispatch`/readiness state.

### Configuration

```text
POST /api/v1/configuration/actions/validate
POST /api/v1/configuration/actions/diff
POST /api/v1/configuration/actions/apply
POST /api/v1/configuration/actions/rollback
```

Configuration activation follows immutable ConfigurationRevision/publication-barrier semantics.

### Integration

```text
POST /api/v1/integrations
POST /api/v1/integrations/{id}/actions/cancel
```

Integration remains separately authorized and never happens merely because a Task/Goal succeeded.

### UNKNOWN force-resolution

```text
POST /api/v1/recovery/findings/{id}/actions/force-resolve
```

or an equivalent exact-obligation endpoint.

Request must include expected revision plus explicit reason/risk acknowledgement and may specify the intended administrative disposition where multiple safe accounting/resource choices exist.

Force resolution creates durable lineage tombstone/audit state. It never fabricates factual usage and cannot be invoked by Agent Control.

## Commands, restore epochs, and idempotency

Every Operator mutation carries a durable Pantheon command identity composed of:

```text
commandEpoch = current RestoreGeneration
commandId
```

The client obtains the current `commandEpoch` from `GET /api/v1/system` and binds it to the command before submission. A normal daemon restart does not rotate it. Disaster restore does.

Within one command epoch:

```text
new commandId
  -> process

same commandEpoch + same commandId + same non-sensitive request hash
  -> return/reconcile prior outcome

same commandEpoch + same commandId + different hash
  -> fail closed conflict
```

A request carrying an old `commandEpoch` fails closed as `stale-command-epoch`, even when the restored database no longer contains the historical `commands` row. Pantheon never treats row absence after restore as proof that the command did not previously execute.

The caller must then treat the pre-restore command outcome as `UNKNOWN`, inspect current resource/external state, and intentionally decide whether another mutation is required. Any new mutation uses the current command epoch and a new command ID.

This restore boundary is deliberately stronger than transport retry idempotency: it prevents a restored snapshot from silently converting a previously consumed command identity into fresh external-effect authority.

Sensitive secret-set mutations remain a deliberate exception only to request hashing: secret bytes are never part of durable request hashes/logs. Their command identity is still bound to the RestoreGeneration, command ID remains single-use, and the durable SecretMutationIntent contains only non-secret metadata/version identity.

## ETag / optimistic concurrency

Mutable resources expose strong opaque ETags derived from authoritative identity+revision.

Stale-sensitive mutations use `If-Match`. Missing mandatory precondition returns `428 Precondition Required`; revision mismatch returns `412 Precondition Failed`.

Require preconditions where a command is based on observed mutable state, for example Goal revision, configuration apply/rollback, budget/config changes, dispatch desired-state changes and exact RecoveryFinding force-resolution.

State-independent idempotent commands such as cancel-if-nonterminal need not require a client-supplied prior revision unless controller semantics need it.

## HTTP completion semantics

Use HTTP status according to what has durably happened:

```text
201  resource created synchronously
200/204 durable semantic mutation completed
202  durable command/intent accepted but external/controller processing continues
```

`202` response points at a Command/intent resource where clients can observe completion.

## Problem Details

Structured errors use `application/problem+json` and stable Pantheon problem types/codes. Clients must not parse human detail text.

Initial vocabulary includes:

```text
not-found
validation
precondition-required
stale-revision
stale-command-epoch
conflict
stale-authority
policy-denied
approval-required
budget-blocked
temporarily-unavailable
subject-fenced
cursor-gone
recovery-force-resolution-required
internal
```

Agent Control defines additional restricted worker-operation problems such as `task-not-active`, `run-not-current`, `candidate-submission-conflict` and request-id conflict.

Cancellation/supersession that committed before Candidate submission therefore yields a deterministic stale-authority/conflict response rather than ambiguous Candidate creation.

## Health/readiness

```text
GET /health/live
  daemon/runtime process functioning

GET /health/ready
  recovery barrier passed + active configuration published + control-plane safe for new authority-bearing work
```

An operator-paused scheduler does not necessarily make the daemon unhealthy/unready: readiness reports whether the control plane **could safely** dispatch if desired, while `GET /api/v1/dispatch` reports whether dispatch is currently desired/effective. Deployments may layer stricter product-specific readiness semantics outside this architecture, but they must not erase the durable pause distinction.

During startup recovery, live may be 200 while ready is 503. Read-only diagnostics may remain available while new dispatch is fenced.

## System discovery

`GET /api/v1/system` exposes non-sensitive compatibility/control-plane metadata such as:

```text
daemonVersion
supported API versions
DB format version
installation ID
active ConfigurationRevision
recovery/ready status
RestoreGeneration / commandEpoch
JournalEpoch/latest sequence
```

`RestoreGeneration` is an authority/idempotency continuity boundary and is independent of `JournalEpoch`, which represents Event Journal continuity. Both may rotate during disaster restore for different reasons.

No secrets/backend-private session state.

## Listing and pagination

Lists use explicit typed filters and opaque keyset cursors, not arbitrary public SQL/query expressions or page-number offsets.

Examples:

```text
?goal=...
?phase=...
?backend=...
?state=...
?limit=...
?cursor=...
```

## Gap-free list + Event watch

A state-list response that supports watching includes an Event Journal `snapshotCursor` obtained from the same SQLite read snapshot:

```text
(epoch, sequence)
```

Client starts Event watch **after** that cursor, preventing a list/watch gap.

## Event streaming

Server-to-client durable Event streaming uses SSE:

```text
GET /api/v1/events/watch?after=<epoch:sequence>
Content-Type: text/event-stream
```

SSE `id` is the resumable Journal cursor; Event ID remains inside payload.

If requested history is pruned/unavailable or belongs to an unreachable pre-restore epoch, return `410 Gone` / `cursor-gone`; client relists current state and resumes from a fresh snapshot cursor.

## Operator principal

V1 local principal is derived from the trusted Operator Control connection/installation context. Request bodies cannot self-declare an authoritative actor/role.

`User-Agent` and similar metadata are diagnostic only.

## CLI

CLI is a thin API client. It never opens SQLite, edits Workspace/Git authority, talks to backends/runtime sockets or reads SecretProvider material directly.

For every mutation the CLI obtains/caches the current `commandEpoch` from the daemon and submits it with a newly generated command ID. If the daemon rejects an old epoch after restore, the CLI reports that prior command outcome is unknown and requires fresh state observation before issuing a replacement mutation; it does not automatically mint a new ID and replay the command.

Representative commands:

```text
pantheon status
pantheon daemon ...

pantheon goal create|get|list|revise|cancel
pantheon task get|list|explain|retry|cancel
pantheon run get|list
pantheon attempt get

pantheon agent list|get
pantheon backend list|get

pantheon resource list
pantheon reservation list
pantheon budget list|get

pantheon workspace list|get
pantheon sandbox list|get

pantheon approval list|approve|deny

pantheon recovery list|show|force-resolve

pantheon config status|validate|diff|apply|history|rollback

pantheon integration list|apply|cancel

pantheon artifact inspect|export
pantheon events
pantheon events watch
pantheon doctor
pantheon version
```

Dispatch CLI sugar should expose the durable desired state explicitly, for example `pantheon dispatch status|pause|resume`; it is an Operator API client over the endpoints above, not a process-local scheduler switch.

No commands directly create Runs/Attempts, set lifecycle phases, delete Reservations or mutate shared Git refs outside semantic controllers.

## `--wait`

`--wait` is client-side observation. CLI sends the normal durable command, then watches Events/resource state until requested terminal condition. Disconnecting CLI does not cancel/alter the durable operation.

## Machine-readable output

Read commands support stable human output plus `-o json`/`-o yaml` where applicable. JSON/YAML stdout contains only structured result; interactive progress/diagnostics go to stderr.

## Doctor

`pantheon doctor` calls daemon diagnostic APIs such as PersistenceInvariantChecker, configuration/source drift, recovery findings, backend/Sandbox health and storage consistency. It does not inspect/mutate `pantheon.db` directly.

## Security

- Operator socket is physically absent/unreachable from untrusted Agent Sandboxes.
- Agent Control route set cannot invoke approvals, configuration, budgets, force-resolution, dispatch control or unrelated resources.
- Raw secrets/tickets/Agent credentials are never returned by generic API reads.
- Artifact refs remain identifiers, not capabilities; read/export is authorized.
- Future remote transport requires explicit authentication/TLS/principal mapping before enablement.

## Core invariants

1. `pantheond` is the only control-plane authority.
2. Operator Control and Agent Control are distinct trust surfaces/principals/route sets.
3. Public API exposes semantic architecture resources/commands, not raw lifecycle/table CRUD.
4. Every normal mutation is idempotent only within the current `(RestoreGeneration, commandId)` identity; old command epochs fail closed after disaster restore even if historical command rows were rewound away. Sensitive secret commands never persist secret bytes in hashes/logs.
5. ETags/If-Match map client optimistic concurrency to controller revision CAS.
6. Dispatch pause/resume mutates durable desired state; `GET /dispatch` distinguishes desired mode from effective dispatchability and ordinary restart never silently resumes a pause.
7. Operator surface includes dispatch, resource/reservation, Workspace/Sandbox, backend, recovery-quarantine/force-resolution and configuration operations required to operate the system.
8. UNKNOWN force-resolution is exact, revision-bound, audited and operator-only.
9. List + Event watch is gap-free through same-snapshot Journal cursor.
10. CLI is only an API client; `--wait` watches durable state/events.
11. Remote/TCP access is deferred until a real authentication/security model exists.
