# Evaluation and Evaluator Registry

## Status

Draft design — Pantheon acceptance verification subsystem specification.

## Purpose

Pantheon must execute acceptance checks without creating an unaccounted second execution plane. Evaluation consumes real resources, may execute untrusted project code, must be reproducible against the exact Candidate being judged, and must never allow an evaluator to become lifecycle or authorization authority.

The central rule is:

> **Evaluation is control-plane verification. Deterministic evaluators execute as explicitly accounted EvaluationOperations; they are not Tasks, Runs, Attempts, or Logical Agents.**

Model-based rubric/reviewer evaluation is deferred from v1.

See also:

- `docs/architecture/task-acceptance-and-completion.md`
- `docs/architecture/artifact-model.md`
- `docs/architecture/workspace-and-git-integration.md`
- `docs/architecture/scheduler-resource-ledger-and-admission.md`
- `docs/architecture/budget-usage-and-rate-limits.md`
- `docs/architecture/permissions-and-capabilities.md`
- `docs/architecture/event-and-observability-model.md`

## 1. Three distinct resources

Pantheon distinguishes:

```text
Evaluator
  trusted logical verification mechanism

EvaluatorVersion
  one immutable concrete definition of that mechanism

EvaluationRound
  one immutable judgment context for one Candidate

EvaluationOperation
  one concrete control-plane execution of one criterion
```

Conceptually:

```text
CandidateResult
      ↓
EvaluationRound
      │
      ├── EvaluationOperation A
      ├── EvaluationOperation B
      └── HumanEvaluation C
                ↓
             Evidence
                ↓
      Acceptance aggregation
```

## 2. Trusted Evaluator Registry

Tasks may reference registered evaluator names, but never embed arbitrary executable hooks.

Allowed:

```yaml
evaluator:
  ref: check://project/unit-tests
  version: sha256:...
```

Forbidden:

```yaml
acceptance:
  command: "whatever the planner generated"
```

Evaluator definitions are governed by trusted system/user/project configuration. Planners, workers, Tasks, models, and executor backends may select only from evaluator references permitted by the Task/Goal contract; they may not define executable evaluator code dynamically.

## 3. Logical ref versus immutable version

A logical evaluator reference such as:

```text
check://project/unit-tests
```

may advance from one definition to another over time.

Every concrete `EvaluatorVersion` is immutable and content-addressed. A material change to command, arguments, result protocol, sandbox profile, timeout, resource requirements, or other execution semantics creates a new version digest.

Historical versions are never rewritten.

## 4. Task acceptance pins evaluator versions

At Task materialization, evaluator logical refs are resolved against the active trusted registry and exact immutable versions are pinned into the Task's immutable acceptance contract.

Therefore editing evaluator configuration while a Task is running does not silently alter that Task's acceptance semantics.

If a pinned version later becomes forbidden or unavailable under current hard policy, Pantheon does not silently substitute a newer version. The Task becomes blocked/reconciliation-required and normal Goal/Task reconciliation decides the next action.

## 5. V1 evaluator kinds

V1 supports only:

```text
check
schema
human
```

### `check`

A deterministic executable validation, for example tests, linters, static analyzers, scanners, or project-specific verifiers.

### `schema`

Trusted in-process structural validation such as JSON Schema validation, Candidate output shape checks, or Artifact structural requirements.

### `human`

Explicit human judgment bound to the exact Candidate and criterion.

`assertion` and `policy` are represented as deterministic `check` or `schema` mechanisms where needed.

Model-assisted `rubric` and `review` evaluators are deferred from v1 because they introduce another model-routing, metering, prompt/version, and reviewer-selection subsystem.

## 6. Evaluator definition

Conceptually:

```yaml
kind: Evaluator
metadata:
  ref: check://project/unit-tests

spec:
  kind: check

  command:
    executable: cargo
    args:
      - test
      - --workspace

  timeout: 10m

  sandbox:
    profile: verification-default

  resources:
    cpu: ...
    memory: ...

  resultProtocol:
    kind: exit-code
    pass:
      - 0
```

The definition is canonicalized and hashed into an immutable EvaluatorVersion.

## 7. No dynamically generated shell strings

Executable evaluators use an executable plus argv vector and an explicitly controlled environment.

Pantheon does not implicitly execute evaluator definitions through `/bin/sh -c`.

Dynamic substitutions, where supported, are typed and validated. Model-authored free-form shell fragments are never interpolated into trusted evaluator commands.

