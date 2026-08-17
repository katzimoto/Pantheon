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

The database cannot by itself prove that it has been replaced by an older valid snapshot, because the evidence needed to make that distinction may have been rewound together with the file. Supported disaster restore therefore establishes restore mode through the crash-safe installation maintenance latch defined below before replacing authoritative SQLite state. Raw/manual database replacement outside that procedure is not a supported safe restore path.

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
ContextSourceSnapshot immutable source-eligibility manifest
ContextPlan           immutable selected semantic context
Run spec/snapshot     immutable
Attempt identity      immutable
PlanningOperation intent/provenance
PlanningRecord        immutable normalized planning result
Candidate             immutable
GoalCompletionCandidate immutable
Evidence              immutable
ConfigurationRevision immutable
EvaluatorVersion      immutable

Goal/Task/Run/Attempt/Planning/Evaluation/Sandbox/etc. status
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
  planning_operations
  planning_attempts
  planning_records

SCHEDULING / EXECUTION
  scheduler_state
  goal_scheduling_state
  task_scheduling_state
  scheduling_claims
  agent_resolutions
  execution_requests
  route_decisions
  execution_bindings
  context_source_snapshots
  context_plans
  run_context_plans
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
  recovery_passes
  failure_records
  recovery_decisions
  recovery_counters
  recovery_findings
  finalization_obligations
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

`goals` stores current revision pointer, phase, terminal target/status revision and current completion-candidate ref. `goal_revisions` is immutable `(goal_id, revision)` content and contains/refs the immutable Goal acceptance contract, including exact pinned EvaluatorVersions plus the ConfigurationRevision/evaluator-registry digest used to resolve them when that revision became authoritative.

`goal_completion_candidates` is immutable/content-addressed and binds the exact Goal/Graph revision, required deliverable bindings, producer Candidate digests and the already-pinned Goal acceptance-contract identity/provenance. It never causes evaluator refs to be re-resolved.

`tasks` stores current phase/status revision/current responsible Run pointer **and the durable selected terminal target whenever finalization has begun**. `task_specs` is immutable and likewise carries the Task acceptance contract with exact EvaluatorVersion pinning performed at Task materialization.

Conceptually the Task lifecycle fields include:

```text
tasks
  id
  phase
  terminal_target          nullable before Finalizing; Succeeded|Failed|Cancelled|Superseded
  terminal_reason_json     nullable bounded structured detail
  status_revision
  active_run_id
  ...
```

The terminal target is authoritative lifecycle intent, not Event-derived narration. V1 keeps it through terminalization so the committed terminal phase and the previously selected target remain auditable and cannot contradict one another.

Row-local terminal-intent coherence is schema-enforced:

```sql
CHECK (
  (phase IN ('Pending', 'Ready', 'Active', 'Waiting', 'Evaluating')
   AND terminal_target IS NULL)
  OR
  (phase = 'Finalizing'
   AND terminal_target IN ('Succeeded', 'Failed', 'Cancelled', 'Superseded'))
  OR
  (phase IN ('Succeeded', 'Failed', 'Cancelled', 'Superseded')
   AND terminal_target = phase)
)
```

Therefore a crash cannot leave a valid `Finalizing` Task whose intended terminal outcome must be guessed, and a terminal Task cannot retain a contradictory stale target.

Key invariant:

```text
Task Active => exactly one nonterminal Run
Task Ready|Waiting => zero nonterminal Runs
```

Row-local Task pointer consistency is enforced declaratively where SQLite can express it:

```sql
CHECK (
  phase NOT IN ('Ready', 'Waiting')
  OR active_run_id IS NULL
)

CHECK (
  phase != 'Active'
  OR active_run_id IS NOT NULL
)
```

These `CHECK`s prove only that the Task row's responsibility pointer is locally consistent with its phase. They do **not** prove that another nonterminal Run row does or does not exist.

V1 makes the one-live-Run rule structurally enforceable by putting immutable Task ownership beside mutable Run terminality. `runs` retains canonical ownership and exposes a composite parent key; `run_status` carries an immutable copy of that ownership:

```text
runs
  id
  task_id
  ...
  UNIQUE (id, task_id)

run_status
  run_id
  task_id          immutable copy of runs.task_id
  phase
  ...
```

The copied holder cannot drift from the immutable Run:

```sql
FOREIGN KEY (run_id, task_id)
  REFERENCES runs(id, task_id)
```

The Task's current responsible-Run pointer is also holder-safe:

```sql
FOREIGN KEY (active_run_id, id)
  REFERENCES runs(id, task_id)
```

When `active_run_id` is non-NULL, that Run therefore belongs to the same Task. Pointer/currentness semantics still belong to Task lifecycle transactions; the FK proves holder identity only.

Because `run_status` now contains both `task_id` and `phase`, v1 can enforce at most one nonterminal Run per Task with a real partial unique index:

```sql
CREATE UNIQUE INDEX one_nonterminal_run_per_task
ON run_status(task_id)
WHERE phase IN ('Active', 'Finalizing');
```

`Active` and `Finalizing` are exactly the nonterminal Run phases. This intentionally keeps a producer Run occupying the live slot while its Task is already `Evaluating` and the Run is still `Finalizing`; a replacement Run cannot commit until that prior Run reaches a terminal phase. Exact Task-phase cardinality (`Active => exactly one`, `Ready|Waiting => zero`) remains controller/invariant-checker logic layered over this relational maximum-one guarantee.

## Temporal TaskGraph

Graph mutations are revisioned. Dependencies may be stored as temporal edges:

```text
created_graph_revision
removed_graph_revision NULL
```

Edge is active at revision R when created <= R and removed is null or > R. Active-edge uniqueness is enforced where possible; cycle validation remains controller transaction logic.

## Durable scheduler state

Scheduler queue structures are process-local caches. The policy/ordering state needed to reproduce scheduler behavior after restart is relational controller state.

V1 stores one installation-wide row:

```text
scheduler_state
  singleton PK
  dispatch_mode              RUNNING|PAUSED
  next_service_sequence      integer >= 1
  revision
  updated_at
```

`dispatch_mode` is operator desired state only. It is not rewritten to represent startup recovery, configuration publication, maintenance or resource availability. Effective permission for T3 is the intersection of `dispatch_mode=RUNNING` with those independent current gates.

`next_service_sequence` is a logical fairness counter. It advances only in a successful T3 transaction and is not a wall-clock timestamp.

Per-Goal scheduler state is concrete relational ownership:

```text
goal_scheduling_state
  goal_id PK/FK -> goals
  base_priority_class        foreground|normal|background
  last_served_sequence       nullable
  revision
  updated_at
```

A NULL `last_served_sequence` means the Goal has never successfully received a Run-intent service charge. Sequence values are compared only within the current durable database history; they are not command/idempotency identities and do not participate in RestoreGeneration.

Per-Task scheduler state is likewise concrete:

```text
task_scheduling_state
  task_id PK/FK -> tasks
  eligible_since             nullable timestamp
  next_attempt_at            nullable timestamp
  last_failure_code          nullable
  last_failure_detail_json   nullable bounded structured detail
  revision
  updated_at
```

`task_conditions`/controller eligibility logic remains authoritative for whether `SchedulingEligible` is currently true. `task_scheduling_state.eligible_since` records the start of the current true interval and is changed only with the corresponding eligibility transition/reconciliation:

```text
False -> True
  eligible_since = now

True remains True across temporary scheduler failures
  preserve eligible_since

True -> False
  eligible_since = NULL

later False -> True
  set a new eligible_since
```

`next_attempt_at` is a scheduler backoff suppression point. A non-NULL future value does not make the Task semantically ineligible and does not reset its eligible waiting age. Structured failure/backoff state is CAS-updated; Event delivery is only a wakeup/audit mechanism.

Cross-row scheduler invariants include:

```text
SchedulingEligible == True  => eligible_since IS NOT NULL
SchedulingEligible == False => eligible_since IS NULL
last_served_sequence, when present, < scheduler_state.next_service_sequence
base_priority_class is a valid configured v1 class
```

These cross-row facts are controller/PersistenceInvariantChecker responsibilities where they cannot be encoded as one row-local CHECK.

Dispatch pause/resume is a normal authoritative Operator command transaction over `scheduler_state.revision` plus Event/Command state. Ordinary daemon restart loads the row; it never defaults a durable `PAUSED` installation back to RUNNING.

