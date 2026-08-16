# August 2026 Architecture Review Resolution

## Purpose

This document records how Pantheon resolved the Critical/High findings from Claude's adversarial architecture review (`claude/pantheon-architecture-review-kue3se`, review commit `5fd665bf8af707897b63aefa21f3e7ce72fee9b6`).

It is a review ledger, not a replacement for the canonical subsystem documents. The referenced `docs/architecture/**` files are the source of truth.

## Verdict after patch sweep

All four Critical findings and all fifteen High findings from the first review now have an architecture-level resolution. Some resolutions intentionally differ from Claude's suggested mechanism where the diagnosis was correct but the proposed fix would weaken another Pantheon invariant.

A second adversarial review is still required before implementation planning/issues are considered frozen.

## Critical findings

| # | Original finding | Resolution | Canonical docs |
|---|---|---|---|
| 1 | No Agent-facing control channel / same-user Operator socket bypass | **Resolved.** Separate Attempt-authenticated Agent Control surface; credential authenticates identity only; Operator Control physically unreachable from untrusted Sandbox. | `agent-control-channel.md`, `sandbox-broker-and-isolation.md`, `permissions-and-capabilities.md`, `public-daemon-api-and-cli.md` |
| 2 | Blocking spawn keeps parent Run/resources and can deadlock children | **Resolved.** Blocking spawn commits parent `terminalTarget=Yielded`; Run safely terminalizes/releases Run capacity; Task then Waiting with zero live Runs; join satisfaction returns Ready and a new Run resumes from ContinuationContext. Joined/detached spawn deferred from v1. | `blocking-spawn-and-run-yield.md`, `task-spawn-and-dynamic-graphs.md`, `task-lifecycle.md`, `run-and-attempt.md` |
| 3 | Evaluator execution is unowned/unaccounted second execution path | **Resolved.** Operator-governed immutable EvaluatorVersions; EvaluationRound/Operation; control-operation reservations/budget; independent verification Sandbox. V1 evaluator kinds check/schema/human; model-based rubric/review deferred rather than creating an extra Agent-Run path. | `evaluation-and-evaluator-registry.md`, `task-acceptance-and-completion.md`, `scheduler-resource-ledger-and-admission.md` |
| 4 | Undefined `policyHash`/reload semantics | **Resolved.** Atomic immutable ConfigurationRevision manifest with domain-specific component digests, compiled/validated before activation; no ambiguous generic policy hash. Live Run authority intersects frozen ceiling with current policy. | `configuration-and-policy-revisions.md`, `permissions-and-capabilities.md`, scheduler/routing/persistence docs |

## High findings

### 5. Task-scoped reservations recreated per Run

**Resolved.** Desired effective claims are separated from incremental claims. Compatible existing Task-scoped reservations are subtracted before Run admission. Persistence enforces one non-released Task reservation per singular `(task_id, resource_key)` family. New Runs get fresh Run-scoped reservations only.

Canonical:

- `scheduler-reservations-ownership-and-leases.md`
- `scheduler-resource-ledger-and-admission.md`
- `execution-offer-routing-and-admission-handshake.md`
- `sqlite-persistence-and-transactions.md`

### 6. `ensureExecution` idempotency delegated to adapters where many backends cannot provide it

**Resolved with explicit launch classes.** Execution Fabric now distinguishes:

```text
KEYED_IDEMPOTENT
OBSERVATIONAL
```

Pantheon may supply keyed semantics through an outer process/session supervisor. OBSERVATIONAL offers are ineligible where ambiguous duplicate execution cannot be safely bounded.

Canonical:

- `execution-fabric.md`
- `run-and-attempt.md`
- `scheduler-dispatch-and-run-intent-reconciliation.md`

### 7. No durable marker that backend launch may have been contacted

**Resolved.** Attempt has durable `NOT_CONTACTED | CONTACT_MAY_HAVE_OCCURRED`; Run Controller commits may-have-contact state immediately before the first external launch call. Missing acknowledgement after that marker is UNKNOWN until proven otherwise.

Canonical:

- `run-and-attempt.md`
- `scheduler-dispatch-and-run-intent-reconciliation.md`
- `sqlite-persistence-and-transactions.md`

