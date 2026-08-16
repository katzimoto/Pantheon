# Run and Attempt

## Status

Draft design — Pantheon execution lifecycle specification.

## Purpose

This document defines Pantheon's execution hierarchy below Task: what a Run represents, what an Attempt represents, and the exact boundaries between reconciliation, execution retry, rerouting, semantic retry, and replanning.

The central hierarchy is:

```text
TASK
bounded semantic outcome
   │
   ▼
RUN
one immutable resolved execution strategy
   │
   ├── Attempt 1
   ├── Attempt 2
   └── ...
        │
        ▼
backend execution lineage
```

The core rule is:

> **A Run owns one immutable ExecutionBinding. An Attempt owns one logical backend-execution lineage identified by one immutable LaunchKey.**

This distinction gives Pantheon four different recovery levels instead of one generic retry mechanism:

```text
RECONCILE
same Attempt / same LaunchKey / same execution lineage

RETRY EXECUTION
new Attempt / same Run / same ExecutionBinding

REROUTE OR SEMANTIC RETRY
new Run / new ExecutionBinding

REPLAN
new or superseding Tasks / TaskGraph mutation
```

See also:

- `docs/architecture/task-object.md`
- `docs/architecture/task-lifecycle.md`
- `docs/architecture/task-acceptance-and-completion.md`
- `docs/architecture/execution-fabric.md`
- `docs/architecture/execution-offer-routing-and-admission-handshake.md`
- `docs/architecture/scheduler-reservations-ownership-and-leases.md`
- `docs/architecture/scheduler-dispatch-and-run-intent-reconciliation.md`

## 1. Task, Run, and Attempt are different contracts

### Task

A Task answers:

> What bounded outcome must eventually be produced and accepted?

The Task is semantic and executor-independent.

### Run

A Run answers:

> What exact resolved execution strategy has Pantheon committed to for pursuing this Task now?

A Run owns exactly one immutable `ExecutionBinding` and the resolved configuration/snapshots required to audit that decision.

### Attempt

An Attempt answers:

> What concrete backend execution lineage are we trying under this Run's exact strategy?

An Attempt owns one immutable `LaunchKey` and execution-specific operational evidence.

## 2. Run identity is the strategy boundary

A Run has one immutable ExecutionBinding.

Conceptually:

```text
Run run_17
  ↓
ExecutionBinding binding_A
  ↓
logical Agent + execution request + selected offer + backend + policy snapshots
```

The Binding cannot be replaced in place.

If any material binding-level decision changes, Pantheon creates a new Run.

Examples that require a new Run include:

- changing the selected logical Agent;
- changing the selected backend/execution offer;
- changing material execution features/configuration;
- changing the ExecutionRequest because new semantic feedback is available;
- rerouting after an execution strategy is judged unsuitable;
- retrying after Acceptance rejection using the rejection evidence as new execution context.

The fact that two Runs eventually resolve to the same Agent/backend does not make them the same Run if their immutable requests/bindings differ.

## 3. Attempt identity is execution continuity

An Attempt is not defined as one OS process.

It represents one logical backend-execution lineage identified by one LaunchKey.

The following may all happen while remaining the **same Attempt**:

- Pantheon daemon restart;
- backend adapter/plugin restart;
- network transport disconnect/reconnect;
- PTY reconnect;
- event-stream loss/recovery;
- backend status retry;
- native session reattachment;
- recovery using persisted backend-private attachment state.

As long as the backend can preserve or recover continuity with the same LaunchKey, Pantheon is reconciling the same Attempt.

## 4. LaunchKey belongs to Attempt

Every Attempt gets one immutable LaunchKey before any backend execution side effect.

```text
Run run_17
│
├─ Attempt attempt_1
│    LaunchKey launch_A
│
└─ Attempt attempt_2
     LaunchKey launch_B
```

The backend must provide idempotent ensure/recovery semantics for a LaunchKey:

```text
ensureExecution(binding_A, launch_A)
  first call  → create/attach execution E
  retry call  → return/attach execution E
```

A retry of the same Attempt must never intentionally create an independent execution F.

If native infrastructure lacks a suitable idempotency primitive, the adapter must maintain backend-private durable state where possible to provide equivalent behavior.

## 5. When a new Attempt is created

A new Attempt is created only when all of the following are true:

1. the prior Attempt's execution continuity is conclusively ended or it definitively never established execution;
2. Pantheon intentionally decides to retry the **same immutable ExecutionBinding**;
3. policy authorizes a fresh execution incarnation.

