# Global Recovery and Crash Reconciliation

## Status

Draft design — Pantheon recovery and crash-safety specification.

## Purpose

This document defines how Pantheon reconstructs safe control after daemon crashes, machine restarts, partial external side effects, storage inconsistencies, database restore, and divergence between SQLite desired state and the external world.

The central rule is:

> **Recovery does not mean returning every object to a known state before Pantheon can operate. Recovery is safe once every durable external-side-effect obligation is either reconciled to a known state or explicitly fenced so that new work cannot conflict with it.**

Pantheon therefore treats restart recovery as ordinary controller reconciliation over durable desired state, not as a separate imperative repair script.

See also:

- `docs/architecture/task-lifecycle.md`
- `docs/architecture/run-and-attempt.md`
- `docs/architecture/recovery-policy.md`
- `docs/architecture/scheduler-reservations-ownership-and-leases.md`
- `docs/architecture/scheduler-dispatch-and-run-intent-reconciliation.md`
- `docs/architecture/budget-usage-and-rate-limits.md`
- `docs/architecture/artifact-model.md`
- `docs/architecture/workspace-and-git-integration.md`

## 1. Sources of truth

Pantheon distinguishes durable authority from observed external state.

```text
SQLite desired state / immutable records
              │
              │ authoritative intent
              ▼
        Controllers
              │
              │ inspect / ensure / terminate / repair
              ▼
        External world
              │
              │ observations / evidence
              ▼
SQLite status / findings / events
```

The authoritative sources are:

- immutable Goal/Task/Run/Attempt/Binding/Candidate/Artifact records;
- durable EvaluationOperation/EvaluationAttempt identities where external verification exists;
- durable SandboxInstance holder/SandboxKey identity and SandboxVerification facts;
- durable desired-state fields;
- ResourceReservations and BudgetHolds;
- current control ownership/fencing records;
- explicit IntegrationIntent and cleanup/finalization obligations.

The following are never authoritative by themselves:

- in-memory queues;
- process-local maps;
- cached backend status;
- stale event streams;
- PID files;
- filesystem/runtime objects without corresponding durable ownership state;
- backend callbacks received without current fencing authority.

In-memory scheduler and controller queues are disposable accelerators. They must be reconstructible from SQLite and external observation.

## 2. Recovery is ordinary reconciliation

Every controller owns a narrow desired-vs-observed contract and must be safe to invoke repeatedly.

Conceptually:

```text
read durable desired state
        ↓
read durable prior observation
        ↓
inspect external reality
        ↓
compare desired vs observed
        ↓
perform at most the required idempotent action
        ↓
persist observation/result
```

Startup recovery invokes the same reconciliation logic used during normal operation. There must not be a second set of ad-hoc startup-only mutation rules.

Periodic safety reconciliation continues after startup so missed events, external drift, and latent inconsistencies are eventually rediscovered.

## 3. Installation identity, restore generation, and daemon incarnation

Pantheon maintains three distinct identities/fences.

### Installation ID

A stable random identifier for one Pantheon control-plane installation.

```text
installationId = persistent across normal daemon restarts and disaster restore of that installation
```

Where practical, external resources created by Pantheon should carry adapter-specific ownership metadata derived from:

- installation ID;
- Pantheon subject ID;
- operation/LaunchKey where appropriate.

The concrete tag/label mechanism is adapter-private.

The Installation ID is used for inventory and orphan detection. It is not authorization.

### RestoreGeneration

`RestoreGeneration` is a fresh unpredictable installation-wide authority/idempotency generation.

```text
normal daemon restart
→ RestoreGeneration unchanged

disaster restore of an older SQLite snapshot
→ RestoreGeneration replaced with a fresh unpredictable value
```

It fences authority whose durable consumption/idempotency history can be rewound by restore, including runtime Grants, CapabilityTickets, broker operations and Operator command identities.

It is deliberately not a monotonic counter restored from the database: an old snapshot can reintroduce a previously used numeric value. The new generation is random/fresh and is committed before any new post-restore authority-bearing mutation or external effect.

`RestoreGeneration` is distinct from:

- Installation ID — stable ownership identity;
- daemon incarnation — process/controller lifetime;
- Run ControlLease epoch/token — Run-control ownership;
- JournalEpoch — Event-stream continuity.

### Daemon incarnation ID

Every daemon start creates a new random `daemonIncarnationId`.

```text
incarnation A dies
        ↓
incarnation B starts
```

The ID is never reused and is recorded in controller ownership records and recovery events.

A daemon incarnation row may record `startedAt` and a best-effort clean `stoppedAt`. A missing stop marker indicates an unclean exit, but a clean marker never allows Pantheon to skip external reconciliation.

## 4. Single-daemon authority in v1

Pantheon v1 is local-first and uses a single active daemon.

Before mutating SQLite or external state, the daemon must acquire an operating-system-backed exclusive installation lock.

A PID file alone is insufficient because PIDs are reusable and do not provide ownership fencing.

If the installation lock cannot be acquired, the process may offer read-only diagnostics where safe but must not start controllers or scheduling.

Future multi-daemon operation requires a distributed coordination design and is outside v1.

## 5. ControlLease fencing uses epoch plus unpredictable lease token

A monotonic Run ownership epoch remains useful for ordering ownership transfers, but epoch alone is not sufficient under database restore because an older snapshot can reintroduce a previously used numeric epoch.

Each acquired/adopted Run ControlLease therefore contains:

```yaml
controlLease:
  run: run_123
  holder: daemon-incarnation://...
  ownershipEpoch: 18
  leaseToken: <fresh-random-token>
  validUntil: ...
```

