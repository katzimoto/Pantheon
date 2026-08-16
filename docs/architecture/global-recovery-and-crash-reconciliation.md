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
- filesystem paths without corresponding durable ownership state;
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

## 3. Installation identity and daemon incarnation

Pantheon maintains two different identities.

### Installation ID

A stable random identifier for one Pantheon control-plane installation.

```text
installationId = persistent across normal daemon restarts
```

Where practical, external resources created by Pantheon should carry adapter-specific ownership metadata derived from:

- installation ID;
- Pantheon subject ID;
- operation/LaunchKey where appropriate.

The concrete tag/label mechanism is adapter-private.

The Installation ID is used for inventory and orphan detection. It is not authorization.

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
- ExecutionBindings;
- ResourceReservations not RELEASED;
- BudgetHolds not settled/released;
- WorkspaceRecords not RELEASED;
- pending IntegrationIntents;
- candidate/evidence finalization work;
- Artifact replicas needed by live work;
- unresolved cleanup/finalization obligations;
- prior unresolved RecoveryFindings.

### E. Authority rotation and fencing

Adopt required Run control by incrementing ownership epoch and rotating lease tokens transactionally.

No old controller incarnation may remain authoritative.

### F. Domain reconciliation

Controllers inspect their external domains and either establish current state or place affected resources into conservative fenced states.

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
Run control         → ControlLease leaseToken + epoch
Workspace           → Workspace ID + deterministic desired path/base
Artifact seal       → content digest
Integration         → IntegrationIntent + expected target OID
Resource release    → Reservation ID
Budget settlement   → Hold/Usage source IDs
```

Pantheon does not need one provider-specific universal transaction protocol. It requires each external domain to expose enough identity/inspection semantics to determine whether an operation happened or to safely repeat it.

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

- executor termination confirmed;
- Run-scoped reservations safe to release;
- BudgetHold settled;
- candidate/evidence state durably sealed;
- workspace outputs preserved before deletion;
- managed Git ref/integration state reconciled;
- sandbox/container cleanup confirmed where required.

This is Pantheon's equivalent of a finalizer pattern: durable deletion intent plus controller-owned cleanup, not immediate record disappearance.

## 11. Never delete evidence needed to recover

Recovery-critical records are retained at least through finalization and configured audit retention.

Pantheon must not physically delete:

- nonterminal Run/Attempt identity;
- LaunchKeys;
- ExecutionBindings;
- unresolved Reservations/Holds;
- Workspace ownership records;
- IntegrationIntents;
- unresolved finalization obligations;
- Artifact/Candidate identities referenced by active acceptance;

merely because an in-memory controller believes the work is over.

Garbage collection is a later operation over terminal, unreferenced, fully finalized state.

## 12. Run and Attempt recovery

For every nonterminal Run:

```text
rotate/acquire ControlLease
        ↓
load current nonterminal Attempt, if any
        ↓
inspect backend by Attempt attachment / LaunchKey
```

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
git worktree inventory / filesystem observation
        ↓
Workspace reconciliation
```

Possible cases:

### Expected workspace exists and is coherent

- verify repository/base/worktree identity;
- repair administrative linkage only through Git-supported repair operations where safe;
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

Use stable Git worktree inventory/repair interfaces rather than editing `.git/worktrees/**` directly.

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
- evaluator work that was in progress is reconciled independently;
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

ResourceReservation non-RELEASED
→ holder reference must exist or reservation is quarantined

BudgetHold unsettled
→ holder/source accounting must remain traceable

Candidate
→ all referenced Artifact identities must exist in metadata

Evidence PASS
→ subject/evaluator bindings must be complete
```

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
- mark an already-applied IntegrationIntent APPLIED;
- fetch a missing Artifact replica from another verified replica;
- use Git's supported worktree repair operation when ownership and expected path are unambiguous.

### Reconciliation required

Inspect external state before deciding, for example:

- uncertain executor launch/termination;
- pending IntegrationIntent after crash;
- workspace that may contain unsealed user/Agent work.

### Quarantine / operator required

Examples:

- active Task with missing immutable Run identity;
- nonterminal Run missing immutable ExecutionBinding;
- reservation whose holder disappeared from authoritative state;
- unexplained shared Git ref mutation;
- database integrity failure;
- foreign-key/logical corruption that cannot be repaired from immutable history.

Pantheon must not repair such cases by guessing or deleting evidence.

## 23. Degraded modes and blast-radius isolation

Recovery failures should be scoped when possible.

Examples:

```text
one backend unavailable
→ block/reroute new work requiring that backend only

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
- schema is unsupported/incompletely migrated;
- global resource/budget accounting is internally contradictory in a way that could cause unsafe double allocation.