## 8. EvaluationRound

When a Candidate becomes authoritative for evaluation, the Acceptance Controller creates one immutable EvaluationRound that freezes the exact judgment context.

Conceptually:

```yaml
evaluationRound:
  id: evalround_123

  task: task_42
  candidate: candidate://sha256/C

  acceptanceContract:
    hash: sha256:A

  evaluators:
    - criterion: tests
      ref: check://project/unit-tests
      version: sha256:E1

    - criterion: output-schema
      ref: schema://project/result
      version: sha256:E2

  policyRevision: ...
  createdAt: ...
```

The Round identity binds Candidate digest, acceptance contract, exact evaluator versions, and the relevant policy/configuration snapshot.

## 9. Candidate is the authoritative subject

Acceptance normally evaluates the immutable CandidateResult digest. Individual criteria may additionally bind constituent Artifact digests.

Evaluation never treats a mutable filesystem path or producing Agent session as acceptance authority.

## 10. EvaluationOperation

A deterministic external check executes as an `EvaluationOperation`, owned by the Acceptance/Evaluation Controller.

Conceptually:

```yaml
evaluationOperation:
  id: evalop_91
  round: evalround_123
  criterion: tests

  candidate: candidate://sha256/C

  evaluator:
    ref: check://project/unit-tests
    version: sha256:E1

  state: PENDING

  execution:
    sandboxProfile: verification-default
    timeout: 10m
```

An EvaluationOperation is deliberately not a Task, Run, Attempt, or Logical Agent.

Where an EvaluationOperation can incur backend-authored billable usage, its durable operation intent additionally freezes a metering-source binding before external contact. The binding is provenance only: it names which backend/metering contract may report factual Usage for this operation and does not make that backend the EvaluationOperation lifecycle owner or convert the operation into normal Agent execution.

## 11. Generic control-operation holder scope

Evaluation consumes real finite resources. ResourceReservation therefore supports a generic third holder scope:

```text
Run
Task
control-operation
```

For evaluation:

```yaml
holder:
  kind: control-operation
  ref: evaluation-operation://evalop_91
```

This preserves the one generic Resource Ledger without stretching Run semantics to non-agent work.

## 12. Resource admission

External EvaluationOperations declare generic resource requirements and pass through the same Resource Ledger/admission authority used by scheduled execution.

Typical claims include:

- verification workspace/materialization capacity;
- sandbox capacity;
- process slots;
- CPU;
- memory;
- other bounded local resources.

Evaluation may never create hidden verification workspaces or containers outside Resource Ledger accounting.

## 13. Evaluation queue is not the Task Scheduler

The Acceptance Controller may maintain a small deterministic queue of runnable EvaluationOperations and ask generic resource admission whether each operation can start.

Evaluation does not require Goal fairness, Logical Agent Resolution, ExecutionOffers, or normal Task scheduling policy.

V1 should prefer a simple oldest-runnable ordering with bounded configured concurrency rather than inventing a second sophisticated scheduler.

## 14. Budget accounting

Pure local deterministic checks normally have no token or monetary BudgetHold, but still consume ResourceReservations.

If an EvaluationOperation has a billable external cost in the future, it uses the existing `control-operation` BudgetHold holder scope and normal Usage/Charge accounting.

Any such backend-authored metering requires an immutable operation-level metering-source binding frozen by Pantheon before the operation can contact that external metering source. Conceptually it binds at least:

```text
control operation identity
reporting backend identity
backend descriptor/revision
metering contract digest/version
```

The reporting backend cannot choose or rewrite this binding. An EvaluationOperation without a frozen external metering-source binding cannot accept backend-authored UsageRecords. Valid delayed usage remains ingestible after operation terminalization when immutable provenance validates it; current operation phase is not used as an ownership check.

Human evaluation does not require a BudgetHold.

Model-based evaluation is deferred together with its model-routing and metering requirements.

## 15. Evaluation admission transaction

Before an external EvaluationOperation starts, Pantheon atomically verifies current authority and commits durable execution intent.

Conceptually:

```text
BEGIN IMMEDIATE

verify Task is Evaluating
verify Candidate is current
verify EvaluationRound is current
verify evaluator version exists
verify current hard policy permits execution
verify criterion is still applicable

assess and reserve generic resources
create EvaluationOperation execution intent
freeze metering-source binding if backend-authored billable usage applies
create BudgetHold if applicable
append Events

COMMIT
```