## SchedulingClaim

One current SchedulingClaim per Task is sufficient; history belongs to Events. It binds Task/Goal/Graph/config revisions and expiry/incarnation.

Claims are not fairness charges. A failed/expired/released claim does not advance Goal service sequence. T3 consumes the successful claim and charges fairness atomically with Run creation.

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

Control-operation reservations likewise retain a concrete relational owner. In v1 accounted holders include EvaluationOperation and PlanningOperation; an implementation must not collapse them into one unconstrained opaque `holder_ref` merely because both share the accounting scope.

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
credentialBindingRegistryDigest
SandboxPlan digest
binding hash/canonical JSON
```

Do not use one ambiguous `policy_hash` field.

`credentialBindingRegistryDigest` must resolve to the immutable CredentialBindingRegistry component of the same `ConfigurationRevision` captured at T3. That registry contains logical credential-mapping authority only; it excludes SecretVersionId and secret material.

ContextPolicy is not hidden inside a generic ExecutionBinding policy field. Its canonical identity is `contextPolicyDigest` on the Run's ContextSourceSnapshot, because Context Builder semantics are frozen through that explicit source snapshot.

A later credential-bearing broker operation resolves its normalized action/resource against both the Run's frozen registry and the current active registry. V1 requires exact `credentialBindingAuthorityDigest` equality for the resolved binding, not equality of the whole registry digest. Therefore unrelated credential-binding changes do not invalidate the Run, while remapping/removing the exact binding fails closed.

## Context source snapshot and ContextPlan attachment

T3 freezes one immutable `ContextSourceSnapshot` for every Run before Context Builder performs selection/retrieval.

The source snapshot is a canonical/content-addressed manifest over exact source identities or stable reconstructable generations. Conceptually:

```text
context_source_snapshots
  digest PK
  config_revision_id              FK -> configuration_revisions
  context_policy_digest
  task_spec_digest/ref
  goal_revision
  graph_revision
  agent_snapshot_id               FK -> agent_snapshots
  starting_workspace_revision_id  nullable FK -> workspace_revisions
  continuation_context_id         nullable FK -> continuation_contexts
  memory_corpus_generation        nullable
  memory_index_generation         nullable
  source_manifest_json            canonical immutable detail
  created_at
```

The canonical manifest may contain additional exact Skill/Artifact/retriever/source-version identities. Any selection-affecting source must be named by an immutable version/digest or stable reconstructable generation identity. Raw secret material is forbidden.

ContextSourceSnapshot freezes **eligibility**, not selected Context. T3 does not run Memory retrieval, prompt rendering, model calls, arbitrary repository exploration, or other external context-building work.

The immutable Run row stores the exact source snapshot immediately:

```text
runs
  id
  task_id
  binding_id
  context_source_snapshot_digest  NOT NULL FK -> context_source_snapshots
  created_at
  UNIQUE (id, task_id)
  UNIQUE (id, context_source_snapshot_digest)
```

A later ContextPlan is also immutable/content-addressed:

```text
context_plans
  digest PK
  source_snapshot_digest          NOT NULL FK -> context_source_snapshots
  builder_version
  selected_manifest_json          canonical immutable selected refs/digests/provenance
  created_at
  UNIQUE (digest, source_snapshot_digest)
```

The Run row is **not** updated to insert the plan after preparation. Instead v1 attaches at most one plan through:

```text
run_context_plans
  run_id                          PK
  context_source_snapshot_digest NOT NULL
  context_plan_digest             NOT NULL
  attached_at

FOREIGN KEY (run_id, context_source_snapshot_digest)
  REFERENCES runs(id, context_source_snapshot_digest)

FOREIGN KEY (context_plan_digest, context_source_snapshot_digest)
  REFERENCES context_plans(digest, source_snapshot_digest)
```

Those composite FKs prove that the attached ContextPlan was built from the exact ContextSourceSnapshot bound to that Run. `run_id` as the primary key enforces at most one attachment. The same content-addressed plan may be referenced by another Run only when the complete source snapshot and selected semantic bytes/provenance are identical; reverse uniqueness is not required.

ContextPlan construction may be retried after crash, but every retry reads the same immutable source snapshot. If a referenced source generation is unavailable, preparation fails/reconciles; Pantheon never substitutes a newer generation into the existing Run.

A ContextPlan may be durably inserted before its one-time attachment transaction; an unattached immutable plan is harmless historical/orphan content. Attachment itself is authoritative Run preparation state and must be idempotent: same Run + same source + same plan succeeds/reconciles; an attempt to attach a different plan to a Run that already has one fails closed.

An Attempt may be created only after the Run has exactly one valid `run_context_plans` attachment.

## Run status

Conceptually:

```text
runs (immutable)
  id
  task_id
  binding_id
  context_source_snapshot_digest
  created_at
  UNIQUE (id, task_id)      # composite parent key for holder-safe FKs
  UNIQUE (id, context_source_snapshot_digest)

run_status (mutable lifecycle + immutable holder copy)
  run_id PK
  task_id                   immutable copy of runs.task_id
  phase                    Active|Finalizing|Completed|Failed|Cancelled|Yielded
  terminal_target          nullable while Active; required while Finalizing
  desired_execution
  revision
  candidate_digest
  current_attempt_id       nullable; when set must belong to this run_id
  control_epoch
  lease_token
  lease_holder/incarnation
  lease_valid_until
  updated_at
```

`run_status` holder identity is constrained by:

```sql
FOREIGN KEY (run_id, task_id)
  REFERENCES runs(id, task_id)
```

and `tasks.active_run_id`, when present, is constrained to a Run owned by that same Task through the composite `(active_run_id, task.id) -> runs(id, task_id)` relationship described above.

Persistence invariant is:

```text
Run Completed => candidate_digest not null
Run Yielded|Failed|Cancelled => candidate may be null
Run Finalizing => terminal_target not null
```

Because both required facts live on `run_status`, v1 encodes them as row-local schema constraints as well as controller invariants:

```sql
CHECK (
  phase != 'Finalizing'
  OR terminal_target IS NOT NULL
)

CHECK (
  phase != 'Completed'
  OR candidate_digest IS NOT NULL
)
```

These constraints reject impossible Run status rows even if a controller bug or migration tries to persist them; normal lifecycle transactions still own when and why a Run transitions.

The live-Run index is also defined on `run_status`, where both immutable Task ownership and nonterminal phase are available:

```sql
CREATE UNIQUE INDEX one_nonterminal_run_per_task
ON run_status(task_id)
WHERE phase IN ('Active', 'Finalizing');
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
  UNIQUE (id, run_id)       # composite parent key for holder-safe FKs
```

Mutable Attempt status records external observation plus durable launch-call boundary. `run_id` is deliberately duplicated as immutable relational identity so the live-Attempt cardinality invariant can be enforced on the status table where terminality lives:

```text
attempt_status
  attempt_id PK
  run_id                    immutable copy of attempts.run_id
  observed_execution
  terminal                  NOT NULL boolean/integer
  revision
  launch_contact_state      NOT_CONTACTED|CONTACT_MAY_HAVE_OCCURRED
  launch_contact_initiated_at
  launch_contact_epoch/incarnation
  started_at
  finished_at
  termination_json
```

The duplicated holder key is not independently mutable. It is constrained back to the immutable Attempt identity:

```sql
FOREIGN KEY (attempt_id, run_id)
  REFERENCES attempts(id, run_id)
```

`run_status.current_attempt_id` is likewise holder-safe rather than a bare cross-Run pointer:

```sql
FOREIGN KEY (current_attempt_id, run_id)
  REFERENCES attempts(id, run_id)
```

Because `attempt_status` now contains both the immutable Run holder and terminality, v1 can enforce the one-live-Attempt rule directly:

```sql
CREATE UNIQUE INDEX one_nonterminal_attempt_per_run
ON attempt_status(run_id)
WHERE terminal = 0;
```

Every Attempt is created with exactly one `attempt_status` row in the same T4/T8 transaction, initially nonterminal. That transaction also updates `run_status.current_attempt_id` when the new Attempt becomes current. A replacement Attempt cannot commit until the previous status row is terminal, so controller serialization and the partial unique index agree on the same safety boundary rather than relying on a cross-table predicate SQLite cannot express.

Attempt creation + LaunchKey + AgentControlSession occurs before backend side effect. T4a may rotate only the AgentControlSession credential verifier/revision while launch contact remains definitively `NOT_CONTACTED`. T4b then marks `CONTACT_MAY_HAVE_OCCURRED` immediately before the first external launch call and freezes the session credential revision for that Attempt.

Crash semantics on an ordinary uninterrupted database history are:

```text
NOT_CONTACTED
  + no other external evidence -> Pantheon can know its launch path never crossed the call boundary

