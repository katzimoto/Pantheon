# SQLite Persistence and Transaction Boundaries

## Status

Canonical Pantheon persistence specification.

## Purpose

Pantheon maps control-plane safety invariants onto one crash-safe relational SQLite database.

> **Anything participating in a Pantheon safety invariant is represented relationally. Canonical JSON stores immutable structured documents/non-query-critical detail; it does not replace foreign keys, uniqueness, revisions, lifecycle, ownership or accounting columns.**

## Physical database

V1 uses one authoritative local SQLite file, conceptually:

```text
~/.pantheon/state/pantheon.db
```

alongside separate Artifact CAS and Workspace storage.

One database is intentional: Task/Run/Reservation/Budget/Event/authorization state frequently changes atomically. Splitting these across attached WAL databases would weaken crash-atomic cross-domain commits.

`system_state` includes the current installation `restore_generation`: a fresh unpredictable value that persists across normal daemon restart and is rotated only at the disaster-restore authority fence. It is distinct from daemon incarnation, Run ownership epoch/lease token, and JournalEpoch.

## SQLite operating rules

V1 locks:

- WAL mode;
- `synchronous=FULL`;
- `foreign_keys=ON`;
- `trusted_schema=OFF`;
- configured bounded `busy_timeout`;
- no shared-cache mode;
- one serialized authoritative writer connection + small bounded read pool;
- `BEGIN IMMEDIATE` for state-dependent authoritative write transactions;
- STRICT tables for authoritative state;
- integral base units for accounting/resource quantities/timestamps;
- opaque TEXT resource IDs in v1;
- 32-byte SHA-256 digests as BLOBs with length constraints where digests are relational fields;
- validated canonical JSON TEXT for hash-bearing structured documents; SQLite JSONB is not canonical identity;
- immediate foreign keys by default; deferred constraints only for demonstrated cyclic transactions;
- no business-logic triggers in v1: controller transactions own lifecycle/accounting/event semantics.

Pantheon bundles/tests against a vetted SQLite version containing required WAL safety fixes rather than trusting an arbitrary host library.

## Immutable documents versus mutable status

Where architecturally meaningful, immutable identity/specification and mutable lifecycle are physically separated.

Examples:

```text
goal_revisions        immutable
TaskSpec              immutable
ExecutionBinding      immutable
Run spec/snapshot     immutable
Attempt identity      immutable
Candidate             immutable
Evidence              immutable
ConfigurationRevision immutable
EvaluatorVersion      immutable
ContextPlan           immutable

Goal/Task/Run/Attempt/Sandbox/etc. status rows
                      revisioned mutable controller state
```

Mutable authoritative rows carry an integer revision. Updates use:

```sql
... WHERE id = ? AND revision = ?
```

and increment the revision; exactly one affected row is required.

## Table families

Conceptual families after the architecture review:

```text
SYSTEM / CONFIGURATION
  system_state
  schema_migrations
  commands
  journal_epochs
  configuration_components
  configuration_revisions
  active_configuration
  configuration_sources
  config_load_attempts
  config_reconciliations

AGENTS / BACKENDS
  agent_snapshots
  agent_registry
  backend_instances

GOALS / TASK GRAPH
  goals
  goal_revisions
  goal_reconciliations
  goal_deliverable_bindings
  goal_completion_candidates
  graph_state
  tasks
  task_specs
  task_conditions
  task_dependencies
  task_input_bindings
  task_spawns
  task_joins
  continuation_contexts
  planning_records

SCHEDULING / EXECUTION
  scheduling_claims
  agent_resolutions
  execution_requests
  route_decisions
  execution_bindings
  runs
  run_status
  attempts
  attempt_status
  backend_attachments
  agent_control_sessions
  agent_requests

SANDBOX / WORKSPACE
  sandbox_instances
  sandbox_status
  sandbox_verifications
  repositories
  workspaces
  workspace_revisions
  integration_intents

RESOURCES
  resource_descriptors
  resource_reservations

BUDGET / USAGE
  budget_accounts
  budget_periods
  budget_holds
  usage_records
  charge_records
  budget_consumptions
  administrative_budget_settlements

AUTHORIZATION
  grants
  authorization_decisions
  capability_tickets
  broker_operations

RECOVERY
  failure_records
  recovery_decisions
  recovery_counters
  recovery_findings
  external_lineage_tombstones

EVALUATION / ACCEPTANCE
  evaluators
  evaluator_versions
  evaluation_rounds
  evaluation_round_evaluators
  evaluation_operations
  evaluation_attempts
  human_evaluation_requests
  evidence
  acceptance_results

ARTIFACTS
  blobs
  artifacts
  artifact_members
  production_records
  production_inputs
  candidates
  candidate_outputs
  retention_pins
  git_artifact_pins

SECRETS METADATA ONLY
  secret_descriptors
  secret_mutation_intents
  credential_leases
  credential_use_records

EVENTS
  event_journal
  event_export_cursors
```