External process/container execution happens only after this durable transaction.

## 16. Independent verification environment

Authoritative evaluation runs against an independently materialized representation of the immutable Candidate.

It never executes against the producing Agent's mutable Task workspace.

The producing worker's own test output is useful development evidence, but does not automatically become authoritative Task Acceptance Evidence.

## 17. Immutable input, disposable scratch

Verification environments may need writable build/test scratch space, but the Candidate source identity remains immutable.

Conceptually:

```text
Candidate source material
  immutable logical input

Evaluator scratch/build output
  writable + disposable

Evaluator result artifacts
  sealed explicitly
```

Evaluator-created scratch may never mutate the authoritative Candidate or its content-addressed identity.

## 18. Verification sandbox

The default verification sandbox is least-privilege:

```text
non-root where supported
no operator control socket
no Agent Control identity
no Pantheon DB
no direct CAS access
no peer Task workspaces
no Git shared-ref authority
no secret mounts
no host/container runtime socket
network disabled by default
bounded CPU/memory/time
```

Network or credentials require an explicit evaluator definition plus current policy authorization; they are never inherited from the producing Run.

A verification SandboxInstance is durably owned by the **EvaluationOperation** through the `control-operation` holder scope. It is not owned by an EvaluationAttempt: the Sandbox must be provisioned and verified before an externally executing EvaluationAttempt is created/launched, and bounded sequential EvaluationAttempts may reuse the same verification Sandbox while its SandboxKey, immutable environment/materialization, verification result, resource ownership and current hard-policy constraints remain valid.

V1 permits at most one current/non-RELEASED verification SandboxInstance per EvaluationOperation. An `UNKNOWN` or otherwise non-released Sandbox cannot be bypassed by provisioning an overlapping replacement; the original SandboxKey is reconciled first or the lineage is explicitly force-resolved under recovery policy.

The Sandbox Broker specification defines the concrete platform mechanisms used to enforce these properties and the relational Run/control-operation holder contract.

## 19. Result protocol

EvaluatorVersion defines how execution output becomes an evaluator verdict.

For example:

```yaml
resultProtocol:
  kind: exit-code
  pass:
    - 0
```

Pantheon distinguishes:

```text
PASS
  evaluator completed and criterion is satisfied

FAIL
  evaluator completed authoritatively and criterion is not satisfied

ERROR
  evaluator could not establish an authoritative verdict

PENDING
  evaluation is incomplete
```

A test failure and evaluator infrastructure failure are different facts. `ERROR` is never converted into `PASS`.

## 20. Evidence

Evaluation produces immutable Evidence bound to the exact Candidate, criterion, EvaluatorVersion, and evaluation provenance.

Conceptually:

```yaml
evidence:
  candidate: candidate://sha256/C
  criterion: tests

  evaluator:
    ref: check://project/unit-tests
    version: sha256:E1

  evaluationOperation: evalop_91

  verdict: FAIL

  details:
    testsPassed: 73
    testsFailed: 2

  artifacts:
    log: artifact://sha256/...
```

Large logs, reports, coverage files, or scanner output become Artifacts referenced by Evidence rather than large inline database/event payloads.

## 21. Evaluation execution retries

One criterion may require more than one low-level evaluation execution attempt when infrastructure fails.

Conceptually:

```text
EvaluationOperation
  ├ EvaluationAttempt 1 → ERROR (sandbox infrastructure)
  └ EvaluationAttempt 2 → FAIL  (authoritative test result)
```

Evaluation retry policy is bounded and applies only to infrastructure/observation failures. A genuine criterion `FAIL` is not retried as if it were transient infrastructure noise.

This small EvaluationAttempt relation is internal to evaluation and does not reuse Task Run/Attempt semantics.

At most one EvaluationAttempt may be nonterminal for an EvaluationOperation. A replacement EvaluationAttempt is created only after the prior attempt is definitively terminal or definitively absent under the evaluation retry policy. `EvaluationAttempt.id` is the stable provider-neutral execution/reconciliation identity; an evaluator helper/backend may map it to opaque native attachment or keyed-launch state, but Pantheon does not fabricate keyed-idempotent semantics where the executor cannot provide them.

## 22. Crash reconciliation

The external execution boundary is persisted **per EvaluationAttempt**, not merely per EvaluationOperation. The operation may have bounded sequential retries, so each attempt must independently record whether its launch path crossed the point where an external evaluator/process could have been contacted.

Canonical flow:

```text
durable EvaluationOperation intent
        ↓
verification Sandbox/materialization becomes ready as required
        ↓
durable EvaluationAttempt created
  launch_contact_state = NOT_CONTACTED
        ↓
T15: durable launch-contact transition
  CONTACT_MAY_HAVE_OCCURRED
        ↓
external evaluator/process execution
        ↓
observation/reconciliation
```

The marker is monotonic:

```text
NOT_CONTACTED
    ↓
CONTACT_MAY_HAVE_OCCURRED
```

It is never reset to `NOT_CONTACTED`.

Crash semantics are:

```text
NOT_CONTACTED
+ no independent external evidence
→ Pantheon knows its evaluator-launch path never crossed the external-call boundary
→ launch/reconciliation may proceed as not applied

CONTACT_MAY_HAVE_OCCURRED
→ the external launch may have happened even if acknowledgement was lost
→ execution outcome is UNKNOWN until the same EvaluationAttempt identity is reconciled or safely terminated
→ no overlapping EvaluationAttempt may be created
```

Sandbox provisioning has its own durable SandboxKey/provisioning reconciliation contract. This EvaluationAttempt marker protects the evaluator/process/remote-check launch boundary and does not duplicate Sandbox lifecycle truth.

Where backend-authored billable usage exists, a lineage durably proven `NOT_CONTACTED` with no independent external-contact evidence cannot justify backend-authored usage. Once any EvaluationAttempt reached `CONTACT_MAY_HAVE_OCCURRED`, delayed usage remains admissible subject to the immutable H1 metering-source provenance and normal idempotency checks; current terminal state is still not a usage-truth predicate.

## 23. Human evaluation

Human Task evaluation is distinct from authorization approval.

A `HumanEvaluationRequest` binds:

- Candidate digest;
- criterion;
- EvaluatorVersion;
- EvaluationRound.

The human supplies a PASS/FAIL judgment that becomes immutable Evidence.

It does not create a Capability Grant, alter permissions, increase a budget, or authorize unrelated actions.

Likewise, a human authorization approval does not satisfy a Task acceptance criterion.

## 24. Evidence staleness

Human and automated Evidence apply only to the exact immutable subject and evaluator version they judged.

Changing Candidate digest, criterion semantics, or evaluator version prevents stale Evidence from satisfying a new EvaluationRound.

Historical Evidence is never rewritten.

## 25. Producing Agent cannot self-grade authoritatively

Tests/checks run inside the producing Run may inform development and may be recorded as ordinary artifacts/events, but do not satisfy registered Acceptance criteria merely because the worker reports success.

Pantheon independently materializes the Candidate and executes the pinned trusted evaluator before producing authoritative Evidence.

## 26. Evaluation and producing Run finalization

Once a Candidate is durable and immutable, EvaluationOperations may proceed while the producing Run is still Finalizing because they no longer depend on its live execution process.

However, if Acceptance fails, `REQUEUE_TASK` may not transition the Task to Ready until the producing Run is terminal. Until then the Task remains Evaluating with a condition such as `PriorRunFinalizing`.

This preserves the invariant that a Ready Task has no nonterminal prior Run.

## 27. Cancellation and current authority

Cancellation/current desired state always wins over applying a later evaluation result.

Evaluation completion re-reads Task/EvaluationRound authority before committing Evidence and aggregate acceptance state.

If cancellation has committed, completed Evidence may remain as immutable history but cannot resurrect the Task or cause Task success.

## 28. Registry evolution

Publishing a new EvaluatorVersion never rewrites old Evidence, old EvaluationRounds, or pinned Task acceptance contracts.

Logical evaluator refs may move to a new current version for newly materialized Tasks only.

Configuration/Policy revisioning owns atomic registry publication and reload, but this subsystem requires immutable evaluator versions and exact pinning.

## 29. Persistence shape

A likely relational model includes:

```text
evaluators
  logical_ref
  current_version
  enabled

evaluator_versions
  digest
  logical_ref
  kind
  definition_json
  created_at

evaluation_rounds
  id
  task_id
  candidate_digest
  acceptance_hash
  policy_revision
  state
  created_at

evaluation_round_evaluators
  round_id
  criterion_id
  evaluator_version

evaluation_operations
  id
  round_id
  criterion_id
  evaluator_version
  usage_reporter_backend_id nullable
  usage_reporter_backend_revision nullable
  metering_contract_digest nullable
  state

evaluation_attempts
  id
  operation_id
  ordinal
  state
  launch_contact_state
  launch_contact_initiated_at nullable
  launch_contact_daemon_incarnation nullable

human_evaluation_requests
  id
  round_id
  criterion_id
  state
```