CONTACT_MAY_HAVE_OCCURRED
  -> lost acknowledgement is UNKNOWN until backend/outer supervisor proves state
```

A restored snapshot is different. A restored `NOT_CONTACTED`, `ABSENT`, missing row, or other negative observation is only a fact about the snapshot point; it cannot prove that no external effect happened after the snapshot. During restore recovery, fresh domain inspection/inventory/fencing must establish current negative certainty before replacement execution or new conflicting authority is permitted.

## Agent Control

`agent_control_sessions` stores one Attempt-scoped identity/session plus immutable `restore_generation`; raw bearer material is not persisted.

Conceptually the safety-relevant fields include:

```text
agent_control_sessions
  id
  attempt_id UNIQUE
  restore_generation       immutable
  credential_revision      integer >= 1
  credential_hash
  credential_rekeyed_at    nullable
  state
  created_at
  revoked_at
  revocation_reason
```

The session copies the current `system_state.restore_generation` when T4/T8 creates it and starts with `credential_revision = 1`. A consequential Agent Control request first authenticates against the **current** `credential_hash` and then requires:

```text
session.restore_generation == system_state.restore_generation
```

before Pantheon looks up or creates the request-idempotency row or applies semantic worker authority. An old-generation restored session remains historical/fenced even if its persisted state says `ACTIVE`; it is never rewritten to current merely because the Attempt still exists externally.

### T4a pre-contact Agent Control rekey

The raw Agent Control bearer is intentionally not persisted. If Pantheon restarts after T4/T8 but before T4b, the same Attempt may remain durably `NOT_CONTACTED` while the original bearer has been lost with process memory.

T4a permits recovery without creating a second Attempt or persisting bearer material. In one authoritative transaction it must re-read and require:

```text
AgentControlSession.state == ACTIVE
AgentControlSession.restore_generation == current RestoreGeneration
Attempt is current/nonterminal
Attempt.launch_contact_state == NOT_CONTACTED
no independent launch-capable external-contact evidence
current Run/ControlLease authority valid
expected credential_revision still current
```

Pantheon generates a fresh bearer outside durable storage, then T4a atomically:

```text
credential_revision += 1
credential_hash = verifier(new bearer)
credential_rekeyed_at = now
append non-secret agent-control.session.rekeyed Event
```

The Event contains identity/revision/reason provenance, never the bearer or verifier. Any previously prepared sandbox-local credential file, adapter bootstrap object, inherited-descriptor plan, or equivalent credential projection is invalidated and must be rebuilt from the new bearer before T4b.

The contact boundary freezes the credential:

```text
NOT_CONTACTED
  -> T4a may replace verifier/revision under the stated preconditions

CONTACT_MAY_HAVE_OCCURRED
  -> credential_revision and credential_hash are immutable for that Attempt
```

This freeze is cross-row lifecycle logic and is not expressible as one row-local SQLite CHECK because `launch_contact_state` belongs to `attempt_status`. Controller serialization/CAS, the T4b precondition, invariant scanning/audit provenance, and crash/fault-injection tests enforce it.

If contact may have occurred and the external lineage later proves absent/terminal, Recovery Policy creates a fresh Attempt/LaunchKey/AgentControlSession for fresh execution. It does not rekey the contacted Attempt merely because the old raw bearer is unavailable.

T4a never changes `restore_generation`; an old-generation session after disaster restore cannot be promoted into current authority by rekeying.

`agent_requests` enforces for current-generation sessions:

```text
PRIMARY/UNIQUE (attempt_id, request_id)
```

plus request hash/operation/state/result/problem refs. Same ID+same hash is idempotent; same ID+different hash fails closed. RestoreGeneration need not be duplicated into every request key because stale-generation sessions fail before request lookup/creation. Credential revision likewise does not enter request identity because a legitimate worker request can occur only after T4b, when the session revision is frozen.

## Sandbox

SandboxInstance has immutable SandboxKey/Plan identity and immutable relational holder ownership. Mutable controller lifecycle and mutable external existence observation live together in `sandbox_status` but remain separate facts. V1 Sandbox holders are exactly:

```text
RUN
CONTROL_OPERATION   # currently EvaluationOperation
```

The immutable ownership shape is equivalent to:

```text
sandbox_instances
  id
  sandbox_key UNIQUE
  holder_kind               RUN|CONTROL_OPERATION
  run_id                    nullable FK -> runs
  evaluation_operation_id   nullable FK -> evaluation_operations
  sandbox_plan_digest
  ...
```

with a CHECK/XOR constraint requiring exactly one concrete holder FK and requiring it to match `holder_kind`:

```text
holder_kind = RUN
  => run_id IS NOT NULL
     AND evaluation_operation_id IS NULL

holder_kind = CONTROL_OPERATION
  => run_id IS NULL
     AND evaluation_operation_id IS NOT NULL
```

Pantheon does not use one opaque polymorphic `holder_ref` as the safety boundary because SQLite could not enforce a real FK to multiple unrelated tables. In v1 the only control-operation Sandbox owner is EvaluationOperation; PlanningOperation does **not** gain Sandbox ownership merely because it is a control operation. Another control-operation type gains an explicit relational edge only when/if its architecture actually requires Sandbox ownership rather than forcing a premature generic supertable.

Provisioning intent and SandboxKey are committed before external runtime calls. The holder cannot be rewritten to another Run/EvaluationOperation after creation.

The mutable shape is explicit:

```text
sandbox_status
  sandbox_id PK/FK -> sandbox_instances
  phase                      REQUESTED|PREPARING|READY|RELEASING|RELEASED|ERROR
  observed_presence          PRESENT|ABSENT|UNKNOWN
  revision
  observed_at
  observation_provenance
  updated_at
```

`phase` is controller lifecycle. `observed_presence` is factual external existence certainty. `UNKNOWN` is not a lifecycle phase, and `ERROR` does not imply `ABSENT`.

Normal release requires:

```text
phase = RELEASING
+ observed_presence = ABSENT
→ phase = RELEASED
```

`phase=RELEASED` with `observed_presence=PRESENT` is invalid. `phase=RELEASED` with `observed_presence=UNKNOWN` is permitted only for an explicit audited force-resolution lineage that has a matching durable `external_lineage_tombstone`/fence; the factual observation remains UNKNOWN rather than being rewritten to ABSENT.

V1 requires at most one current/replacement-authoritative SandboxInstance per Run holder and per EvaluationOperation holder. Ordinarily any Sandbox whose phase is not `RELEASED`, or whose presence is unresolved, occupies that holder slot. A force-resolved `RELEASED+UNKNOWN` lineage is excluded only because its durable tombstone/fence prevents the old SandboxKey from regaining authority. A bare lifecycle rewrite never frees the slot.

Because mutable Sandbox lifecycle/presence is kept in `sandbox_status`, the architecture does not prescribe a fictitious cross-table partial index here: the Sandbox desired-state transaction serializes and rechecks holder occupancy, observation certainty and any tombstone exception, and PersistenceInvariantChecker verifies them. Exact DDL may denormalize an active-holder key or use another relational technique if implementation needs declarative uniqueness without collapsing immutable identity and mutable status.

SandboxVerification records factual verification of expected SandboxKey, immutable holder identity, environment identity, mounts/materialization, network, privilege controls, Agent Control exposure where applicable, and limits before `SandboxReady=True`. A normal Attempt requires its Run Sandbox verification; an externally executing EvaluationAttempt requires its owning EvaluationOperation verification Sandbox to be READY first.

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

Backend-authored control-operation usage validates the symmetric immutable provenance on the referenced concrete control-operation record. Any control operation that can accept such usage must freeze, before external contact, relational immutable fields equivalent to:

```text
usage_reporter_backend_id
usage_reporter_backend_revision
metering_contract_digest
```

The fields are absent together for an operation that does not accept backend-authored usage and complete together for one that does. The reporting backend cannot create or rewrite this ownership. For EvaluationOperations and externally metered PlanningOperations these fields belong to the immutable operation intent; they do not create an ExecutionBinding or transfer lifecycle ownership from the corresponding controller.

Usage ingestion rejects a control-operation record when `backend_id` does not equal the operation's frozen `usage_reporter_backend_id`, when the meter/units are outside the frozen contract, or when no external metering-source binding exists.

Current terminal/running state is not an ownership predicate: delayed otherwise-valid factual usage may arrive after terminalization or administrative resolution. Where a separately durable launch/contact marker proves that the external lineage was never contacted, that evidence may reject impossible usage; the persistence of that launch boundary is a distinct execution-reconciliation invariant.

In restore mode, a negative launch/contact fact recovered only from the restored snapshot is not by itself proof about the post-snapshot external history. Usage/execution reconciliation first applies the restore-specific external-domain certainty rule before treating such a negative fact as current proof.

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

For a credential-bearing Run operation, the same transaction additionally resolves the normalized action/resource through the Run's immutable `ExecutionBinding.credentialBindingRegistryDigest` and through the current active CredentialBindingRegistry, requires both exact bindings to exist, and requires equality of their canonical `credentialBindingAuthorityDigest` before any secret material is retrieved.

The durable broker operation carries non-secret provenance equivalent to:

```text
broker_operations
  ...
  run_id                                  nullable where operation is Run-owned/triggered
  frozen_credential_binding_registry_digest  nullable
  current_config_revision_id              nullable
  current_credential_binding_registry_digest nullable
  credential_binding_authority_digest     nullable
  secret_descriptor_id / logical SecretRef nullable
  secret_version_id_used                  nullable factual result/provenance
