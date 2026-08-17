# Second Adversarial Architecture Review — August 2026

## Scope

Full re-read of `docs/architecture/**` (37 documents, ~12.6k lines), `schemas/agent-v1alpha1.schema.json`, and `docs/reviews/2026-08-architecture-review-resolution.md`, against the first review (`claude/pantheon-architecture-review-kue3se`, commit `5fd665bf8af707897b63aefa21f3e7ce72fee9b6`) and the correction sweep that followed it.

No code was written and no GitHub issues were created.

---

## A. Executive verdict

The correction sweep is real, not asserted. All four Critical and fifteen High findings have genuine, load-bearing resolutions in the canonical documents, and the corpus is now unusually self-consistent: I could not find a single surviving instance of `Waiting`-with-live-Run, `Finalizing ⇒ Candidate`, yielded-Run resumption, worktree-as-security-boundary, `native/workspace/isolated` security classes, v1 joined/detached spawn, v1 PROGRESSIVE planning, v1 model-authoritative evaluation, or usage rejected on control epoch.

One **new Critical** remains, and it is a real hole rather than a wording slip. Every document forbids the untrusted Agent from reaching host authority, but no document constrains *Pantheon's own* Git execution against repository state the Agent fully controls. `workspace-and-git-integration.md:145` has the controller build the result tree with a "controller-owned temporary index" inside the Agent's writable clone. Git honours `.git/config`, `.gitattributes` and `.git/hooks/**` found there, so an Agent can execute code as the daemon user — the exact user that owns `pantheon.db`, the operator socket, raw CAS and SecretProvider authority. This defeats findings #1 and #15 through a path neither of them models.

Four new High findings are bounded contract additions rather than redesigns: control-operation UsageRecords have no backend-ownership rule (the Attempt case is validated, the control-operation case is not); database restore fences executors, Git and secrets but never authorization Grants, broker operations or commandIds; the EvaluationOperation launch/contact marker is mandated in prose but absent from persistence; and verification Sandboxes have no persistable holder scope.

Remaining Medium/Low items are two stale `policyHash`/`policyRevision` strings, a schema that still accepts deferred Genome-promotion values, several invariants left as prose where a SQLite `CHECK` is expressible, and one broken cross-reference.

V1 is implementable by one strong Rust engineer plus coding agents. The semantics are decided; what is left is module/DDL design, tests and fault injection.

**Verdict: READY AFTER SMALL ARCHITECTURE PATCHES** — gated on C1 being patched before implementation issues are generated.

---

## B. Original finding closure matrix

