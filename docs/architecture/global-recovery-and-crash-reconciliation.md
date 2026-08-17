# Global Recovery and Crash Reconciliation

## Status

Canonical Pantheon recovery and crash-safety specification.

## Purpose

This document defines how Pantheon reconstructs safe control after daemon crashes, machine restarts, partial external side effects, storage inconsistencies, database restore, and divergence between SQLite desired state and the external world.

The central rule is:

> **Recovery does not mean returning every object to a known state before Pantheon can operate. Recovery is safe once every durable external-side-effect obligation is either reconciled to a known state or explicitly fenced so that new work cannot conflict with it.**

Pantheon therefore treats restart recovery as ordinary controller reconciliation over durable desired state, not as a separate imperative repair script.

See also:

- `docs/architecture/task-lifecycle.md`
- `docs/architecture/run-and-attempt.md`
- `docs/architecture/planner-and-task-decomposition.md`
- `docs/architecture/evaluation-and-evaluator-registry.md`
- `docs/architecture/goal-lifecycle-and-completion-controller.md`
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

- immutable Goal/GoalRevision/GoalCompletionCandidate records and pinned Goal acceptance contracts;
- immutable Task/TaskSpec/Run/Attempt/Binding/Candidate/Artifact records and pinned Task acceptance contracts;
- durable PlanningOperation/PlanningAttempt identities plus immutable PlanningRecord results where external planning exists;
- immutable EvaluationRound typed-subject identity, exact pinned evaluator set, Evidence and AcceptanceResult records;
- durable EvaluationOperation/EvaluationAttempt identities where external verification exists;
- durable SandboxInstance holder/SandboxKey identity, Sandbox lifecycle phase, external presence observation and SandboxVerification facts;
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
- backend callbacks received without current fencing authority;
- a bare Task/Goal ID without the concrete immutable EvaluationRound subject it is supposed to judge.

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

Pantheon maintains distinct ownership/authority identities and fences.

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

It fences authority whose durable consumption/idempotency history can be rewound by restore, including runtime Grants, CapabilityTickets, broker operations, Operator command identities, and Agent Control sessions.

It is deliberately not a monotonic counter restored from the database: an old snapshot can reintroduce a previously used numeric value. The new generation is random/fresh and is committed before any new post-restore authority-bearing mutation or external effect.

`RestoreGeneration` is distinct from:

- Installation ID — stable ownership identity;
- daemon incarnation — process/controller lifetime;
- Run ControlLease epoch/token — Run-control ownership;
- JournalEpoch — Event-stream continuity.

### Restore-entry latch

A restored database cannot reliably announce that it is an older version of itself. The evidence needed to detect the rewind may have been rewound with the database.

Therefore disaster restore is a **supported installation-maintenance procedure**, not an ordinary daemon startup that Pantheon tries to infer after the fact. Before replacing `pantheon.db`, the maintenance path acquires exclusive installation authority and durably creates a small out-of-database `restore.pending` latch containing at least:

```text
restoreOperationId       fresh non-reused random ID
expectedInstallationId
backup identity/digest
createdAt
```

The latch contains no secret, credential, Grant, bearer capability, or external-effect authority. It is only a crash-safe indication that ordinary startup is forbidden until the matching restore authority fence has committed. It is excluded from SQLite backup payloads and is fsynced together with its parent-directory metadata before database replacement begins.

The same `restoreOperationId` is recorded in the durable restore RecoveryPass at T0. This gives restart-safe interpretation:

```text
latch exists + no matching fenced RecoveryPass
→ T0 has not been proven committed
→ perform/complete T0 before authority-bearing work

latch exists + matching IN_PROGRESS restore RecoveryPass
→ T0 already committed for this restore operation
→ resume the same restore pass; do not rotate again merely because the latch survived

matching IN_PROGRESS restore RecoveryPass + latch already cleared
→ resume restore reconciliation from SQLite durable state
```

The latch is cleared durably only after Pantheon establishes that the matching T0 transaction committed.

Raw/manual replacement of `pantheon.db` without first establishing this supported restore latch is outside the safe disaster-restore contract. Pantheon cannot repair that omission by trusting a rewound database to prove its own rewind.

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

The supported disaster-restore maintenance path uses the same exclusive installation authority while establishing/removing the restore latch and replacing the database snapshot. A normal daemon must not race the restore procedure.

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

RestoreGeneration does not replace ControlLease fencing. RestoreGeneration prevents replay of rewound authorization/command/worker authority; ControlLease token+epoch fences Run-controller ownership.

## 6. Startup phases

Startup is staged so unsafe external actions remain blocked until the persisted world has been fenced.