```

The credential-binding fields are required together for a credential-bearing Run operation and absent together when the operation does not use CredentialBinding authority. `credential_binding_authority_digest` covers logical action/resource scope, SecretRef, broker mechanism class and credential-use constraints; it excludes `secret_version_id_used` and secret bytes. `secret_version_id_used` may be populated only as factual material-use provenance after the broker resolves the current usable SecretDescriptor/version; changing it never broadens the broker operation's authority.

The broker operation is the durable redemption result. A retry/reconciliation of that operation follows its original exact binding authority and external idempotency identity; it never re-resolves the operation into a different credential authority merely because current configuration changed later.

Capability tickets, if represented, are single-use/short-lived references and are revalidated at redemption, including `ticket.restore_generation == current restore_generation`; issuance alone is not durable bearer authority. Credential-bearing ticket redemption performs the same frozen/current exact-binding compatibility check before broker-operation creation.

After disaster restore, rows whose generation differs from current are not deleted or rewritten to current. Old-generation Grants/Tickets are non-redeemable historical authority. If an operator re-affirms the permission, a new Grant is created under the current generation.

Old-generation `broker_operations` are **reconciliation-only**. Their restored state may be compared with external reality using the original operation/idempotency identity and frozen credential-binding provenance, but no controller may issue/reissue the external effect from that row merely because it appears `PENDING`, incomplete, or absent from later history. If the outcome cannot be established, the operation/domain remains UNKNOWN/fenced until explicit recovery resolution.

## Explicit finalization obligations

`finalization_obligations` is a narrow residual mechanism, not a mirror of every finalization predicate.

Most cleanup/finalization facts are already authoritative in their owning tables and are recomputed from those rows after restart: Run/Attempt status, Sandbox phase/presence/tombstone, ResourceReservation, BudgetHold/Usage, WorkspaceRevision, Artifact retention and IntegrationIntent. Pantheon does not duplicate those facts into `finalization_obligations` merely to create a second source of truth.

An explicit row is created only when a required finalization action has its own retry/uncertainty state and no other owning domain row already represents that action. V1 uses concrete relational owner edges rather than an unconstrained `subject_kind + subject_id` pair. Conceptually:

```text
finalization_obligations
  id
  owner_kind                  TASK|GOAL|RUN|WORKSPACE
  task_id                     nullable FK -> tasks
  goal_id                     nullable FK -> goals
  run_id                      nullable FK -> runs
  workspace_id                nullable FK -> workspaces
  key                         stable owner-local obligation key
  state                       PENDING|SATISFIED|UNCERTAIN
  operation_key               nullable stable external/idempotency correlation
  revision
  detail_json                 bounded non-authoritative detail/provenance
  created_at
  updated_at
```

A CHECK/XOR constraint requires exactly one concrete owner FK and requires it to match `owner_kind`. V1 adds another owner kind only when a concrete finalization action actually needs independent durable state; control-operation, Sandbox, Reservation, Budget and Integration lifecycles are not pushed into this table merely because they can participate in finalization.

Within one owner, `key` identifies one logical explicit obligation and is unique for that owner. State transitions are controller-owned/CAS-checked. `UNCERTAIN` preserves ambiguity; it is never rewritten to `SATISFIED` merely because a timeout elapsed.

Task finalization therefore evaluates two layers:

```text
durable task.terminal_target
+
required predicates from authoritative owning-domain rows
+
explicit finalization_obligations for residual independent actions
→ terminal transition permitted only when safe
```

A Task entering `Finalizing` writes its `terminal_target` in the same authoritative lifecycle transaction. Explicit obligation rows that are known at that transition may be inserted in the same transaction; obligations discovered deterministically later must still be created before their external action occurs. Events never substitute for either terminal intent or explicit obligation state.

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

A tombstoned LaunchKey/session/control-operation attempt can never regain current control authority. The same rule applies to a force-resolved SandboxKey: a `RELEASED+UNKNOWN` Sandbox may be considered closed only when its matching durable tombstone/fence prevents that uncertain external runtime from regaining Pantheon authority or being mistaken for the replacement Sandbox. Late observations may be recorded as history/usage but cannot mutate the current execution lineage.

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

Git-backed code changeset objects need the Git-specific preservation contract in `docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md`: authoritative Git objects must be pinned/preserved before the SQLite Artifact/Candidate reference is committed, or the changeset payload must be independently present in Pantheon CAS.

## Candidate submission transaction (T6)

Candidate submission is cancellation/supersession-race-safe:

```text
[required CAS/Git payload already durable/pinned]

BEGIN IMMEDIATE

re-read/validate:
  AgentControlSession current generation + state
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

If cancellation/supersession won the status CAS first, T6 fails with stale/conflict and creates no current Candidate. A restored old-generation AgentControlSession fails before T6 request authority is established. Because `Finalizing` remains in the live-Run partial index, this producer Run continues to occupy the Task's unique nonterminal Run slot while acceptance proceeds.

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

The prior Run is already outside the `Active|Finalizing` live-Run index before T9 makes the Task schedulable again.

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

Historical hash-bearing config rows are never rewritten in place by migration. The immutable `credentialBindings` ConfigurationRevision component is stored in `configuration_components` like other compiled components and is addressed by `credentialBindingRegistryDigest`; it contains no SecretVersion/material state.

## Planning

Every authoritative planning invocation creates one durable `PlanningOperation`. A purely local/deterministic planning path may complete without any external attempt, but an external Planner/backend call uses `PlanningAttempt` contact state before crossing that boundary.

Conceptual operation fields include immutable decision/execution provenance plus revisioned lifecycle state:

```text
planning_operations
  id
  goal_id
  goal_revision
  expected_graph_revision
  trigger_kind / reconciliation_ref
  planning_input_digest
  planner_agent_snapshot/version nullable
  config_revision_id
  executor_backend_id nullable
  executor_backend_revision nullable
  metering_contract_digest nullable
  state
  revision
  created_at
  finished_at
```

The external backend/metering fields are absent together for local planning and complete as required for an externally metered Planner path. They are frozen before the first PlanningAttempt contact and are never selected/rewritten by the Planner response.

External attempt lineage is relational:

```text
planning_attempts
  id
  operation_id FK -> planning_operations
  ordinal
  state
  contact_state                    NOT_CONTACTED|CONTACT_MAY_HAVE_OCCURRED
  contact_initiated_at             nullable until contact transition
  contact_daemon_incarnation       nullable until contact transition
  external_attachment_json         nullable adapter-private correlation
  created_at
  finished_at
```