| # | Status | Evidence / current docs | Remaining issue |
|---|--------|-------------------------|-----------------|
| 1 | **CLOSED** | `agent-control-channel.md` §§1–3, 7, 19–20, 26; `sandbox-broker-and-isolation.md` "Mandatory control-plane isolation", "Agent Control exposure"; `permissions-and-capabilities.md` "Request path"; `public-daemon-api-and-cli.md` "Trust boundary" | Credential authenticates identity only; Attempt-scoped session; operator socket physically excluded. Note C1 bypasses the *outcome* of this finding via a different path. |
| 2 | **CLOSED** | `blocking-spawn-and-run-yield.md` §§1–5, 11–13, 20; `task-lifecycle.md` "Waiting", inv. 4–6; `run-and-attempt.md` "Yielded", "Blocking yield"; `task-spawn-and-dynamic-graphs.md` "V1 spawn mode" | Parent finalizes to `Yielded`, releases Run capacity, Task `Waiting` with zero live Runs, join → `Ready` → new Run from ContinuationContext. Consistent in all five documents. |
| 3 | **CLOSED** | `evaluation-and-evaluator-registry.md` §§1–2, 5, 10–15; `task-acceptance-and-completion.md` "V1 evaluator kinds"; `scheduler-resource-ledger-and-admission.md` "Evaluation/control operations" | check/schema/human only; EvaluationOperation is not a Run; control-operation reservations. New H3/H4 concern its *persistence*, not the semantic decision. |
| 4 | **PARTIALLY CLOSED** | `configuration-and-policy-revisions.md` §§1–2, 11, inv. 7; `execution-offer-routing-and-admission-handshake.md:148,219`; `scheduler-ready-task-eligibility.md:36`; `sqlite-persistence-and-transactions.md:267`; `overview.md:140` | Two stale residues survive: `event-and-observability-model.md:339` still emits `policyHash: sha256:...`; `evaluation-and-evaluator-registry.md:198` and `:534` still use `policyRevision`/`policy_revision` where `configRevision` + `evaluatorRegistryDigest` is canonical (`configuration-and-policy-revisions.md` §11). See M1/M2. |
| 5 | **CLOSED** | `scheduler-reservations-ownership-and-leases.md` "Critical rule", inv. 3–5; `scheduler-resource-ledger-and-admission.md` "Incremental claim set"; `execution-offer-routing-and-admission-handshake.md` "Desired versus incremental resources"; `sqlite-persistence-and-transactions.md` "ResourceReservation" | Desired vs incremental claims separated; singular `(task_id, resource_key)` uniqueness stated in all four. |
| 6 | **CLOSED** | `execution-fabric.md` "Launch semantics", inv. 6; `run-and-attempt.md` "LaunchKey and launch semantics"; `scheduler-dispatch-and-run-intent-reconciliation.md` "Launch semantics" | `KEYED_IDEMPOTENT|OBSERVATIONAL`; unsafe observational offers filtered at routing; outer supervisor may supply keyed semantics. |
| 7 | **CLOSED** | `run-and-attempt.md` "Durable pre-launch contact marker"; `scheduler-dispatch-and-run-intent-reconciliation.md` "Pre-launch contact marker"; `sqlite-persistence-and-transactions.md:306–334` (T4b) | Correct conservative asymmetry: can over-report UNKNOWN, cannot falsely prove absence. Same discipline is required-but-unpersisted for EvaluationOperations — see H3. |
| 8 | **CLOSED** | `goal-lifecycle-and-completion-controller.md` (whole); `goal-resource.md` "Lifecycle"; `goal-revision-reconciliation.md` | Planning→Active→Evaluating→Finalizing→terminal; success from required deliverables + optional acceptance, explicitly not all-Tasks-terminal. |
| 9 | **CLOSED** | `run-and-attempt.md` "Finalizing", inv. 4–5, 12; `sqlite-persistence-and-transactions.md:296–304` — *"The obsolete invariant `Run Finalizing => Candidate exists` is invalid."* | Every Finalizing Run has `terminalTarget`; only `Completed` requires a Candidate. Explicit repudiation of the old rule. |
| 10 | **CLOSED** | `task-lifecycle.md` "Cancellation precedence"; `run-and-attempt.md` "Candidate submission"; `sqlite-persistence-and-transactions.md` T6; `agent-control-channel.md` §18; `public-daemon-api-and-cli.md` Problem Details | Commit-order decides; Candidate-first stays immutable history; loser gets deterministic `stale-authority`/`conflict`. |
| 11 | **CLOSED** | `recovery-policy.md` "REQUEUE_TASK" + inv. 5; `task-lifecycle.md` "Acceptance rejection and requeue" + inv. 8; `sqlite-persistence-and-transactions.md` T9; `evaluation-and-evaluator-registry.md` §26 | `PriorRunFinalizing` condition holds the Task in Evaluating; T9 preconditions require prior Run terminal. |
| 12 | **CLOSED** | `recovery-policy.md` "Operator force-resolution of UNKNOWN" + "No automatic timeout force-release"; `budget-usage-and-rate-limits.md` "Unknown final usage"; `sqlite-persistence-and-transactions.md` "Recovery tombstones"; `public-daemon-api-and-cli.md` force-resolve | Audited lineage tombstoning + administrative settlement; explicitly does not fabricate Usage/Charge. The intentional divergence from the proposed fix is correct. |
| 13 | **CLOSED** | `artifact-model.md` "Code changeset Artifact" (all four sub-sections) + inv. 6–8; `workspace-and-git-integration.md` "Candidate sealing", "Git object retention" + inv. 7 | CAS-complete manifest; Git patch bytes excluded from identity; Git refs are optional pins. Stronger than proposed. C1 concerns the *capture* path, not this identity decision. |
| 14 | **PARTIALLY CLOSED** | `budget-usage-and-rate-limits.md` "Usage provenance and idempotency", "Controller lease epoch is not usage truth", inv. 3–6; `sqlite-persistence-and-transactions.md:376–383` | Attempt usage is fully namespaced and Binding-validated. **Control-operation usage has no equivalent ownership rule** — `budget-usage-and-rate-limits.md:79` validates only the Attempt case. See H1. |
| 15 | **PARTIALLY CLOSED** | `sandbox-broker-and-isolation.md` "Mandatory control-plane isolation", "Filesystem exposure", "Credential isolation", inv. 5; `permissions-and-capabilities.md` "Hard security exclusions"; `workspace-and-git-integration.md` "Security exclusions" | The exclusion list is complete and correct for *ambient Agent authority*. It does not cover **controller-initiated execution against Agent-controlled repository state**, which re-crosses the same boundary. See C1. |
| 16 | **CLOSED** | `permissions-and-capabilities.md` "Atomic Grant use-count redemption", "Capability tickets", inv. 5–6; `sqlite-persistence-and-transactions.md` "Grants and broker operation redemption" T10; `secret-store-and-credential-brokering.md` §25 | One `BEGIN IMMEDIATE`: re-evaluate under current policy + CAS use-count + create exact broker operation. Tickets revalidated at redemption. Restore interaction is unhandled — see H2. |
| 17 | **CLOSED** | `goal-revision-reconciliation.md` "Critical supersession rule" + inv. 4; `task-lifecycle.md` "Supersession" + inv. 9; `taskgraph-dependencies.md` "Supersession" | `Finalizing/terminalTarget=Superseded` path mandatory; UNKNOWN keeps Task Finalizing. |
| 18 | **CLOSED** | `context-builder.md` (whole, esp. "Run boundary", "Provider/session boundary", "Blocking continuation") ; `run-and-attempt.md` Run identity; `blocking-spawn-and-run-yield.md` §14 | Deterministic, provider-neutral, content-addressed ContextPlan attached before Attempt creation; provider conversation is never durable truth. |
| 19 | **CLOSED** | `public-daemon-api-and-cli.md` "Read endpoints", "Required operator mutation surface", CLI section | backends, resources, reservations, workspaces, sandboxes, dispatch pause/resume, configuration, recovery findings/decisions, audited force-resolve all present. |

**Score: 15 CLOSED, 4 PARTIALLY CLOSED, 0 OPEN, 0 FIX-INTRODUCED-A-NEW-ISSUE.**

No resolution introduced a regression. The four partials are residue and omission, not contradiction.

---

## C. New Critical / High findings

### C1 (CRITICAL) — Controller-executed Git inside an Agent-writable repository is an unmodelled sandbox escape

**Where:** `workspace-and-git-integration.md:145` ("controller-owned temporary index"), `:164–176` (candidate sealing flow), "Git object retention"; `global-recovery-and-crash-reconciliation.md:532` (`git worktree` inventory/repair), "Workspace and Git reconciliation"; `sandbox-broker-and-isolation.md` "Git authority boundary".