The new Attempt receives:

- a new Attempt ID;
- the next Run-local ordinal;
- a new immutable LaunchKey.

Example:

```text
Run 17 / Binding A

Attempt 1 / launch_A
  ↓
execution definitively exits
  ↓
retry policy chooses SAME binding
  ↓
Attempt 2 / launch_B
```

## 6. UNKNOWN never creates a new Attempt

Pantheon must distinguish definite termination/absence from uncertainty.

```text
EXITED / definitively absent
= execution continuity is known to be over or never established

UNKNOWN
= Pantheon cannot establish whether execution still exists
```

While an Attempt is `UNKNOWN`:

- the Attempt remains nonterminal;
- the Run remains Task owner;
- relevant ResourceReservations remain charged/UNCERTAIN;
- Pantheon retries inspection/reattachment/recovery;
- Pantheon must not create another Attempt merely to make progress.

This prevents duplicate execution when an external session continues after communication loss.

## 7. Reconciliation is not retry

Pantheon uses the word **reconciliation** for actions that attempt to understand or restore control of the same execution lineage.

Examples:

```text
same Run
same Binding
same Attempt
same LaunchKey

transport retry
status retry
adapter restart
reattach
recover backend attachment
```

None of those consume an execution retry by definition.

Retry accounting is handled by the Failure/Retry policy subsystem, not by counting reconciliation loops.

## 8. New Run versus new Attempt

The decision boundary is deliberately mechanical:

```text
Same ExecutionBinding?
  │
  ├─ yes + fresh execution needed → new Attempt
  │
  └─ no → new Run
```

Examples:

### Same Run, new Attempt

- backend process/session crashes definitively;
- retry policy says the same selected execution strategy should be tried again;
- no semantic input or Binding field changes.

### New Run

- select another backend offer;
- select another logical Agent;
- alter execution requirements;
- use Acceptance rejection feedback;
- use new project/task evidence that changes the ExecutionRequest;
- escalation policy selects a different execution strategy.

## 9. Acceptance rejection normally creates a new Run

A candidate result is immutable once submitted for evaluation.

If Acceptance fails, the next worker should normally receive new information:

```text
previous candidate
+
Acceptance evidence
+
structured rejection feedback
```

That changes the semantic execution context and therefore creates a new ExecutionRequest/Binding.

So the normal flow is:

```text
Run 1
  ↓
Candidate 1
  ↓
Acceptance FAIL
  ↓
Task returns Ready
  ↓
Run 2 with rejection feedback
```

not:

```text
Run 1
  └─ Attempt 2 with silently changed semantic context
```

Attempts are primarily execution-layer retries; Runs are strategy/semantic-execution retries.

## 10. One candidate per Run

A Run may durably submit at most one candidate result.

The candidate records at least:

- producing Task;
- producing Run;
- producing Attempt;
- output/artifact references;
- candidate digest;
- submission timestamp/provenance.

Pantheon must not mutate a candidate in place during Acceptance.

If more work is required after rejection, a later Run produces another candidate.

This gives clean provenance for:

- Acceptance;
- routing metrics;
- executor reliability;
- Agent Genome learning;
- retry analysis;
- audit/history.

## 11. Candidate submission changes responsibility

When `task.submit_result` (or equivalent) passes structural validation and the candidate is durably recorded:

```text
Task Active → Evaluating
Run Active → Finalizing
Run desired execution → stopped
```

The Run Controller then reconciles the current Attempt toward termination and releases eligible Run-scoped execution capacity when safe.

Task-scoped resources such as a worktree may remain available to Acceptance/review/finalization.

## 12. Run lifecycle

Run lifecycle should remain intentionally small.

Recommended v1 phases:

```text
Active
  ↓
Finalizing
  ↓
Completed
Failed
Cancelled
```

### Active

The Run owns responsibility for progressing its Task under one immutable Binding.

The Run may be:

- still preparing;
- LaunchReady but with no Attempt yet;
- reconciling an Attempt;
- executing;
- between sequential Attempts while retry policy decides.

These distinctions belong in conditions/status, not separate phases.

### Finalizing

The Run has stopped producing new semantic work and is sealing its outcome, stopping execution, settling usage/budget state, and releasing eligible Run-scoped resources.

### Completed

The Run successfully produced and durably submitted its one candidate and completed execution responsibility.

**Run Completed does not mean Task Succeeded.**

