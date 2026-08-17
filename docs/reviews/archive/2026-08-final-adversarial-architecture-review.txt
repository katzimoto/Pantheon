# August 2026 Final Adversarial Architecture Review

## Status

Adversarial review report from August 2026. **Not canonical** — see `docs/reviews/README.md`. Nothing here is an implementation requirement. The canonical subsystem documents under `docs/architecture/` remain the source of truth; where this report and a contract disagree, the contract wins.

This is a *report*, not a ledger. It records what one reviewer found. It does not record a disposition, and no finding below has been written into a canonical contract. A separate decision is required before any of it changes architecture.

## Scope

Final independent adversarial review performed before the architecture is frozen and decomposed into implementation issues.

```text
repository   katzimoto/Pantheon
baseline     main @ 1199f68fa9a922af88ee80ec326d72be3ae8f95a
branch       claude/pantheon-adversarial-review-mzw51g
```

The review was read-only against the architecture: no canonical document was modified, and no finding was "fixed" in place. The mission was narrowly falsificationist — attempt to prove that the current canonical architecture still contains an implementation-blocking defect, and otherwise conclude that it is ready.

Deliberate v1 exclusions (distributed control plane, Kubernetes, remote public Agent Control, speculative Attempts, autonomous recursive planning, joined/detached spawn, worker `task.graph.propose`, model-authoritative acceptance, automatic Genome promotion, multi-node HA, live worker credential rotation) were treated as settled and were not reopened. Reserved vocabulary was not treated as a v1 implementation requirement.

Review bar:

> Could a competent implementation team now implement Pantheon v1 without having to invent a new security, authority, lifecycle, recovery, accounting or durable-identity rule?

## Verdict

```text
FINAL VERDICT:
BLOCKED
```

Two findings, neither of which is a redesign request. Both corrections are additive clarifications to existing contracts — roughly one paragraph each, with no new subsystem, table, controller, generation or abstraction.

| ID | Severity | Confidence | Subject |
|---|---|---|---|
| PAN-ADV-01 | HIGH | MEDIUM | External idempotency identities are not required to be unreusable across a database rewind |
| PAN-ADV-02 | MEDIUM | HIGH | `SchedulingEligible` is defined incompatibly by two canonical contracts |

---

## PAN-ADV-01

**Severity:** HIGH
**Title:** No canonical rule requires external idempotency/reconciliation identities (LaunchKey, SandboxKey, PlanningAttempt/EvaluationAttempt IDs, broker-operation external identity) to be unreusable across a database rewind

### Canonical evidence

`docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`, **§8 Durable external-operation rule**, enumerates the per-domain external identities:

```text
Attempt launch      → LaunchKey
Planning call       → PlanningAttempt ID + contact marker
Evaluation launch   → EvaluationAttempt ID + launch-contact marker
Sandbox             → SandboxKey + immutable Run/EvaluationOperation holder
Broker operation    → stable broker-operation/external idempotency identity
Operator command    → RestoreGeneration + commandId
```

The same document states the rewind hazard and its fix for *other* identities.

**§3 RestoreGeneration:**

> "It is deliberately not a monotonic counter restored from the database: an old snapshot can reintroduce a previously used numeric value. The new generation is random/fresh and is committed before any new post-restore authority-bearing mutation or external effect."

**§5 ControlLease fencing uses epoch plus unpredictable lease token:**

> "…epoch alone is not sufficient under database restore because an older snapshot can reintroduce a previously used numeric epoch. Each acquired/adopted Run ControlLease therefore contains … `leaseToken: <fresh-random-token>`"

Freshness or non-reuse is therefore explicitly mandated for `RestoreGeneration`, `leaseToken`, `restoreOperationId` ("fresh non-reused random ID"), `daemonIncarnationId` ("The ID is never reused"), `JournalEpoch`, `SecretVersionId` and Agent Control bearers (`docs/architecture/execution/agent-control-channel.md` §5: "cryptographically random; at least 256 bits of entropy"). Operator command identity is generation-scoped as `(restore_generation, command_id)` in `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`, **§Commands**.

No equivalent property is stated for the identities in the §8 table. The only constraints that exist are:

- `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`, **§SQLite operating rules**: "opaque TEXT resource IDs in v1" — opaque, which is neither unpredictable nor non-reused;
- **§Attempt and launch-contact state**: `attempts … launch_key UNIQUE`; **§Sandbox**: `sandbox_instances … sandbox_key UNIQUE`. Both are uniqueness *within the surviving database* — precisely the scope a restore rewinds;
- `docs/architecture/execution/run-and-attempt.md`, **§LaunchKey and launch semantics**: "Every Attempt receives one immutable `LaunchKey` before any external execution side effect." Immutable, with no generation or entropy rule.