```text
PROCESS START
    ↓
A. installation lock
    ↓
B. restore-entry mode check + storage recovery / validation
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

Ordinary restart has no pending restore operation and preserves the existing RestoreGeneration. Supported disaster restore enters startup with the out-of-database restore latch and executes/resumes the matching restore authority fence in §27 before any normal authority-bearing mutation or external effect.

### A. Installation lock

Acquire exclusive v1 daemon authority.

### B. Restore-entry mode and storage recovery/validation

Before treating the opened database as ordinary history, inspect the installation restore latch. A pending restore operation forces restore mode; the database is never allowed to downgrade that fact to ordinary startup.

Open SQLite normally so SQLite can perform its own journal/WAL recovery. Validate installation identity, schema/migration compatibility and configured database consistency checks before controllers are allowed to perform side effects.

If the latch is present and no matching committed restore RecoveryPass exists, T0 remains required. If the matching restore RecoveryPass is already IN_PROGRESS, startup resumes that pass rather than rotating a second RestoreGeneration.

### C. Incarnation registration

Persist the new daemon incarnation and keep the global dispatch gate closed. Incarnation bookkeeping does not grant effect authority and cannot bypass pending T0.

### D. Durable inventory

Load at least:

- nonterminal Goals and Tasks;
- Active/Evaluating/Finalizing Goals and Tasks;
- current GoalCompletionCandidates and Task Candidates referenced by active acceptance;
- nonterminal Runs and Attempts;
- nonterminal PlanningOperations and PlanningAttempts plus unresolved planning control-operation accounting;
- active/current EvaluationRounds, their concrete typed subjects, Evidence/AcceptanceResults and nonterminal EvaluationOperations/EvaluationAttempts;
- ExecutionBindings;
- every SandboxInstance whose phase is not RELEASED, plus any `RELEASED+UNKNOWN` Sandbox lacking a valid force-resolution tombstone/fence, together with its durable holder/latest status/SandboxVerification;
- ResourceReservations not RELEASED;
- BudgetHolds not settled/released;
- WorkspaceRecords not RELEASED;
- pending IntegrationIntents;
- candidate/evidence/finalization work;
- Artifact replicas needed by live work;
- unresolved cleanup/finalization obligations;
- prior unresolved RecoveryFindings.

An EvaluationRound is inventoried by its own durable identity and concrete subject edge. Recovery never reconstructs Goal-level EvaluationRound ownership by pretending it has a Task ID.

Sandbox inventory is **not derived only by walking Runs**. Verification Sandboxes belong to EvaluationOperations and must remain discoverable/reconcilable even when no Run owns them. PlanningOperations do not own Sandboxes in v1 unless a future architecture adds an explicit concrete holder edge.

In restore mode the inventory also includes Grants, CapabilityTickets, broker operations, Commands, AgentControlSessions, and the matching restore RecoveryPass because restored rows may represent authority/idempotency history older than external reality.

### E. Authority rotation and fencing

Adopt required Run control by incrementing ownership epoch and rotating lease tokens transactionally.

No old controller incarnation may remain authoritative.

In restore mode, the new RestoreGeneration has already been committed before this point. Old-generation Grants/Tickets cannot redeem, old-generation broker operations are reconciliation-only, and old-generation AgentControlSessions cannot authorize semantic worker requests.

### F. Domain reconciliation

Controllers inspect their external domains and either establish current state or place affected resources into conservative fenced states.

Sandbox holder/SandboxKey reconciliation is a prerequisite for issuing a new launch in any execution lineage that requires that Sandbox. Run and Evaluation controllers may inspect their execution domains concurrently, but neither a normal Attempt nor an EvaluationAttempt may launch/relaunch through an unresolved required Sandbox. Sandbox lifecycle `phase` and factual `observedPresence` are reconciled independently; lifecycle ERROR/RELEASING never substitutes for proof of external absence.

Planning Controller independently reconciles nonterminal PlanningAttempts by their durable PlanningAttempt identity/correlation. An unresolved `CONTACT_MAY_HAVE_OCCURRED` Planner call is not permission to issue an overlapping replacement call.

Evaluation Controller first validates the EvaluationRound subject relationship before it treats any evaluator process/Evidence as actionable acceptance state. A Round with a broken/mismatched typed subject is quarantined rather than guessed into Task or Goal ownership.

Restore mode has an additional certainty rule: negative observations recovered only from the snapshot are historical facts about the snapshot point. They do not prove that an external effect did not occur after that point. Fresh external inspection/inventory or an equivalent current fence must establish absence before a replacement/conflicting effect is authorized.

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

In restore mode the barrier also requires the matching restore RecoveryPass/T0 generation fence to exist durably. A surviving latch without a matching committed fence is a global stop condition, not a warning.

### H. Dispatch gate

Only after the barrier is satisfied may the Scheduler commit new Runs.

Planner/Graph reconciliation may also continue only within its own safe fences: creating a new external PlanningAttempt is effect-creating control work and cannot bypass an unresolved prior PlanningAttempt or the global recovery barrier.

Evaluation/acceptance recovery may preserve historical Evidence while the global barrier is closed, but applying that Evidence to Task/Goal lifecycle always requires the concrete Round subject and owning controller currentness checks.

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

A single uncertain Run, PlanningOperation or EvaluationOperation must not freeze all Goals indefinitely when its accounting/authority blast radius can be safely fenced.

A stale but internally valid EvaluationRound similarly does not require a global stop: its Evidence remains historical and its owning Task/Goal simply cannot consume it as current acceptance.

Global dispatch remains disabled only when Pantheon cannot establish safe accounting/authority boundaries system-wide, such as database integrity failure, unreconciled installation ownership, or an incomplete disaster-restore generation fence.

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
Planning call       → PlanningAttempt ID + contact marker
Evaluation launch   → EvaluationAttempt ID + launch-contact marker
Agent Control       → AgentControlSession + RestoreGeneration + Attempt request identity
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

Evaluation subject identity itself is not an external-operation idempotency key. It is the immutable semantic target that every EvaluationOperation/Evidence record must resolve through.

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

### Restore-specific negative evidence rule

A disaster restore creates a temporal cut: the SQLite snapshot may be older than the current external world.

Therefore any negative fact whose only evidence is the restored snapshot is not current negative proof. Examples include:

```text
Attempt.launchContactState = NOT_CONTACTED
PlanningAttempt.contactState = NOT_CONTACTED
EvaluationAttempt.launchContactState = NOT_CONTACTED
observed execution/sandbox = ABSENT
broker operation row absent or PENDING
Agent request/Operator command row absent
no later Event/Evidence/PlanningRecord row present
```

These facts remain useful historical evidence about the snapshot point, but they cannot establish `NOT_APPLIED` for the post-snapshot interval. The external domain must be freshly inventoried/inspected, or an explicit isolation/fencing property must make a conflicting effect impossible, before Pantheon may authorize replacement work or conclude that an external side effect did not occur.

This rule is restore-specific. On an uninterrupted authoritative database history, the normal durable launch/contact markers retain their ordinary negative-proof semantics.

EvaluationRound subject validity is a separate relational fact: a restored Round still points to the exact immutable Task Candidate or GoalCompletionCandidate from the snapshot, but that alone does not make the subject current for lifecycle application after post-snapshot revisions.

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

- executor/evaluator/planner external contact/termination/result certainty where applicable;
- Run/control-operation reservations safe to release;
- BudgetHold settled;
- candidate/evidence/planning result state durably sealed where required;
- workspace outputs preserved before deletion;
- managed Git ref/integration state reconciled;
- Run or verification Sandbox cleanup confirmed absent, or explicitly force-resolved with a durable lineage fence plus a separate safe capacity/accounting disposition where physical occupancy remains uncertain.

This is Pantheon's equivalent of a finalizer pattern: durable deletion intent plus controller-owned cleanup, not immediate record disappearance.

## 11. Never delete evidence needed to recover

Recovery-critical records are retained at least through finalization and configured audit retention.

Pantheon must not physically delete:

- nonterminal Run/Attempt identity;
- nonterminal PlanningOperation/PlanningAttempt identity and contact facts;
- active/current EvaluationRound identity, typed subject edge, pinned evaluator bindings, AcceptanceResult/Evidence required by current acceptance;
- nonterminal EvaluationOperation/EvaluationAttempt identity;
- LaunchKeys and evaluation/planning launch-contact facts;
- ExecutionBindings;
- current GoalCompletionCandidate/Task Candidate identities referenced by active acceptance;
- non-RELEASED SandboxInstance holder/SandboxKey identity and required verification history;
- `RELEASED+UNKNOWN` Sandbox history/tombstone/fence needed to explain why replacement authority was safe despite unresolved physical existence;
- unresolved Reservations/Holds;
- Workspace ownership records;
- IntegrationIntents;
- unresolved finalization obligations;
- Artifact/Candidate identities referenced by active acceptance;

merely because an in-memory controller believes the work is over.

Historical stale EvaluationRounds/Evidence may later age out only under configured retention after no current lifecycle/recovery obligation references them.

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

A normal Attempt may not be newly launched/relaunched until its required Run-owned Sandbox is reconciled and verified. Existing external execution may be inspected concurrently, but unresolved Sandbox presence is never interpreted as permission to provision a replacement Sandbox.

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

On ordinary uninterrupted history, an Attempt that is durably `NOT_CONTACTED` and has no independent external evidence may use the launch-contact rule from `run-and-attempt.md` as proof that Pantheon's launch path never crossed the call boundary. In restore mode, a restored `NOT_CONTACTED` value is snapshot evidence only and does **not** establish absence for the post-snapshot interval; fresh backend inventory/inspection or an equivalent current fence is required before launch/replacement decisions rely on it.

If a backend supports inventory of Pantheon-owned executions, recovery should also compare that inventory against durable Attempts to discover dangling executions.

Unknown/dangling native executions are quarantined and reported before destructive cleanup.

### Agent Control after restore

AgentControlSession is part of external-execution authority, not merely authentication metadata.

Every session is immutably bound to the RestoreGeneration in which it was created. Ordinary restart preserves that generation and therefore preserves same-Attempt session continuity. Disaster restore rotates the generation, so restored sessions from the snapshot are old-generation authority even if their row says `ACTIVE`.

Before an Agent request lookup/idempotency decision or semantic worker mutation, Agent Control requires:

```text
session.restoreGeneration == current RestoreGeneration
```

A mismatch fails closed. The old session row is not rewritten to current. A still-running pre-restore worker may be inspected/reconciled or terminated by Pantheon controllers, but its credential cannot submit Candidates, spawn Tasks, invoke broker actions, or create/replay Agent requests.

Pantheon does not automatically mint a replacement current-generation AgentControlSession for the same Attempt solely because recovery would otherwise stall. Same-Attempt credential/session rotation is a separate protocol and cannot be invented implicitly by restore recovery.

### PlanningOperation and PlanningAttempt recovery

For every nonterminal PlanningOperation with external execution:

```text
load current nonterminal PlanningAttempt, if any
        ↓
