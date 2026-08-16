# Public Daemon API and CLI

## Status

Draft design — Pantheon public control-plane interface specification.

## Purpose

Pantheon needs one public control surface for humans, automation, future UIs, and integrations without creating alternate paths around its controllers, persistence invariants, authorization model, or recovery semantics.

The central rule is:

> **`pantheond` is the only control-plane authority. Every CLI, UI, automation, or future remote client talks to the daemon API; none may open SQLite, mutate workspaces, manipulate Git refs, or call ExecutorBackends directly.**

```text
pantheon CLI
future UI
future integrations
      │
      ▼
 Public Control API
      │
      ▼
   pantheond
      │
 ┌────┼─────────────┐
 ▼    ▼             ▼
SQLite Controllers  Brokers
                    │
                    ▼
              external world
```

See also:

- `docs/architecture/sqlite-persistence-and-transactions.md`
- `docs/architecture/event-and-observability-model.md`
- `docs/architecture/global-recovery-and-crash-reconciliation.md`
- `docs/architecture/permissions-and-capabilities.md`
- `docs/architecture/workspace-and-git-integration.md`
- `docs/architecture/run-and-attempt.md`

## 1. V1 transport

V1 uses HTTP/1.1 + JSON over a Unix-domain stream socket.

Conceptually:

```text
~/.pantheon/run/pantheon.sock
```

The parent directory is private to the local user and the socket is not exposed over TCP by default.

The application protocol is deliberately independent of Unix sockets so that a future authenticated HTTPS listener can expose the same API semantics.

V1 is local-first, single-user, and local-only by default.

## 2. CLI boundary

The `pantheon` CLI is a thin client of the daemon API.

It never:

- opens `pantheon.db`;
- mutates Git worktrees directly;
- updates repository refs directly;
- calls ExecutorBackends directly;
- edits ResourceReservations/BudgetHolds directly;
- bypasses authorization or Recovery Policy.

Even diagnostic commands such as `pantheon doctor` ask `pantheond` to execute the appropriate invariant checker/reconciliation inspection.

This prevents the CLI from becoming a privileged second control plane.

## 3. API versioning

The public API is versioned independently of the Pantheon binary, SQLite schema, Agent manifest, and Event schema.

V1 base path:

```text
/api/v1
```

Public resources may use:

```yaml
apiVersion: pantheon/v1
kind: Task
```

Incompatible public API semantic changes require a new major API namespace such as `/api/v2`.

## 4. OpenAPI contract

Pantheon maintains a machine-readable OpenAPI contract under an API directory such as:

```text
api/
├── openapi.yaml
└── schemas/
```

The OpenAPI document describes HTTP operations, public schemas, Problem Details, query parameters, and response contracts.

The local Unix-socket transport may be described with a Pantheon extension because standard OpenAPI server URLs do not model filesystem sockets directly.

Conceptually:

```yaml
x-pantheon-transport:
  default: unix
  socket: ~/.pantheon/run/pantheon.sock
```

The public API contract remains transport-independent.

## 5. Public API is semantic, not raw CRUD

Pantheon does not expose internal tables or controller-owned lifecycle fields as arbitrary CRUD resources.

Forbidden examples:

```text
PATCH /runs/run_123 { phase: Completed }
DELETE /resource-reservations/res_123
POST /attempts
```

Public mutations represent semantic commands such as:

- create/revise/cancel a Goal;
- cancel a Task;
- adjust a Budget;
- approve/deny an approval request;
- request integration;
- create a bounded recovery override.

Controllers remain authoritative for internal phase transitions, retries, Attempt creation, reservation release, routing, and reconciliation.

## 6. Public resource reads

Representative read endpoints:

```text
GET /api/v1/goals/{id}
GET /api/v1/tasks/{id}
GET /api/v1/runs/{id}
GET /api/v1/attempts/{id}
GET /api/v1/artifacts/{digest}
GET /api/v1/candidates/{digest}
GET /api/v1/agents
GET /api/v1/backends
GET /api/v1/budgets
GET /api/v1/recovery/findings
```

These return normalized architectural resources, not raw SQLite rows or backend-private state.

## 7. Optimistic concurrency with ETag / If-Match

Mutable public resources expose a strong ETag derived from their authoritative revision.

Conceptually:

```http
ETag: "goal_123:7"
```

A mutation based on previously observed state uses `If-Match`:

```http
If-Match: "goal_123:7"
```