The `restore_generation` column is attached only to `grants`, `capability_tickets`, `broker_operations` and `agent_control_sessions` (`docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`, **§Grants and broker operation redemption**, **§Agent Control**). It is not attached to `attempts`, `sandbox_instances`, `planning_attempts` or `evaluation_attempts`, and it does not participate in the external identity a broker actually transmits to a third-party system.

Illustrative identifiers across the canonical documents are mixed, and several read as sequential: `run_17`, `attempt_456`, `join_44`, `brokerop_77`, `evalop_91`, `workspace-rev_82`.

### Concrete failure scenario

1. Backup snapshot `S` is taken. In `S`, Run `run_17` has one Attempt with ordinal 1 and `launch_contact_state = NOT_CONTACTED`.
2. After `S`, the daemon runs normally. Attempt 1 is contacted and reaches `EXITED`. Recovery Policy creates Attempt ordinal 2 with `LaunchKey = L2`, T4b commits, and `ensureExecution(L2)` reaches a `KEYED_IDEMPOTENT` backend. A worker starts.
3. Disk failure. The operator performs the supported disaster restore of `S`: `restore.pending` latch → T0 → fresh `RestoreGeneration`.
4. Restore recovery inventories the backend and finds a Pantheon-owned execution keyed `L2` with no matching durable Attempt. Under **§22 Repair policy** ("runtime Sandbox discovered with no corresponding durable SandboxInstance ownership record"; "Unknown/dangling native executions are quarantined and reported before destructive cleanup") that lineage is quarantined. Quarantine fences *the finding*; it does not reserve the key value.
5. Restored Attempt 1 is freshly inspected — as **§9 Restore-specific negative evidence rule** requires — and is proven `ABSENT`, because its process genuinely did exit before the failure. Recovery Policy legitimately authorizes `RETRY_ATTEMPT`.
6. T8 creates a new Attempt for `run_17`. If the implementation derives `LaunchKey` deterministically from durable relational state — `run_id ‖ ordinal`, or a `rowid`-backed `attempt_id` — the ordinal counter was rewound with the database, so the new Attempt is minted with `LaunchKey = L2` again. The `attempts.launch_key UNIQUE` constraint passes: the earlier `L2` row no longer exists.
7. T4b commits and `ensureExecution(L2, …)` is issued. Because the offer is `KEYED_IDEMPOTENT` — "repeated ensure/recover for the same LaunchKey is guaranteed to address one execution lineage" (`docs/architecture/execution/execution-fabric.md`, **§Launch semantics**) — the backend **adopts the still-live step-2 execution** instead of starting the authorized fresh one.

Pantheon now believes a new Attempt with a new `AgentControlSession` is running. The live worker holds the pre-restore bearer and is fenced by the `session.restoreGeneration` mismatch, so it can never submit. The Run cannot progress, and its usage and contact reconciliation are bound to the wrong external lineage.

The same construction applies to the other §8 identities:

- **`SandboxKey`.** A post-snapshot Sandbox is force-resolved or quarantined; a post-restore `ensureSandbox(SandboxKey, SandboxPlan)` (`docs/architecture/security/sandbox-broker-and-isolation.md`, **§Sandbox lifecycle and external identity**) re-mints the same key and adopts a runtime whose materialization and `AmbientAuthorityEnvelope` were verified against a different Run's SandboxPlan. `SandboxVerification` then checks the wrong object against the wrong expected identity.
- **Broker operations.** `docs/architecture/security/permissions-and-capabilities.md`, **§Atomic Grant use-count redemption**, requires the exact broker operation to be created before the external effect, and `docs/architecture/security/secret-store-and-credential-brokering.md` §25 requires a retry to reuse "that operation's original binding authority and external idempotency identity". If the external idempotency key derives from a rewound `broker_operations` id, a genuinely new, newly-authorized post-restore effect — an authorized `git.push`, an external `service.mutate` — is presented to the external system under a key it has already seen. It is silently deduplicated against the old operation, or the old operation's result is returned and recorded as this one's outcome.

### Invariant violated

