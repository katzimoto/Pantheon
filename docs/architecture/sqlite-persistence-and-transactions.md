# SQLite Persistence and Transaction Boundaries

## Status

Draft design — Pantheon persistence subsystem specification.

## Purpose

This document maps Pantheon's control-plane architecture onto one crash-safe relational SQLite database and defines the transaction boundaries that preserve cross-subsystem invariants.

The central rule is:

> **Anything participating in a Pantheon safety invariant is represented relationally. JSON stores immutable structured documents and non-query-critical detail; it does not replace foreign keys, uniqueness constraints, revisions, lifecycle columns, or accounting columns.**

Pantheon uses SQLite as a real relational control-plane database, not as a generic JSON document store.

See also:

- `docs/architecture/global-recovery-and-crash-reconciliation.md`
- `docs/architecture/event-and-observability-model.md`
- `docs/architecture/scheduler-reservations-ownership-and-leases.md`
- `docs/architecture/budget-usage-and-rate-limits.md`
- `docs/architecture/artifact-model.md`
- `docs/architecture/workspace-and-git-integration.md`

## 1. One authoritative SQLite database

V1 uses one authoritative database file:

```text
~/.pantheon/
├── state/
│   └── pantheon.db
├── store/
│   └── objects/...
└── workspaces/...
```

Do not split Tasks, budgets, resources, events, Runs, or authorization across multiple attached SQLite databases.

Pantheon requires transactions such as:

```text
ResourceReservations
+ BudgetHolds
+ ExecutionBinding
+ Run
+ Task → Active
+ Event Journal rows
```

to survive a crash atomically. One database file is therefore part of the correctness model.

CAS bytes and Git workspaces remain external reconciled stores because their side effects cannot participate in the SQLite transaction itself.

## 2. SQLite build baseline

Pantheon should bundle and test against a known SQLite build rather than depending on an arbitrary system library.

V1 baseline:

```text
SQLite 3.53.4
```

or a newer explicitly tested release.

The selected build must include the WAL-reset corruption fix first released in SQLite 3.51.3.

The Pantheon release process owns compatibility testing for the chosen SQLite version and compile-time options.

## 3. Connection model

V1 uses:

```text
PersistenceStore
├── one serialized authoritative writer connection
└── a small bounded pool of read connections
```

SQLite WAL permits concurrent readers while preserving one-writer semantics. Pantheon should embrace this rather than create many competing writer connections.

For state-dependent authoritative commits, the writer uses `BEGIN IMMEDIATE` so write ownership is established before validating and applying the decision.

Controllers may compute proposals from prior reads, but the authoritative writer transaction must re-read/revalidate current revisions before commit.

A configured `busy_timeout` is a defensive fallback, not the primary concurrency mechanism.

Shared-cache mode is disabled.

## 4. Connection baseline

Every Pantheon connection establishes a known configuration, including:

```sql
PRAGMA foreign_keys = ON;
PRAGMA trusted_schema = OFF;
PRAGMA busy_timeout = ...;
```