**Gap:** The corpus contains zero occurrences of `hook`, `gitattributes`, `filter`, `smudge`, `fsmonitor`, or `core.*` in any security context. Every document reasons about what the Agent may *reach*; none reasons about what Pantheon *executes* against bytes the Agent wrote.

**Failure scenario:**

1. A `code.implement` Task runs under `CONTAINER` isolation with `workspace.strategy: isolated-clone` — the configuration `agent-manifest.md` explicitly *prefers* for untrusted shell. The Agent legitimately owns the working tree, index, local refs and Task-local Git metadata (`workspace-and-git-integration.md`, "Isolated Git state is preferred for untrusted shell").
2. Within its own writable workspace the Agent writes:
   ```
   .git/config      [filter "z"] clean = /bin/sh -c 'cat ~/.pantheon/state/pantheon.db | ...'
   .gitattributes   * filter=z
   ```
   Equivalent variants: `core.hooksPath`, `core.fsmonitor`, `diff.<d>.textconv`, or a plain `.git/hooks/pre-commit`. Nothing in the SandboxPlan mount rules forbids this — `.git` inside `/workspace` is exactly the state the Task is supposed to own.
3. The Agent calls `task.submit_result`. Per the canonical sealing flow, the controller quiesces the workspace and **builds the result tree using a controller-owned temporary index** — i.e. runs `git add`/`git write-tree` against that repository.
4. Git reads the repository-local config and attributes and executes the clean filter. If the Workspace Controller runs in the daemon process (the natural reading of "controller-owned", and required for it to write into Pantheon CAS), that command runs **as the daemon user, outside the Sandbox**, with access to `pantheon.db`, the operator socket, raw CAS, active configuration and SecretProvider authority.

The Agent never touched a forbidden mount, never called a forbidden verb, and never escaped its container. It simply left a landmine in a directory Pantheon promised it could own, and waited for Pantheon to step on it. The same exposure exists on the recovery path (`git worktree list/repair` over quarantined/dangling repositories) and on optional Git object pinning.

This nullifies the practical effect of findings #1 and #15.

**Smallest correction:** Add a **"Hostile repository state"** section to `workspace-and-git-integration.md` and a matching invariant in `sandbox-broker-and-isolation.md`:

- Repository metadata under an Agent-writable Workspace (`.git/config`, `.git/hooks/**`, `.gitattributes`, `.gitmodules`, `.git/info/**`) is **untrusted input**, in the same class as reference data in `context-builder.md`'s trust strata.
- All Pantheon-initiated Git execution against Agent-writable repository state runs either inside the Sandbox or in an equally-confined controller-owned helper — never with ambient daemon-user authority.
- That execution disables Git's code-execution surface explicitly: `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_SYSTEM=/dev/null`, `-c core.hooksPath=/dev/null`, `-c core.fsmonitor=`, no clean/smudge/textconv filter drivers, `-c protocol.*.allow=never`.
- Add invariant: *"Pantheon never executes a repository-configurable tool with host authority against Agent-writable repository state."*
- Add failure code `workspace.hostile-repository-state` in the `sandbox.invariant-violation` severity class.
- Extend `agent-manifest.md` inv. 4 and the `overview.md` security boundary to name this as a distinct boundary from ambient Sandbox authority.

This is an addition consistent with every existing invariant; it does not redesign the Workspace or Sandbox subsystem.

---

### H1 (HIGH) — Control-operation UsageRecords have no backend-ownership validation

**Where:** `budget-usage-and-rate-limits.md:69` (identity includes `attempt_id (or explicit control-operation id)`), `:75–82` (acceptance rules); `sqlite-persistence-and-transactions.md:376–383`; `execution-fabric.md:230`.

**Gap:** The Attempt branch is airtight — *"the immutable Run Binding names the reporting backend as the backend responsible for that Attempt lineage"* (`:79`), restated in persistence at `:383` and asserted as inv. 4. The control-operation branch has **no corresponding rule anywhere**. Persistence only adds *"a CHECK ensuring exactly one execution/control-operation subject"* — a shape constraint, not an ownership constraint.

**Failure scenario:** Backend B is registered for a cheap local model. It is compromised, or merely buggy. It posts a UsageRecord with `backend_id = B`, `control_operation_id = evalop_91` (an EvaluationOperation belonging to another Goal, whose IDs are enumerable from the operator API or guessable), a fresh `adapter_operation_key` and meter `tokens = 5,000,000`. Namespaced idempotency passes (novel identity). The "exactly one subject" CHECK passes. No rule requires B to actually own `evalop_91`. Pantheon ingests it as factual usage, writes a ChargeRecord and BudgetConsumption, and — per the correct-in-itself overdraw rule — marks the victim Goal's account overdrawn. A HARD account then denies new Hold authority and stalls unrelated work. Because `budget-usage-and-rate-limits.md` rightly forbids clamping or rewriting factual usage, the poisoned record cannot simply be deleted; it requires administrative settlement.

Finding #14's fix closed the Attempt half of the door and left the control-operation half open.

**Smallest correction:** In `budget-usage-and-rate-limits.md` "Usage provenance and idempotency", add the symmetric clause: *for control-operation usage, the record is accepted only when the durable EvaluationOperation/control-operation record names the reporting backend as its executor and is in a state in which usage is possible.* Mirror it in `sqlite-persistence-and-transactions.md:383` and add to inv. 4 ("a backend may report usage only for the execution **or control operation** lineage it owns"). Add the corresponding check to the PersistenceInvariantChecker's "Usage provenance/backend ownership" line.

---