interpret contact_state
        ↓
inspect/reconcile same PlanningAttempt identity/correlation where external contact may have occurred
        ↓
recover immutable PlanningRecord if a valid result can be established
        ↓
Graph Controller independently revalidates Goal/Graph/policy before materialization
```

On uninterrupted authoritative history:

```text
NOT_CONTACTED + no independent external evidence
→ Pantheon's external Planner call path did not cross its call boundary

CONTACT_MAY_HAVE_OCCURRED
→ provider may have executed, charged, or produced a result
→ reconcile same PlanningAttempt identity
→ no overlapping PlanningAttempt while unresolved
```

If the Planner/backend supports stable request lookup/correlation, Pantheon uses the original PlanningAttempt identity/adapter attachment. If the backend cannot establish whether an ambiguously contacted request executed, the attempt remains UNKNOWN/fenced rather than being blindly resent.

PlanningOperation holds relevant control-operation ResourceReservations/BudgetHolds until finalization/reconciliation proves them safe to release. A local deterministic PlanningOperation with no external attempt has no invented external recovery obligation.

A recovered Planner response has **zero direct Graph authority**. Pantheon may record the immutable PlanningRecord/result as historical truth, but a stale GoalRevision/GraphRevision means Graph Controller rejects materialization rather than rewriting the operation or applying an old proposal.

In restore mode, a restored PlanningAttempt `NOT_CONTACTED` value or absence of a PlanningRecord is historical snapshot evidence only. Fresh planner-domain inspection/correlation or an equivalent current fence must establish that no post-snapshot request/result exists before replacement external planning relies on that negative fact.

### Sandbox holder reconciliation

Recovery treats Sandbox controller lifecycle and external existence certainty as separate durable facts:

```text
phase:
  REQUESTED | PREPARING | READY | RELEASING | RELEASED | ERROR

observedPresence:
  PRESENT | ABSENT | UNKNOWN
```

`UNKNOWN` is never a lifecycle phase. `ERROR` or a cleanup timeout never proves absence.

Recovery independently walks every Sandbox whose phase is not `RELEASED`, and also any `RELEASED+UNKNOWN` row that lacks a valid matching force-resolution tombstone/fence. It resolves the immutable holder and re-inspects the same SandboxKey.

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
- persist factual `observedPresence` independently from lifecycle `phase`;
- restore/refresh factual SandboxVerification where required;
- keep corresponding ResourceReservation capacity charged until absence or a separately safe capacity disposition is established;
- never provision an overlapping second Sandbox for the same holder while prior existence is UNKNOWN and unfenced.

Normal cleanup uses:

```text
RELEASING + ABSENT
→ RELEASED
```

while ambiguous combinations remain conservative:

```text
ERROR + UNKNOWN
→ runtime may still exist
→ remain fenced; no blind replacement

RELEASING + UNKNOWN
→ destruction outcome ambiguous
→ remain fenced; no blind replacement
```

`RELEASED+PRESENT` is an invariant violation. `RELEASED+UNKNOWN` is valid only when an explicit audited force-resolution produced a matching durable lineage tombstone/fence. The observation stays `UNKNOWN`; Pantheon never fabricates `ABSENT` to make cleanup look successful.

A force-resolution tombstone fences the old SandboxKey's **authority/replacement identity**. It does not automatically prove underlying CPU/RAM/disk/container capacity is physically free. Recovery must separately keep/quarantine capacity or record another domain-specific safe accounting disposition before allocating capacity that could conflict with the uncertain runtime.

If the holder is terminal but Sandbox cleanup is incomplete, the Sandbox remains a cleanup/finalization obligation and capacity is not released merely because the holder stopped executing.

If the durable holder is missing, the holder-kind/FK relationship is inconsistent, or an inventoried external Sandbox has no corresponding durable SandboxInstance, quarantine it. Do not reinterpret it as free capacity or automatically destroy it.

PlanningOperation is deliberately absent from this Sandbox holder list in v1. If future planning gains a Sandbox, recovery gains the matching concrete holder edge/inventory rule rather than treating all control operations as implicit Sandbox owners.

### EvaluationRound subject recovery

Before EvaluationOperation/Evidence reconciliation, Recovery validates the immutable Round subject relationship:

```text
EvaluationRound
  subjectKind = TASK_CANDIDATE
  -> exactly one existing Candidate FK
  -> Candidate resolves to its immutable Task/TaskSpec acceptance contract

OR

EvaluationRound
  subjectKind = GOAL_COMPLETION_CANDIDATE
  -> exactly one existing GoalCompletionCandidate FK
  -> candidate resolves to its immutable GoalRevision acceptance contract