Database initialization establishes:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
```

Pantheon favors control-plane durability over maximum write throughput.

## 5. STRICT relational schema

Pantheon-owned relational tables use SQLite `STRICT` tables.

Example:

```sql
CREATE TABLE tasks (
    id              TEXT PRIMARY KEY,
    goal_id         TEXT NOT NULL,
    phase           TEXT NOT NULL,
    revision        INTEGER NOT NULL,
    active_run_id   TEXT,
    created_at_us   INTEGER NOT NULL,
    updated_at_us   INTEGER NOT NULL
) STRICT;
```

Authoritative values use strict SQLite types:

```text
INTEGER
REAL
TEXT
BLOB
ANY
```

`REAL` is avoided for exact accounting values.

## 6. Integer base units

All authoritative quantities use integral base units.

Examples:

```text
money     → integer micros/nanos
CPU       → integral normalized unit such as millicores
memory    → bytes
time      → microseconds
tokens    → integer count
credits   → account-defined integer scale
```

Resource/Budget descriptors define the unit and scale.

Floating-point arithmetic must not determine budget or admission authority.

## 7. Time representation

Internal timestamps are stored as UTC Unix microseconds in `INTEGER` columns.

API/CLI boundaries render RFC 3339 timestamps.

Wall-clock timestamps are not ordering authority. Durable Event order is `(journalEpoch, sequence)`.

## 8. IDs

V1 retains Pantheon's opaque, human-debuggable TEXT identifiers such as:

```text
goal_...
task_...
run_...
attempt_...
binding_...
```

Do not introduce a parallel surrogate-integer identity system without measured need.

## 9. WITHOUT ROWID policy

Do not blanket-apply `WITHOUT ROWID`.

Ordinary resource/status tables remain normal rowid tables. `WITHOUT ROWID` may be used selectively for narrow junction tables whose natural primary key is already a compact composite key, such as:

```text
candidate_outputs
task_dependencies
artifact_members
```

Performance choices are validated with measurement.

## 10. JSON boundary

JSON is used for immutable structured documents and non-query-critical detail, for example:

```text
Run snapshots
ExecutionRequests
ExecutionBindings
failure evidence
Event payloads
```

Critical relationships are also represented by typed relational columns with FKs, uniqueness constraints, checks, and indexes.

Pantheon-owned hash-bearing structured documents are stored as validated canonical JSON TEXT:

```sql
spec_json TEXT NOT NULL CHECK(json_valid(spec_json, 1))
```

SQLite JSONB is not Pantheon's canonical persisted interchange/hash representation.

Query-critical values are duplicated into typed columns intentionally.

## 11. Digest representation

SHA-256 digests are stored internally as 32-byte BLOBs:

```sql
spec_hash BLOB NOT NULL CHECK(length(spec_hash) = 32)
```

External representations use algorithm-qualified strings such as:

```text
sha256:<hex>
```

Algorithm agility may be introduced when a second algorithm is actually required.

## 12. Foreign keys

Durable relationships use real foreign keys whenever both entities live in SQLite.

Examples:

```text
Run → Task
Attempt → Run
Reservation → Run or Task
Candidate → Run
Evidence → Candidate
Workspace → Task
IntegrationIntent → Candidate
```

Foreign keys are immediate by default.

Use `DEFERRABLE INITIALLY DEFERRED` only where a demonstrated cyclic transaction genuinely needs it.

## 13. CHECK constraints

Database checks enforce closed invariants such as:

```text
lifecycle enum values
boolean 0/1 values
nonnegative quantities
digest lengths
exactly-one-of holder columns
```

Do not use closed CHECK enums for extensible namespaces such as Task types, competencies, failure codes, Event types, Artifact kinds, or resource keys.

## 14. Partial unique indexes

Partial unique indexes encode important concurrency invariants directly in SQLite.

### At most one nonterminal Run per Task

```sql
CREATE UNIQUE INDEX ux_runs_one_live_per_task
ON runs(task_id)
WHERE phase IN ('Active', 'Finalizing');
```

The exact index may target the mutable Run status representation in the physical schema, but the invariant is database-enforced.

### At most one nonterminal Attempt per Run

```sql
CREATE UNIQUE INDEX ux_attempts_one_live_per_run
ON attempts(run_id)
WHERE terminal = 0;
```

Again, the final physical placement may use `attempt_status`; the invariant must remain unique and database-enforced.

### Active graph edge uniqueness

```sql
CREATE UNIQUE INDEX ux_task_dependency_active
ON task_dependencies(
    goal_id,
    upstream_task_id,
    downstream_task_id,
    dependency_kind
)
WHERE removed_graph_revision IS NULL;
```

Additional partial uniqueness is introduced only for real invariants.

## 15. No business-logic triggers in v1

Use CHECK, FK, UNIQUE, and partial indexes for relational integrity.

Do not hide lifecycle transitions, Event creation, budget accounting, reservation release, or recovery behavior inside SQL triggers.

Rust transaction code explicitly performs all authoritative mutations and inserts the corresponding Event Journal rows.

This keeps transaction behavior visible, testable, traceable, and fault-injectable.

## 16. Immutable specification vs mutable status

Where immutability is architecturally important, use separate physical records for immutable contract data and mutable observed/desired status.

Examples include:

```text
Run immutable strategy      + Run status
Attempt immutable LaunchKey + Attempt status
Artifact immutable identity + replica state
Goal revisions              + current Goal status
```

Example conceptual Run layout:

```text
runs
  id
  task_id
  binding_id
  snapshot_hash
  snapshot_json
  created_at