Every authoritative controller mutation must verify the current lease identity, not merely the numeric epoch.

Conceptually:

```text
command.runId == current.runId
AND
command.ownershipEpoch == current.ownershipEpoch
AND
command.leaseToken == current.leaseToken
```

Whenever control is adopted after daemon restart or restore, Pantheon rotates the lease token before issuing external commands.

Adapters should propagate the fencing identity to native execution controls where practical. A backend inability to enforce fencing internally does not weaken Pantheon's own authoritative-state checks.

RestoreGeneration does not replace ControlLease fencing. RestoreGeneration prevents replay of rewound authorization/command authority; ControlLease token+epoch fences Run-controller ownership.

## 6. Startup phases

Startup is staged so unsafe external actions remain blocked until the persisted world has been fenced.

```text
PROCESS START
    ↓
A. installation lock
    ↓
B. storage recovery / validation
    ↓
C. daemon incarnation registration
    ↓
D. recovery inventory
    ↓
E. authority rotation / fencing
    ↓
F. domain reconciliation
    ↓
G. recovery barrier satisfied
    ↓
H. scheduler dispatch enabled
```

These phases are not user-facing Task phases.

Ordinary restart preserves the existing RestoreGeneration. Disaster restore executes the additional restore authority fence in §27 before any normal authority-bearing mutation or external effect.

### A. Installation lock

Acquire exclusive v1 daemon authority.

### B. Storage recovery and validation

Open SQLite normally so SQLite can perform its own journal/WAL recovery. Validate schema/migration compatibility and run configured database consistency checks before controllers are allowed to perform side effects.

### C. Incarnation registration

Persist the new daemon incarnation and keep the global dispatch gate closed.

### D. Durable inventory

Load at least:

- nonterminal Goals and Tasks;
- Active/Evaluating/Finalizing Tasks;
- nonterminal Runs and Attempts;
- nonterminal EvaluationOperations and EvaluationAttempts;
- ExecutionBindings;
- every SandboxInstance not RELEASED plus its durable holder and latest SandboxVerification;
- ResourceReservations not RELEASED;
- BudgetHolds not settled/released;
- WorkspaceRecords not RELEASED;
- pending IntegrationIntents;
- candidate/evidence finalization work;
- Artifact replicas needed by live work;
- unresolved cleanup/finalization obligations;
- prior unresolved RecoveryFindings.

Sandbox inventory is **not derived only by walking Runs**. Verification Sandboxes belong to EvaluationOperations and must remain discoverable/reconcilable even when no Run owns them.

In restore mode the inventory also includes Grants, CapabilityTickets, broker operations and Commands because their restored rows may represent authority/idempotency history older than external reality.

### E. Authority rotation and fencing

Adopt required Run control by incrementing ownership epoch and rotating lease tokens transactionally.

No old controller incarnation may remain authoritative.

In restore mode, the new RestoreGeneration has already been committed before this point; old-generation Grants/Tickets cannot redeem and old-generation broker operations are reconciliation-only.

### F. Domain reconciliation

Controllers inspect their external domains and either establish current state or place affected resources into conservative fenced states.

Sandbox holder/SandboxKey reconciliation is a prerequisite for issuing a new launch in any execution lineage that requires that Sandbox. Run and Evaluation controllers may inspect their execution domains concurrently, but neither a normal Attempt nor an EvaluationAttempt may launch/relaunch through an unresolved required Sandbox.

### G. Recovery barrier

The startup barrier is satisfied when every durable external-side-effect obligation has reached one of:

```text
RECONCILED
known and safe

FENCED
unknown or degraded, but no new conflicting work can be admitted

QUARANTINED
inconsistent and explicitly blocked from automated destructive action
```

The barrier does **not** require all external uncertainty to disappear.

### H. Dispatch gate

Only after the barrier is satisfied may the Scheduler commit new Runs.

## 7. Recovery barrier versus global freeze

Pantheon should minimize blast radius.

Example:

```text
Attempt A = UNKNOWN
→ its reservations remain UNCERTAIN
→ backend capacity remains charged
→ Run A remains fenced

unrelated backend B = healthy
unrelated resources = reconciled
→ new work may use remaining safe capacity on B
```

A single uncertain Run must not freeze all Goals indefinitely.

Global dispatch remains disabled only when Pantheon cannot establish safe accounting/authority boundaries system-wide, such as database integrity failure or unreconciled installation ownership.

## 8. Durable external-operation rule

Every consequential external side effect follows:

```text
durable intent / precondition state
        ↓
external operation
        ↓
external observation / acknowledgement
        ↓
durable observed result
```

Never:

```text
external side effect
        ↓
hope to record it later without stable identity
```

Each domain provides an idempotency/reconciliation identity appropriate to the operation:

```text
Attempt launch      → LaunchKey
Evaluation launch   → EvaluationAttempt ID + launch-contact marker
Run control         → ControlLease leaseToken + epoch
Sandbox             → SandboxKey + immutable Run/EvaluationOperation holder
Workspace           → Workspace ID + deterministic desired path/base
Artifact seal       → content digest
Integration         → IntegrationIntent + expected target OID
Broker operation    → stable broker-operation/external idempotency identity
Operator command    → RestoreGeneration + commandId
Resource release    → Reservation ID
Budget settlement   → Hold/Usage source IDs
```

Pantheon does not need one provider-specific universal transaction protocol. It requires each external domain to expose enough identity/inspection semantics to determine whether an operation happened or to safely repeat it.