```

The Round's acceptance hash, criterion set and exact EvaluatorVersions must match the pinned immutable acceptance contract for that subject. A Round with both/neither subject FKs, a dangling concrete subject, or evaluator bindings inconsistent with the owning semantic contract is a relational/logical corruption finding and is quarantined. Recovery never guesses the intended owner from Events, Task IDs, criterion names or nearby history.

Currentness is distinct from validity:

```text
valid historical Task Candidate Round
+ Task now cancelled/requeued/new Candidate
→ Evidence remains history
→ cannot mutate current Task

valid historical GoalCompletionCandidate Round
+ GoalRevision/candidate advanced
→ Evidence remains history
→ cannot mutate current Goal
```

### EvaluationOperation and EvaluationAttempt recovery

For every externally executing nonterminal EvaluationOperation:

```text
validate parent EvaluationRound typed subject
        ↓
resolve/reconcile EvaluationOperation-owned verification Sandbox
        ↓
load current nonterminal EvaluationAttempt, if any
        ↓
interpret launch_contact_state
        ↓
inspect/reconcile same EvaluationAttempt identity where external contact may have occurred
```

On uninterrupted history, `NOT_CONTACTED` with no independent evidence means the evaluator launch path did not cross its call boundary. `CONTACT_MAY_HAVE_OCCURRED` remains UNKNOWN until the same EvaluationAttempt identity is reconciled/terminated. No overlapping EvaluationAttempt or replacement verification Sandbox is created from ambiguity.

In restore mode, a restored EvaluationAttempt `NOT_CONTACTED` value is historical snapshot evidence only. Fresh evaluation-domain inspection/inventory or a current isolation fence must establish that no post-snapshot evaluator execution exists before the negative value can authorize launch/replacement.

A verification Sandbox can survive from EvaluationAttempt 1 to a later bounded EvaluationAttempt 2 only after attempt 1 is definitively terminal and only while the Sandbox's immutable identity/materialization, verification, resource ownership and current policy remain valid.

A recovered evaluator result may be committed as immutable Evidence only when its EvaluationOperation/Round/criterion/EvaluatorVersion provenance is valid. Lifecycle application then rechecks current typed subject ownership through the Task Controller or Goal Completion Controller; the evaluator/recovery scanner never directly changes Task/Goal phase.

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

Planner backends/adapters should likewise preserve the strongest feasible correlation/inspection contract for a known PlanningAttempt ID/attachment. A provider may expose native idempotency or request lookup; another may only support conservative observation. Pantheon does not claim keyed idempotency when the external mechanism lacks it.

If a restored control-plane snapshot is older than external execution state and a backend cannot inventory/correlate Pantheon-owned work, Pantheon must conservatively block conflicting new execution on that backend/domain until an operator resolves the ambiguity or isolation guarantees prove duplicate execution impossible.

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

For Sandbox capacity, lifecycle phase alone never proves capacity free. `observedPresence=UNKNOWN` keeps capacity charged/UNCERTAIN unless recovery has a separate domain-specific safe accounting disposition. A force-resolution tombstone can fence the old SandboxKey from authority/replacement identity, but it does not manufacture physical absence or automatically release capacity.

For PlanningOperation control-operation capacity, an unresolved PlanningAttempt independently means the external planner work may still be using/charging the reserved resource. The reservation remains charged/UNCERTAIN until the planning lineage is reconciled or explicitly resolved.

Missing ownership evidence is never interpreted as free capacity.

Capacity publishers may refresh `allocatable`, but existing reservations remain accounting authority until safe release is proven.

## 15. Budget and usage reconciliation

Usage truth and spending authority remain separate.

On restart:

- replay/ingest backend usage using stable operation/source IDs;
- de-duplicate previously processed usage;
- convert confirmed held quantity to consumed quantity;
- never reduce consumed usage because an Attempt/control-operation attempt failed;
- keep unused hold headroom reserved while external execution/contact is UNKNOWN;
- settle/release only the provably unused remainder when Run/control work is finalized.

If external billing/allowance state is authoritative, refresh the external snapshot and record freshness before relying on new headroom.

If actual external usage exceeds a Pantheon limit, record the true usage and mark the budget overdrawn; do not clamp history to the configured ceiling.

PlanningOperation backend-authored usage is accepted only under the immutable metering-source provenance frozen before PlanningAttempt contact. A missing Planner response does not prove zero usage; UNKNOWN contact retains/fences unresolved hold headroom conservatively.

During disaster restore, restored negative contact/usage observations are not assumed to describe the post-snapshot interval. Usage remains factual and may arrive late under immutable source provenance even while worker/control authority from the old generation is fenced.

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

Task Candidates and GoalCompletionCandidates are immutable/content-addressed acceptance subjects.

On restart, first validate every active/current EvaluationRound's typed subject relationship and pinned acceptance contract before applying acceptance state.

For Task acceptance:

```text
EvaluationRound.subject = TASK_CANDIDATE
→ exact Candidate exists
→ Candidate belongs to Task/TaskSpec
→ Round acceptance hash/EvaluatorVersions match immutable TaskSpec acceptance contract
→ Task currentness decides whether Evidence may still affect lifecycle
```

For Goal acceptance:

```text
EvaluationRound.subject = GOAL_COMPLETION_CANDIDATE
→ exact GoalCompletionCandidate exists
→ candidate belongs to exact GoalRevision/Graph/deliverable snapshot
→ Round acceptance hash/EvaluatorVersions match immutable GoalRevision acceptance contract
→ Goal/current completion-candidate currentness decides whether Evidence may still affect lifecycle
```

A Goal completion Round never requires or fabricates `task_id`. A Task Candidate Round never points to a GoalCompletionCandidate.

Existing Evidence remains valid historical evidence only for the same Round subject/criterion/EvaluatorVersion. A subject becoming stale for current lifecycle does not rewrite or delete its Evidence.

Evaluator work that was in progress is reconciled through its EvaluationOperation, verification Sandbox and EvaluationAttempt identities. ERROR/UNKNOWN evaluator state never becomes PASS.

Task success/finalization is derived only after all required Evidence for its current Task-Candidate Round is durably present and Task currentness is rechecked. Goal acceptance/finalization is derived only after all required Evidence for its current GoalCompletionCandidate Round is present and Goal currentness is rechecked by Goal Completion Controller.

If a worker had mutable output but no Task Candidate was durably committed before the crash, Pantheon must not infer a candidate from logs or narration. It may later seal the preserved frozen workspace only through the normal candidate-sealing path if Run/Recovery policy still authorizes that operation and the exact intended output can be established.

If a Goal was structurally ready but its GoalCompletionCandidate was not durably committed before the crash, Recovery recomputes readiness from durable accepted deliverable bindings/current Goal/Graph state through the normal Goal Completion Controller; it does not invent a completion candidate from Event narration.

After disaster restore, a still-running worker from the pre-restore history cannot use an old-generation AgentControlSession to submit a missing Task Candidate. Its external process may be reconciled/finalized, but worker semantic authority remains fenced unless a separately defined recovery protocol creates new current authority.

PlanningRecord recovery is separate from Candidate/Acceptance: a recovered Planner result is immutable planning provenance and may be retained even when its proposed GraphPatch is stale. It never becomes a Candidate or bypasses Graph Controller validation.

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

AgentControlSession accepted for semantic request
→ session RestoreGeneration must equal current RestoreGeneration

PlanningAttempt nonterminal
→ parent PlanningOperation must exist and no sibling PlanningAttempt is nonterminal
→ external metering provenance, when present, must match the immutable PlanningOperation

PlanningRecord
→ parent PlanningOperation exists
→ external attempt reference, when present, belongs to that operation

EvaluationRound
→ subjectKind is TASK_CANDIDATE xor GOAL_COMPLETION_CANDIDATE
→ exactly one concrete subject FK exists and resolves
→ acceptance hash/criterion EvaluatorVersions match owning immutable TaskSpec or GoalRevision contract

EvaluationOperation
→ parent EvaluationRound exists
→ criterion/EvaluatorVersion belongs to that Round

EvaluationAttempt nonterminal
→ parent EvaluationOperation must exist and no sibling EvaluationAttempt is nonterminal

Evidence / AcceptanceResult
→ exact EvaluationRound exists
→ subject/criterion/EvaluatorVersion provenance matches that Round

current Task acceptance application
→ Round subject is Task's exact current Candidate and Task is current/Evaluating

current Goal acceptance application
→ Round subject is Goal's exact current GoalCompletionCandidate and represented GoalRevision is current/Evaluating

SandboxInstance
→ exactly one valid holder exists: Run xor EvaluationOperation
→ phase and observedPresence are independently valid domains
→ phase=RELEASED + observedPresence=PRESENT is invalid
→ phase=RELEASED + observedPresence=UNKNOWN requires matching durable force-resolution tombstone/fence
→ no overlapping replacement-authoritative Sandbox exists for the same holder while an older Sandbox is non-RELEASED or unresolved/unfenced

ResourceReservation non-RELEASED
→ holder reference must exist or reservation is quarantined

BudgetHold unsettled
→ holder/source accounting must remain traceable

Candidate / GoalCompletionCandidate
→ all referenced Artifact/producer identities required by the immutable subject must exist
```