### H2 (HIGH) — Database restore does not fence authorization or command state

**Where:** `global-recovery-and-crash-reconciliation.md:839–857` (§27 Backup/Restore), inv. 22; `permissions-and-capabilities.md` "Atomic Grant use-count redemption"; `sqlite-persistence-and-transactions.md` T10, "Commands".

**Gap:** §27 is thorough about domains it names — executors, Git refs, worktrees, object stores — and `secret-store-and-credential-brokering.md` §29 independently handles SecretProvider drift. Step 5 says "inventories every external domain capable of containing Pantheon-owned state", but **authorization state is not an external domain**: Grants, `broker_operations`, `capability_tickets` and `commands` live *inside* the restored database and are silently rewound with it, while the external effects they authorized already happened and are not inventoriable.

**Failure scenario A (Grant replay):** An operator approves a `uses: 1` Grant for `git.push` to a production repo. The Agent redeems it; T10 CAS-consumes the use and creates the broker operation; the push lands. The next day, disaster restore rolls the database back to a snapshot from before redemption. Restore recovery rotates lease tokens, inventories backends and Git refs, and satisfies the barrier. The Grant row is now back at `remaining uses: 1`, unexpired and in scope; the `broker_operations` row proving prior redemption is gone. The next `action.invoke` re-evaluates under current policy — which permits it, because the Grant looks unspent — and pushes again. One-use authority was consumed twice from one human approval. No invariant is violated; every check passes on rewound data.

**Failure scenario B (command idempotency loss):** Automation retries operator commands with stable `commandId`s. `sqlite-persistence-and-transactions.md` promises "same command ID + same hash returns/reconciles prior outcome". After restore, the `commands` rows are gone, so a retried Goal-revision, budget-adjustment or integration command is executed a second time as if new.

**Smallest correction:** Extend `global-recovery-and-crash-reconciliation.md` §27 "Restore" with an authorization-fencing step between current steps 4 and 5:

- All Grants that predate the restore point are marked `SUSPENDED_PENDING_RECONCILIATION`; redemption fails closed until an operator re-affirms or expires them.
- Introduce a redemption epoch rotated on restore, so pre-restore broker-operation idempotency keys can never be matched or re-created.
- State explicitly that `commandId` idempotency is **not** preserved across restore, and that clients must treat post-restore command outcomes as unknown.
- Add to inv. 22 and to `permissions-and-capabilities.md` inv. 5.

---

### H3 (HIGH) — EvaluationOperation launch/contact boundary is mandated in prose but absent from persistence

**Where:** `evaluation-and-evaluator-registry.md:440–452` (§22) requires `durable EvaluationOperation intent → durable launch/contact marker → external execution → observation`; `sqlite-persistence-and-transactions.md:306–334` provides `launch_contact_state` only on `attempt_status`; `:509` describes evaluation persistence with no contact fields; `evaluation_operations`/`evaluation_attempts` in the table families carry only `state`.

**Gap:** The architecture correctly generalised the finding-#7 discipline to evaluation, but the persistence design never received the columns, and the PersistenceInvariantChecker list does not mention it. There is also no `T`-family transaction for the evaluation launch boundary (T4b exists only for Attempts).

**Failure scenario:** An EvaluationOperation for `check://project/integration-tests` is admitted; the admission transaction commits reservations and intent. The controller invokes the verification SandboxBackend. The daemon crashes after the container-create call is issued but before any observation is durable. On restart, `evaluation_attempts.state = PENDING` is indistinguishable from "never launched" — exactly the ambiguity the Attempt marker exists to remove. §22's rule ("Pantheon does not launch an overlapping replacement until the prior operation is reconciled") has no durable fact to reconcile against, so the controller either stalls the criterion indefinitely or launches a duplicate. Duplicate verification consumes real accounted capacity, and for any evaluator granted network under §18's explicit-authorization escape, it can double an external side effect.

**Smallest correction:** Mirror the Attempt columns onto `evaluation_attempts` (`launch_contact_state`, `launch_contact_initiated_at`, `launch_contact_epoch/incarnation`) in `sqlite-persistence-and-transactions.md` "Evaluation"; add a named transaction `T15 EVALUATION LAUNCH CONTACT MARKER`; add *"Evaluation attempt launch contact state present before external verification execution"* to the invariant-checker list and to core invariant 7.

---

### H4 (HIGH) — Verification Sandboxes have no persistable holder scope

**Where:** `evaluation-and-evaluator-registry.md` §18 and `sandbox-broker-and-isolation.md:164` ("EvaluationOperations use separate verification Sandboxes and never the producer Run Sandbox"); `sandbox-broker-and-isolation.md:153–162` and `sqlite-persistence-and-transactions.md:361` both state SandboxInstance is Run-scoped.

**Gap:** The resource ledger was correctly generalised to a three-way holder scope (`Task | Run | control-operation`) in three documents. **SandboxInstance was not.** Both the Sandbox document and the persistence design describe SandboxInstance ownership only as Run-scoped, and `sandbox_instances` has no holder-kind column. A verification Sandbox therefore has no expressible owner.

**Failure scenario:** An EvaluationOperation provisions a verification Sandbox with a durable `SandboxKey`. The daemon crashes. Startup recovery (`global-recovery-and-crash-reconciliation.md` §12, §31) reconciles Sandboxes by walking nonterminal Runs. This Sandbox belongs to no Run. It is either (a) invisible to reconciliation and leaks as an untracked container holding accounted capacity, or (b) discovered by SandboxBackend inventory with no matching ownership record and classified "dangling/unknown ownership → quarantine" — a false RecoveryFinding on every crash during evaluation. Goal finalization's *"no active EvaluationOperation under the Goal"* check also has no Sandbox ownership edge to traverse.