At most one nonterminal PlanningAttempt may exist per PlanningOperation. Because operation ownership and attempt lifecycle state are both present on `planning_attempts`, v1 can use a real partial unique index/constraint over `operation_id` for the nonterminal state domain rather than relying solely on controller convention.

`PlanningAttempt.id` is the provider-neutral reconciliation/correlation identity. Adapter-private IDs may supplement it but do not replace it.

`planning_records` stores immutable normalized results/proposals and binds at least:

```text
planning_operation_id
planning_attempt_id nullable for local planning
proposal_digest / canonical proposal
parse/normalization provenance
created_at
```

A PlanningRecord is not lifecycle authority and is never proof that its proposal was materialized. Graph Controller separately rechecks GoalRevision/GraphRevision/current policy before GraphPatch commit.

PlanningOperation may own concrete control-operation ResourceReservations/BudgetHolds. It does not own a normal Run/ExecutionBinding, AgentControlSession or v1 Sandbox. A future Planner Sandbox requires an explicit concrete `sandbox_instances` holder FK/architecture change rather than piggybacking on the generic control-operation accounting scope.

Crash/contact semantics on uninterrupted history are:

```text
NOT_CONTACTED + no independent external evidence
  -> Pantheon's external Planner call boundary was not crossed

CONTACT_MAY_HAVE_OCCURRED
  -> external result/usage may exist
  -> reconcile same PlanningAttempt identity
  -> no overlapping attempt while unresolved
```

Restore-mode negative evidence uses the same restore-specific rule as other external domains: a restored `NOT_CONTACTED` row is snapshot evidence only until fresh inspection/fencing establishes the post-snapshot interval.

## Evaluation

EvaluationRound is an immutable judgment context over exactly one concrete immutable subject. V1 subject types are:

```text
TASK_CANDIDATE
GOAL_COMPLETION_CANDIDATE
```

The relational shape is equivalent to:

```text
evaluation_rounds
  id
  subject_kind                         TASK_CANDIDATE|GOAL_COMPLETION_CANDIDATE
  task_candidate_digest                nullable FK -> candidates
  goal_completion_candidate_digest     nullable FK -> goal_completion_candidates
  acceptance_hash
  config_revision_id
  evaluator_registry_digest
  state
  created_at
```

The subject relationship is not an opaque polymorphic string. A row-local CHECK plus concrete FKs enforce exactly one subject:

```sql
CHECK (
  (subject_kind = 'TASK_CANDIDATE'
   AND task_candidate_digest IS NOT NULL
   AND goal_completion_candidate_digest IS NULL)
  OR
  (subject_kind = 'GOAL_COMPLETION_CANDIDATE'
   AND task_candidate_digest IS NULL
   AND goal_completion_candidate_digest IS NOT NULL)
)
```

Task/Goal lifecycle ownership is derived through that concrete immutable subject. `evaluation_rounds` does not need a generic `task_id` that is meaningless for Goal acceptance.

The Round freezes the acceptance hash, exact criterion/EvaluatorVersion rows, and evaluator-resolution provenance copied from the owning immutable semantic contract:

```text
TASK_CANDIDATE
  -> TaskSpec acceptance contract pinned at Task materialization

GOAL_COMPLETION_CANDIDATE
  -> GoalRevision acceptance contract pinned at GoalRevision commit
```

Round creation never resolves logical evaluator refs against current registry state. `config_revision_id`/`evaluator_registry_digest` are reproducibility provenance only; current hard/current authorization policy is still checked when EvaluationOperations execute.

`evaluation_round_evaluators` binds each criterion to the exact pinned `EvaluatorVersion`.

External deterministic checks use `EvaluationOperation` with control-operation ResourceReservations/BudgetHolds where required; EvaluationAttempts are small execution/reconciliation identities, not Runs. EvaluationOperation binds the Round/criterion/evaluator version; it derives the exact immutable subject from the Round rather than carrying an independently mutable/redundant Candidate field.

A billable EvaluationOperation that accepts backend-authored factual usage carries immutable operation-intent fields equivalent to `usage_reporter_backend_id`, `usage_reporter_backend_revision`, and `metering_contract_digest`, frozen before external contact. These fields are nullable only as an all-or-none group for operations with no backend-authored metering and are never mutable lifecycle status.

Each externally executing EvaluationAttempt carries its own durable launch-contact state:

```text
evaluation_attempts
  id
  operation_id
  ordinal
  state
  launch_contact_state                 NOT_CONTACTED|CONTACT_MAY_HAVE_OCCURRED
  launch_contact_initiated_at          nullable until contact transition
  launch_contact_daemon_incarnation    nullable until contact transition
```

Creation initializes `launch_contact_state = NOT_CONTACTED`. Immediately before the first external evaluator/process/remote-check call for that EvaluationAttempt, T15 durably changes it to `CONTACT_MAY_HAVE_OCCURRED` and records timestamp/incarnation. The transition is monotonic and never resets.

`EvaluationAttempt.id` is the stable provider-neutral reconciliation identity. External helpers/backends may bind that identity to native state/attachments where supported, but absence of native keyed idempotency leaves ambiguous contact UNKNOWN rather than authorizing a new attempt.

At most one nonterminal EvaluationAttempt may exist per EvaluationOperation. Because attempt lifecycle state is relational on `evaluation_attempts`, v1 enforces this with a partial unique index/constraint over `operation_id` for the nonterminal state domain. A new EvaluationAttempt is permitted only after the prior one is definitively terminal/absent under bounded evaluation retry policy.

`human_evaluation_requests` binds `round_id + criterion_id` and obtains the exact subject from the Round; it does not duplicate an ambiguous Task/Goal subject reference.

`evidence` binds the exact `evaluation_round`, criterion, EvaluatorVersion, provenance and verdict. Any self-contained copied subject identity must exactly match the Round's concrete FK. Evidence from a Task Candidate can never satisfy a GoalCompletionCandidate Round or vice versa merely because IDs/criterion text resemble one another.

`acceptance_results` aggregate required criterion Evidence for one exact Round. Applying that result to lifecycle is owned by the concrete subject's controller:

```text
TASK_CANDIDATE
  -> Task/Acceptance Controller rechecks Task still Evaluating with this exact current Candidate

GOAL_COMPLETION_CANDIDATE
  -> Goal Completion Controller rechecks Goal still Evaluating with this exact current completion candidate and GoalRevision
```

For usage reconciliation on an uninterrupted history, an EvaluationOperation whose every attempt is durably `NOT_CONTACTED` and has no independent external-contact evidence cannot justify backend-authored usage. `CONTACT_MAY_HAVE_OCCURRED` permits only factual reconciliation under the frozen metering-source provenance; it does not prove that usage occurred. After disaster restore, restored negative contact state is historical snapshot evidence only until the external evaluation domain is freshly reconciled.

The EvaluationOperation, not an EvaluationAttempt, owns the verification Sandbox through `sandbox_instances.evaluation_operation_id`. This holder remains stable across bounded sequential EvaluationAttempts while that Sandbox remains valid.

## Secret metadata

SQLite stores only SecretDescriptor/provider locator/non-secret random version IDs/status/intents/lease metadata/use records. It never stores long-lived secret bytes or hashes of secret bytes.

CredentialBindingRegistry/component content is configuration authority, not secret material state. Broker/credential-use provenance may reference a logical SecretRef/SecretDescriptor and factual `SecretVersionId` used, but SecretVersionId never participates in `credentialBindingAuthorityDigest` or expands a Run's frozen authority.

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

Restore is not an ordinary startup, and restore mode cannot be inferred reliably from the restored SQLite file itself. V1 therefore supports disaster restore only through an explicit installation-maintenance procedure that establishes a crash-safe **out-of-database restore latch** before replacing authoritative SQLite state.

Conceptually the installation-local marker is:

```text
restore.pending
  restore_operation_id      fresh random/non-reused ID
  expected_installation_id
  backup_identity/digest
  created_at
```

The marker contains no authority, secret material, Grant, credential, or bearer token. It is a safety latch only. It lives outside `pantheon.db`, is excluded from database backup payloads, and is made durable together with its parent-directory metadata before the database replacement starts.

Supported high-level ordering is:

```text
acquire exclusive installation maintenance lock
        ↓
create + fsync restore.pending
        ↓
install the selected consistent SQLite snapshot
        ↓
open/validate installation identity + schema + integrity
while all authority/effect gates remain closed
        ↓
T0 restore fence
        ↓
clear restore.pending durably only after T0 is known committed
        ↓
restore-domain reconciliation
        ↓
recovery barrier
        ↓
normal authority/dispatch
```

T0 is one authoritative transaction:

```text
BEGIN IMMEDIATE

verify restore mode from restore.pending or matching incomplete restore RecoveryPass
verify restore_operation_id + installation identity
write fresh unpredictable system_state.restore_generation
rotate JournalEpoch as required by event-history semantics
create/update RecoveryPass:
  mode = restore
  restore_operation_id = marker ID
  prior_restored_generation = historical only
  new_restore_generation = freshly written generation
  state = IN_PROGRESS
record daemon-incarnation linkage
append restore-fence audit Event in the new journal epoch

COMMIT
```

The freshly generated RestoreGeneration must not be derived by incrementing a value from the restored snapshot; an old backup may contain a previously used number. Normal daemon restart never performs T0.

The restore-operation ID makes the latch restart-safe:

- crash before T0 -> `restore.pending` remains and the next daemon still knows ordinary startup is forbidden;
- crash after T0 but before latch removal -> the matching durable `RecoveryPass(mode=restore, restore_operation_id=...)` proves that this restore operation already crossed T0, so startup resumes that pass without rotating another generation merely because the marker remains;
- latch removal happens only after the matching T0 commit is established;
- a restored database with an incomplete matching restore RecoveryPass resumes restore recovery even if the external latch was already durably cleared after T0.

After T0:

- old-generation Grants and CapabilityTickets cannot redeem;
- old-generation broker operations are reconciliation-only;
- Operator commands carrying an old commandEpoch are rejected before command-row lookup/creation;
- old-generation AgentControlSessions cannot authorize semantic Agent requests before request-row lookup/creation;
- Run ControlLease tokens still rotate separately before Run/external commands;
- restored negative observations are not current proof of absence until the corresponding external domain is freshly inspected/reconciled;
- domain recovery reconciles/fences external state before normal dispatch resumes.

Pantheon cannot make arbitrary out-of-band replacement of `pantheon.db` safe by inspecting the replaced database after the fact. Raw/manual file replacement without first establishing the supported restore latch is therefore explicitly outside the safe disaster-restore contract and must not be presented as equivalent to normal restore recovery.

## Migrations / backup

Use `PRAGMA user_version` plus immutable checksummed `schema_migrations`. Unknown newer schema fails startup. Controllers/scheduler are disabled during migration. Back up through SQLite's supported online backup/snapshot mechanism, never by naively copying only `pantheon.db` while live WAL state exists.

Use `PRAGMA application_id` before declaring the DB format stable.

## Invariant checker

A deterministic PersistenceInvariantChecker verifies at least:

```text
Task phase/active_run_id row-local consistency
Task terminal_target coherence: non-finalizing nonterminal => NULL; Finalizing => present; terminal => matches phase
Task Finalizing completion only from safe authoritative domain predicates + resolved explicit finalization obligations
explicit finalization obligation owner-kind/FK XOR valid; state in PENDING|SATISFIED|UNCERTAIN
Task active_run_id belongs to the same Task through runs.task_id
run_status holder matches runs.task_id
one nonterminal Run per Task
Active Task -> exactly one nonterminal Run
Ready/Waiting Task -> zero nonterminal Runs
scheduler_state singleton exists; dispatch_mode in RUNNING|PAUSED; next_service_sequence valid
goal_scheduling_state Goal FK/priority valid; last_served_sequence < next_service_sequence when non-NULL
task_scheduling_state Task FK valid; SchedulingEligible true iff current eligible interval has eligible_since
temporary scheduler backoff does not reset eligible_since or mutate Task lifecycle
committed Run/T3 fairness charge exists atomically for its selected Goal service sequence
ExecutionBinding credentialBindingRegistryDigest resolves to the credentialBindings component of the same ConfigurationRevision
Run.context_source_snapshot_digest resolves to immutable ContextSourceSnapshot
ContextSourceSnapshot config_revision/context_policy binding is complete and component digest matches that ConfigurationRevision
at most one run_context_plans row per Run
RunContextPlan source snapshot matches both its Run and ContextPlan through composite FKs
Attempt exists/current -> parent Run has one valid ContextPlan attachment
GoalRevision acceptance criteria -> exact pinned EvaluatorVersions + evaluator-resolution provenance
GoalCompletionCandidate acceptance identity -> owning GoalRevision pinned contract
Finalizing Run -> terminal_target present
Completed Run -> Candidate exists
attempt_status holder matches attempts.run_id
run_status.current_attempt_id belongs to the same Run
one nonterminal Attempt per Run
one current AgentControlSession per Attempt
AgentControlSession credential_revision >= 1
AgentControlSession pre-contact rekey only while parent Attempt is current-generation NOT_CONTACTED
AgentControlSession credential revision/verifier frozen after CONTACT_MAY_HAVE_OCCURRED
accepted Agent Control request -> session.restore_generation == current RestoreGeneration
one nonterminal PlanningAttempt per PlanningOperation
PlanningAttempt contact state valid/monotonic; contact provenance present when CONTACT_MAY_HAVE_OCCURRED
PlanningRecord binds existing PlanningOperation and matching external PlanningAttempt when applicable
external PlanningOperation metering backend/contract frozen before PlanningAttempt contact
EvaluationRound subject_kind/FK XOR valid (Task Candidate xor GoalCompletionCandidate)
EvaluationRound acceptance/evaluator versions match owning immutable TaskSpec/GoalRevision contract
EvaluationRound currentness resolves through its concrete Task/Goal subject
Evidence subject/criterion/EvaluatorVersion matches its EvaluationRound
one nonterminal EvaluationAttempt per EvaluationOperation
EvaluationAttempt launch-contact state valid/monotonic; contact provenance present when CONTACT_MAY_HAVE_OCCURRED
one live Task-scoped reservation per singular (Task, ResourceKey)
Reservation holder validity, including concrete PlanningOperation/EvaluationOperation control-operation holder
Sandbox holder XOR/FK validity (Run xor EvaluationOperation)
Sandbox phase is in REQUESTED|PREPARING|READY|RELEASING|RELEASED|ERROR
Sandbox observed_presence is independently PRESENT|ABSENT|UNKNOWN
Sandbox RELEASED+PRESENT is invalid
Sandbox RELEASED+UNKNOWN requires a matching durable force-resolution tombstone/fence
at most one current/replacement-authoritative Sandbox per Run/EvaluationOperation holder
SandboxVerification holder/SandboxKey identity matches SandboxInstance
Budget aggregate == immutable ledger reconstruction
Usage provenance/backend ownership for Attempt and concrete control-operation subjects
Grant/CapabilityTicket redemption generation == current RestoreGeneration
credential-bearing executable broker operation binds Run frozen registry + exact authority digest + current compatible registry provenance
credential-bearing broker operation SecretVersionId, when present, is factual provenance and never binding authority
new/executable broker operation generation == current RestoreGeneration
old-generation broker operations are reconciliation-only
current Operator command epoch == current RestoreGeneration before command creation
restore RecoveryPass restore_operation_id/generation consistent with any surviving restore latch
Candidate outputs -> existing Artifacts/Blobs
Workspace/Sandbox ownership consistency
IntegrationIntent/Git state consistency
Event epoch/sequence sanity
```

Scheduler durable state is authoritative rather than Event-reconstructed. `scheduler_state.dispatch_mode` is desired state only; recovery/configuration readiness remains a separate current gate. T3 CAS/revalidation ties a successful Run to its Goal service-sequence charge in the same transaction, while `eligible_since`/backoff coherence is checked against current scheduler eligibility state. A queue rebuild may not rewrite these facts.

ContextSourceSnapshot identity is immutable on `runs`; ContextPlan attachment is a separate one-time row. The composite Run/source and Plan/source FKs relationally enforce exact-source attachment, while source availability/reconstructability remains a controller/recovery predicate. An Attempt cannot become current without a valid attachment.