In restore mode the scanner additionally checks that any newly redeemable Grant/Ticket, executable broker operation, accepted Operator command, or semantic Agent Control session belongs to the current RestoreGeneration. Old-generation broker operations and AgentControlSessions may remain only as historical/reconciliation state and may not become executable authority.

The scanner also verifies that a restore RecoveryPass carries the same `restoreOperationId` as any still-present restore latch and that the pass records the fresh generation established for that restore operation.

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
- reattach a known Attempt using the same LaunchKey on ordinary uninterrupted history;
- resume/reconcile a known PlanningAttempt by the same durable attempt identity when external correlation makes that safe;
- re-inspect a known Sandbox by the same SandboxKey and durable holder;
- advance Sandbox `RELEASING -> RELEASED` after fresh observation establishes `ABSENT`;
- mark an already-applied IntegrationIntent APPLIED;
- fetch a missing Artifact replica from another verified replica;
- use Git's supported worktree repair operation when ownership and expected path are unambiguous;
- classify a valid but stale EvaluationRound/Evidence set as historical without rewriting it.

### Reconciliation required

Inspect external state before deciding, for example:

- uncertain executor/evaluator/planner external contact/termination/result;
- restored negative launch/contact state that cannot establish the post-snapshot interval;
- pre-restore worker execution whose Agent Control session is old-generation;
- Sandbox existence/cleanup `UNKNOWN` for a Run or EvaluationOperation, including `ERROR+UNKNOWN` or `RELEASING+UNKNOWN`;
- pending IntegrationIntent after crash;
- old-generation broker operation whose effect may have occurred after the restored snapshot;
- workspace that may contain unsealed user/Agent work;
- current Task/Goal acceptance whose external EvaluationAttempt is unresolved but whose typed Round subject remains valid.

### Quarantine / operator required

Examples:

- active Task with missing immutable Run identity;
- nonterminal Run missing immutable ExecutionBinding;
- nonterminal PlanningAttempt whose parent PlanningOperation/meters cannot be established;
- EvaluationRound with both/neither concrete subject FK;
- EvaluationRound whose concrete subject is missing or whose evaluator bindings conflict with the pinned owning acceptance contract;
- Evidence claiming a subject/EvaluatorVersion different from its Round;
- Sandbox with missing/inconsistent holder;
- Sandbox `RELEASED+PRESENT`;
- Sandbox `RELEASED+UNKNOWN` without a matching force-resolution tombstone/fence;
- runtime Sandbox discovered with no corresponding durable SandboxInstance ownership record;
- reservation whose holder disappeared from authoritative state;
- unexplained shared Git ref mutation;
- old-generation broker operation whose external outcome cannot be inventoried/established;
- database integrity failure;
- restore latch/install identity mismatch;
- foreign-key/logical corruption that cannot be repaired from immutable history.

Pantheon must not repair such cases by guessing or deleting evidence.

## 23. Degraded modes and blast-radius isolation

Recovery failures should be scoped when possible.

Examples:

```text
one backend unavailable
→ block/reroute new work requiring that backend only

one PlanningAttempt UNKNOWN
→ fence that PlanningOperation and retain its accounting authority
→ unrelated planning/work may continue if resource/budget/authority boundaries remain safe

one verification Sandbox UNKNOWN
→ fence that EvaluationOperation and retain its capacity unless separately proven safe to reallocate
→ unrelated evaluation/work may continue if capacity/authority remain safe

one stale Goal EvaluationRound
→ retain its immutable Evidence as history
→ reject it for current Goal completion
→ unrelated Goals/evaluation continue

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
- `restore.pending` exists but its matching T0/restore RecoveryPass cannot be established;
- disaster-restore RestoreGeneration fence has not been durably committed;
- schema is unsupported/incompletely migrated;
- global resource/budget accounting is internally contradictory in a way that could cause unsafe double allocation.

## 24. API behavior during startup recovery

Once the storage gate is safe, Pantheon may expose inspection/status APIs before dispatch is enabled.

Desired-state writes that do not create immediate external side effects may be accepted and queued during ordinary startup, but the dispatch gate remains closed until the recovery barrier is satisfied.

During disaster restore, no authority-broadening or effect-creating Operator mutation may be accepted until the matching T0 has committed the new RestoreGeneration. Requests carrying a pre-restore command epoch fail closed rather than being reinterpreted as new commands.

Agent Control is stricter: an old-generation session cannot perform semantic Agent requests even after T0. Its external Attempt remains recovery state, not worker authority under the new generation.

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

The snapshot necessarily includes the then-current RestoreGeneration, Grants, CapabilityTickets, broker operations, Commands and AgentControlSessions. Those rows are historical authority after an older backup is restored until the post-restore authority fence is established.

The snapshot also contains immutable Task/Goal acceptance subjects/Rounds/Evidence from its point in history. Those records remain historical truth after restore but are not automatically current if the external/post-snapshot world or later semantic revisions diverged before failure.

The `restore.pending` latch is installation-maintenance state and is deliberately not part of the SQLite backup payload.

### Supported restore entry

Pantheon cannot safely distinguish a normal restart from arbitrary rollback of `pantheon.db` by consulting the rolled-back database alone. Safe v1 disaster restore therefore begins **before** the database is replaced:

```text
acquire exclusive installation maintenance lock
        ↓