A disaster restore never creates permission to replace an existing operation identity with a fresh one solely because the restored row looks incomplete. That would turn uncertainty into duplicate effect authority.

## 9. External operation certainty

Controllers normalize external operation outcomes into three broad certainty classes:

```text
CONFIRMED
external result established

NOT_APPLIED
controller can prove the external effect did not happen

UNKNOWN
operation may or may not have happened
```

`UNKNOWN` never authorizes an independent replacement side effect.

The controller first inspects/reconciles using the same stable identity. Recovery Policy may act only after the domain has established enough certainty for the proposed recovery scope.

## 10. Cleanup and finalization obligations

Destructive cleanup must not be represented as a single irreversible delete command.

Pantheon maintains durable finalization obligations for resources that own external state.

Conceptually:

```yaml
finalizationObligation:
  subject: run://123
  key: executor-stopped
  status: PENDING
```

or:

```yaml
finalizationObligation:
  subject: workspace://456
  key: immutable-output-preserved
  status: SATISFIED
```

Minimum states:

```text
PENDING
SATISFIED
UNCERTAIN
```

A resource may enter a logical terminating/finalizing state before its obligations are satisfied, but Pantheon must not erase authoritative ownership information or release protected capacity until the relevant obligations are satisfied.

Typical obligations include:

- executor/evaluator termination confirmed;
- Run/control-operation reservations safe to release;
- BudgetHold settled;
- candidate/evidence state durably sealed;
- workspace outputs preserved before deletion;
- managed Git ref/integration state reconciled;
- Run or verification Sandbox cleanup confirmed where required.

This is Pantheon's equivalent of a finalizer pattern: durable deletion intent plus controller-owned cleanup, not immediate record disappearance.

## 11. Never delete evidence needed to recover

Recovery-critical records are retained at least through finalization and configured audit retention.

Pantheon must not physically delete:

- nonterminal Run/Attempt identity;
- nonterminal EvaluationOperation/EvaluationAttempt identity;
- LaunchKeys and evaluation launch-contact facts;
- ExecutionBindings;
- non-RELEASED SandboxInstance holder/SandboxKey identity and required verification history;
- unresolved Reservations/Holds;
- Workspace ownership records;
- IntegrationIntents;
- unresolved finalization obligations;
- Artifact/Candidate identities referenced by active acceptance;

merely because an in-memory controller believes the work is over.

Garbage collection is a later operation over terminal, unreferenced, fully finalized state.

## 12. Execution and Sandbox recovery

### Run and Attempt recovery

For every nonterminal Run:

```text
rotate/acquire ControlLease
        ↓
resolve/reconcile required Run Sandbox holder + SandboxKey
        ↓
load current nonterminal Attempt, if any
        ↓
inspect backend by Attempt attachment / LaunchKey
```

A normal Attempt may not be newly launched/relaunched until its required Run-owned Sandbox is reconciled and verified. Existing external execution may be inspected concurrently, but unresolved Sandbox state is never interpreted as permission to provision a replacement Sandbox.

Possible observations:

### RUNNING / STARTING

- current Attempt remains nonterminal;
- relevant reservations become/remain ACTIVE;
- usage metering resumes/reconciles;
- Run Controller continues normal reconciliation.

### EXITED / definitive absence

- persist the definitive Attempt observation first;
- settle any usage that can be established;
- hand evidence to Recovery Policy;
- do not create another Attempt inside the recovery scanner itself.

### UNKNOWN

- Attempt remains nonterminal;
- reservations remain/enter UNCERTAIN;
- unresolved BudgetHold headroom remains held conservatively;
- no replacement Attempt is created;
- schedule future reconciliation.

If a backend supports inventory of Pantheon-owned executions, recovery should also compare that inventory against durable Attempts to discover dangling executions.

Unknown/dangling native executions are quarantined and reported before destructive cleanup.

### Sandbox holder reconciliation

Recovery independently walks **every non-RELEASED SandboxInstance**, resolves its immutable holder, and re-inspects the same SandboxKey.

```text
SandboxInstance
  holder = Run
  → reconcile as that Run's execution Sandbox

SandboxInstance
  holder = control-operation / EvaluationOperation
  → reconcile as that EvaluationOperation's verification Sandbox
```

For a valid live holder:

- inspect/reconcile the existing SandboxKey;
- restore/refresh factual SandboxVerification where required;
- keep corresponding ResourceReservation capacity charged until release is confirmed;
- never provision an overlapping second Sandbox for the same holder while prior existence is UNKNOWN/non-RELEASED.

If the holder is terminal but Sandbox cleanup is incomplete, the Sandbox remains a cleanup/finalization obligation and capacity is not released merely because the holder stopped executing.

If the durable holder is missing, the holder-kind/FK relationship is inconsistent, or an inventoried external Sandbox has no corresponding durable SandboxInstance, quarantine it. Do not reinterpret it as free capacity or automatically destroy it.

### EvaluationOperation and EvaluationAttempt recovery

For every externally executing nonterminal EvaluationOperation:

```text
resolve/reconcile EvaluationOperation-owned verification Sandbox
        ↓
load current nonterminal EvaluationAttempt, if any
        ↓
interpret H3 launch_contact_state
        ↓
inspect/reconcile same EvaluationAttempt identity where external contact may have occurred
```

`NOT_CONTACTED` with no independent evidence means the evaluator launch path did not cross its call boundary. `CONTACT_MAY_HAVE_OCCURRED` remains UNKNOWN until the same EvaluationAttempt identity is reconciled/terminated. No overlapping EvaluationAttempt or replacement verification Sandbox is created from ambiguity.