The EvaluationRound typed-subject XOR is row-local and FK-enforced; semantic currentness and acceptance-contract matching remain controller/invariant-checker responsibilities across the owning TaskSpec/GoalRevision. Task terminal intent and local pointer facts are schema `CHECK` constraints; finalization completion remains cross-row controller/invariant-checker logic over the authoritative owning-domain rows plus any explicit residual obligation records. Explicit finalization-obligation ownership is concrete-FK/XOR constrained rather than opaque. RunStatus holder identity and Task current-Run holder identity are composite-FK constrained, while one nonterminal Run per Task is partial-unique constrained on `run_status.task_id` for `Active|Finalizing`. Exact phase/cardinality semantics (`Active => exactly one`, `Ready|Waiting => zero`) remain controller/invariant-checker responsibilities. Attempt holder/current-pointer consistency is FK-constrained and live-Attempt cardinality is partial-unique constrained. PlanningAttempt/EvaluationAttempt live-cardinality is relationally constrained on their concrete operation IDs. Sandbox lifecycle and external existence are separate status columns; ordinary RELEASED requires observed absence, while a RELEASED+UNKNOWN force-resolution is valid only with a durable tombstone/fence that preserves factual uncertainty. The Agent Control pre-contact rekey freeze remains a cross-row lifecycle invariant because `attempt_status.launch_contact_state` and the session verifier live on different rows; controller transactions and audit/invariant tests enforce it. Credential-binding compatibility is likewise cross-row/immutable-component logic: ExecutionBinding freezes one registry digest, broker redemption resolves exact frozen/current component entries and persists the matching authority digest on the broker operation. The checker remains valuable for cross-row semantic invariants and corruption/drift detection.

Violations create RecoveryFindings/quarantine rather than silent unsafe repair.

## Named transaction families

```text
T0  DISASTER-RESTORE AUTHORITY FENCE
T1  GOAL REVISION + ACCEPTANCE PINNING
T2  GRAPH PATCH
T3  SCHEDULER RUN-INTENT + CONTEXT-SOURCE FREEZE + FAIRNESS CHARGE
T3a CONTEXT PLAN ATTACHMENT
T4  ATTEMPT + AGENT-CONTROL IDENTITY
T4a PRE-CONTACT AGENT-CONTROL REKEY
T4b LAUNCH CONTACT MARKER
T5  USAGE INGESTION
T6  CANDIDATE SUBMISSION
T7  TYPED ACCEPTANCE/EVIDENCE COMMIT
T8  RETRY ATTEMPT
T9  REQUEUE AFTER PRIOR RUN TERMINAL
T10 AUTHORIZATION/GRANT REDEMPTION
T11 WORKSPACE/SANDBOX DESIRED STATE
T12 INTEGRATION STATE
T13 CONFIGURATION ACTIVATION
T14 UNKNOWN FORCE-RESOLUTION/TOMBSTONE
T15 EVALUATION LAUNCH CONTACT MARKER
T16 PLANNING EXTERNAL-CONTACT MARKER
```

T1 resolves any Goal acceptance logical evaluator refs against the expected active trusted evaluator registry/ConfigurationRevision before commit, then atomically inserts the immutable GoalRevision with exact EvaluatorVersion/evaluator-resolution provenance, advances the current Goal revision through expected-revision CAS, creates reconciliation work and appends Events. No evaluator external call occurs inside T1.

Before T3, the Scheduler/control path resolves and canonicalizes the exact immutable/versioned source identities required for the ContextSourceSnapshot under the captured ConfigurationRevision. T3 revalidates `scheduler_state.dispatch_mode=RUNNING`, the open recovery/configuration dispatch gates, Task Ready/current SchedulingEligible/expected scheduler state revisions, current Goal/Graph/config/admission authority and the captured ConfigurationRevision. It verifies the selected ExecutionBinding's `credentialBindingRegistryDigest` is the immutable `credentialBindings` component of that same captured ConfigurationRevision, inserts/reuses the immutable ContextSourceSnapshot, creates the immutable ExecutionBinding/Run with `task_id + context_source_snapshot_digest`, inserts the matching `run_status` row with the same immutable `task_id` and `phase=Active`, atomically moves the Task to `Active` with `active_run_id` pointing at that Run, consumes the SchedulingClaim, charges the selected Goal's `last_served_sequence = scheduler_state.next_service_sequence`, increments `next_service_sequence`, clears/normalizes the Task's temporary scheduler failure/backoff state, and appends Events.

The Run and fairness charge therefore commit or roll back together. A concurrent pause/priority/fairness-state mutation that invalidates the captured scheduler revisions causes T3 to abort/recompute; a pre-T3 routing/admission failure never consumes fairness service. The composite FKs prove Run holder relationships and the `one_nonterminal_run_per_task` partial unique index rejects a second `Active|Finalizing` Run for the Task.

T3 performs no Memory retrieval, model/prompt rendering, arbitrary repository traversal, backend call, Sandbox call, SecretProvider call or other external context construction. Stable source-generation identities and immutable configuration component identities may be prepared as data before T3, but the exact snapshot/Binding bound to the Run becomes authoritative only with T3.

T3a is the one-time ContextPlan attachment transaction after deterministic Context Builder preparation. It re-reads the Run/source identity, verifies no existing `run_context_plans` row, verifies the immutable ContextPlan's `source_snapshot_digest` equals the Run's frozen snapshot, inserts the attachment and appends its Event/condition update. The composite FKs enforce the same source relationship. Same Run + same attachment is idempotent; a different plan for an already-attached Run fails closed. No external source/backend call occurs inside T3a.

T4/T8 create the immutable Attempt, its nonterminal `attempt_status` row with the same `run_id`, the Attempt-scoped AgentControlSession bound to the current RestoreGeneration with `credential_revision = 1`, and the matching `run_status.current_attempt_id` update in one authoritative transaction. T4/T8 additionally require one valid `run_context_plans` row for the parent Run. T8 may commit only after the prior Attempt for that Run is definitively terminal; the partial unique index is the database backstop against overlapping nonterminal lineages.

T4a is a short authoritative recovery transaction used only when the current-generation Attempt is still durably `NOT_CONTACTED` and the raw bearer needed for first launch was lost. It rechecks session/Attempt/Run/ControlLease state plus the expected current credential revision, increments `credential_revision`, replaces `credential_hash`, records non-secret rekey provenance/Event, and commits before rebuilding the launch credential projection. It never changes Attempt ID, LaunchKey, AgentControlSession ID, or RestoreGeneration.

T4b verifies the same current Attempt/Run/control authority **and the exact current AgentControlSession credential revision** before atomically setting `launch_contact_state = CONTACT_MAY_HAVE_OCCURRED`, recording initiation provenance, and appending its Event. After T4b commits, T4a is permanently forbidden for that Attempt. Only after T4b may Pantheon cross the external ensureExecution/create boundary using the launch package built for that verified credential revision.

T7 loads the EvaluationRound's concrete typed subject and exact pinned criterion/EvaluatorVersion, rechecks the owning lifecycle object's currentness, creates immutable Evidence, updates criterion/Aggregate AcceptanceResult state and settles applicable control-operation accounting in one authoritative transaction. For `TASK_CANDIDATE`, Task must still be Evaluating with that exact current Candidate. For `GOAL_COMPLETION_CANDIDATE`, Goal must still be Evaluating with that exact current completion candidate and represented GoalRevision current for completion. T7 never lets the evaluator itself transition Task or Goal lifecycle; owning controllers apply current aggregate results through their lifecycle transitions.

T10 re-reads current Attempt/Run/Task authority, current ConfigurationRevision/authz policy, current RestoreGeneration and Grant/ticket state, then atomically consumes bounded Grant/ticket authority and creates/transitions the exact broker operation. For credential-bearing Run operations T10 additionally loads the Run's frozen `credentialBindingRegistryDigest`, resolves the exact normalized action/resource from that immutable registry and the current active registry, requires exact `credentialBindingAuthorityDigest` equality, rechecks `secret.use`/SecretDescriptor usability, and persists the non-secret binding provenance on the broker operation. Secret material is retrieved only after T10 commits. A later retry follows the same broker-operation identity/provenance rather than resolving a new credential mapping.

A Task lifecycle transaction that first enters `Finalizing` must set the selected `terminal_target` in that same authoritative commit. Explicit residual finalization obligations known at that point may be inserted in the same commit. No external finalization action may run before either its owning domain intent/state or its explicit `finalization_obligation` identity has been durably established. The later terminal transition re-reads the same `terminal_target`, all required authoritative domain predicates and any explicit obligation states; it never infers completion from Events.