validate selected backup metadata enough to identify intended installation
        ↓
create durable restore.pending with fresh restoreOperationId
        ↓
replace/install the selected consistent SQLite snapshot
        ↓
start/open Pantheon in forced restore mode
```

If the process crashes after the database replacement but before T0, the external latch survives and forces restore mode on the next startup. Raw file replacement that skips this step is not equivalent to the supported recovery procedure.

### Restore authority fence

After restoring an older snapshot:

```text
DO NOT immediately enable Scheduler dispatch
DO NOT redeem restored Grants/Tickets
DO NOT execute restored pending broker operations
DO NOT accept an old command epoch as a new command
DO NOT accept semantic Agent Control from restored old-generation sessions
DO NOT blindly resend restored PlanningAttempts whose post-snapshot outcome is unknown
DO NOT treat restored negative observations as proof that later external effects never happened
DO NOT apply restored acceptance merely from Task/Goal phase without revalidating the exact typed EvaluationRound subject
```

External executors, Planner/evaluator calls, Sandboxes, Git refs, worktrees, object stores, credential-backed operations and other services may contain effects created after the snapshot. Those effects are not rewound when SQLite is restored.

Restore recovery therefore:

1. verifies exclusive installation authority and the durable `restore.pending` operation identity;
2. opens/validates the restored SQLite installation identity, schema and integrity while all effect-creating gates remain closed;
3. creates a new daemon incarnation as bookkeeping without granting effect authority;
4. **commits a fresh unpredictable RestoreGeneration as T0 for this `restoreOperationId`**, rotates JournalEpoch separately for event continuity, and records `RecoveryPass(mode=restore, restoreOperationId, priorRestoredGeneration, newRestoreGeneration, IN_PROGRESS)` in the same transaction;
5. durably clears `restore.pending` only after the matching T0 commit is established; if a crash occurs before clearing it, the matching RecoveryPass causes resume rather than another generation rotation;
6. rotates all active Run ControlLease tokens before Run/executor commands;
7. treats every restored Grant/CapabilityTicket from the old generation as non-redeemable historical authority; re-affirmation creates a new current-generation Grant rather than reactivating the old row;
8. treats every restored old-generation broker operation as reconciliation-only: inspect by the original stable identity where possible, never reissue merely because restored SQLite says `PENDING`/incomplete;
9. rejects Operator mutations carrying an old `(commandEpoch, commandId)` before command-row lookup/creation; callers must treat the prior outcome as UNKNOWN and inspect current state before intentionally issuing a new command;
10. rejects semantic Agent Control from any session whose immutable session RestoreGeneration differs from current **before** `agent_requests` lookup/creation; the external Attempt may still be inspected/terminated/reconciled by controllers;
11. inventories/reconciles every external domain capable of containing Pantheon-owned state, including known PlanningAttempt correlations and Run-/EvaluationOperation-owned Sandboxes, and fences effects newer than or absent from the restored database;
12. treats snapshot-only PlanningAttempt/Attempt/EvaluationAttempt `NOT_CONTACTED`, Sandbox `ABSENT`, missing result rows and similar negative evidence as historical until fresh domain inspection/current fencing establishes the post-snapshot interval;
13. validates every active acceptance Round's concrete Task Candidate xor GoalCompletionCandidate subject and pinned evaluator contract before accepting its Evidence/AcceptanceResult as current lifecycle input;
14. requires operator action for un-inventoriable ambiguous domains/operations or corrupted acceptance-subject relationships;
15. opens normal mutation/dispatch only after the recovery barrier is satisfied.

A restored database snapshot is never permission to blindly replay historical external operations or blindly reapply historical acceptance to a currently different subject.

### Restore-operation crash semantics

The restore latch and durable RecoveryPass form one handshake:

```text
restore.pending exists
+ no matching RecoveryPass/T0
→ restore fence still required

restore.pending exists
+ matching IN_PROGRESS RecoveryPass with new generation
→ T0 already committed
→ resume same restore operation
→ clear stale latch when safe

restore.pending absent
+ matching IN_PROGRESS RecoveryPass
→ T0 committed and latch was already cleared
→ continue reconciliation
```

Pantheon never rotates repeated generations merely because the process crashed after T0.

### Grant replay prevention

A one-use Grant consumed after the backup may appear unused again after restore. The RestoreGeneration mismatch makes it impossible to redeem that restored Grant, independent of the restored use counter.

If the operator wants the same authority again, they explicitly approve/re-affirm it under the current generation. Pantheon therefore preserves the semantic meaning of a bounded human approval even when the database history recording its consumption was lost.

### Agent Control replay prevention

An AgentControlSession is minted under one RestoreGeneration. A worker may still physically possess its raw credential after the database is restored, while the restored `agent_requests` table may lack requests that the worker already made after the snapshot.

Therefore Agent Control first requires the session generation to equal the current RestoreGeneration. An old-generation request never reaches request-row lookup/creation, so row absence cannot convert a previously executed worker request into fresh authority.

This fence does not assert that the external worker stopped. The Attempt/executor is reconciled independently. The session remains historical/fenced and is not rewritten to current. Automatic same-Attempt credential reminting is not part of this restore correction.

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

If CONFIRMED, record the reconciled historical outcome. If NOT_APPLIED is provable under current external evidence, Recovery Policy/operator may intentionally create new current-generation authority if the effect is still desired. If UNKNOWN, remain fenced; do not rotate the operation identity and retry.

A restored PENDING/ABSENT row is not by itself `NOT_APPLIED`; the restore-specific negative evidence rule applies.

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
stop creating new Attempts/control-operation external work
        ↓
persist final controller observations possible within shutdown policy
        ↓
flush/close SQLite cleanly
        ↓
record best-effort incarnation stoppedAt as final durable daemon step
        ↓
release installation lock
```