A verification Sandbox can survive from EvaluationAttempt 1 to a later bounded EvaluationAttempt 2 only after attempt 1 is definitively terminal and only while the Sandbox's immutable identity/materialization, verification, resource ownership and current policy remain valid.

## 13. Backend recovery contract

ExecutorBackend should support the strongest feasible version of:

```text
inspect/reconcile known Attempt by LaunchKey/attachment
```

and may additionally support:

```text
inventory Pantheon-owned executions for installation ID
```

Inventory is optional for ordinary restart correctness when all durable Attempt records are intact, but it becomes highly valuable for disaster recovery and orphan detection.

If a restored control-plane snapshot is older than external execution state and a backend cannot inventory Pantheon-owned work, Pantheon must conservatively block new execution on that backend until an operator resolves the ambiguity or isolation guarantees prove duplicate execution impossible.

SandboxBackend similarly reconciles by the durable SandboxKey and may inventory Pantheon-owned runtime objects by installation identity. Inventory does not become ownership authority: a matching durable SandboxInstance and valid holder relationship remain required.

## 14. Resource ledger reconciliation

ResourceReservations are authoritative capacity commitments.

Recovery never recomputes reservations solely from current CPU/RAM/process utilization.

For each non-RELEASED reservation:

```text
holder exists and is live
→ reconcile with holder/domain

holder terminal and finalization proves unused
→ release idempotently

holder missing / inconsistent
→ QUARANTINE reservation
→ continue charging capacity
```

For Sandbox capacity, an unresolved SandboxInstance is independent evidence that capacity may still be occupied. A Run/EvaluationOperation becoming terminal does not by itself release that capacity; Sandbox absence/release must be established according to its own lifecycle.

Missing ownership evidence is never interpreted as free capacity.

Capacity publishers may refresh `allocatable`, but existing reservations remain accounting authority until safe release is proven.

## 15. Budget and usage reconciliation

Usage truth and spending authority remain separate.

On restart:

- replay/ingest backend usage using stable operation/source IDs;
- de-duplicate previously processed usage;
- convert confirmed held quantity to consumed quantity;
- never reduce consumed usage because an Attempt failed;
- keep unused hold headroom reserved while external execution is UNKNOWN;
- settle/release only the provably unused remainder when Run/control work is finalized.

If external billing/allowance state is authoritative, refresh the external snapshot and record freshness before relying on new headroom.

If actual external usage exceeds a Pantheon limit, record the true usage and mark the budget overdrawn; do not clamp history to the configured ceiling.

## 16. Workspace and Git reconciliation

For every non-RELEASED WorkspaceRecord:

```text
SQLite WorkspaceRecord
        +
confined Git/filesystem observation
        ↓
Workspace reconciliation
```

Workspace recovery obeys the hostile-repository boundary in `workspace-and-git-integration.md`. Agent-writable Git state is observation input, never controller authority. In particular, recovery does not derive a trusted repository/common-dir/object-store/configuration path by following an Agent-controlled `.git` gitfile, `commondir`, object alternate, configuration include, remote/helper declaration or equivalent repository indirection.

Durable Pantheon Workspace/repository records define the controller-trusted roots that recovery is allowed to inspect. Any operation that must interpret Agent-owned Git metadata runs inside the Agent Sandbox or an equivalently confined controller-owned helper. Privileged controller Git is permitted only against controller-owned/trusted Git control state using the sterile execution profile; it never points the daemon's ambient authority at Agent-writable repository configuration.

Possible cases:

### Expected workspace exists and is coherent

- verify repository/base/worktree identity against durable Pantheon ownership state;
- repair administrative linkage only through Git-supported repair operations where safe and only through the confined/sterile execution boundary appropriate to the repository state;
- restore correct READY/FROZEN observation.

### Workspace record exists but path is missing

- mark Workspace condition `Missing`;
- do not silently recreate it if unsealed mutable work may have been lost;
- if all required immutable output is already sealed, recovery/finalization may proceed without it;
- otherwise emit immutable failure/recovery evidence and let Recovery Policy decide fresh-workspace retry/replan/human action.

### Git worktree exists without Pantheon ownership record

- classify as dangling/unknown ownership;
- quarantine;
- never auto-prune solely because Git considers it stale.

### Git administrative state is repairable

Use stable Git worktree inventory/repair interfaces rather than editing `.git/worktrees/**` directly, but only after establishing the hostile-repository confinement rule above. Git-supported repair is not by itself a privilege boundary.

### Hostile repository boundary cannot be established

Emit `workspace.hostile-repository-state`, fence/quarantine the Workspace, and require a safe rematerialization or operator resolution. Recovery never falls back to running Git with ambient daemon/control-plane authority merely to obtain an inventory or repair result.

## 17. Git integration recovery

IntegrationIntent makes shared-ref mutation recoverable.

Suppose an intent records:

```text
expectedTarget = Y
integrationCommit = Z
```

After restart:

### target == Z

The CAS update already happened.

```text
IntegrationIntent → APPLIED
```

### target == Y

The update did not happen or can be safely treated as unapplied.

Revalidate current policy/preconditions and retry the same compare-and-swap operation if still desired.

### target is neither Y nor Z

The target moved independently.

```text
IntegrationIntent → STALE / needs recomputation
```

Do not force-update or overwrite the newer ref.

### target state cannot be established

```text
IntegrationIntent → UNCERTAIN
```

No second blind ref mutation is issued.

## 18. Artifact/CAS crash consistency