Secret material is never stored in SQLite.

## Goal and Task

`goals` stores current revision pointer, phase, terminal target/status revision and current completion-candidate ref. `goal_revisions` is immutable `(goal_id, revision)` content.

`tasks` stores current phase/status revision/current responsible Run pointer. `task_specs` is immutable.

Key invariant:

```text
Task Active => exactly one nonterminal Run
Task Ready|Waiting => zero nonterminal Runs
```

Use a partial unique index on Runs equivalent to one nonterminal Run per Task, with controller transactions also checking Task phase/active_run_id.

## Temporal TaskGraph

Graph mutations are revisioned. Dependencies may be stored as temporal edges:

```text
created_graph_revision
removed_graph_revision NULL
```

Edge is active at revision R when created <= R and removed is null or > R. Active-edge uniqueness is enforced where possible; cycle validation remains controller transaction logic.

## SchedulingClaim

One current SchedulingClaim per Task is sufficient; history belongs to Events. It binds Task/Goal/Graph/config revisions and expiry/incarnation.

## ResourceReservation

ResourceReservations use explicit holder foreign keys/scope for:

```text
Task
Run
control-operation
```

and integral quantities.

Critical uniqueness:

> For resource families modeled as one logical Task reservation per key, enforce at most one non-released Task-scoped reservation per `(task_id, resource_key)`.

Admission diffs desired Task-scoped claims against existing compatible Task reservations; a new Run does not recreate the Workspace reservation.

Run-scoped reservations remain fresh per Run.

## ExecutionBinding

ExecutionBinding is immutable and stores/refs:

```text
task
selected Agent/version
ExecutionRequest/Offer hashes
backend + descriptor revision
ConfigurationRevision
routePolicyDigest
executionProfileDigest
frozen authz ceiling digest
SandboxPlan digest
binding hash/canonical JSON
```

Do not use one ambiguous `policy_hash` field.

## Run status

Conceptually:

```text
runs (immutable)
  id
  task_id
  binding_id
  snapshot/context refs
  created_at

run_status (mutable)
  run_id PK/FK
  phase                    Active|Finalizing|Completed|Failed|Cancelled|Yielded
  terminal_target          nullable while Active; required while Finalizing
  desired_execution
  revision
  candidate_digest
  current_attempt_id
  control_epoch
  lease_token
  lease_holder/incarnation
  lease_valid_until
  updated_at
```

Persistence invariant is:

```text
Run Completed => candidate_digest not null
Run Yielded|Failed|Cancelled => candidate may be null
Run Finalizing => terminal_target not null
```

The obsolete invariant `Run Finalizing => Candidate exists` is invalid.

## Attempt and launch-contact state

Attempt immutable identity:

```text
attempts
  id
  run_id
  ordinal
  launch_key UNIQUE
  created_at
```

Mutable Attempt status records external observation plus durable launch-call boundary:

```text
attempt_status
  attempt_id
  observed_execution
  terminal
  revision
  launch_contact_state     NOT_CONTACTED|CONTACT_MAY_HAVE_OCCURRED
  launch_contact_initiated_at
  launch_contact_epoch/incarnation
  started_at
  finished_at
  termination_json
```

Attempt creation + LaunchKey + AgentControlSession occurs before backend side effect. A second authoritative transaction marks `CONTACT_MAY_HAVE_OCCURRED` immediately before the first external launch call.

Crash semantics:

```text
NOT_CONTACTED
  + no other external evidence -> Pantheon can know its launch path never crossed the call boundary

CONTACT_MAY_HAVE_OCCURRED
  -> lost acknowledgement is UNKNOWN until backend/outer supervisor proves state
```

## Agent Control

`agent_control_sessions` stores one Attempt-scoped identity verifier/session state; raw bearer material is not persisted.

`agent_requests` enforces:

```text
PRIMARY/UNIQUE (attempt_id, request_id)
```

plus request hash/operation/state/result/problem refs. Same ID+same hash is idempotent; same ID+different hash fails closed.

## Sandbox

SandboxInstance is normally Run-scoped and has immutable SandboxKey/Plan identity plus mutable desired/observed status. Provisioning intent is committed before external runtime calls.