`evaluation_attempts.launch_contact_state` is created as `NOT_CONTACTED` and may transition only to `CONTACT_MAY_HAVE_OCCURRED`. The timestamp/incarnation are written with that transition and are provenance, not a separate evaluation ownership epoch. At most one nonterminal EvaluationAttempt exists per EvaluationOperation; the persistence layer enforces this with a partial unique constraint/index over the operation identity and nonterminal state domain.

The metering-source columns are immutable operation intent and are either absent together for a non-backend-metered operation or complete together before external contact. They are not an ExecutionBinding.

Verification Sandbox ownership is persisted in `sandbox_instances`, not duplicated on EvaluationAttempt. A control-operation Sandbox points relationally to its owning `evaluation_operations` row; its holder remains stable across bounded sequential EvaluationAttempts.

Exact DDL belongs to the implementation schema pass.

## 30. Atomic evaluation result commitment

When an evaluator establishes a result, Pantheon commits it against current authority atomically.

Conceptually:

```text
BEGIN IMMEDIATE

verify EvaluationRound current
verify Candidate digest unchanged
verify criterion/evaluator version matches
verify Task is still Evaluating/current
verify operation provenance

create immutable Evidence
settle operation resources/budget
update criterion evaluation state
aggregate required criteria
append Events

COMMIT
```

Task lifecycle changes and rejection recovery remain owned by Task/Acceptance/Recovery controllers rather than the evaluator itself.

## 31. Authority separation

An evaluator may report only its criterion result and structured evidence.

It may not:

- transition Task phase directly;
- select another Run;
- choose recovery;
- increase budget;
- broaden permissions;
- modify Candidate content;
- change evaluator registry configuration.

Acceptance Controller aggregates Evidence. Recovery Policy decides what rejection means. Task Controller owns Task lifecycle.

## v1 scope

Include:

- trusted Evaluator Registry;
- immutable content-addressed EvaluatorVersions;
- exact evaluator pinning in Task acceptance;
- `check`, `schema`, and `human` evaluator kinds;
- EvaluationRound;
- EvaluationOperation and bounded low-level EvaluationAttempts;
- generic `control-operation` resource holder scope;
- generic Resource Ledger admission;
- independent verification materialization/Sandbox with explicit EvaluationOperation ownership;
- immutable Evidence;
- human acceptance request semantics;
- crash reconciliation;
- cancellation/current-authority fencing.

Defer:

- model-based rubric graders;
- independent model reviewer Agents;
- learned evaluator selection;
- weighted/quorum acceptance;
- distributed evaluation workers;
- shared remote evaluation cache.

## Core invariants

1. **Evaluation is control-plane verification, not ordinary Task Agent execution.**
2. **Tasks reference only trusted registered evaluators and pin immutable versions.**
3. **V1 evaluator kinds are check, schema, and human.**
4. **The Candidate and evaluator versions are frozen in an EvaluationRound.**
5. **External evaluation consumes Resource Ledger capacity and cannot run as hidden work.**
6. **Authoritative checks run against immutable Candidate materialization, never a producer's live workspace.**
7. **Producing Agents cannot self-grade authoritatively.**
8. **Evaluator infrastructure ERROR is distinct from criterion FAIL.**
9. **Evaluators produce Evidence; they do not own Task lifecycle or recovery.**
10. **Cancellation/current authority overrides applying stale evaluation results.**
11. **Registry changes never rewrite historical Evidence or existing Task acceptance semantics.**
12. **Model-based authoritative evaluation is deferred from v1.**
13. **Backend-authored EvaluationOperation usage is accepted only from the backend frozen in immutable operation-level metering provenance; that binding does not redefine EvaluationOperation lifecycle/execution ownership.**
14. **Every externally executing EvaluationAttempt has a durable monotonic launch-contact marker committed before its external evaluator/process call.**
15. **At most one EvaluationAttempt per EvaluationOperation is nonterminal; ambiguous contact remains on the same attempt identity and never authorizes an overlapping retry.**
16. **A verification Sandbox is durably owned by its EvaluationOperation, not by an EvaluationAttempt; at most one current/non-RELEASED verification Sandbox exists for that operation and ambiguous Sandbox existence blocks replacement.**