## 24. API behavior during startup recovery

Once the storage gate is safe, Pantheon may expose inspection/status APIs before dispatch is enabled.

Desired-state writes that do not create immediate external side effects may be accepted and queued, but the dispatch gate remains closed until the recovery barrier is satisfied.

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

### Restore

After restoring an older snapshot:

```text
DO NOT immediately enable Scheduler dispatch
```

External executors, Git refs, worktrees, and object stores may contain effects created after the snapshot.

Restore recovery therefore:

1. acquires installation authority;
2. creates a new daemon incarnation;
3. rotates all active ControlLease tokens before external commands;
4. keeps the dispatch gate closed;
5. inventories every external domain capable of containing Pantheon-owned state;
6. reconciles or fences state newer than/absent from the restored database;
7. requires operator action for un-inventoriable ambiguous domains;
8. opens dispatch only after the recovery barrier is satisfied.

A restored database snapshot is never permission to blindly replay historical external operations.

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
usage ingestion before/after budget debit
candidate Artifact durable put before/after SQLite metadata
Task candidate commit before lifecycle transition
executor termination before reservation release
workspace remove before reservation release
integration commit-object creation before CAS ref update
CAS ref update before IntegrationIntent acknowledgement
finalization obligation satisfaction before terminal transition
```

For each crash point, restart Pantheon and assert that the resulting state is equivalent to either the operation not having happened or having happened exactly once, never a duplicate unsafe effect.

Property/invariant tests should continuously assert:

- no duplicate active Attempt created under UNKNOWN execution;
- no released reservation while external use is uncertain;
- no BudgetHold double-debit from replayed usage;
- no acceptance against corrupt/mismatched Artifact bytes;
- no shared Git ref overwrite after stale CAS expectation;
- no Active Task without exactly one responsible nonterminal Run in valid state;
- no controller command accepted under stale lease token.

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

A pass is not required to reach zero findings before scheduler dispatch. It must only reach the recovery barrier: every relevant unresolved item is safely fenced.

## 31. Controller order and dependencies

Pantheon should avoid one giant global recovery controller.

A practical startup dependency order is:

```text
Storage / Installation Authority
        ↓
Run + Attempt ownership/reconciliation
        ↓
Workspace / Sandbox reconciliation
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

Controllers may operate concurrently where dependencies permit, but each publishes enough condition/fencing state for downstream controllers to decide safely.

The global Recovery Coordinator owns only startup gating, pass bookkeeping, and cross-domain invariant scans. It does not absorb domain-specific repair logic.

## 32. v1 scope

Include:

- single-daemon installation lock;
- stable Installation ID and per-start daemon incarnation ID;
- Run ControlLease token rotation plus ownership epoch;
- staged startup and global dispatch gate;
- recovery barrier based on reconciled/fenced/quarantined obligations;
- periodic safety reconciliation using normal controller code;
- finalization obligations for cleanup safety;
- Run/Attempt, Resource, Budget, Workspace, Artifact and Integration reconciliation rules;
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
14. **Workspace recovery never silently recreates potentially lost unsealed mutable work.**
15. **Integration recovery is determined by expected/current/result Git OIDs and compare-and-swap semantics, never force-updating shared refs.**
16. **CAS recovery verifies digest and size; extra immutable objects are safe orphans, while missing/corrupt referenced replicas block consumers but do not mutate Artifact identity.**
17. **Logical invariant violations are durable RecoveryFindings and have explicit auto-repair, reconcile, fence, quarantine, or operator-required dispositions.**
18. **Recovery failures are scoped to the smallest safe blast radius; only authority/storage/global-accounting ambiguity stops all dispatch.**
19. **Pantheon uses SQLite on reliable local storage with WAL, `synchronous=FULL`, and SQLite 3.51.3+ or an official WAL-reset-fix backport.**
20. **Routine startup runs `quick_check` plus `foreign_key_check`; full `integrity_check` is used for suspected corruption/deep diagnosis.**
21. **Live backups use SQLite-supported snapshot APIs; raw database-file copies are not the normal backup mechanism.**
22. **Restoring an old SQLite snapshot always triggers restore recovery/inventory because external state may be newer than the snapshot.**
23. **Clean daemon shutdown and cancellation of external work are separate intents.**
24. **Crash/fault-injection testing at external-side-effect boundaries is a required v1 quality gate.**
25. **A small Recovery Coordinator gates startup and scans invariants; domain controllers retain domain-specific reconciliation logic.**