Clean daemon shutdown does not inherently mean cancelling every external Attempt/PlanningAttempt. External lifetime is independent where the backend supports that behavior.

Explicit `cancel work` and `stop daemon` are different user intents.

A backend whose native execution necessarily dies with the daemon will simply be reconciled as EXITED/failed on the next start according to its domain contract.

## 29. Crash/fault-injection testing is required

Recovery correctness cannot be validated only with happy-path unit tests.

The v1 test plan must inject process termination/crash boundaries around at least:

```text
Run-intent transaction commit
Attempt creation before ensureExecution
backend ensure after external start before acknowledgement
PlanningOperation/PlanningAttempt creation before external Planner contact
PlanningAttempt contact marker before/after external Planner request acknowledgement/result
Task Candidate EvaluationRound creation before/after Evidence commit
GoalCompletionCandidate EvaluationRound creation before/after Evidence commit
verification Sandbox intent before/after SandboxBackend ensure
Sandbox cleanup request before/after runtime deletion and before/after ABSENT observation
Sandbox ERROR/RELEASING persistence while external observation is UNKNOWN
EvaluationAttempt creation/contact marker before/after evaluator launch
usage ingestion before/after budget debit
candidate Artifact durable put before/after SQLite metadata
Task candidate commit before lifecycle transition
GoalCompletionCandidate commit before Goal Active->Evaluating transition
executor/planner/evaluator termination or result certainty before reservation release
Sandbox release before reservation release
workspace remove before reservation release
integration commit-object creation before CAS ref update
CAS ref update before IntegrationIntent acknowledgement
finalization obligation satisfaction before terminal transition
```

Restore tests additionally construct an older consistent snapshot, perform newer external/control effects, then execute the **supported restore-entry procedure** and assert at least:

```text
restore.pending survives crash after DB replacement but before T0
crash after T0 but before latch removal resumes the same restoreOperationId without a second generation rotation
consumed one-use Grant cannot redeem again
old-generation CapabilityTicket cannot redeem
restored PENDING broker operation cannot execute again without reconciliation proof
old commandEpoch + commandId cannot become a new command when its row is absent
old-generation AgentControlSession cannot create/replay agent_requests or submit/spawn/invoke
restored PlanningAttempt NOT_CONTACTED/missing PlanningRecord does not authorize resend until fresh domain reconciliation establishes absence
restored NOT_CONTACTED/ABSENT snapshot facts do not authorize replacement work until fresh domain reconciliation proves current absence
restored EvaluationRound cannot be applied to a different/current Task Candidate or GoalCompletionCandidate merely because lifecycle phase matches
fresh RestoreGeneration is different from every value recovered from the snapshot
Run- and EvaluationOperation-owned Sandboxes are inventoried/reconciled by durable SandboxKey+holder
```

For each crash point, restart Pantheon and assert that the resulting state is equivalent to either the operation not having happened or having happened exactly once, never a duplicate unsafe effect.

Property/invariant tests should continuously assert:

- no duplicate active Attempt created under UNKNOWN execution;
- no overlapping PlanningAttempt under ambiguous external planning contact;
- no overlapping EvaluationAttempt under ambiguous evaluation contact;
- every EvaluationRound has exactly one concrete Task Candidate xor GoalCompletionCandidate FK;
- no Evidence/AcceptanceResult subject or EvaluatorVersion can differ from its EvaluationRound;
- no Task acceptance application from a Round whose Candidate is not the Task's exact current Candidate;
- no Goal acceptance application from a Round whose GoalCompletionCandidate/GoalRevision is not exact/current;
- Sandbox `phase` and `observedPresence` always remain in their separate domains;
- no `RELEASED+PRESENT` Sandbox;
- no `RELEASED+UNKNOWN` Sandbox without a matching force-resolution tombstone/fence;
- no overlapping replacement-authoritative Sandbox for one Run/EvaluationOperation holder while older presence is unresolved/unfenced;
- no Sandbox without exactly one valid durable holder;
- no released reservation while external use/Sandbox existence is uncertain unless a separate safe capacity/accounting disposition exists;
- no BudgetHold double-debit from replayed usage;
- no PlanningRecord treated as Graph authority without current revision/precondition validation;
- no acceptance against corrupt/mismatched Artifact bytes;
- no shared Git ref overwrite after stale CAS expectation;
- no Active Task without exactly one responsible nonterminal Run in valid state;
- no controller command accepted under stale lease token;
- no Grant/CapabilityTicket redeemed across RestoreGeneration;
- no old-generation broker operation reissued as an external effect;
- no Operator command accepted under a stale commandEpoch;
- no semantic Agent Control request accepted from an old-generation AgentControlSession;
- no snapshot-only negative observation treated as current post-restore proof of absence.

## 30. Recovery passes

Pantheon may record a lightweight RecoveryPass for audit/diagnostics:

```yaml
recoveryPass:
  id: recovery-pass_...
  mode: startup | periodic | manual | restore
  daemonIncarnation: ...
  restoreOperationId: restore-op_...      # restore mode only
  priorRestoredGeneration: ...            # historical metadata only
  newRestoreGeneration: ...               # restore mode only
  state: IN_PROGRESS | BARRIER_SATISFIED | COMPLETE
  startedAt: ...
  barrierSatisfiedAt: ...
  completedAt: ...
  findings:
    unresolved: 2
    quarantined: 1
```

Restore-mode RecoveryPass records the old restored generation as historical metadata, the newly committed RestoreGeneration, and the external restore-operation identity. The old generation is never treated as current authority.

A matching IN_PROGRESS restore pass is also the durable proof that T0 already committed for that `restoreOperationId`; a surviving `restore.pending` latch after such a crash does not cause another generation rotation.

A pass is not required to reach zero findings before scheduler dispatch. It must only reach the recovery barrier: every relevant unresolved item is safely fenced.

## 31. Controller order and dependencies

Pantheon should avoid one giant global recovery controller.

A practical startup dependency order is:

```text
Installation lock + restore-entry latch interpretation
        ↓
Storage / Installation Authority validation
        ↓
RestoreGeneration T0 fence / matching RecoveryPass (restore mode only)
        ↓
Run ControlLease adoption + PlanningOperation/EvaluationOperation intent inventory
        ↓
Authorization / broker-operation / Agent Control generation reconciliation
        ↓
Workspace/materialization ownership reconciliation
        ↓
Sandbox holder + SandboxKey + phase/presence reconciliation
        ↓
Run Attempt + PlanningAttempt + EvaluationAttempt external-execution/contact reconciliation
        ↓
Resource + Budget accounting reconciliation
        ↓
Artifact / Task Candidate / GoalCompletionCandidate availability
        ↓
EvaluationRound typed-subject + Evidence/AcceptanceResult reconciliation
        ↓
Integration reconciliation
        ↓
Task / Goal lifecycle + planning/graph reconciliation
        ↓
Scheduler dispatch gate
```