T11 creates/transitions Sandbox desired state only after re-reading the concrete holder and checking that no conflicting current Sandbox exists for that Run/EvaluationOperation. Creation commits the immutable holder FKs, SandboxKey and initial `sandbox_status` before any runtime call. Lifecycle phase and external observed presence are updated independently from factual inspection. Ordinary release commits `RELEASED` only after `ABSENT` is established; an administratively force-resolved `RELEASED+UNKNOWN` lineage requires the matching durable tombstone/fence in the same authoritative recovery path. Release never erases holder identity or factual observation needed for audit/reconciliation.

T15 is a short authoritative transaction that verifies the EvaluationAttempt is current/nonterminal and still `NOT_CONTACTED`, then atomically sets `launch_contact_state = CONTACT_MAY_HAVE_OCCURRED`, records initiation time/daemon incarnation, and appends its Event. Only after T15 commits may Pantheon cross that attempt's external evaluator/process call boundary. No external process/backend/runtime call occurs inside T15.

PlanningOperation intent, immutable input/Goal/Graph/config/backend/metering provenance and required control-operation Reservations/Holds are committed before external planning contact. A corresponding PlanningAttempt is created `NOT_CONTACTED` before T16.

T16 is a short authoritative transaction that verifies the PlanningOperation/PlanningAttempt is current, the expected Goal/Graph planning fence is still applicable for issuing the call, required Holds/Reservations remain valid, and the attempt is still `NOT_CONTACTED`; it then atomically changes `contact_state = CONTACT_MAY_HAVE_OCCURRED`, records daemon/time provenance and appends its Event. Only after T16 commits may Pantheon invoke the external Planner/backend. A later stale Goal/Graph revision can invalidate Graph materialization without erasing factual PlanningAttempt/Usage history.

Dispatch pause/resume and Goal priority changes are authoritative scheduler-state CAS transactions executed through normal Operator command idempotency. They update `scheduler_state`/`goal_scheduling_state` plus Command/Event state atomically but do not create/cancel Runs. Task eligibility/backoff transitions similarly update `task_scheduling_state` with the controller decision that changed/reconciled SchedulingEligible; queue mutation alone is never authoritative.

Never perform network/Git/process/backend/secret-store/container-runtime/model/external-context-source calls inside a SQLite transaction.

## Core invariants

1. One authoritative SQLite database provides cross-subsystem atomicity.
2. Writer serialization prevents physical write contention; row revisions/CAS prevent logical stale decisions.
3. Safety-critical relationships are relationally constrained where SQLite can express them.
4. JSON is never a substitute for ownership/revision/accounting columns.
5. Scheduler queues are disposable: durable `scheduler_state`, `goal_scheduling_state`, `task_scheduling_state` and SchedulingClaims are the authoritative scheduling inputs across restart.
6. Durable `dispatch_mode` is operator desired state, separate from recovery/configuration gates; PAUSED survives ordinary restart and forbids T3 without cancelling existing Runs.
7. Goal fairness uses durable logical service sequence; T3 advances it atomically with successful Run intent, while failed/rolled-back scheduling attempts do not charge service.
8. Task `eligible_since` represents the current continuous scheduler-eligible interval; temporary backoff does not reset it or mutate Task lifecycle.
9. Task-scoped reservations are unique/reused across Runs.
10. Every Run binds exactly one immutable ContextSourceSnapshot at T3; that snapshot contains the exact `ConfigurationRevision + contextPolicyDigest` and every selection-affecting source/version/generation identity required for deterministic context construction.
11. ContextPlan attachment is separate from the immutable Run row: `run_context_plans` permits at most one plan per Run and composite FKs prove that plan derives from the Run's exact frozen source snapshot. Attempt creation requires the attachment.
12. Context Builder/recovery may retry preparation only against that same source snapshot; it never substitutes current "latest" ContextPolicy/Memory/index/Skill/source state into an existing Run.
13. GoalRevision and TaskSpec acceptance contracts freeze exact permitted EvaluatorVersions before EvaluationRound creation; registry movement never silently rewrites those semantics.
14. EvaluationRound owns exactly one concrete relational subject (`TASK_CANDIDATE` xor `GOAL_COMPLETION_CANDIDATE`); no opaque generic subject reference or Task-only ownership is used.
15. Evidence/AcceptanceResult must match the exact EvaluationRound subject and pinned evaluator contract; Task and Goal lifecycle controllers separately recheck current authority before applying results.
16. Task terminal intent is durable row state: non-finalizing nonterminal Tasks have no target, `Finalizing` Tasks have exactly one selected target, and terminal Tasks retain a target matching their terminal phase. Finalization completion is reconstructed from authoritative domain rows plus only those explicit residual obligations that need independent retry/uncertainty state.
17. RunStatus carries immutable Task holder identity constrained back to `runs`; Task `active_run_id` is constrained to a Run of the same Task; and a real partial unique index over `run_status.task_id` enforces at most one `Active|Finalizing` Run per Task. Task phase-specific exact-zero/exact-one semantics remain controller/invariant-checker rules.
18. Normal Attempt holder identity/current pointer are FK-constrained and at most one Attempt per Run may be nonterminal through a real partial unique index over `attempt_status.run_id`; retries never overlap an unresolved prior lineage.
19. External contact boundaries are durable before normal Attempt, EvaluationAttempt and PlanningAttempt calls; ambiguous contact never authorizes an overlapping replacement lineage.
20. Agent Control bearer material is never persisted; a current-generation session may replace its verifier only under T4a while its Attempt is durably `NOT_CONTACTED`, and T4b freezes that credential revision before external launch contact.
21. Every authoritative planning invocation has a PlanningOperation; external planning has at most one nonterminal PlanningAttempt at a time, while PlanningRecord remains immutable result/provenance rather than Graph authority.
22. Usage identity is Pantheon-namespaced; a backend may report only for an Attempt ExecutionBinding or concrete control-operation metering binding that immutably names it, and delayed factual usage is not rejected solely for stale controller epoch or current terminal state.
23. Grant use/redemption and exact broker-operation creation are one CAS transaction under current policy and current RestoreGeneration.
24. Disaster restore is entered through a crash-safe out-of-database restore latch; SQLite alone is never assumed capable of detecting that its own history was rewound.
25. T0 rotates a fresh unpredictable RestoreGeneration exactly once per restore operation before any new authority-bearing mutation/effect; the matching durable RecoveryPass makes crash-after-T0 resume-safe.
26. Restored Grants/Tickets cannot redeem, restored broker operations cannot be reissued from stale state, and old-generation AgentControlSessions cannot authorize worker semantic requests or use T4a to promote themselves.
27. Operator command idempotency is scoped by `(RestoreGeneration, commandId)` and stale epochs fail before row absence can be interpreted as a new command.
28. Restored negative observations are historical snapshot evidence; fresh domain reconciliation is required before they can authorize replacement/conflicting external work.
29. Cancellation/supersession can beat Candidate submission through Task revision CAS.
30. Requeue occurs only after previous responsible Run terminal.
31. Force-resolution tombstones stale lineages without fabricating factual Usage or external absence.
32. Event rows are committed with their authoritative mutation, but state tables remain source of truth.
33. Sandbox lifecycle phase and external existence certainty are distinct durable fields. Ordinary `RELEASED` requires observed `ABSENT`; `RELEASED+UNKNOWN` is valid only with an explicit durable force-resolution tombstone/fence, and `UNKNOWN` never authorizes blind replacement.
34. SandboxInstance ownership is relational and immutable: exactly one Run or v1 EvaluationOperation owns each Sandbox; PlanningOperation has no implicit Sandbox ownership in v1, and ambiguous/unreleased Sandbox existence blocks an overlapping replacement for the same holder unless the old lineage is explicitly tombstoned/fenced.
35. ExecutionBinding freezes `credentialBindingRegistryDigest` from its ConfigurationRevision; existing Runs never silently switch to a later credential-mapping registry.
36. Credential-bearing broker redemption requires exact frozen/current `credentialBindingAuthorityDigest` equality and persists that non-secret authority provenance before secret retrieval; SecretVersionId is factual material-use metadata, not authority.