**Smallest correction:** Give `sandbox_instances` the same explicit holder triple used by `resource_reservations` (`holder_kind ∈ {Run, control-operation}` + holder FK) in `sqlite-persistence-and-transactions.md` "Sandbox"; update `sandbox-broker-and-isolation.md` "Ownership and lifetime" to show the control-operation branch alongside the Run branch; add the verification-Sandbox path to `global-recovery-and-crash-reconciliation.md` §12/§31 reconciliation order.

---

## D. Remaining Medium / Low inconsistencies

| ID | Sev | Location | Issue | Correction |
|----|-----|----------|-------|-----------|
| M1 | Med | `event-and-observability-model.md:339` | Authorization-event example still emits `policyHash: sha256:...`, the exact field finding #4 declared obsolete. The Events surface is where auditors read authorization evidence, so the stale field would ship. | Replace with `configRevision: cfgrev_43` + `authzPolicyDigest: sha256:...` per `configuration-and-policy-revisions.md` §2/§11. |
| M2 | Med | `evaluation-and-evaluator-registry.md:198`, `:534` | EvaluationRound example uses `policyRevision:` and the persistence shape uses `policy_revision`, contradicting `configuration-and-policy-revisions.md:§11` which specifies `configRevision` + `evaluatorRegistryDigest` for EvaluationRound, and `scheduler-ready-task-eligibility.md:36` which explicitly bans "ambiguous generic `policyRevision`". | Rename both to `configRevision` and add `evaluatorRegistryDigest`. |
| M3 | Med | `schemas/agent-v1alpha1.schema.json:190–205` | `learning.reflection` accepts `after-task|on-failure|manual` and `learning.promotion.*` accepts `automatic|eval-gated` — the post-v1 pipeline `agent-genome.md` defers. `agent-manifest.md` only says manifests *"should"* configure it disabled (non-normative). A v1 daemon would accept a manifest promising automatic Genome promotion it cannot honour. | Either restrict the v1 schema to `disabled`, or add a normative statement that v1 config compilation rejects non-`disabled` learning values, mirroring `blocking-spawn-and-run-yield.md` §20's treatment of `joined`/`detached`. |
| M4 | Med | `sqlite-persistence-and-transactions.md:213`, `:296–304`, invariant-checker list | `Ready\|Waiting ⇒ zero live Run`, `Finalizing ⇒ terminal_target`, `Completed ⇒ candidate_digest` are left to controller logic plus a post-hoc checker, though all three are expressible as table `CHECK`s given the existing columns (`tasks.phase`/`active_run_id`; `run_status.phase`/`terminal_target`/`candidate_digest`). Since v1 bans triggers, `CHECK` is the only declarative enforcement available and it is being left unused. | Add the three `CHECK` constraints to the persistence design; keep the checker as defence in depth. |
| M5 | Med | `sqlite-persistence-and-transactions.md:306–334`, checker line "one nonterminal Attempt per Run" | The partial-unique-index technique used for Runs requires the terminal discriminator in the indexed table, but `terminal` lives on `attempt_status` while identity lives on `attempts`. The document does not say how the constraint is realised. | State the chosen mechanism (e.g. a nullable `run_id` on `attempt_status` under a partial unique index, or denormalise `terminal` onto `attempts`), as was done explicitly for Runs. |
| L1 | Low | `task-lifecycle.md:17` | "See also" points at `goal-revision-and-reconciliation.md`; the file is `goal-revision-reconciliation.md`. Only broken cross-reference in the corpus. | Fix the filename. |
| L2 | Low | `schemas/agent-v1alpha1.schema.json` + `agent-manifest.md` example | `genome.memory.retrieval.mode: adaptive` is undefined against `context-builder.md` inv. 12 ("V1 context selection/trimming is deterministic"). Whether "adaptive" means deterministic relevance scoring or model-driven selection is unstated. | Define `adaptive` as deterministic retriever behaviour with a frozen retriever/policy version, or rename to avoid implying model judgement. |
| L3 | Low | 11 documents | `## Status` is split between "Draft design", "Canonical" and "Accepted architecture correction" with no semantic difference — `global-recovery`, `agent-control-channel`, `evaluation`, `secret-store`, `configuration`, `goal-lifecycle`, `context-builder`, `blocking-spawn`, `event-model`, `task-object`, `fairness` all carry canonical invariants while labelled Draft. | Normalise to one status vocabulary before issue generation, so "Draft" does not read as "not yet binding". |

---

## E. Crash / race retest matrix