- `docs/architecture/overview.md`, **§Persistence and external effects**, and `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md` **Key decision 11**: "UNKNOWN external outcome never authorizes a blind replacement side effect" — together with §29's crash-test acceptance criterion, "the resulting state is equivalent to either the operation not having happened or having happened exactly once, never a duplicate unsafe effect."
- **§8**: "A disaster restore never creates permission to replace an existing operation identity with a fresh one solely because the restored row looks incomplete." The rewound-key case is the mirror image — a *fresh* operation silently inherits an *existing* external identity — and is not covered by that sentence or any other.
- `docs/architecture/execution/run-and-attempt.md` core invariant 8: "Every Attempt has one immutable LaunchKey and one Attempt-scoped AgentControlSession identity." In the scenario, two distinct Attempts across the restore boundary share one LaunchKey, so LaunchKey no longer identifies one execution lineage.

### Why implementation cannot safely infer the answer

The architecture repeatedly and deliberately specifies entropy or non-reuse wherever a rewind hazard exists, and specifies generation-scoping for command identity. Its silence on the §8 external identities therefore reads as a positive statement that ordinary durable identifiers suffice. `attempts.launch_key UNIQUE` and `sandbox_instances.sandbox_key UNIQUE` actively suggest that in-database uniqueness is the intended guarantee — which is exactly the guarantee a restore invalidates.

A team choosing `LaunchKey = f(run_id, ordinal)` — natural, since `attempts` already carries `(run_id, ordinal)` and the contracts describe LaunchKey as "immutable" rather than "unpredictable" — satisfies every written rule and still produces the failure above. Choosing among "mint from fresh entropy", "scope by RestoreGeneration" and "in-database uniqueness is enough" is a durable-identity and external-effect-safety decision, not a coding preference.

The restore contract's existing mitigations do not close this:

- Fresh domain inspection (**§9**) governs whether the *restored* lineage may be treated as absent. It says nothing about the key assigned to a *subsequent* lineage.
- Quarantine of a dangling execution (**§22**) fences the finding, not the identity namespace.
- The non-inventoriable-backend rule (**§13**: "Pantheon must conservatively block conflicting new execution on that backend/domain until an operator resolves the ambiguity") covers only backends that cannot inventory. The collision above requires an *inventoriable* backend — the case the architecture currently treats as the safe one.

### Smallest architecture correction

Add one rule to **§8** of `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`, and mirror a single sentence into the identity definitions in `docs/architecture/execution/run-and-attempt.md`, `docs/architecture/security/sandbox-broker-and-isolation.md`, `docs/architecture/goals-and-planning/planner-and-task-decomposition.md` and `docs/architecture/evaluation-and-acceptance/evaluation-and-evaluator-registry.md`:

> Every identity in this table that is transmitted to, or matched against, an external system is minted from fresh unpredictable entropy and is never derived from rewindable durable counters, ordinals or row identifiers. In-database uniqueness is not sufficient, because a restored snapshot can re-mint a value that already addresses a live external lineage.

Add one assertion to the **§29** restore test list, alongside the existing "fresh RestoreGeneration is different from every value recovered from the snapshot":

> a post-restore Attempt/Sandbox/PlanningAttempt/EvaluationAttempt/broker operation never re-mints an external identity present in the pre-failure external world.

No new table, controller, generation or abstraction is required.

### Why this is not merely an implementation detail

It is a durable-identity rule that determines whether disaster restore can produce a duplicate or mis-bound external effect. The architecture already treats exactly this class of decision as architectural for `RestoreGeneration`, `leaseToken`, `restoreOperationId` and command identity, and states the reasoning explicitly in both places. Leaving it unstated for the identities that actually cross the external boundary is an omission within the same contract, not a free choice among safe options.

### Confidence

MEDIUM

---

## PAN-ADV-02

**Severity:** MEDIUM
**Title:** `SchedulingEligible` is defined incompatibly by two canonical contracts, and the persistence invariant derived from it destroys `eligible_since`

### Canonical evidence

`docs/architecture/scheduling/scheduler-ready-task-eligibility.md`, **§Eligibility predicate**:

```text
A Task is Scheduler-eligible only when all hard logical gates pass:
  …
  system/Goal dispatch control allows new work
  Task notBefore/backoff satisfied
  …
```

and, in the same document, **§`notBefore` / backoff**:

> "Recovery/backoff may attach a durable `notBefore`. Until elapsed, the Task remains Ready but is **not scheduler-eligible**. This is not a separate Task phase."

`docs/architecture/scheduling/scheduler-task-ordering-and-fairness.md`, **§Eligibility interval semantics**, states the opposite:

> "A temporary scheduler backoff does **not** make the Task semantically ineligible and does not reset `eligible_since`. It only suppresses consideration until `next_attempt_at`…"

and its **§Conceptual selection algorithm** evaluates `dispatch_mode` and `nextAttemptAt` as gates *separate from* `SchedulingEligible`:

```text
require scheduler_state.dispatch_mode == RUNNING
require recovery/configuration gates permit dispatch

eligible = Tasks where:
  phase == Ready
  SchedulingEligible == True
  eligible_since IS NOT NULL
  (nextAttemptAt IS NULL OR nextAttemptAt <= now)
```

`docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`, **§Durable scheduler state**, promotes the relationship to a durable invariant:

```text
SchedulingEligible == True  => eligible_since IS NOT NULL
SchedulingEligible == False => eligible_since IS NULL
```

and the same document's **§Invariant checker** lists both halves of the conflict as checker duties:

```text
task_scheduling_state Task FK valid; SchedulingEligible true iff current eligible interval has eligible_since
temporary scheduler backoff does not reset eligible_since or mutate Task lifecycle
```

That document also contradicts the eligibility contract directly: "`next_attempt_at` is a scheduler backoff suppression point. A non-NULL future value **does not make the Task semantically ineligible** and does not reset its eligible waiting age."

### Concrete failure scenario

1. Tasks `A1` (Goal A) and `B1` (Goal B) become Ready; both get `eligible_since = T0`. `A1` is in the `background` class and is accruing the bounded aging boost required by **§Bounded aging / starvation protection**.
2. Routing for `A1` returns `TEMP_UNAVAILABLE` — the required backend is momentarily saturated. Per the fairness contract the scheduler releases the claim and CAS-writes `next_attempt_at = T0 + 30s`.
3. An implementation that built `SchedulingEligible` from the canonical eligibility predicate — which lists "Task notBefore/backoff satisfied" as a hard gate — now computes `SchedulingEligible = False` for `A1`.
4. The durable invariant `SchedulingEligible == False => eligible_since IS NULL` forces the stated transition rule "True → False ⇒ `eligible_since = NULL`" to fire, discarding `A1`'s waiting interval.
5. At `T0 + 30s` the condition flips back and `A1` receives a **new** `eligible_since = T0 + 30s`. Its accumulated waiting age and its aging boost are gone.
6. Under sustained contention `A1` is repeatedly backed off, so `eligible_since` is repeatedly reset and the aging boost never matures — the exact starvation the aging mechanism exists to bound. Within Goal A, ordering by `eligibleSince ASC` also degrades, because every recently-contended Task now sorts newer than a never-contended sibling.
7. The same mechanism fires installation-wide on `POST /api/v1/dispatch/actions/pause`: the eligibility predicate's "system/Goal dispatch control allows new work" gate flips `SchedulingEligible = False` for every Ready Task, nulling every `eligible_since`. On resume, all Tasks are minted with an identical fresh `eligible_since` and within-Goal ordering collapses to `TaskId ASC` — while `docs/architecture/operations/public-daemon-api-and-cli.md`, **§Dispatch control**, asserts that pause "does not cancel or stop already-committed Runs/Attempts, revoke existing execution authority, release resources, or pretend existing external work stopped."

An implementation built from the fairness and persistence reading instead treats backoff and `dispatch_mode` as selection-time suppression gates and preserves `eligible_since` — producing materially different, non-interoperable scheduler semantics and a different PersistenceInvariantChecker verdict on the same database.

### Invariant violated

- `docs/architecture/scheduling/scheduler-task-ordering-and-fairness.md` v1 decision 11 ("Temporary scheduling failure uses durable scheduler backoff plus event-driven wakeup **without changing Task phase or eligible waiting age**") and decision 12 ("Bounded aging prevents indefinite starvation").
- `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` core invariant 8: "Task `eligible_since` represents the current continuous scheduler-eligible interval; temporary backoff does not reset it or mutate Task lifecycle."
- `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md` **§20 Logical invariant scanner**, which asserts both "SchedulingEligible true has durable eligibleSince / SchedulingEligible false has no current eligibleSince" **and** "temporary backoff never rewrites Task phase or eligible waiting-age origin". Under the eligibility document's predicate these two scanner rules are mutually unsatisfiable.

### Why implementation cannot safely infer the answer