run_status
  run_id
  phase
  desired_execution
  revision
  control_epoch
  lease_token
  lease_holder
  lease_valid_until
  updated_at
```

Not every entity needs a two-table split; use it where the domain boundary is material.

## 17. Table families

The physical schema should roughly contain these families:

```text
SYSTEM
  system_state
  schema_migrations
  journal_epochs
  commands

AGENTS / BACKENDS
  agent_snapshots
  agent_registry
  backend_instances

GOALS / TASK GRAPH
  goals
  goal_revisions
  goal_reconciliations
  graph_state
  tasks
  task_specs
  task_conditions
  task_dependencies
  task_input_bindings
  task_spawns
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

AUTHORIZATION
  grants
  authorization_decisions
  capability_tickets

RECOVERY
  failure_records
  recovery_decisions
  recovery_counters
  recovery_findings

ARTIFACTS / ACCEPTANCE
  blobs
  artifacts
  artifact_members
  production_records
  production_inputs
  candidates
  candidate_outputs
  evidence
  acceptance_results
  retention_pins

WORKSPACE / GIT
  repositories
  workspaces
  workspace_revisions
  integration_intents

EVENTS
  event_journal
  event_export_cursors
```

The number of tables reflects distinct invariants, not service boundaries. They all remain in one embedded database.

## 18. Goal revision storage

Conceptually:

```text
goals
  id
  current_revision
  phase
  revision
  ...

goal_revisions
  goal_id
  revision
  spec_hash
  spec_json
  created_at
  created_by

PRIMARY KEY(goal_id, revision)
```

Goal revision mutation:

```text
BEGIN IMMEDIATE
verify expected Goal status revision
insert GoalRevision N+1
update goals.current_revision/status revision
create reconciliation obligation
insert Event(s)
COMMIT
```

Goal revision rows are immutable.

## 19. Task specification storage

Conceptually:

```text
tasks
  id
  goal_id
  phase
  status_revision
  active_run_id
  created_at
  updated_at

task_specs
  task_id
  spec_hash
  spec_json
  created_graph_revision
```

Once materialized, `task_specs` is immutable. Semantic replacement creates a new/superseding Task rather than updating the Task contract in place.

## 20. TaskGraph temporal edges

Avoid copying the entire graph on every revision.

Conceptually:

```text
graph_state
  goal_id
  current_revision
  status_revision

task_dependencies
  goal_id
  upstream_task_id
  downstream_task_id
  dependency_kind
  created_graph_revision
  removed_graph_revision NULL
```

An edge is active at revision R when:

```text
created_graph_revision <= R
AND (
  removed_graph_revision IS NULL
  OR removed_graph_revision > R
)
```

Graph mutations remain atomic and revisioned. Cycle detection stays in the Graph controller because SQL constraints do not conveniently encode the DAG invariant.

## 21. SchedulingClaim representation

V1 may keep one current SchedulingClaim row per Task:

```text
scheduling_claims
  task_id PRIMARY KEY
  claim_id UNIQUE
  owner_incarnation
  task_revision
  goal_revision
  graph_revision
  policy_hash
  acquired_at
  expires_at
```

Claim history is already preserved in the Event Journal.

Expired claims are safely replaceable using transactional validation/CAS semantics.

## 22. ExecutionBinding storage

`ExecutionBinding` is immutable.

Conceptually:

```text
execution_bindings
  id
  task_id
  agent_id
  request_hash
  offer_hash
  backend_id
  backend_descriptor_revision
  policy_hash
  route_policy_hash
  binding_hash
  binding_json
  decided_at