| # | Scenario | Verdict | Why |
|---|----------|---------|-----|
| 1 | Crash before / after T3 Run-intent commit | **SAFE** | Before: no Run exists; SchedulingClaim expires and another cycle retries (`scheduler-reservations…` "SchedulingClaim"). After: Run/Binding/Reservations/Holds all committed atomically; Run Controller reconciles from durable state (`scheduler-dispatch…` "Restart"). No external effect occurs inside T3. |
| 2 | Crash after Attempt creation, before contact marker | **SAFE** | `attempt_status.launch_contact_state = NOT_CONTACTED` is durable proof the launch path never crossed the call boundary (`run-and-attempt.md`, `sqlite-persistence…:330–334`). Controller may commit the marker and launch, or conclude ABSENT. |
| 3 | Crash after contact marker, before / during backend call | **SAFE** | `CONTACT_MAY_HAVE_OCCURRED` ⇒ UNKNOWN until proven otherwise; same LaunchKey reused; no replacement Attempt. Deliberately over-conservative in the safe direction — the asymmetry is stated explicitly. |
| 4 | Lost launch ack, KEYED_IDEMPOTENT backend | **SAFE** | Repeated `ensureExecution` with the same LaunchKey addresses one lineage (`execution-fabric.md` "Launch semantics"). Reconciliation is not a retry and does not duplicate usage. |
| 5 | Lost launch ack, OBSERVATIONAL backend | **SAFE** | Stays UNKNOWN; no blind re-create. Such offers are filtered at routing where duplicates would violate the safety envelope (`execution-offer-routing…` "Candidate validation"). Correctly refuses to launder retry-usually-works into idempotency. |
| 6 | Daemon restart with live Attempt / Sandbox | **SAFE** | Installation lock → incarnation → inventory → lease-token rotation → domain reconciliation → barrier → dispatch gate (`global-recovery…` §6). Same Attempt/LaunchKey/AgentControlSession preserved (`agent-control-channel.md` §9). Sandbox re-inspected by the same SandboxKey. |
| 7 | UNKNOWN + operator force-resolution + late callback | **SAFE** | `external_lineage_tombstones` permanently fences the LaunchKey; late callbacks are retained as history/anomaly evidence but cannot reacquire current authority (`recovery-policy.md` "Late observations", `sqlite-persistence…` "Recovery tombstones"). |
| 8 | UNKNOWN force-resolution + late Usage record | **SAFE** | Administrative settlement is stored separately from factual Usage; late valid usage still ingests on immutable backend+Attempt provenance and may create truthful overdraw (`budget…` inv. 9, `recovery-policy.md` "Recovery"). The refusal to fabricate consumption is the correct call. |
| 9 | Two concurrent requests consuming the last one-use Grant | **SAFE** *(but see H2)* | Single `BEGIN IMMEDIATE`: re-evaluate under current policy, CAS the use-count, create the exact broker operation — one transaction, serialized writer. Only the restore path breaks this, which is H2, not a live race. |
| 10 | Cancellation vs Candidate submission | **SAFE** | Commit order decides via Task-revision CAS; loser gets `stale-authority`/`candidate-submission-conflict`; Candidate-first remains immutable history. Stated identically in four documents. |
| 11 | Acceptance rejection while producing Run Finalizing | **SAFE** | RecoveryDecision may be recorded early; Task holds in Evaluating with `PriorRunFinalizing`; T9 requires prior Run terminal before Ready. Preserves the live-Run uniqueness index rather than deadlocking against it. |
| 12 | Blocking child yield with all global Run slots occupied | **SAFE** | Parent yield releases `resource://limit/global/runs` before Task→Waiting, so the child cannot be starved by its own parent — the deadlock finding #2 targeted. *Residual note:* if parent finalization is blocked by UNKNOWN, its slot stays UNCERTAIN and a single-slot install cannot run the child; this is the intended conservative choice, bounded by operator force-resolution, and blast-radius scoping is documented (`global-recovery…` §7). Worth an explicit worked example in `blocking-spawn-and-run-yield.md` §6. |
| 13 | Task retry / new Run while Task Workspace reservation exists | **SAFE** | Incremental claim set subtracts compatible Task-scoped reservations; positive delta or explicit resize only; persistence enforces one non-released Task reservation per `(task_id, resource_key)`. Directly closes finding #5. |
| 14 | Git GC after accepted `code.changeset`, before integration | **SAFE** | Changeset is CAS-complete; Integration materialises from Pantheon CAS; Git pins are optimisation only, and GC checks integration obligations before releasing them (`artifact-model.md` "Garbage collection"). |
| 15 | Git target-ref CAS race during integration | **SAFE** | IntegrationIntent persisted before mutation; ref update guarded by `expected_target_oid`; post-crash resolution by comparing expected/intended/actual; "neither Y nor Z" → STALE, never force-update. |
| 16 | Configuration activation during routing / T3 | **SAFE** | One `configRevision` captured for the whole cycle and rechecked immediately before T3; mismatch aborts and recomputes. Publication barrier prevents observing a DB-active revision under a stale in-process snapshot. |
| 17 | Policy tightening while Sandbox cannot dynamically tighten | **SAFE** | `configuration-and-policy-revisions.md` §13: tighten if physically possible, otherwise stop/finalize the Run; *"Pantheon must never claim a security policy is active for an execution whose physical sandbox cannot enforce it."* |
| 18 | Goal revision while Task Active / while Goal Evaluating | **SAFE** | Active Task → SUPERSEDE → `Finalizing/terminalTarget=Superseded` → terminal only after the responsible Run is safe. Goal Evaluating → completion candidate becomes stale history, evaluation stopped where safe, Goal returns to Active reconciliation. |
| 19 | Restore older SQLite backup, newer external execution / secret state | **NEEDS CLARIFICATION** | Executors, Git, worktrees, object stores and JournalEpoch are correctly handled (§27); SecretProvider drift fails closed (`secret-store…` §29). **Authorization and command state are not addressed** — see H2. Everything §27 names is safe; what it omits is not. |

**Totals: 17 SAFE, 2 NEEDS CLARIFICATION, 0 UNSAFE.**

Note that scenario 19's clarification is H2, and no listed scenario exercises C1 — C1 is a security bypass, not a crash/race, and appears in section F.

---

## F. Security retest