Content-addressing simplifies external recovery because extra immutable objects are safe while missing referenced objects are detectable.

A local CAS durable-put contract should be conceptually:

```text
write temporary object
        ↓
compute/verify size + digest
        ↓
make bytes durable according to storage backend
        ↓
atomically publish object under digest identity
        ↓
make directory/index update durable where required
        ↓
only then create authoritative SQLite references
```

An interrupted write must never be exposed under the final digest path unless its exact bytes verify against that digest.

### CAS object exists but SQLite metadata does not

Treat as an orphan immutable object. It may be retained through a grace period and later garbage-collected.

Do not invent provenance.

### SQLite references object but local replica is missing

Mark the replica `MISSING`.

If another trusted replica exists, retrieve and verify digest/size before marking AVAILABLE.

If no replica exists, retain Artifact identity/provenance but block operations requiring its bytes.

### Bytes do not match digest

Mark replica `CORRUPT`, never mutate Artifact identity, and never allow Acceptance to consume the mismatched content.

## 19. Candidate and acceptance recovery

Candidates are immutable and content-addressed.

On restart:

- a Task in Evaluating must still reference an exact Candidate digest;
- existing Evidence remains valid only if bound to the same subject/evaluator/policy revisions;
- evaluator work that was in progress is reconciled through its EvaluationOperation, verification Sandbox and EvaluationAttempt identities;
- ERROR/UNKNOWN evaluator state never becomes PASS;
- Task success/finalization is derived only after all required evidence is durably present.

If a worker had mutable output but no Candidate was durably committed before the crash, Pantheon must not infer a candidate from logs or narration. It may later seal the preserved frozen workspace only through the normal candidate-sealing path if Run/Recovery policy still authorizes that operation and the exact intended output can be established.

## 20. Logical invariant scanner

External reconciliation is not enough. Pantheon also scans durable relational invariants.

Examples:

```text
Task.phase == Active
→ exactly one nonterminal active Run must own responsibility

Run nonterminal
→ immutable ExecutionBinding must exist

Attempt nonterminal
→ parent Run must be nonterminal and current ownership known

EvaluationAttempt nonterminal
→ parent EvaluationOperation must exist and no sibling EvaluationAttempt is nonterminal

SandboxInstance non-RELEASED
→ exactly one valid holder exists: Run xor EvaluationOperation
→ no overlapping current Sandbox exists for that same holder

ResourceReservation non-RELEASED
→ holder reference must exist or reservation is quarantined

BudgetHold unsettled
→ holder/source accounting must remain traceable

Candidate
→ all referenced Artifact identities must exist in metadata

Evidence PASS
→ subject/evaluator bindings must be complete
```

In restore mode the scanner additionally checks that any newly redeemable Grant/Ticket, executable broker operation and accepted Operator command belongs to the current RestoreGeneration. Old-generation broker operations may remain only in reconciliation/fenced history.

Violations are classified, not silently patched.

## 21. RecoveryFinding

Every discovered inconsistency that requires nontrivial reconciliation should be representable as a durable finding.

Conceptually:

```yaml
recoveryFinding:
  id: finding_...
  recoveryPass: recovery-pass_...

  subject:
    kind: workspace
    ref: workspace://...

  code: workspace.missing
  severity: degraded

  observation:
    ...

  disposition: FENCED
  firstObservedAt: ...
  lastObservedAt: ...
```

Useful dispositions:

```text
AUTO_REPAIRED
RECONCILING
FENCED
QUARANTINED
OPERATOR_REQUIRED
RESOLVED
```

Recovery findings are observability/audit facts. They do not replace the canonical domain object's status.

## 22. Repair policy

Pantheon distinguishes repair classes.

### Safe automatic repair

Examples:

- rebuild disposable in-memory queue;
- refresh a derived condition;
- reattach a known Attempt using the same LaunchKey;
- re-inspect a known Sandbox by the same SandboxKey and durable holder;
- mark an already-applied IntegrationIntent APPLIED;
- fetch a missing Artifact replica from another verified replica;
- use Git's supported worktree repair operation when ownership and expected path are unambiguous.

### Reconciliation required

Inspect external state before deciding, for example:

- uncertain executor/evaluator launch/termination;
- Sandbox existence/cleanup UNKNOWN for a Run or EvaluationOperation;
- pending IntegrationIntent after crash;
- old-generation broker operation whose effect may have occurred after the restored snapshot;
- workspace that may contain unsealed user/Agent work.

### Quarantine / operator required

Examples:

- active Task with missing immutable Run identity;
- nonterminal Run missing immutable ExecutionBinding;
- non-RELEASED Sandbox with missing/inconsistent holder;
- runtime Sandbox discovered with no corresponding durable SandboxInstance ownership record;
- reservation whose holder disappeared from authoritative state;
- unexplained shared Git ref mutation;
- old-generation broker operation whose external outcome cannot be inventoried/established;
- database integrity failure;
- foreign-key/logical corruption that cannot be repaired from immutable history.

Pantheon must not repair such cases by guessing or deleting evidence.

## 23. Degraded modes and blast-radius isolation

Recovery failures should be scoped when possible.

Examples:

```text
one backend unavailable
→ block/reroute new work requiring that backend only

one verification Sandbox UNKNOWN
→ fence that EvaluationOperation and retain its capacity
→ unrelated evaluation/work may continue if capacity/authority remain safe

one repository workspace corrupt
→ fence Tasks using that repository/workspace

one Artifact replica corrupt with verified remote replica
→ repair replica without global stop

one IntegrationIntent conflicted
→ block that integration only
```