```

Mutable reservation/accounting/execution state references the Binding separately.

## 23. Run storage

Immutable Run data:

```sql
CREATE TABLE runs (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES tasks(id),
    binding_id      TEXT NOT NULL REFERENCES execution_bindings(id),
    snapshot_hash   BLOB NOT NULL CHECK(length(snapshot_hash)=32),
    snapshot_json   TEXT NOT NULL CHECK(json_valid(snapshot_json,1)),
    created_at_us   INTEGER NOT NULL
) STRICT;
```

Conceptual mutable status:

```sql
CREATE TABLE run_status (
    run_id               TEXT PRIMARY KEY REFERENCES runs(id),
    phase                TEXT NOT NULL,
    desired_execution    TEXT NOT NULL,
    revision             INTEGER NOT NULL,
    candidate_digest     BLOB,
    control_epoch        INTEGER NOT NULL,
    lease_token          BLOB NOT NULL,
    lease_holder         TEXT,
    lease_valid_until_us INTEGER,
    updated_at_us        INTEGER NOT NULL
) STRICT;
```

Control fencing state is authoritative status.

## 24. Attempt storage

Immutable:

```text
attempts
  id
  run_id
  ordinal
  launch_key
  created_at
```

Constraints include:

```text
UNIQUE(run_id, ordinal)
UNIQUE(launch_key)
```

Mutable:

```text
attempt_status
  attempt_id
  observed_execution
  terminal
  revision
  started_at
  finished_at
  termination_json
  updated_at
```

Adapter-private recovery state:

```text
backend_attachments
  attempt_id
  schema_version
  opaque_state BLOB
  updated_at
```

## 25. Resource storage

Conceptual descriptors:

```text
resource_descriptors
  resource_key
  allocation_mode
  unit
  capacity
  allocatable
  health
  revision
  observed_at
```

Reservations:

```text
resource_reservations
  id
  resource_key
  run_id NULL
  task_id NULL
  quantity
  state
  created_at
  updated_at
```

V1 enforces exactly one non-null holder among `run_id` and `task_id`, matching the two supported holder scopes.

## 26. Budget accounting

Budget authority intentionally combines fast mutable aggregates with immutable accounting records.

Fast authority:

```text
budget_periods
  id
  account_id
  limit_amount
  consumed_amount
  held_amount
  revision
  ...
```

Immutable explanation:

```text
budget_consumptions
  id
  period_id
  hold_id
  charge_id
  quantity
  created_at
```

Usage/charge transaction atomically:

```text
insert UsageRecord
insert ChargeRecord
insert BudgetConsumption
update BudgetPeriod consumed/held counters
insert Event(s)
```

Recovery/invariant checking can recompute immutable consumption totals and compare them to aggregate authority.

## 27. Usage idempotency

Usage source identity is protected by a UNIQUE constraint.

Conceptually:

```text
usage_records
  id
  source_operation_key
  meter
  quantity
  quality
  attempt_id
  run_id
  occurred_at
  recorded_at

UNIQUE(source_operation_key, meter)
```

Duplicate/replayed backend observations cannot create a second accounting debit for the same normalized operation and meter.

## 28. CAS/SQLite commit protocol

Content-addressed storage uses an asymmetric commit protocol.

Correct order:

```text
write object to CAS temp
        ↓
fsync/durably finalize
        ↓
atomic rename to digest path
        ↓
verify digest and size
        ↓
BEGIN SQLite
  Blob metadata
  Artifact
  Candidate if applicable
  Event(s)
COMMIT
```

A crash after CAS finalize but before SQLite commit produces only an unreferenced CAS object, which is safe and garbage-collectable.

SQLite must not commit a reference to bytes that have not first been durably materialized and verified.

This differs intentionally from executor/Git/workspace external side effects, for which durable DB intent precedes potentially dangerous external mutation.

## 29. Artifact storage

Conceptually:

```text
blobs
  digest BLOB PRIMARY KEY
  size
  state
  verified_at

artifacts
  digest BLOB PRIMARY KEY
  artifact_kind
  manifest_json
  created_at

artifact_members
  artifact_digest
  ordinal
  name
  media_type
  blob_digest
  size
```

`artifact_members` is a candidate for `WITHOUT ROWID` because it is narrow and naturally composite-keyed.

Replica state is separate from immutable Artifact identity.

## 30. Candidate storage

Conceptually:

```text
candidates
  digest BLOB PRIMARY KEY
  task_id
  run_id UNIQUE
  candidate_json
  created_at
```

`UNIQUE(run_id)` enforces the one-candidate-per-Run v1 invariant.

Outputs:

```text
candidate_outputs
  candidate_digest
  output_name
  artifact_digest