| Attack | Result | Basis |
|--------|--------|-------|
| Arbitrary Agent shell → control plane | **BLOCKED for ambient reach; BYPASSED via C1** | Mount/privilege/network exclusions are complete and `isolation.control-plane` is mandatory for model-driven shell. But the Agent does not need to reach out — it plants repository config and waits for the controller to execute Git with host authority. |
| Direct Git metadata manipulation | **BYPASSED (C1)** | Untrusted shell correctly cannot write the *authoritative* common-dir. It can fully write its *Task-local* `.git`, which Pantheon then executes against. The boundary was drawn around ref authority, not code execution. |
| Host sockets / credential agents | **BLOCKED** | `SSH_AUTH_SOCK`, GPG, cloud agents, platform keychain and container-runtime sockets excluded in `sandbox-broker…` "Credential isolation" + `permissions…` hard exclusions; credentialed operations are brokered. |
| Raw CAS enumeration / mutation | **BLOCKED** | Raw CAS never mounted; approved content is broker-materialised read-only; `artifact.seal` validates source path/resource and Pantheon computes all digests; Artifact refs are identifiers, not capabilities. |
| Agent Control impersonation / replay | **BLOCKED** | Attempt-scoped session, ≥256-bit credential, verifier-only persistence, all context server-derived from the session; `(attempt_id, request_id)` idempotency with same-ID/different-hash failing closed. |
| Stale Attempt requests | **BLOCKED** | Session revoked on Attempt terminalization/cancellation/tombstoning; T6 re-reads current Attempt/Run/Task/revision under CAS; `agent-control-channel.md` §18 fences post-yield semantic verbs. |
| Stale capability ticket / Grant | **BLOCKED live; WEAK across restore (H2)** | Tickets are short-lived, single-use, revalidated at redemption and never bearer authority; use-count CAS is transactional. Restore rewinds the ledger — H2. |
| Malicious backend usage claims | **PARTIALLY BLOCKED (H1)** | Attempt usage is bound to the immutable Binding's named backend. Control-operation usage has no ownership rule and can be forged against another Goal's EvaluationOperation. |
| Malicious evaluator definition / result | **BLOCKED** | Trusted registry only; no Task-embedded commands; no `/bin/sh -c`; executable+argv with controller-owned environment; immutable pinned versions; evaluators return Evidence only and cannot transition Task phase, choose recovery, or alter budget/permissions; `ERROR` never becomes `PASS`. |
| Malicious Artifact / reference-data prompt injection | **BLOCKED at the semantic layer** | `context-builder.md` trust strata make reference content data even when it reads as instructions, lower strata cannot override higher, and `agent-genome.md` keeps unvalidated reflections out of Run context. Authority is never derived from context. |
| Configuration relaxation / tightening races | **BLOCKED** | Frozen ceiling ∩ current policy: relaxation never broadens a live Run; tightening applies immediately and forces Run termination where physical enforcement is impossible; one captured revision per operation with pre-commit recheck. |

The authorization and containment model is sound in design. C1 is a gap in *threat coverage* — the model never asks what Pantheon itself executes against attacker-controlled bytes — rather than a flaw in the model's logic.

---

## G. Persistence / schema retest

| Required capability | Status | Note |
|---|---|---|
| one live Run per Task | **Expressible** | Partial unique index on Runs (`:213`) + controller checks. |
| Ready/Waiting ⇒ zero live Run | **Prose only** | Expressible as a `tasks` `CHECK` on `phase`/`active_run_id`; currently controller + checker only — M4. |
| Finalizing Run ⇒ terminalTarget | **Prose only** | `run_status.terminal_target` exists; add `CHECK` — M4. |
| Completed Run ⇒ Candidate | **Prose only** | `run_status.candidate_digest` exists; add `CHECK` — M4. |
| one nonterminal Attempt per Run | **Mechanism unstated** | Discriminator split across `attempts`/`attempt_status` — M5. |
| one AgentControlSession per Attempt | **Enforced** | `agent_control_sessions.attempt_id UNIQUE`. |
| one live singular Task reservation per resource key | **Enforced** | Partial uniqueness on `(task_id, resource_key)`, stated in three documents. |
| Attempt launch contact state | **Enforced** | `attempt_status.launch_contact_*` + T4b. |
| backend+Attempt usage provenance uniqueness | **Enforced for Attempts; missing for control operations** | H1. |
| Grant use-count + broker operation CAS | **Enforced** | T10 single transaction; restore interaction is H2. |
| external lineage tombstones | **Enforced** | `external_lineage_tombstones` with expected revision, actor, reason, evidence. |
| control-operation resource holder | **Enforced** | Explicit holder scopes on `resource_reservations`. |
| evaluation tables | **Present but incomplete** | Full family exists; missing launch-contact columns (H3) and using stale `policy_revision` (M2). |
| Sandbox identity/status | **Present but incomplete** | `sandbox_instances`/`_status`/`_verifications` exist; no holder scope for control-operation ownership (H4). |
| configuration revisions/components | **Enforced** | Full family + singleton `active_configuration` pointer + immutability rule. |
| secret metadata without secret bytes | **Enforced** | Metadata-only families; explicit prohibition including hashes of secret bytes. |

The persistence document is the strongest in the corpus — STRICT tables, integral base units, digest-as-BLOB with length constraints, `BEGIN IMMEDIATE` discipline, no business-logic triggers, an explicit invariant checker, and fourteen named transaction families. The gaps above are omissions at the edges, not structural weaknesses.

---

## H. V1 simplification audit — deferred features leaking into mandatory v1