`docs/architecture/README.md` names `docs/architecture/scheduling/scheduler-ready-task-eligibility.md` as the canonical owner of "Which Tasks are logically eligible to be considered — nothing about capacity", and that document enumerates backoff and dispatch control as hard gates of that single predicate. It never introduces a second, narrower `SchedulingEligible` condition. The fairness and persistence contracts use the identical term for a strictly narrower condition and explicitly forbid backoff from affecting it.

Because `SchedulingEligible` is not merely a query filter but the driver of a **durable** column (`task_scheduling_state.eligible_since`), a durable biconditional invariant and two invariant-checker rules, the implementer cannot defer the choice: whichever reading is picked writes different rows and yields different checker verdicts on identical histories.

`docs/README.md` classifies this situation directly: "Where two canonical documents appear to conflict, treat it as a defect. Report it rather than choosing a side."

This is distinct from an under-specified aging *policy*. Exact boost durations and backoff curves are correctly deferred to configuration. What is contradicted is the definition of the interval those durations are measured over.

### Smallest architecture correction

Add one clarifying paragraph to `docs/architecture/scheduling/scheduler-ready-task-eligibility.md` separating the two predicates it currently conflates, and move the two offending gates out of the hard list:

> `SchedulingEligible` is the durable per-Task semantic condition whose current true interval is recorded by `task_scheduling_state.eligible_since`. Installation `dispatch_mode` and `task_scheduling_state.next_attempt_at` are **selection-time suppression gates**, not inputs to `SchedulingEligible`: a paused or backed-off Task remains `SchedulingEligible=True` with its `eligible_since` interval intact, and is simply not considered until the gate clears.

The `notBefore` / backoff subsection's sentence becomes:

> the Task remains Ready and `SchedulingEligible`, but is suppressed from the current selection cycle until `next_attempt_at` elapses.

No schema change, no new state, and no change to the fairness or persistence contracts.

### Why this is not merely an implementation detail

The disputed term controls a durable column, a stated persistence biconditional and two PersistenceInvariantChecker rules that cannot both hold under one reading. It determines whether starvation protection and within-Goal ordering — properties the fairness contract explicitly promises — survive ordinary backoff and operator pause. Two competent teams reading the canonical map's designated owner versus its two consumers would build divergent, mutually inconsistent durable state.

### Confidence

HIGH

---

## Coverage summary

Cross-domain lifecycles and authority chains traced end-to-end during this review:

**Goal, planning and graph**

- Goal → Planner → PlanningOperation/PlanningAttempt/PlanningRecord → GraphPatch, including T16 contact fencing, the zero-Graph-authority rule for recovered Planner responses, and control-operation accounting under UNKNOWN contact.
- Goal revision → reconciliation classification → supersession, checked against the "never terminalize a Task around a live Run" rule and the `tasks.terminal_target` CHECK.
- Goal completion: readiness → immutable GoalCompletionCandidate → `GOAL_COMPLETION_CANDIDATE` Round → Evidence → Goal Completion Controller → Finalizing → terminal quiescence, including the zero-criteria structural path.

**Scheduling and execution**

- The full T3 chain: eligibility → fairness selection → SchedulingClaim → Agent+Offer routing → SandboxPlan feasibility → incremental resource admission → BudgetHold → atomic Run/Binding/ContextSourceSnapshot/fairness-charge commit. The fairness charge is atomic with Run creation in every document that mentions it.
- T3 → T3a → T4/T4a/T4b → `ensureExecution`, including the pre-contact rekey window, credential-projection invalidation, the frozen-verifier boundary after `CONTACT_MAY_HAVE_OCCURRED`, and the `run_context_plans` composite-FK proof that Attempt creation cannot precede exact-source ContextPlan attachment.
- Cancellation/supersession versus T6 Candidate submission in both orders, including the immutable-Candidate-survives-cancellation rule and the `one_nonterminal_run_per_task` partial index keeping a `Finalizing` producer in the live slot through `Evaluating` until T9.
- Acceptance rejection → RecoveryDecision → `PriorRunFinalizing` → T9 requeue, checked against the Ready ⇒ zero-nonterminal-Runs invariant and the unique-live-Run deadlock argument.
- Blocking spawn → Run Finalizing/Yielded → capacity settlement → Run Yielded / Task Waiting → Join → Ready → new Run, including UNKNOWN blocking yield completion, the dual idempotency layers, and Agent Control authority loss after yield intent.

**Security and authority**