PRIMARY KEY(candidate_digest, output_name)
```

## 31. Acceptance evidence storage

Conceptually:

```text
evidence
  id
  candidate_digest
  artifact_digest NULL
  task_id
  criterion_id
  evaluator_ref
  evaluator_version_hash
  verdict
  details_json
  started_at
  completed_at
```

Aggregate:

```text
acceptance_results
  candidate_digest PRIMARY KEY
  acceptance_spec_hash
  verdict
  completed_at
```

Evidence is structurally bound to the exact Candidate digest it evaluated.

## 32. Workspace storage

Conceptually:

```text
workspaces
  id
  task_id UNIQUE
  repository_id
  requested_base_ref
  resolved_base_oid
  isolation
  phase
  local_path
  revision
  created_at
  updated_at
```

`local_path` is materialization/observed state, not identity.

Immutable checkpoints:

```text
workspace_revisions
  id
  workspace_id
  base_oid
  tree_oid
  observed_head_oid
  reason
  created_at
```

Run snapshots reference WorkspaceRevision IDs rather than mutable paths.

## 33. IntegrationIntent storage

Conceptually:

```text
integration_intents
  id
  candidate_digest
  changeset_digest
  repository_id
  target_ref
  expected_target_oid
  result_commit_oid
  policy_hash
  desired
  state
  revision
  created_at
  updated_at
```

Git compare-and-swap occurs outside SQLite and is reconciled afterward.

Pattern:

```text
durable IntegrationIntent
        ↓
Git update-ref CAS
        ↓
possible crash
        ↓
reconcile current Git ref
        ↓
record APPLIED / STALE / CONFLICT
```

Never run Git mutation inside a DB transaction.

## 34. Recovery storage

Immutable facts:

```text
failure_records
  id
  subject_kind
  subject_ref
  origin
  code
  certainty
  fingerprint
  evidence_json
  occurred_at

recovery_decisions
  id
  task_id
  run_id
  attempt_id
  failure_id
  action
  not_before
  policy_hash
  state_revision
  created_at
```

Mutable discovered anomaly:

```text
recovery_findings
  id
  subject_kind
  subject_ref
  category
  state
  severity
  first_seen
  last_seen
  resolution_json
```

A FailureRecord/RecoveryDecision is historical fact; a RecoveryFinding represents an ongoing inconsistency that may later be resolved.

## 35. Event Journal sequencing

Do not use `AUTOINCREMENT` as the authoritative journal sequence mechanism.

Use:

```text
journal_state
  singleton
  current_epoch
  next_sequence

journal_epochs
  epoch
  parent_epoch
  reason
  created_at
```

Inside the same authoritative transaction that mutates state:

```text
sequence = next_sequence
next_sequence += 1
INSERT event_journal(...)
```

Constraint:

```text
UNIQUE(journal_epoch, sequence)
```

Event ID remains globally unique independently of sequence.

A disaster recovery history branch rotates JournalEpoch.

## 36. Command idempotency

Persistence provides the primitive for API/CLI mutation idempotency before the transport layer is defined:

```text
commands
  id
  actor
  operation
  request_hash
  status
  result_ref
  created_at
  completed_at
```

Rules:

```text
same commandId + same request hash
→ return/reconcile prior result

same commandId + different request hash
→ fail closed
```

This prevents duplicate Goal creation, cancellation, approvals, or other mutations after client/network retries.

## 37. Optimistic revision/CAS

Every mutable control-plane record has an integer `revision`.

Example:

```sql
UPDATE run_status
SET
  phase = ?,
  revision = revision + 1,
  updated_at_us = ?
WHERE
  run_id = ?
  AND revision = ?;