This is a dependency graph, not a requirement that every controller execute serially. In particular, execution/contact inspection may proceed in parallel where safe, but a new Attempt/EvaluationAttempt launch that requires a Sandbox waits for that holder's Sandbox reconciliation/verification result, and a new external PlanningAttempt waits for prior PlanningAttempt certainty/accounting fences.

Acceptance lifecycle application waits for both immutable subject availability and EvaluationRound typed-subject validation; it never relies on Event ordering alone.

Controllers may operate concurrently where dependencies permit, but each publishes enough condition/fencing state for downstream controllers to decide safely.

The global Recovery Coordinator owns only startup gating, restore-entry/generation fencing/pass bookkeeping, and cross-domain invariant scans. It does not absorb domain-specific repair logic.

## 32. v1 scope

Include:

- single-daemon installation lock;
- stable Installation ID and per-start daemon incarnation ID;
- explicit crash-safe restore-entry latch outside the restored SQLite snapshot;
- one restoreOperationId/T0/RecoveryPass handshake that prevents both missed and repeated generation rotation;
- fresh RestoreGeneration rotation on disaster restore;
- generation-bound Grants/CapabilityTickets/broker operations, Operator commands, and Agent Control sessions;
- Run ControlLease token rotation plus ownership epoch;
- staged startup and global dispatch gate;
- recovery barrier based on reconciled/fenced/quarantined obligations;
- periodic safety reconciliation using normal controller code;
- finalization obligations for cleanup safety;
- Run/Attempt, PlanningOperation/PlanningAttempt and EvaluationOperation/EvaluationAttempt recovery;
- concrete EvaluationRound Task-Candidate/GoalCompletionCandidate subject reconciliation and exact-subject Evidence/AcceptanceResult validation;
- immutable PlanningRecord recovery with independent GraphRevision/GoalRevision revalidation before materialization;
- holder-driven Run/EvaluationOperation Sandbox reconciliation by durable SandboxKey with lifecycle phase distinct from external presence certainty;
- restore-specific rule that snapshot-only negative observations are not current post-snapshot proof of absence;
- authorization/broker, Resource, Budget, Workspace, Artifact and Integration reconciliation rules;
- durable RecoveryFindings;
- invariant scanning and quarantine;
- SQLite integrity/version checks and supported backup/restore procedure;
- restore-specific recovery mode;
- crash/fault-injection tests around every external-side-effect and acceptance-currentness boundary.

Defer:

- active-active/multi-daemon Pantheon;
- distributed consensus/lease service;
- automatic destructive orphan reaping;
- automated database page-level salvage;
- cross-machine CAS replication protocol;
- live migration of running Attempts;
- arbitrary executable/Sandbox-capable Planner authority without an explicit future holder/security design;
- automatic same-Attempt Agent Control credential rotation after disaster restore;
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
12. **Planning recovery preserves PlanningOperation/PlanningAttempt identity; ambiguous external planning contact never authorizes an overlapping Planner call, and recovered PlanningRecord output remains subject to current Graph/Goal validation.**
13. **Evaluation recovery preserves EvaluationRound's exact concrete subject (`TASK_CANDIDATE` xor `GOAL_COMPLETION_CANDIDATE`), pinned evaluator contract and EvaluationAttempt identity; historical Evidence never gains authority over a different/current subject.**
14. **Task and Goal lifecycle controllers, not evaluators/recovery scanners, separately apply AcceptanceResult after rechecking their exact current subject/revision.**
15. **ResourceReservations remain accounting authority during recovery; observed utilization cannot free them.**
16. **Budget/Usage replay is idempotent and truthful; uncertain work retains unspent hold headroom conservatively.**
17. **Workspace recovery never silently recreates potentially lost unsealed mutable work and never interprets Agent-writable Git metadata with ambient controller authority.**
18. **Integration recovery is determined by expected/current/result Git OIDs and compare-and-swap semantics, never force-updating shared refs.**
19. **CAS recovery verifies digest and size; extra immutable objects are safe orphans, while missing/corrupt referenced replicas block consumers but do not mutate Artifact identity.**
20. **Logical invariant violations are durable RecoveryFindings and have explicit auto-repair, reconcile, fence, quarantine, or operator-required dispositions.**
21. **Recovery failures are scoped to the smallest safe blast radius; only authority/storage/global-accounting ambiguity stops all dispatch.**
22. **Pantheon uses SQLite on reliable local storage with WAL, `synchronous=FULL`, and SQLite 3.51.3+ or an official WAL-reset-fix backport.**
23. **Routine startup runs `quick_check` plus `foreign_key_check`; full `integrity_check` is used for suspected corruption/deep diagnosis.**
24. **Live backups use SQLite-supported snapshot APIs; raw database-file copies are not the normal backup mechanism.**
25. **Safe disaster restore is an explicit maintenance ceremony begun before database replacement with a durable out-of-database `restore.pending` latch; a rewound database is never trusted to detect its own rewind.**
26. **One restoreOperationId links the latch to T0/RecoveryPass so crash-before-T0 cannot fall through to normal startup and crash-after-T0 cannot rotate a second generation merely because the latch remains.**
27. **Restoring an old SQLite snapshot rotates a fresh unpredictable RestoreGeneration before any new authority-bearing mutation/effect, because external and human/worker-authority histories may be newer than the snapshot.**
28. **Restored old-generation Grants/Tickets are non-redeemable; re-affirmation creates new current-generation authority rather than reviving rewound use counts.**
29. **Restored old-generation broker operations are reconciliation-only and never authorize blind re-execution from restored PENDING/incomplete state.**
30. **Operator command idempotency is scoped by `(RestoreGeneration, commandId)`; stale command epochs fail closed even when historical command rows are absent.**
31. **AgentControlSession authority is RestoreGeneration-bound; old-generation worker credentials fail before Agent request lookup/creation and are not rewritten to current after restore.**
32. **Snapshot-only negative observations such as restored `NOT_CONTACTED`, `ABSENT` or row absence do not prove the post-snapshot external world; fresh inspection/inventory/fencing is required before replacement/conflicting work.**
33. **Clean daemon shutdown and cancellation of external work are separate intents.**
34. **Crash/fault-injection testing at external-side-effect, acceptance-currentness and restore-replay boundaries is a required v1 quality gate.**
35. **A small Recovery Coordinator gates startup and scans invariants; domain controllers retain domain-specific reconciliation logic.**
36. **Sandbox recovery is holder-driven, not Run-traversal-driven: lifecycle phase and external `PRESENT|ABSENT|UNKNOWN` presence are separate durable facts; ordinary RELEASED requires ABSENT, while force-resolved UNKNOWN stays factually UNKNOWN behind an explicit lineage fence and separate capacity disposition.**