- Credential authority chain: frozen `credentialBindingRegistryDigest` → exact `credentialBindingAuthorityDigest` frozen/current equality → `secret.use` → SecretDescriptor usability → broker operation committed before material retrieval; including rotation-behind-same-SecretRef and remap-denies-old-Run.
- Grant/CapabilityTicket/broker-operation redemption under RestoreGeneration fencing, use-count CAS, and reconciliation-only semantics for old-generation operations.
- SecretMutationIntent and CredentialLease recovery, including the no-replay-of-bytes rule and DRIFTED/UNKNOWN blast-radius fencing.
- Sandbox: immutable holder XOR (Run ⊻ EvaluationOperation), `phase` versus `observedPresence` separation, the `RELEASED+UNKNOWN` tombstone requirement, capacity retention under UNKNOWN, and holder-driven rather than Run-traversal recovery inventory.
- Hostile repository and filesystem boundary: sterile controller projection versus confined inspection, root-confined/no-follow capture, symlink-as-data, and the TOCTOU rule binding validation and payload read to the same object identity.

**Artifacts, workspace and integration**

- Candidate sealing → CAS-complete `code.changeset` with changed-path before/after preimages → Integration three-way plus ref CAS, including integration correctness after Task-local Git objects disappear and the optional-Git-pin fail-closed rule.

**Persistence, recovery and backup**

- Composite holder FKs, both partial unique indexes, lifecycle CHECKs, usage provenance namespacing, control-operation metering-source freezing, `finalization_obligations` XOR ownership, `(restore_generation, command_id)` command identity, and Event/command causality pairing.
- Staged startup, the `restore.pending` ↔ T0 ↔ RecoveryPass handshake in both crash orders, the restore-specific negative-evidence rule across every external domain, barrier versus `dispatch_mode=PAUSED`, and blast-radius scoping.
- ControlPlaneSnapshot versus DurableStateBackup, snapshot-derived retention closure, GC exclusion opened before snapshot capture, and CAS-staged-before-DB restore ordering.

## Residual implementation decisions

Listed to demonstrate the boundary the review applied between architecture and implementation. All of the following are correctly left open and are safe to leave open:

- Concrete Rust crate and module boundaries, and the exact `ExecutorBackend` / `SandboxBackend` / `SecretProvider` trait surfaces — `docs/architecture/execution/execution-fabric.md` explicitly defers these.
- Exact SQL DDL spelling, column types and index names where the invariant is already fixed, including whether Sandbox holder-slot uniqueness uses a denormalized active-holder key or controller serialization plus the invariant checker. `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` names both as acceptable.
- Adapter-private transport for Agent Control (socket proxy, vsock, private endpoint, native tool bridge, adapter mediation) and adapter-private backend attachment encoding.
- Retrieval technique for `adaptive` Memory preload (BM25, vector, hybrid), constrained only by the determinism and frozen-provenance contract in `docs/architecture/agents-and-context/context-builder.md`.
- Concrete backoff durations, aging boost magnitudes, evaluation concurrency limits and priority-class numeric values — all explicitly configuration rather than architecture.
- Whether an `OBSERVATIONAL` offer is admitted for a given Task. The UNKNOWN-never-replaces rule preserves safety regardless, so this is an availability and liveness policy choice.
- Backup storage layout and the operator backup CLI surface. `docs/architecture/persistence-and-recovery/backup-and-restore.md` §14 deliberately leaves the surface open while fixing the contract.
- Whether the yield-completion transaction's `Task Active → Waiting` clause is skipped when a cancellation has already moved the Task to `Finalizing/Cancelled`. Both readings terminalize the Run toward its recorded `terminalTarget=Yielded` and satisfy every stated invariant.

## Final statement

Two substantiated problems were found, so a clean verdict cannot be issued at this commit.

PAN-ADV-02 is the higher-confidence of the two and the cheaper to close. It is a verbatim contradiction between `docs/architecture/scheduling/scheduler-ready-task-eligibility.md` and both `docs/architecture/scheduling/scheduler-task-ordering-and-fairness.md` and `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`, over a term that drives a durable column and two invariant-checker rules. `docs/README.md` directs that such conflicts be reported rather than resolved by a reviewer, which is what this report does.

PAN-ADV-01 is the more consequential. The architecture establishes rewind-resistant identity discipline for every authority generation it names, then omits it for precisely the identities that cross the external-effect boundary, leaving an implementation free to choose a rewindable derivation that breaks the exactly-once external-effect guarantee after disaster restore.

Both corrections are additive clarifications to existing contracts, and neither reopens a deliberate v1 simplification. Everything else traced above held up under crash, race, authority and restore analysis.