```

The transaction requires exactly one affected row.

No row means the decision was made against stale state and the controller re-reads/reconciles.

Serialized physical writes and logical revision CAS solve different problems; Pantheon uses both.

## 38. Named transaction boundaries

Persistence exposes transaction-shaped operations rather than arbitrary controller SQL.

### T1 Goal revision

```text
GoalRevision
+ current revision pointer
+ reconciliation obligation
+ Event(s)
```

### T2 Graph patch

```text
Task/edge/input mutations
+ Graph revision
+ Event(s)
```

### T3 Scheduler commitment

```text
SchedulingClaim validation
+ ResourceReservations
+ initial BudgetHolds
+ ExecutionBinding
+ Run
+ Task → Active
+ claim consumption
+ Event(s)
```

### T4 Attempt creation

```text
Run/ownership validation
+ Attempt
+ LaunchKey
+ Event(s)
```

### T5 Usage ingestion

```text
Usage
+ Charge
+ BudgetConsumption
+ Budget aggregate conversion
+ Event(s)
```

### T6 Candidate submission

CAS bytes are already durable, then:

```text
Candidate
+ output bindings
+ Workspace → Frozen
+ Run → Finalizing
+ Task → Evaluating
+ Event(s)
```

### T7 Acceptance commit

```text
Evidence / aggregate verdict
+ lifecycle mutation where applicable
+ Event(s)
```

### T8 Retry Attempt

```text
RecoveryDecision validation
+ recovery counter charge
+ Attempt
+ LaunchKey
+ Event(s)
```

### T9 Task requeue

```text
Run terminal/final status as applicable
+ Task → Ready
+ RecoveryContext
+ scheduler notBefore
+ locally safe accounting/resource mutations
+ Event(s)
```

### T10 Authorization/Grant

```text
Grant use/state
+ AuthorizationDecision
+ Event(s)
```

### T11 Workspace desired state

```text
Workspace intent/phase
+ Task reservation
+ Event
```

Git/workspace external operation occurs after commit and is reconciled.

### T12 Integration state

```text
IntegrationIntent/status
+ Event
```

Git CAS occurs outside the transaction and is reconciled afterward.

Network, Git, process-launch, and backend calls never occur inside authoritative SQLite write transactions.

## 39. Migration discipline

Use both:

```text
PRAGMA user_version
```

and:

```text
schema_migrations
```

`user_version` is the quick numeric schema level.

`schema_migrations` records:

```text
version
name
checksum
applied_at
Pantheon build
```

Rules:

- migration files become immutable once released;
- checksum is verified before execution;
- versions are monotonic;
- a database from a newer unknown schema version causes startup refusal;
- failed migrations roll back;
- scheduler/controllers do not run during schema migration.

For destructive/nontrivial migration, take a consistent SQLite backup first.

## 40. Database identity

Use SQLite's application metadata:

```text
PRAGMA application_id
PRAGMA user_version
```

Choose/register a stable Pantheon application ID before freezing the production DB format.

`system_state` also records at least:

```text
installation_id
created_at
current_journal_epoch
database_format_version
```

## 41. Backup discipline

Never make a live backup by casually copying only `pantheon.db` while WAL is active.

Use the SQLite Online Backup API or another SQLite-supported consistent snapshot mechanism.

The backup represents the control-plane database only. Restoring it still requires the Global Recovery barrier because external executors, workspaces, Git refs, and CAS may be newer than the backup.

## 42. Query planner maintenance

For long-running connections, follow current SQLite `PRAGMA optimize` guidance rather than hard-coding unrestricted periodic `ANALYZE` jobs.

Pantheon should run appropriate `PRAGMA optimize` operations at connection/startup/periodic points and after meaningful schema/index changes, then validate behavior with real query plans.

## 43. Indexing discipline

Indexes are driven by actual controller/reconciliation/scheduler query patterns.

Likely initial indexes include:

```text
tasks(phase, updated_at)
tasks(goal_id, phase)

runs(task_id)
run_status(phase)

attempts(run_id, ordinal)
attempt_status(terminal, observed_execution)

scheduling_claims(expires_at)
resource_reservations(resource_key, state)
budget_holds(period_id, state)
workspaces(phase)
integration_intents(state)
recovery_findings(state, severity)

event_journal(journal_epoch, sequence)
event_journal(subject_ref, sequence)
event_journal(event_type, sequence)
```

Use `EXPLAIN QUERY PLAN`, runtime metrics, and actual workload profiling before adding speculative indexes.

## 44. PersistenceInvariantChecker

Global Recovery includes a deterministic `PersistenceInvariantChecker` for cross-table/domain invariants SQL cannot conveniently express.

Examples:

```text
Task Active
→ exactly one responsible nonterminal Run

Run Finalizing
→ Candidate exists

nonterminal Attempt
→ parent Run is nonterminal

active ResourceReservation
→ valid holder exists

BudgetPeriod
→ aggregate counters reconcile with immutable accounting records