### 8. Goal lifecycle/completion controller missing

**Resolved.** Goal phases are:

```text
Planning -> Active -> Evaluating -> Finalizing -> Succeeded|Failed|Cancelled
```

Goal Completion Controller owns deliverable bindings/completion candidate/evaluation/finalization. Goal success is not all-Tasks-terminal.

Canonical:

- `goal-resource.md`
- `goal-lifecycle-and-completion-controller.md`
- `goal-revision-reconciliation.md`

### 9. `Run Finalizing => Candidate` contradiction and no Run terminalTarget

**Resolved.** Every Finalizing Run has durable `terminalTarget = Completed|Failed|Cancelled|Yielded`. Only:

```text
Run Completed => Candidate exists
```

Yielded/Failed/Cancelled may have no Candidate.

Canonical:

- `run-and-attempt.md`
- `sqlite-persistence-and-transactions.md`

### 10. Candidate submission vs cancellation precedence unspecified

**Resolved.** T6 and Agent Control submission revalidate expected Task revision/current Run/Attempt/AgentControlSession and no cancellation/supersession fence. **The state transition that commits first wins.** Cancel/supersede-first makes submit fail stale/conflict; Candidate-first preserves immutable Candidate history even if cancellation follows.

Canonical:

- `task-lifecycle.md`
- `run-and-attempt.md`
- `sqlite-persistence-and-transactions.md`
- `public-daemon-api-and-cli.md`

### 11. REQUEUE_TASK could make Ready while old Run still nonterminal

**Resolved.** Requeue decision may be recorded early but T9 requires the prior responsible Run to be terminal. Until then Task remains Evaluating/appropriate current state with `PriorRunFinalizing`. Scheduler cannot create next Run early.

Canonical:

- `recovery-policy.md`
- `task-lifecycle.md`
- `sqlite-persistence-and-transactions.md`

### 12. UNKNOWN obligations had no bounded/operator escape hatch

**Resolved with explicit administrative tombstoning.** Operator-only force-resolution targets an exact unresolved obligation, requires reason/risk acknowledgement, tombstones/fences the old LaunchKey/Sandbox/external lineage and performs explicit resource/accounting settlement.

**Intentional difference from the original proposed fix:** force resolution does **not** fabricate factual Usage/Charge by pretending the unresolved BudgetHold was fully consumed. Late legitimate usage remains recordable and may produce truthful overdraw.

Canonical:

- `recovery-policy.md`
- `budget-usage-and-rate-limits.md`
- `scheduler-reservations-ownership-and-leases.md`
- `sqlite-persistence-and-transactions.md`
- `public-daemon-api-and-cli.md`

### 13. `code.changeset` depended on prunable Git ODB / Git-rendered patch identity

**Resolved more strongly than proposed.** `code.changeset` is now CAS-complete: canonical changed-path entries reference Pantheon CAS Blobs; Git-generated patch text is derived review output, not identity. Git tree/commit IDs remain provenance/verification, and controller-owned Git refs may optionally pin objects for efficient integration, but Artifact correctness never relies solely on Git GC retention.

Canonical:

- `artifact-model.md`
- `workspace-and-git-integration.md`
- `sqlite-persistence-and-transactions.md`

### 14. Usage source key unnamespaced; proposal to fence usage on control epoch

**Resolved with corrected factual-usage semantics.** Source identity is namespaced by:

```text
backend_id + attempt/control-operation + adapter_operation_key + meter
```

Attempt usage is accepted only for the backend named in the immutable Binding.

**Intentional difference from the original proposed fix:** Pantheon does **not** reject otherwise-valid delayed usage merely because a controller epoch changed. Usage is factual observation, not a stale authority command. Epoch/incarnation may be retained as provenance/anomaly evidence.

Canonical:

- `budget-usage-and-rate-limits.md`
- `execution-fabric.md`
- `sqlite-persistence-and-transactions.md`

### 15. Sandbox did not exclude Pantheon state/CAS/policy/peer Workspaces