Global mutation/dispatch must stop for conditions such as:

- SQLite integrity cannot be established;
- installation lock/authority is ambiguous;
- disaster-restore RestoreGeneration fence has not been durably committed;
- schema is unsupported/incompletely migrated;
- global resource/budget accounting is internally contradictory in a way that could cause unsafe double allocation.

## 24. API behavior during startup recovery

Once the storage gate is safe, Pantheon may expose inspection/status APIs before dispatch is enabled.

Desired-state writes that do not create immediate external side effects may be accepted and queued during ordinary startup, but the dispatch gate remains closed until the recovery barrier is satisfied.

During disaster restore, no authority-broadening or effect-creating Operator mutation may be accepted until the new RestoreGeneration has been committed. Requests carrying a pre-restore command epoch fail closed rather than being reinterpreted as new commands.

Safety-reducing operations should remain available where possible, including:

- cancel/pause desired state;
- revoke grants;
- tighten policy/budget;

Such requests still follow normal durable reconciliation and may remain pending if external status is unknown.

Operations that broaden authority or intentionally create new external work must not bypass the recovery barrier.

## 25. SQLite operational requirements

Pantheon v1 relies on SQLite for atomic durable state transitions.

Recommended requirements:

- database on a reliable local filesystem, not an untrusted/broken network-locking filesystem;
- WAL mode for normal concurrency;
- `synchronous=FULL` for the control-plane database unless an operator knowingly chooses weaker durability;
- short write transactions, using `BEGIN IMMEDIATE` where Pantheon must acquire write authority before validating and mutating a decision;
- foreign keys enabled and checked;
- no raw copying of a live SQLite database file without its journal/WAL state;
- backups via SQLite Online Backup API, `VACUUM INTO`, or another SQLite-supported consistent snapshot mechanism;
- never delete/move a hot `-wal`/journal during recovery.

### SQLite version floor for WAL

Pantheon should require SQLite **3.51.3 or newer**, or an official version containing the WAL-reset bug backport, when using WAL with multiple concurrent connections.

The actual linked SQLite version must be checked at startup and reported in diagnostics.

## 26. Database integrity checks

Startup should not treat successful file open as proof of logical integrity.

Recommended v1 policy:

```text
normal startup
→ PRAGMA quick_check
→ PRAGMA foreign_key_check

if quick check fails, corruption is suspected,
or operator requests deep diagnosis
→ PRAGMA integrity_check
```

`quick_check` is suitable for routine validation because it performs most structural checks faster than the full integrity check; full integrity checking additionally validates index/table consistency and uniqueness constraints.

Failure of integrity validation places Pantheon into storage-degraded/read-only recovery mode. Controllers do not perform new external mutations until the database has been recovered/restored by an explicit operator procedure.

## 27. Backups and disaster recovery

A valid SQLite backup protects authoritative control-plane history but does not rewind the external world.

Therefore restoring a backup is fundamentally different from normal daemon restart.

### Backup

Create consistent snapshots using SQLite-supported online backup mechanisms. Record at least:

- backup creation time;
- schema/migration version;
- Pantheon installation ID;
- backup digest/checksum;
- application version.

The snapshot necessarily includes the then-current RestoreGeneration, Grants, CapabilityTickets, broker operations and Commands. Those rows are historical after an older backup is restored until the post-restore authority fence is established.

### Restore authority fence

After restoring an older snapshot:

```text
DO NOT immediately enable Scheduler dispatch
DO NOT redeem restored Grants/Tickets
DO NOT execute restored pending broker operations
DO NOT accept an old command epoch as a new command
```

External executors, Sandboxes, Git refs, worktrees, object stores, credential-backed operations and other services may contain effects created after the snapshot. Those effects are not rewound when SQLite is restored.

Restore recovery therefore:

1. acquires installation authority and validates that the snapshot belongs to the intended installation;
2. opens/validates SQLite schema/integrity while all effect-creating gates remain closed;
3. creates a new daemon incarnation;
4. **commits a fresh unpredictable RestoreGeneration as the first post-restore authority transition**, and rotates JournalEpoch separately for event continuity;
5. rotates all active Run ControlLease tokens before Run/executor commands;
6. treats every restored Grant/CapabilityTicket from the old generation as non-redeemable historical authority; re-affirmation creates a new current-generation Grant rather than reactivating the old row;
7. treats every restored old-generation broker operation as reconciliation-only: inspect by the original stable identity where possible, never reissue merely because restored SQLite says `PENDING`/incomplete;
8. rejects Operator mutations carrying an old `(commandEpoch, commandId)` before command-row lookup/creation; callers must treat the prior outcome as UNKNOWN and inspect current state before intentionally issuing a new command;
9. inventories every external domain capable of containing Pantheon-owned state, including Run- and EvaluationOperation-owned Sandboxes, and reconciles/fences effects newer than or absent from the restored database;
10. requires operator action for un-inventoriable ambiguous domains/operations;
11. opens normal mutation/dispatch only after the recovery barrier is satisfied.

A restored database snapshot is never permission to blindly replay historical external operations.

### Grant replay prevention

A one-use Grant consumed after the backup may appear unused again after restore. The RestoreGeneration mismatch makes it impossible to redeem that restored Grant, independent of the restored use counter.

If the operator wants the same authority again, they explicitly approve/re-affirm it under the current generation. Pantheon therefore preserves the semantic meaning of a bounded human approval even when the database history recording its consumption was lost.

### Broker-operation reconciliation after restore

A restored broker operation may describe an external side effect that happened after the snapshot but before the failure.