SandboxVerification records factual verification of expected environment identity, mounts, network, privilege controls, Agent Control exposure and limits before `SandboxReady=True`/Attempt creation.

## Budget

BudgetPeriods may maintain mutable aggregate counters (`consumed`, `held`, revision) for efficient admission while immutable `budget_consumptions`/Usage/Charge facts provide audit/reconciliation.

All aggregate mutations happen in the same transaction as their immutable ledger facts.

UNKNOWN administrative settlement is stored separately from factual Usage/Charge. Force resolution never fabricates Usage.

## Usage provenance

UsageRecord idempotency must be namespaced by Pantheon provenance, equivalent to:

```text
(backend_id, attempt_id/control_operation_id, adapter_operation_key, meter)
```

with a CHECK ensuring exactly one execution/control-operation subject where applicable.

Attempt usage validates that the immutable ExecutionBinding names the reporting backend and the applicable frozen metering contract.

Backend-authored control-operation usage validates the symmetric immutable provenance on the referenced control-operation record. Any control operation that can accept such usage must freeze, before external contact, relational immutable fields equivalent to:

```text
usage_reporter_backend_id
usage_reporter_backend_revision
metering_contract_digest
```

The fields are absent together for an operation that does not accept backend-authored usage and complete together for one that does. The reporting backend cannot create or rewrite this ownership. For EvaluationOperations these fields belong to the immutable operation intent; they do not create an ExecutionBinding or transfer lifecycle ownership from the Evaluation Controller.

Usage ingestion rejects a control-operation record when `backend_id` does not equal the operation's frozen `usage_reporter_backend_id`, when the meter/units are outside the frozen contract, or when no external metering-source binding exists.

Current terminal/running state is not an ownership predicate: delayed otherwise-valid factual usage may arrive after terminalization or administrative resolution. Where a separately durable launch/contact marker proves that the external lineage was never contacted, that evidence may reject impossible usage; the persistence of that launch boundary is a distinct execution-reconciliation invariant.

Controller epoch/incarnation may be stored as provenance but is not a rejection key for delayed otherwise-valid factual usage.

## Grants and broker operation redemption

Grant use-count and current-policy redemption are authoritative relational transitions.

The following authority-bearing rows carry a relational `restore_generation` copied from the current `system_state.restore_generation` when created:

```text
grants
capability_tickets
broker_operations
```

A consequential broker redemption transaction rechecks current Attempt/Task/Run authority, current ConfigRevision/authz digest, current RestoreGeneration, Grant scope/expiry/remaining uses and exact operation idempotency. It requires `grant.restore_generation == current restore_generation`, then CAS-consumes one Grant use and creates/transitions the exact broker operation under that same generation in the **same transaction**.

Capability tickets, if represented, are single-use/short-lived references and are revalidated at redemption, including `ticket.restore_generation == current restore_generation`; issuance alone is not durable bearer authority.

After disaster restore, rows whose generation differs from current are not deleted or rewritten to current. Old-generation Grants/Tickets are non-redeemable historical authority. If an operator re-affirms the permission, a new Grant is created under the current generation.

Old-generation `broker_operations` are **reconciliation-only**. Their restored state may be compared with external reality using the original operation/idempotency identity, but no controller may issue/reissue the external effect from that row merely because it appears `PENDING`, incomplete, or absent from later history. If the outcome cannot be established, the operation/domain remains UNKNOWN/fenced until explicit recovery resolution.

## Recovery tombstones

Operator force-resolution of irrecoverable UNKNOWN creates an immutable/durable lineage tombstone, conceptually:

```text
external_lineage_tombstones
  id
  subject_kind
  subject_id
  launch_key/sandbox_key/etc.
  expected_revision
  actor
  reason
  evidence_json/ref
  created_at
```

A tombstoned LaunchKey/session can never regain current control authority. Late observations may be recorded as history/usage but cannot mutate the current execution lineage.

## Artifacts / CAS

For ordinary content-addressed local objects, durable bytes precede DB references:

```text
temp write
fsync/finalize
atomic rename to digest path
verify
SQLite transaction adds Blob/Artifact/Candidate/Event refs
```

An orphan durable CAS object is harmless/GC-able; a DB reference to missing bytes is not.

Git-backed code changeset objects need the Git-specific preservation contract in `workspace-and-git-integration.md`: authoritative Git objects must be pinned/preserved before the SQLite Artifact/Candidate reference is committed, or the changeset payload must be independently present in Pantheon CAS.

## Candidate submission transaction (T6)

Candidate submission is cancellation/supersession-race-safe:

```text
[required CAS/Git payload already durable/pinned]

BEGIN IMMEDIATE

re-read/validate:
  AgentControlSession current
  Attempt current
  Run Active/current responsible Run
  Task Active + expected Task status revision
  no cancellation/supersession/finalization fence committed
  Candidate inputs/Artifacts valid

insert Candidate + outputs
freeze/checkpoint Workspace as required
Run Active -> Finalizing
Run terminal_target = Completed
Task Active -> Evaluating
append Events

COMMIT
```

If cancellation/supersession won the status CAS first, T6 fails with stale/conflict and creates no current Candidate.

## Acceptance and requeue transaction (T9)

Acceptance/recovery may decide REQUEUE while producer Run is still Finalizing, but the Task must not become Ready yet.

T9 precondition:

```text
prior responsible Run is terminal
Task is still the current nonterminal Task state eligible for requeue
RecoveryDecision/rejection evidence current
Goal/Graph/config/policy current
no cancellation/supersession fence
```

Then:

```text
Task -> Ready
active_run_id -> NULL
install RecoveryContext/notBefore as needed
safe accounting/resource updates
append Events
```

This preserves the partial unique live-Run index.

## Blocking-yield final transaction

After parent Run execution is safely stopped and Run-scoped capacity/holds settled:

```text
BEGIN IMMEDIATE
verify Run Finalizing/terminal_target=Yielded
verify no unresolved Run execution obligation
create/verify WorkspaceRevision checkpoint
Run -> Yielded
Task Active -> Waiting
Task.active_run_id -> NULL
append Events
COMMIT
```

Task-scoped Workspace reservation remains live.

## Configuration activation

Immutable ConfigurationRevision/components are compiled/validated before touching active state. Activation changes one `active_configuration` pointer and Event in an authoritative transaction, with a short publication barrier between DB/current in-memory snapshot as defined by the configuration architecture.

Historical hash-bearing config rows are never rewritten in place by migration.

## Evaluation

EvaluationRound pins Candidate/acceptance/evaluator versions. External deterministic checks use `EvaluationOperation` with control-operation ResourceReservations/BudgetHolds where required; EvaluationAttempts are small execution/reconciliation identities, not Runs.

A billable EvaluationOperation that accepts backend-authored factual usage carries immutable operation-intent fields equivalent to `usage_reporter_backend_id`, `usage_reporter_backend_revision`, and `metering_contract_digest`, frozen before external contact. These fields are nullable only as an all-or-none group for operations with no backend-authored metering and are never mutable lifecycle status.

## Secret metadata

SQLite stores only SecretDescriptor/provider locator/non-secret random version IDs/status/intents/lease metadata/use records. It never stores long-lived secret bytes or hashes of secret bytes.

## Event Journal

The Event Journal is append-only durable history/outbox, not primary state. Events for authoritative mutations are inserted in the same transaction as the state change.

Sequence is explicit `(journal_epoch, sequence)` with a singleton next-sequence allocator in the same write transaction. Disaster restore rotates JournalEpoch rather than pretending restored history is continuous.

JournalEpoch and RestoreGeneration are intentionally separate: JournalEpoch fences event-stream continuity, while RestoreGeneration fences runtime authority and command/idempotency continuity. A future journal-only rotation must not revoke Grants, and an authority-generation decision must not depend on Event retention mechanics.

## Commands

`commands` stores operator idempotency identity, actor, operation, non-sensitive request hash, status/result refs/timestamps. Command identity is relationally scoped by:

```text
restore_generation / command_epoch
command_id
```

Normal uniqueness is `(restore_generation, command_id)`. For a request, Pantheon first compares the supplied `command_epoch` with current `system_state.restore_generation` **before** treating command-row absence as a new command. A mismatch fails closed as stale command authority even if restoration removed the historical command row.

Within the current generation, same command ID+same hash returns/reconciles prior outcome; same command ID+different hash fails closed. After disaster restore, callers must treat old-generation outcomes as unknown and intentionally issue a new command ID under the new generation only after current-state reconciliation.

Sensitive secret-set operations are an explicit exception only to persisted request hashing: secret bytes are never part of durable request hashes; command ID remains single-use and generation-bound, and secret mutation intent uses only non-secret metadata/version identity.

## Disaster-restore authority fence (T0)

Restore is not an ordinary startup. After SQLite integrity/schema validation and acquisition of the installation lock, but before scheduler dispatch, authorization redemption, broker execution, Operator mutations or other new authority-bearing external effects, Pantheon commits one restore fence transaction:

```text
BEGIN IMMEDIATE

verify restore mode + installation identity
write fresh unpredictable system_state.restore_generation
rotate JournalEpoch as required by event-history semantics
record restore RecoveryPass/incarnation linkage
append restore-fence audit Event in the new journal epoch

COMMIT
```

The freshly generated RestoreGeneration must not be derived by incrementing a value from the restored snapshot; an old backup may contain a previously used number. Normal daemon restart never performs T0.

After T0:

- old-generation Grants and CapabilityTickets cannot redeem;
- old-generation broker operations are reconciliation-only;
- Operator commands carrying an old commandEpoch are rejected before command-row lookup/creation;
- Run ControlLease tokens still rotate separately before Run/external commands;
- domain recovery reconciles/fences external state before normal dispatch resumes.

## Migrations / backup

Use `PRAGMA user_version` plus immutable checksummed `schema_migrations`. Unknown newer schema fails startup. Controllers/scheduler are disabled during migration. Back up through SQLite's supported online backup/snapshot mechanism, never by naively copying only `pantheon.db` while live WAL state exists.

Use `PRAGMA application_id` before declaring the DB format stable.

## Invariant checker

A deterministic PersistenceInvariantChecker verifies at least:

```text
Active Task -> exactly one nonterminal Run
Ready/Waiting Task -> zero nonterminal Runs
Finalizing Run -> terminal_target present
Completed Run -> Candidate exists
one nonterminal Attempt per Run
one current AgentControlSession per Attempt
one live Task-scoped reservation per singular (Task, ResourceKey)
Reservation holder validity
Budget aggregate == immutable ledger reconstruction
Usage provenance/backend ownership for Attempt and control-operation subjects
Grant/CapabilityTicket redemption generation == current RestoreGeneration
new/executable broker operation generation == current RestoreGeneration
old-generation broker operations are reconciliation-only
current Operator command epoch == current RestoreGeneration before command creation
Candidate outputs -> existing Artifacts/Blobs
Workspace/Sandbox ownership consistency
IntegrationIntent/Git state consistency
Event epoch/sequence sanity
```

Violations create RecoveryFindings/quarantine rather than silent unsafe repair.

## Named transaction families

```text
T0  DISASTER-RESTORE AUTHORITY FENCE
T1  GOAL REVISION
T2  GRAPH PATCH
T3  SCHEDULER RUN-INTENT COMMIT
T4  ATTEMPT + AGENT-CONTROL IDENTITY
T4b LAUNCH CONTACT MARKER
T5  USAGE INGESTION
T6  CANDIDATE SUBMISSION
T7  ACCEPTANCE/EVIDENCE COMMIT
T8  RETRY ATTEMPT
T9  REQUEUE AFTER PRIOR RUN TERMINAL
T10 AUTHORIZATION/GRANT REDEMPTION
T11 WORKSPACE/SANDBOX DESIRED STATE
T12 INTEGRATION STATE
T13 CONFIGURATION ACTIVATION
T14 UNKNOWN FORCE-RESOLUTION/TOMBSTONE
```

Never perform network/Git/process/backend/secret-store/container-runtime calls inside a SQLite transaction.

## Core invariants

1. One authoritative SQLite database provides cross-subsystem atomicity.
2. Writer serialization prevents physical write contention; row revisions/CAS prevent logical stale decisions.
3. Safety-critical relationships are relationally constrained where SQLite can express them.
4. JSON is never a substitute for ownership/revision/accounting columns.
5. Task-scoped reservations are unique/reused across Runs.
6. Run Finalizing always records terminalTarget; only Completed requires Candidate.
7. Launch contact boundary is durable before external launch call.
8. Usage identity is Pantheon-namespaced; a backend may report only for an Attempt ExecutionBinding or control-operation metering binding that immutably names it, and delayed factual usage is not rejected solely for stale controller epoch or current terminal state.
9. Grant use/redemption and exact broker-operation creation are one CAS transaction under current policy and current RestoreGeneration.
10. Disaster restore rotates a fresh unpredictable RestoreGeneration before any new authority-bearing mutation/effect; restored Grants/Tickets cannot redeem and restored broker operations cannot be reissued from stale state.
11. Operator command idempotency is scoped by `(RestoreGeneration, commandId)` and stale epochs fail before row absence can be interpreted as a new command.
12. Cancellation/supersession can beat Candidate submission through Task revision CAS.
13. Requeue occurs only after previous responsible Run terminal.
14. Force-resolution tombstones stale lineages without fabricating factual Usage.
15. Event rows are committed with their authoritative mutation, but state tables remain source of truth.