The candidate still requires Task Acceptance.

### Failed

The Run concluded without a usable candidate and policy decided that no further Attempt under this Binding should occur.

The Task may later return to Ready for a new Run, depending on Retry/Escalation policy.

### Cancelled

The Run was intentionally terminated because its desired work was cancelled, superseded by enclosing state, or no longer authorized.

Run cancellation does not imply Task cancellation; the enclosing Task/Goal controller determines its own lifecycle.

## 13. Run conditions instead of phase explosion

Do not create Run phases for operational preparation detail.

Use controller-owned conditions such as:

```text
WorkspaceReady
SandboxReady
ContextReady
PolicyReady
LaunchReady
CandidateSubmitted
ExecutionStopped
ResourcesReleased
BudgetSettled
```

This keeps high-level lifecycle stable while allowing detailed observability.

## 14. Attempt lifecycle is observation-oriented

Attempt does not need a large semantic phase machine.

An Attempt records:

```text
nonterminal / terminal
+
normalized observed execution state
+
backend attachment
+
timing
+
usage
+
termination/failure evidence
```

Normalized execution observations are:

```text
ABSENT
STARTING
RUNNING
EXITED
UNKNOWN
```

These are observations, not a required linear sequence. Pantheon may first observe an Attempt only after it has already exited.

The Failure/Retry subsystem can later derive categories such as infrastructure failure, process failure, clean termination without candidate, cancellation, and retryability from the immutable evidence.

## 15. Attempts are sequential in v1

A Run may have multiple Attempts over time, but at most one may be nonterminal.

```text
Run
├─ Attempt 1 terminal
├─ Attempt 2 terminal
└─ Attempt 3 current
```

Pantheon v1 does not perform speculative duplicate Attempts, hedged execution, or race multiple execution incarnations under one Binding.

This avoids major complexity in:

- workspace mutation;
- side effects;
- token/cost accounting;
- authorization;
- ResourceReservations;
- candidate authority.

## 16. Attempt is created after preparation and before backend execution

A Run can exist before any Attempt.

Correct flow:

```text
Run committed
  ↓
prepare workspace/sandbox/context/policy
  ↓
LaunchReady=True
  ↓
BEGIN
create Attempt
assign ordinal
assign LaunchKey
set Run.currentAttemptId
COMMIT
  ↓
ensureExecution(...)
```

If preparation itself fails, the Run may fail without any backend Attempt having occurred.

If Pantheon crashes after Attempt commit but before backend execution is contacted, restart resumes the same Attempt/LaunchKey safely.

## 17. Backend attachment belongs to Attempt

The backend may need opaque, versioned state to reattach to the concrete execution lineage.

Conceptually:

```yaml
backendAttachment:
  attempt: attempt_1
  backend: executor://A
  schemaVersion: 3
  opaqueState: ...
```

Pantheon persists but does not interpret this payload.

Provider session IDs, process identifiers, PTY internals, runtime handles, and similar implementation details remain adapter-private.

## 18. Workspace normally survives Attempts

An Attempt does not receive a fresh Task workspace by default.

The normal ownership hierarchy is:

```text
Task
  └─ Task-scoped workspace reservation
       ↓
Run
  ├─ Attempt 1
  └─ Attempt 2
```

This allows useful filesystem/code changes produced before an execution crash to remain available to a later Attempt under the same strategy.

Each Attempt should eventually record the workspace revision/checkpoint it began from for provenance, but exact checkpoint semantics belong to the Workspace/Git subsystem.

## 19. Usage is measured at Attempt and aggregated upward

Actual execution usage originates at Attempt level because concrete backend execution happens there.

Conceptually:

```text
Attempt usage
   ↓ aggregate
Run usage
   ↓ aggregate
Task usage
   ↓ aggregate
Goal usage
```

Example:

```text
Run 1
  Attempt 1 → 31k normalized tokens
  Attempt 2 → 47k normalized tokens

Run aggregate → 78k
```

Failed Attempts still consume budget. Actual spend is never refunded merely because the Attempt or Run failed.

Detailed usage/budget semantics are defined by the Budget & Usage subsystem.

## 20. Run Manifest becomes the immutable Run specification

Pantheon previously described a separate immutable `RunManifest` containing hashes of Agent/Genome/policy/workspace/executor configuration.

That data should not be maintained as an unrelated duplicate object.

Instead, the canonical Run has an immutable specification/snapshot portion that contains or references the reproducibility data required for the Run.