Correct handling is:

```text
old-generation broker operation
        ↓
inspect external system using original stable operation/idempotency identity
        ↓
CONFIRMED | NOT_APPLIED | UNKNOWN
```

If CONFIRMED, record the reconciled historical outcome. If NOT_APPLIED is provable, Recovery Policy/operator may intentionally create new current-generation authority if the effect is still desired. If UNKNOWN, remain fenced; do not rotate the operation identity and retry.

### Operator command identity after restore

Operator command idempotency is scoped by:

```text
RestoreGeneration + commandId
```

A restored database may have lost a `commands` row for a command that already produced an external/control-plane effect. Therefore row absence alone can never make an old-epoch request new. `public-daemon-api-and-cli.md` requires stale command epochs to fail closed.

The client observes the new command epoch, treats pre-restore command outcome as UNKNOWN, inspects current resource state, then deliberately chooses whether a new command with a new ID is required.

### JournalEpoch is separate

Restore also rotates JournalEpoch because restored Event history is discontinuous. JournalEpoch is not reused as RestoreGeneration: event-retention/stream continuity and authority/idempotency continuity are independent semantics.

## 28. Clean shutdown

A clean daemon shutdown uses the same durability philosophy.

Recommended sequence:

```text
close dispatch gate
        ↓
stop creating new Attempts/external work
        ↓
persist final controller observations possible within shutdown policy
        ↓
flush/close SQLite cleanly
        ↓
record best-effort incarnation stoppedAt as final durable daemon step
        ↓
release installation lock
```

Clean daemon shutdown does not inherently mean cancelling every external Attempt. Backend execution lifetime is independent where the backend supports that behavior.

Explicit `cancel work` and `stop daemon` are different user intents.

A backend whose native execution necessarily dies with the daemon will simply be reconciled as EXITED on the next start.

## 29. Crash/fault-injection testing is required

Recovery correctness cannot be validated only with happy-path unit tests.

The v1 test plan must inject process termination/crash boundaries around at least:

```text
Run-intent transaction commit
Attempt creation before ensureExecution
backend ensure after external start before acknowledgement
verification Sandbox intent before/after SandboxBackend ensure
EvaluationAttempt creation/contact marker before/after evaluator launch
usage ingestion before/after budget debit
candidate Artifact durable put before/after SQLite metadata
Task candidate commit before lifecycle transition
executor/evaluator termination before reservation release
Sandbox release before reservation release
workspace remove before reservation release
integration commit-object creation before CAS ref update
CAS ref update before IntegrationIntent acknowledgement
finalization obligation satisfaction before terminal transition
```

Restore tests additionally construct an older consistent snapshot, perform newer external/control effects, restore the old snapshot, and assert at least:

```text
consumed one-use Grant cannot redeem again
old-generation CapabilityTicket cannot redeem
restored PENDING broker operation cannot execute again without reconciliation proof
old commandEpoch + commandId cannot become a new command when its row is absent
fresh RestoreGeneration is different from every value recovered from the snapshot
Run- and EvaluationOperation-owned Sandboxes are inventoried/reconciled by durable SandboxKey+holder
```

For each crash point, restart Pantheon and assert that the resulting state is equivalent to either the operation not having happened or having happened exactly once, never a duplicate unsafe effect.

Property/invariant tests should continuously assert:

- no duplicate active Attempt created under UNKNOWN execution;
- no overlapping EvaluationAttempt under ambiguous evaluation contact;
- no overlapping current Sandbox for one Run/EvaluationOperation holder;
- no non-RELEASED Sandbox without exactly one valid durable holder;
- no released reservation while external use/Sandbox existence is uncertain;
- no BudgetHold double-debit from replayed usage;
- no acceptance against corrupt/mismatched Artifact bytes;
- no shared Git ref overwrite after stale CAS expectation;
- no Active Task without exactly one responsible nonterminal Run in valid state;
- no controller command accepted under stale lease token;
- no Grant/CapabilityTicket redeemed across RestoreGeneration;
- no old-generation broker operation reissued as an external effect;
- no Operator command accepted under a stale commandEpoch.

## 30. Recovery passes

Pantheon may record a lightweight RecoveryPass for audit/diagnostics:

```yaml
recoveryPass:
  id: recovery-pass_...
  mode: startup | periodic | manual | restore
  daemonIncarnation: ...
  startedAt: ...
  barrierSatisfiedAt: ...
  completedAt: ...
  findings:
    unresolved: 2
    quarantined: 1
```

Restore-mode RecoveryPass records the old restored generation (as historical metadata where available) and the newly committed RestoreGeneration without treating the old value as authority.

A pass is not required to reach zero findings before scheduler dispatch. It must only reach the recovery barrier: every relevant unresolved item is safely fenced.

## 31. Controller order and dependencies

Pantheon should avoid one giant global recovery controller.

A practical startup dependency order is:

```text
Storage / Installation Authority
        ↓
RestoreGeneration fence (restore mode only)
        ↓
Run ControlLease adoption + EvaluationOperation intent inventory
        ↓
Authorization / broker-operation reconciliation
        ↓
Workspace/materialization ownership reconciliation
        ↓
Sandbox holder + SandboxKey reconciliation
        ↓
Run Attempt + EvaluationAttempt external-execution reconciliation
        ↓
Resource + Budget accounting reconciliation
        ↓
Artifact / Candidate / Evidence availability
        ↓
Integration reconciliation
        ↓
Task / Goal lifecycle reconciliation
        ↓
Scheduler dispatch gate
```