| Deferred feature | Leaks into v1? | Detail |
|---|---|---|
| Model semantic Agent ranker | **No** | Deferred in `logical-agent-resolution.md` §Ranking + inv. 5; deterministic precedence only. |
| PROGRESSIVE planning | **No** | `planner-and-task-decomposition.md` "V1 planning modes" + inv. 3; runtime discovery routed to bounded blocking spawn. |
| joined / detached spawn | **No** | Reserved-but-rejected in `blocking-spawn…` §20, `task-spawn…` inv. 9, `planner…:127`. §20 correctly requires policy/schema to reject the names. |
| `after_terminal`, quorum/conditional gates | **No** | `taskgraph-dependencies.md` inv. 6; v1 is `requires_success` only. |
| Model-based authoritative evaluation | **No** | Deferred in five places across `evaluation…` and `task-acceptance…`; `assertion`/`policy` compile down to `check`/`schema`. |
| Distributed scheduler / fleet | **No** | Single-daemon installation lock; multi-daemon explicitly out of v1. |
| **Automatic Genome promotion** | **YES — schema only** | `agent-genome.md` and `agent-manifest.md` defer it in prose, but `schemas/agent-v1alpha1.schema.json:190–205` still *accepts* `reflection: after-task` and `promotion.*: automatic`. The v1 schema admits configuration v1 cannot honour. M3. |
| Adaptive memory retrieval | **Ambiguous** | `mode: adaptive` appears in schema and manifest example without a definition reconciling it to `context-builder.md` inv. 12. L2. |
| CredentialLease (dynamic secrets) | **No** | Lifecycle defined now *because it creates recovery obligations* — correct forward-compatibility, not a leak; v1 may ship static providers only. |
| Weighted/quorum acceptance | **No** | `required`/`advisory` only; explicitly no weighting in v1. |

One genuine leak (M3), one ambiguity (L2). Otherwise the deferral discipline is clean and consistently repeated at both the prose and invariant level.

---

## I. Exact patch list

Files that still need changes:

1. **`docs/architecture/workspace-and-git-integration.md`** — add "Hostile repository state" section; add invariants (C1). Fix the sealing-flow description so tree capture is confined.
2. **`docs/architecture/sandbox-broker-and-isolation.md`** — add the controller-side-execution boundary invariant and `workspace.hostile-repository-state` failure code (C1); add control-operation holder branch to "Ownership and lifetime" (H4).
3. **`docs/architecture/global-recovery-and-crash-reconciliation.md`** — add authorization/command fencing to §27 Restore and inv. 22 (H2); confine Git worktree inventory/repair (C1); add verification-Sandbox reconciliation to §12/§31 (H4).
4. **`docs/architecture/budget-usage-and-rate-limits.md`** — add control-operation ownership validation clause and amend inv. 4 (H1).
5. **`docs/architecture/sqlite-persistence-and-transactions.md`** — control-operation usage ownership (H1); evaluation launch-contact columns + `T15` (H3); `sandbox_instances` holder scope (H4); three `CHECK` constraints (M4); state the one-nonterminal-Attempt mechanism (M5).
6. **`docs/architecture/evaluation-and-evaluator-registry.md`** — `policyRevision`→`configRevision` + `evaluatorRegistryDigest` at `:198` and `:534` (M2); reference the new persisted contact marker in §22 (H3).
7. **`docs/architecture/event-and-observability-model.md`** — replace `policyHash` at `:339` (M1).
8. **`docs/architecture/permissions-and-capabilities.md`** — amend inv. 5 for restore-time Grant fencing (H2).
9. **`docs/architecture/agent-manifest.md`** — make v1 learning-disabled normative rather than "should" (M3); extend inv. 4 for C1.
10. **`docs/architecture/overview.md`** — add the controller-side-execution boundary to the security-boundary summary (C1).
11. **`schemas/agent-v1alpha1.schema.json`** — restrict `learning.*` to `disabled` for v1, or document compile-time rejection (M3); define or rename `retrieval.mode: adaptive` (L2).
12. **`docs/architecture/task-lifecycle.md`** — fix the `goal-revision-and-reconciliation.md` cross-reference (L1).
13. **All `## Status` lines** — normalise Draft/Canonical/Accepted vocabulary (L3).

---

## J. V1 feasibility

**Yes.** One strong Rust engineer working with coding agents can implement a coherent v1 from these documents without inventing major semantics, once C1 is patched.

The decisive evidence is that the hard questions are already answered in a form an implementer can execute against: fourteen named transaction families with stated preconditions; explicit commit ordering at every external-effect boundary; a durable-intent → effect → reconcile pattern applied uniformly with a per-domain idempotency identity table; three-valued external certainty (`CONFIRMED|NOT_APPLIED|UNKNOWN`) with UNKNOWN never authorizing replacement work; and a deterministic invariant checker specified as a component rather than left as an aspiration.

Sorting what remains:

**Genuine missing semantic decisions** (must be settled before implementation issues): C1's confinement rule for controller-side Git; H1's control-operation usage ownership; H2's restore-time authorization fencing. H3 and H4 are near-mechanical extensions of decisions already made for Attempts and Reservations.

**Rust / module / API implementation design** (not architecture — do not request more architecture for these): the `ExecutorBackend` and `SandboxBackend` trait surfaces (`execution-fabric.md` explicitly defers these, correctly); concrete DDL and index definitions; the writer-serialization and read-pool topology; the Cedar schema and the compiler that produces `AuthorizationComponent`; controller task/actor structure and wakeup plumbing; the Agent Control HTTP/tool bridge; the OpenAPI document; CAS layout and GC traversal.

**Tests / fault injection**: the eleven crash boundaries in `global-recovery…` §29 plus the property assertions there are already a usable v1 test plan; add the two new boundaries implied by H3.

**Post-v1**: everything in section H's deferred column, plus remote Agent Control, multi-daemon, and monetary tariff conversion.

The main implementation risk is not ambiguity but volume — the SQLite layer carries most of the safety burden, and the invariant checker should be built early rather than last, since it is the mechanism by which every prose invariant becomes testable.

**Final verdict: READY AFTER SMALL ARCHITECTURE PATCHES.**