Conceptually:

```yaml
run:
  id: run_123
  task: task_456

  spec:
    binding:
      ref: binding_789
      hash: sha256:...

    snapshots:
      task: sha256:...
      agent: sha256:...
      soul: sha256:...
      behavior: sha256:...
      skills: ...
      memory: ...
      policy: sha256:...

    workspace:
      ref: workspace://...

  status:
    phase: Active
    currentAttempt: attempt_1
```

Exact schema is deferred until persistence/API design, but the architectural principle is fixed:

> **Run is the immutable audit unit for one resolved execution strategy.**

## 21. Failure evidence is immutable; policy is separate

Run/Attempt controllers record facts, not retry policy conclusions.

Example Attempt evidence:

```yaml
failure:
  origin: execution
  category: process-exit
  evidence:
    exitCode: 137
    observedAt: ...
```

The later Failure/Retry subsystem decides whether this means:

- reconcile same Attempt;
- create another Attempt;
- conclude Run Failed;
- create another Run;
- replan;
- ask the user;
- fail the Task.

Do not hard-code `Attempt failed => decrement retry counter` into Attempt lifecycle.

## 22. Cancellation and Goal revision

If Goal reconciliation or user cancellation invalidates an active Run:

```text
Run desired execution → stopped
Run → Finalizing / Cancelled target
```

The current Attempt is reconciled toward termination.

UNKNOWN termination remains fail-closed and retains resources until safe reconciliation.

A terminal Run is never reopened.

If later work is required, Pantheon creates another Run or another Task as appropriate.

## 23. Core invariants

Pantheon v1 should enforce:

```text
Task Active
⇒ exactly one nonterminal Run owns Task
```

```text
Run
⇒ exactly one immutable ExecutionBinding
```

```text
Run
⇒ at most one submitted candidate
```

```text
Run
⇒ at most one nonterminal Attempt
```

```text
Attempt
⇒ exactly one immutable LaunchKey
```

```text
UNKNOWN Attempt execution
⇒ no replacement Attempt
```

```text
Binding changes
⇒ new Run
```

```text
fresh execution under same Binding
⇒ new Attempt
```

## v1 scope

Include:

- Task → Run → Attempt hierarchy;
- immutable ExecutionBinding per Run;
- immutable LaunchKey per Attempt;
- reconciliation vs retry distinction;
- sequential Attempts only;
- one candidate per Run;
- Run lifecycle `Active`, `Finalizing`, `Completed`, `Failed`, `Cancelled`;
- conditions for operational detail;
- Attempt-level normalized execution observations and evidence;
- Attempt-level usage aggregation upward;
- Task/Run-shared workspace semantics;
- immutable Run audit/spec snapshot.

Defer:

- speculative concurrent Attempts;
- speculative duplicate Runs;
- migration of a live Attempt between backends;
- arbitrary in-place Binding mutation;
- detailed failure/retry policy;
- detailed Budget/Usage schema;
- detailed workspace checkpoint schema.

## Key decisions

1. **Task → Run → Attempt is Pantheon's canonical execution hierarchy.**
2. **A Run owns exactly one immutable ExecutionBinding.**
3. **An Attempt owns exactly one immutable LaunchKey.**
4. **Attempt represents logical backend-execution continuity, not one OS process.**
5. **Reconnect, adapter restart, daemon restart, and reattachment stay within the same Attempt when continuity is preserved.**
6. **A definitively ended execution followed by a fresh execution under the same Binding creates a new Attempt.**
7. **UNKNOWN execution never permits a replacement Attempt.**
8. **Any material Binding change creates a new Run.**
9. **Acceptance rejection normally produces a new Run because semantic execution context changes.**
10. **A Run submits at most one immutable candidate.**
11. **Run Completed means candidate/execution responsibility completed; it does not mean Task Acceptance passed.**
12. **Attempts are sequential in v1; at most one nonterminal Attempt exists per Run.**
13. **Attempt is persisted with LaunchKey before any backend execution side effect.**
14. **Retry classification/counting belongs to a separate Failure/Retry subsystem.**
15. **Actual usage originates at Attempt level and aggregates upward.**
16. **Task workspace normally survives across Attempts and Runs as controlled by Task-scoped ownership.**
17. **The former RunManifest concept becomes the immutable spec/snapshot portion of the canonical Run resource.**
18. **Raw failure and termination evidence is preserved immutably for policy, observability, metrics, and learning.**