If the resource changed since the client read it, the daemon rejects the mutation instead of silently overwriting newer state.

Use this for stale-sensitive operations such as:

- Goal revision;
- Budget limit change;
- dispatch-policy modification;
- IntegrationIntent modification;
- other replacement/update operations derived from observed current state.

Operations whose semantics are intentionally state-independent, such as an idempotent cancel request, need not require `If-Match`.

If a required precondition is omitted, return `428 Precondition Required`. If it is supplied but stale, return `412 Precondition Failed`.

## 8. Durable command identity

Every mutating public operation carries a Pantheon `commandId`.

Conceptually:

```json
{
  "commandId": "cmd_01K...",
  "goal": { }
}
```

The daemon persists the command identity and request hash.

Rules:

```text
new commandId
→ execute command

same commandId + same request hash
→ return the previously recorded outcome

same commandId + different request hash
→ fail closed as command identity misuse/conflict
```

This provides transport-independent idempotency across retries, tests, future transports, and automation.

`commandId` remains separate from HTTP transport metadata.

## 9. Command versus resource

A Command records the external mutation request and its durable outcome.

Conceptually:

```yaml
kind: Command
metadata:
  id: cmd_123
operation: integration.request
status:
  phase: accepted
result:
  ref: integration_456
```

A Command is not another workflow engine. It answers whether a request was accepted/completed/failed and which durable resource or intent it created.

Long-running control-plane work continues under normal controller/reconciliation semantics after the request connection closes.

## 10. HTTP success semantics

Use status codes according to the durable state established when the request returns.

Typical examples:

```text
201 Created
resource was durably created

200 OK / 204 No Content
requested desired-state mutation committed synchronously

202 Accepted
request was durably accepted, but consequential processing continues asynchronously
```

For asynchronous commands, the response may include a `Location` pointing to the Command resource.

## 11. Error model

All structured HTTP errors use RFC 9457 Problem Details:

```text
Content-Type: application/problem+json
```

Conceptually:

```json
{
  "type": "urn:pantheon:problem:stale-revision",
  "title": "Resource revision is stale",
  "status": 412,
  "detail": "Goal goal_123 is now at revision 9.",
  "instance": "urn:pantheon:problem-instance:prb_123",
  "pantheonCode": "STALE_REVISION",
  "currentRevision": 9,
  "commandId": "cmd_..."
}
```

Clients inspect stable type/code/structured fields rather than parsing human `detail` text.

Initial Problem families should include:

```text
not-found
validation
precondition-required
stale-revision
conflict
policy-denied
approval-required
budget-blocked
temporarily-unavailable
subject-fenced
cursor-gone
internal
```

## 12. Health and recovery readiness

Expose distinct health endpoints:

```text
GET /health/live
GET /health/ready
```

`live` means the daemon process/runtime is functioning.

`ready` means the global recovery barrier has passed and Pantheon may safely dispatch new work.

During startup recovery, liveness may be healthy while readiness returns `503 Service Unavailable`.

Read-only diagnostic endpoints may remain available while scheduling is fenced.

## 13. List + Watch consistency

Pantheon needs gap-free transition from a current-state list to subsequent Event streaming.

A list response therefore includes an Event Journal snapshot cursor obtained from the same SQLite read snapshot as the returned resource list.

Conceptually:

```yaml
items:
  - ...
metadata:
  snapshotCursor:
    epoch: journal_abc
    sequence: 48291
  nextCursor: ...
```

A client then watches Events strictly after that cursor.

Because authoritative state and the Event Journal commit in the same SQLite transaction domain, this avoids losing committed events between list and watch establishment.

## 14. Event watch via Server-Sent Events

Representative endpoint:

```text
GET /api/v1/events/watch?after=journal_abc:48291
```

Response:

```text
Content-Type: text/event-stream
```

Example frames:

```text
id: journal_abc:48292
event: pantheon.task.phase.changed.v1
data: {...}

id: journal_abc:48293
event: pantheon.run.created.v1
data: {...}
```

The SSE `id` is the resumable journal cursor (`JournalEpoch:sequence`), while the durable Pantheon Event ID remains inside the Event payload.

Clients may reconnect using `Last-Event-ID` or an explicit `after` cursor.

## 15. Cursor expiration/history branch

If a requested watch cursor is no longer available because history was pruned or the database was restored into another JournalEpoch, return:

```text
410 Gone
```

with a `cursor-gone` Problem response.