**Resolved.** Untrusted model-driven shell requires `isolation.control-plane`; Sandbox ambient authority excludes Operator socket, DB/config, raw CAS, peer Workspaces, SecretProvider administration, authoritative Git common-dir state, host credential agents and container/hypervisor runtime sockets.

Canonical:

- `sandbox-broker-and-isolation.md`
- `permissions-and-capabilities.md`
- `workspace-and-git-integration.md`

### 16. Grant use-count / ticket redemption not atomic under policy change

**Resolved.** Consequential redemption performs current authority/config re-evaluation + Grant use-count CAS + exact broker-operation creation in one SQLite transaction. Internal tickets are short-lived single-use references and do not bypass current policy at redemption.

Canonical:

- `permissions-and-capabilities.md`
- `sqlite-persistence-and-transactions.md`
- `secret-store-and-credential-brokering.md`

### 17. Goal reconciliation could terminalize Superseded Task around live Run

**Resolved.** `SUPERSEDE` drives Task through `Finalizing/terminalTarget=Superseded`; responsible Run is stopped/reconciled and UNKNOWN keeps Task Finalizing. Terminal Superseded only after obligations are safe.

Canonical:

- `goal-revision-reconciliation.md`
- `task-lifecycle.md`

### 18. Context Builder missing

**Resolved.** Deterministic Context Builder produces immutable provider-neutral content-addressed ContextPlan before Attempt creation. Backend adapters render the plan; provider conversations are never durable Pantheon truth. Blocking continuation and Acceptance feedback create new Run context rather than depending on old provider sessions.

Canonical:

- `context-builder.md`
- `run-and-attempt.md`

### 19. Operator API missing dispatch/resources/reservations/Workspaces/backends/recovery quarantine

**Resolved.** Operator API/CLI now includes read/operator surfaces for backends, resources, reservations, Workspaces, Sandboxes, dispatch pause/resume, configuration, recovery findings/decisions and audited UNKNOWN force-resolution. CLI remains a thin daemon API client.

Canonical:

- `public-daemon-api-and-cli.md`

## Additional architecture corrections made during resolution

The review fixes exposed several adjacent contracts that are now also explicit:

1. **Secret Store / Credential Broker.** Raw Agent `secret.read` is hard-denied in v1; `secret.use` means Pantheon-owned brokered operation, not secret injection into arbitrary shell. Long-lived secret bytes are never stored in SQLite.
2. **Sandbox/Git boundary.** For untrusted coding shell, v1 prefers Task-local isolated Git state rather than writable linked-worktree common-dir authority.
3. **Agent Genome v1 scope.** SOUL/BEHAVIOR/approved Skills/bounded Memory are static frozen Run inputs; automatic reflection/promotion is deferred.
4. **Dynamic Task spawn v1 scope.** Blocking only; joined/detached/quorum/semantic dedup are deferred.
5. **Model-based authoritative evaluator review is deferred.** V1 uses deterministic check/schema plus human when deterministic evaluation is insufficient.
6. **Architecture overview is now provider-neutral and links the current security/lifecycle boundaries.**

## Second-review focus

A second adversarial review should now prioritize **cross-document residual contradictions**, not redesign the resolved subsystems. In particular verify:

- no stale `policyHash` semantics remain authoritative where domain-specific config digests are required;
- no document still permits Task Ready/Waiting with a live Run;
- no document assumes linked worktree alone is security isolation;
- no path exposes Operator Control/raw secrets/raw CAS to Agent Sandboxes;
- no Recovery path creates replacement execution while old lineage UNKNOWN;
- no requeue transaction can beat prior Run terminalization;
- no usage/accounting path fabricates actual consumption during UNKNOWN force resolution;
- code.changeset remains reconstructable without relying on Task Git ODB;
- configuration relaxation cannot broaden an existing Run;
- every external-effect domain follows durable intent/idempotency identity -> external effect -> reconciliation.

## Implementation readiness gate

Do not generate implementation GitHub issues until the second architecture review has classified remaining Critical/High issues. If that review returns no unresolved Critical and only small High/Medium documentation corrections, perform the final consistency patch and then proceed to Rust module/trait boundaries and the v1 implementation issue graph.