This is a dependency graph, not a requirement that every controller execute serially. In particular, execution inspection may proceed in parallel where safe, but a new Attempt/EvaluationAttempt launch that requires a Sandbox waits for that holder's Sandbox reconciliation/verification result.

Controllers may operate concurrently where dependencies permit, but each publishes enough condition/fencing state for downstream controllers to decide safely.

The global Recovery Coordinator owns only startup gating, restore-generation fencing/pass bookkeeping, and cross-domain invariant scans. It does not absorb domain-specific repair logic.

## 32. v1 scope

Include:

- single-daemon installation lock;
- stable Installation ID and per-start daemon incarnation ID;
- fresh RestoreGeneration rotation on disaster restore;
- generation-bound Grants/CapabilityTickets/broker operations and Operator commands;
- Run ControlLease token rotation plus ownership epoch;
- staged startup and global dispatch gate;
- recovery barrier based on reconciled/fenced/quarantined obligations;
- periodic safety reconciliation using normal controller code;
- finalization obligations for cleanup safety;
- Run/Attempt and EvaluationOperation/EvaluationAttempt recovery;
- holder-driven Run/EvaluationOperation Sandbox reconciliation by durable SandboxKey;
- authorization/broker, Resource, Budget, Workspace, Artifact and Integration reconciliation rules;
- durable RecoveryFindings;
- invariant scanning and quarantine;
- SQLite integrity/version checks and supported backup procedure;
- restore-specific recovery mode;
- crash/fault-injection tests around every external-side-effect boundary.

Defer:

- active-active/multi-daemon Pantheon;
- distributed consensus/lease service;
- automatic destructive orphan reaping;
- automated database page-level salvage;
- cross-machine CAS replication protocol;
- live migration of running Attempts;
- global transaction protocol across external systems.

## Key decisions

1. **Recovery is ordinary idempotent controller reconciliation over durable desired state, not separate startup mutation logic.**
2. **SQLite durable state is authority; external state is observed evidence; in-memory queues/caches are disposable.**
3. **Pantheon v1 uses a stable Installation ID, unique daemon incarnation IDs, and an OS-backed single-daemon installation lock.**
4. **Run control fencing uses both monotonic ownership epoch and a fresh unpredictable lease token; token rotation occurs on adoption/restart/restore before external commands.**
5. **Scheduler dispatch remains closed during startup until every prior external-side-effect obligation is reconciled, fenced, or quarantined.**
6. **The recovery barrier does not require all uncertainty to be resolved; scoped UNKNOWN state may remain while unrelated safe work continues.**
7. **Every consequential external action has durable intent/preconditions before the side effect and durable observation afterward.**
8. **UNKNOWN external outcome never authorizes a blind replacement side effect.**
9. **Cleanup uses durable finalization obligations; ownership/capacity records are not erased until required cleanup is confirmed.**
10. **Missing ownership or inconsistent durable state fails closed and is quarantined rather than guessed/released.**
11. **Executor recovery preserves Attempt/LaunchKey identity; replacement Attempts are created only by Recovery Policy after definitive termination.**
12. **ResourceReservations remain accounting authority during recovery; observed utilization cannot free them.**
13. **Budget/Usage replay is idempotent and truthful; uncertain work retains unspent hold headroom conservatively.**
14. **Workspace recovery never silently recreates potentially lost unsealed mutable work and never interprets Agent-writable Git metadata with ambient controller authority.**
15. **Integration recovery is determined by expected/current/result Git OIDs and compare-and-swap semantics, never force-updating shared refs.**
16. **CAS recovery verifies digest and size; extra immutable objects are safe orphans, while missing/corrupt referenced replicas block consumers but do not mutate Artifact identity.**
17. **Logical invariant violations are durable RecoveryFindings and have explicit auto-repair, reconcile, fence, quarantine, or operator-required dispositions.**
18. **Recovery failures are scoped to the smallest safe blast radius; only authority/storage/global-accounting ambiguity stops all dispatch.**
19. **Pantheon uses SQLite on reliable local storage with WAL, `synchronous=FULL`, and SQLite 3.51.3+ or an official WAL-reset-fix backport.**
20. **Routine startup runs `quick_check` plus `foreign_key_check`; full `integrity_check` is used for suspected corruption/deep diagnosis.**
21. **Live backups use SQLite-supported snapshot APIs; raw database-file copies are not the normal backup mechanism.**
22. **Restoring an old SQLite snapshot rotates a fresh unpredictable RestoreGeneration before any new authority-bearing mutation/effect, because external and human-authority consumption histories may be newer than the snapshot.**
23. **Restored old-generation Grants/Tickets are non-redeemable; re-affirmation creates new current-generation authority rather than reviving rewound use counts.**
24. **Restored old-generation broker operations are reconciliation-only and never authorize blind re-execution from restored PENDING/incomplete state.**
25. **Operator command idempotency is scoped by `(RestoreGeneration, commandId)`; stale command epochs fail closed even when historical command rows are absent.**
26. **Clean daemon shutdown and cancellation of external work are separate intents.**
27. **Crash/fault-injection testing at external-side-effect and restore-replay boundaries is a required v1 quality gate.**
28. **A small Recovery Coordinator gates startup and scans invariants; domain controllers retain domain-specific reconciliation logic.**
29. **Sandbox recovery is holder-driven, not Run-traversal-driven: every non-RELEASED Sandbox is reconciled by immutable SandboxKey plus exactly one Run/EvaluationOperation holder, and ambiguous Sandbox existence blocks overlapping replacement/launch.**