The client then relists current state, receives a fresh snapshot cursor, and resumes watch from there.

## 16. Historical Event API

Streaming and historical inspection are separate operations.

```text
GET /api/v1/events/watch
```

provides live/resumable streaming.

```text
GET /api/v1/events?after=...&type=...&subject=...&limit=...
```

provides historical queries.

Higher-level commands such as `pantheon task explain` may query Events, Evidence, FailureRecords, and RecoveryDecisions to construct a structured explanation.

## 17. Pagination

Lists use opaque keyset cursors rather than page numbers/offsets.

Conceptually:

```text
GET /api/v1/tasks?goal=goal_123&phase=Ready&limit=100&cursor=...
```

Response metadata contains an opaque `nextCursor`.

Clients must not parse cursor internals.

## 18. Filtering

V1 exposes explicit typed query parameters for supported filters.

Examples:

```text
?goal=goal_123
?phase=Ready
?type=code.debug
?agent=agent://coder
?since=...
```

Pantheon does not expose a public SQL-like or arbitrary expression query language in v1.

Persistence representation remains private.

## 19. Actor identity and authentication boundary

Actor identity is derived by the daemon from the authenticated connection context.

Clients cannot authoritatively submit:

```yaml
actor: owner
```

For local single-user v1, Unix-socket filesystem ownership/permissions identify the local installation principal.

Diagnostic request metadata such as `User-Agent` is not authority.

Future remote/multi-user transports must establish authenticated principals before being enabled.

## 20. Remote listening

V1 does not expose an unauthenticated TCP listener.

Future remote access requires a separate transport/security design covering at least:

- TLS;
- authentication;
- principal mapping;
- authorization boundaries;
- secret handling;
- remote-rate/abuse controls where relevant.

The application-level API can remain the same.

## 21. Explainability and private reasoning

Public API may expose structured architecture facts such as:

- Task/Run/Attempt state;
- Artifacts/Candidates/Evidence;
- RecoveryDecision;
- routing reasons;
- Agent eligibility reasons;
- FailureRecords;
- Event history.

It does not expose:

- hidden chain-of-thought;
- private model reasoning scratchpads;
- secrets/credentials;
- bearer capability tickets;
- provider-private opaque session state.

`pantheon explain` derives explanations from durable structured facts and decisions.

## 22. CLI shape

The CLI is resource-oriented and intentionally excludes internal controller-owned creation/mutation operations.

Representative commands:

```text
pantheon status

pantheon goal create
pantheon goal get
pantheon goal list
pantheon goal revise
pantheon goal cancel

pantheon task get
pantheon task list
pantheon task cancel
pantheon task explain

pantheon run get
pantheon run list
pantheon attempt get

pantheon agent list
pantheon agent get

pantheon budget list
pantheon budget get
pantheon budget adjust

pantheon approval list
pantheon approval approve
pantheon approval deny

pantheon integration list
pantheon integration apply
pantheon integration cancel

pantheon events
pantheon events watch

pantheon doctor
pantheon version
```

Do not expose direct commands such as:

```text
pantheon run create
pantheon attempt create
pantheon task set-phase
pantheon reservation delete
```

## 23. Recovery override UX

An ergonomic command such as:

```text
pantheon task retry task_123
```

must not directly create a Run or Attempt.

It maps to an explicit recovery-override resource/command, for example:

```text
POST /api/v1/tasks/task_123/recovery-overrides
```

Recovery Policy remains authoritative about what action is now allowed and the Scheduler remains authoritative for creating a new Run.

## 24. `--wait`

CLI `--wait` is implemented client-side.

Example:

```text
pantheon goal create goal.yaml --wait
```

The Goal is created through the normal API command. The CLI then watches Events/current state until the requested completion condition occurs.

Disconnecting the CLI does not cancel or alter the Goal.

No server transaction or controller lifetime is tied to the client connection.

## 25. Output modes

Read-oriented CLI commands support stable output modes such as:

```text
-o table
-o json
-o yaml
```

Human-readable table/summary output may be the interactive default.

Machine-readable stdout must remain free from progress text or decoration. Progress/debug output goes to stderr where appropriate.

## 26. CLI exit-code contract

V1 can use a small stable shell classification:

```text
0 success
1 unexpected/internal failure
2 CLI usage/input error
3 resource not found
4 conflict/stale precondition
5 denied/approval required
6 temporarily unavailable/fenced
7 requested Pantheon operation completed unsuccessfully
```