Candidate outputs
→ Artifact metadata exists

Artifact metadata
→ referenced Blob metadata exists

Workspace READY
→ owning Task exists and is not terminal-cleaned

Integration APPLIED
→ result OID recorded

Event Journal
→ current epoch/sequence state is sane
```

Run the checker:

```text
at startup after SQLite integrity checks
periodically as a safety pass
on explicit `pantheon doctor`
```

Impossible state creates a RecoveryFinding and fences the affected subject. Do not silently repair unless the recovery rule is provably safe.

## 45. Crash-injection testing

Every named transaction/external-side-effect boundary receives fault-injection tests around points such as:

```text
before BEGIN
after each logical mutation
before Event insertion
after Event insertion
before COMMIT
after COMMIT
before external side effect
after external side effect
before observation persistence
```

Then restart Pantheon and verify convergence without:

```text
duplicate executors
lost budgets
double charges
premature reservation release
wrong Git refs
missing accepted Artifacts
impossible lifecycle state
```

Crash safety is verified as a subsystem property, not inferred from successful unit tests alone.

## 46. Persistence API boundary

Controllers must not execute arbitrary ad-hoc SQL.

The persistence module exposes transaction-shaped operations corresponding to architecture contracts.

Conceptually:

```text
Controller
   │
   ▼
Persistence transaction API
   │
   ├─ validate expected revisions
   ├─ enforce DB constraints
   ├─ mutate authoritative state
   └─ insert durable Event(s)
           │
           ▼
        COMMIT
           │
           ▼
external side effect / controller wakeup
```

This boundary is essential for reviewability, invariant enforcement, audit completeness, and crash testing.

## V1 decisions

1. One authoritative SQLite database file; no multi-database control-plane split under WAL.
2. Bundle/test a known modern SQLite baseline; start v1 development on 3.53.4 or newer explicitly validated version.
3. WAL + `synchronous=FULL`, private caches, foreign keys ON, trusted schema OFF.
4. One serialized authoritative writer plus bounded readers.
5. Use `BEGIN IMMEDIATE` for state-dependent authoritative commits.
6. Use STRICT tables for Pantheon-owned relational state.
7. Use integer base units for authoritative resources, budgets, cost, and time.
8. Store timestamps as UTC integer microseconds; Event journal sequence provides durable ordering.
9. Keep opaque Pantheon resource IDs as TEXT in v1.
10. Use `WITHOUT ROWID` selectively rather than globally.
11. Critical relationships are relational; JSON never replaces foreign keys/constraints/revisions.
12. Store canonical hash-bearing JSON as validated TEXT, not SQLite JSONB.
13. Store SHA-256 digests internally as 32-byte BLOBs.
14. Use immediate FKs by default and defer only demonstrated cyclic cases.
15. Use CHECK, UNIQUE, and partial unique indexes for closed safety invariants.
16. Avoid business-logic triggers in v1.
17. Physically separate immutable contract/snapshot state from mutable status where architecturally important.
18. TaskGraph uses temporal revisioned edges rather than full graph copies.
19. Binding, Candidate, Artifact, GoalRevision, TaskSpec, and Run strategy records are immutable.
20. Database constraints enforce one live Run per Task and one live Attempt per Run.
21. Budget authority combines mutable aggregate counters with immutable accounting records.
22. Usage idempotency is backed by database uniqueness constraints.
23. CAS bytes become durable before SQLite may reference them.
24. Dangerous external executor/Git/workspace side effects use durable-DB-intent-first reconciliation instead.
25. Journal sequence is explicit per JournalEpoch; do not depend on AUTOINCREMENT semantics.
26. Mutating public commands have durable command IDs and request hashes.
27. Every mutable status row uses revision/CAS in addition to serialized physical writes.
28. Cross-subsystem commits are exposed as named transaction APIs.
29. Migrations are checksummed, immutable, transactional, and run before controllers/scheduler start.
30. Backups use SQLite-supported consistent snapshot APIs.
31. Follow current `PRAGMA optimize` guidance and profile actual controller queries.
32. Indexes are query-driven rather than speculative.
33. A deterministic PersistenceInvariantChecker covers cross-domain invariants SQL cannot express.
34. Every critical transaction/external-side-effect boundary receives crash-injection testing.