API clients should prefer structured Problem Details over shell exit-code semantics.

## 27. System discovery

Expose a system discovery endpoint such as:

```text
GET /api/v1/system
```

Conceptual response:

```yaml
daemonVersion: ...
supportedApiVersions:
  - pantheon/v1

database:
  formatVersion: ...

installation:
  id: ...

recovery:
  ready: true

journal:
  epoch: ...
  latestSequence: ...
```

Do not expose secrets, capability tokens, or backend-private configuration.

## 28. Architectural resource vocabulary

The public API uses Pantheon architectural concepts such as:

```text
Goal
Task
TaskGraph
Run
Attempt
Artifact
Candidate
Evidence
Agent
Backend
Budget
Grant
Approval
RecoveryFinding
RecoveryDecision
Workspace
IntegrationIntent
Event
Command
```

Database implementation concepts such as `run_status`, `budget_consumptions`, or exporter cursors remain private.

## 29. Compatibility policy

Within `/api/v1`, compatible additive evolution may include:

- new optional response fields;
- new endpoints;
- new optional query parameters;
- new Event types;
- new values in documented extensible string namespaces.

V1 must not silently:

- change an existing field's meaning;
- remove required fields;
- change identifier semantics;
- broaden an operation's authority.

Incompatible semantic change requires a new API version.

Clients should tolerate unknown optional fields where documented schemas permit them.

## 30. Conformance testing

V1 implementation should include integration tests for at least:

- OpenAPI request/response conformance;
- Problem type/code stability;
- ETag and `If-Match` behavior;
- 428/412 precondition behavior;
- `commandId` retry/idempotency semantics;
- same `commandId` with different request rejection;
- list/watch gap freedom;
- SSE reconnect/resume;
- 410 cursor recovery;
- CLI/API compatibility;
- recovery-barrier readiness behavior.

## V1 non-goals

Defer:

- public unauthenticated TCP listening;
- multi-user identity/role system;
- arbitrary public SQL/filter expressions;
- websocket-specific control protocol;
- gRPC dependency for the public API;
- direct DB access by clients;
- direct backend/Git/workspace access by clients;
- generic lifecycle CRUD;
- hidden chain-of-thought APIs.

## Key decisions

1. **`pantheond` is the sole control-plane authority; clients never access SQLite or external execution systems directly.**
2. **V1 is HTTP/1.1 + JSON over a local Unix-domain socket.**
3. **Remote/TCP exposure is deferred until transport authentication/security are designed.**
4. **The CLI is a thin client of exactly the same public API.**
5. **The public API is independently versioned under `/api/v1`.**
6. **OpenAPI describes the public HTTP contract.**
7. **Public mutations are semantic commands, not raw CRUD over internal lifecycle state.**
8. **Controller-owned internal resources cannot be arbitrarily created/deleted through generic endpoints.**
9. **Mutable resources expose strong ETags based on authoritative revision.**
10. **Stale-sensitive mutations use `If-Match`; missing required preconditions return 428 and stale preconditions return 412.**
11. **Every mutation uses Pantheon's durable `commandId` and request hash for retry idempotency.**
12. **HTTP success codes reflect whether durable work is complete or merely accepted for asynchronous processing.**
13. **Errors use RFC 9457 Problem Details with stable Pantheon Problem types/codes.**
14. **Liveness and recovery/scheduling readiness are distinct health signals.**
15. **List responses include an Event Journal snapshot cursor from the same SQLite snapshot.**
16. **List + Event watch is gap-free by watching strictly after that snapshot cursor.**
17. **Event streaming uses SSE with `JournalEpoch:sequence` as the resumable stream ID.**
18. **Unavailable/pruned/pre-restore cursors return 410; clients relist and resume.**
19. **Lists use opaque keyset cursors and explicit typed filters.**
20. **Actor identity is server-derived, never self-declared by request payload.**
21. **Structured explainability uses Events/Evidence/decisions, not hidden reasoning.**
22. **CLI `--wait` is a read/watch behavior, not a different server mutation path.**
23. **Machine-readable CLI output is stable and kept cleanly separate from progress/debug output.**
24. **Public resources use architectural vocabulary; persistence tables remain implementation-private.**
25. **API v1 evolves additively; incompatible semantics require v2.**
26. **OpenAPI conformance, idempotency, optimistic concurrency and list/watch behavior are first-class integration tests.**
