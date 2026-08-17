# Evaluation and Evaluator Registry

## Status

Canonical Pantheon acceptance verification subsystem specification.

## Purpose

Pantheon must execute acceptance checks without creating an unaccounted second execution plane. Evaluation consumes real resources, may execute untrusted project code, must be reproducible against the exact immutable subject being judged, and must never allow an evaluator to become lifecycle or authorization authority.

The central rule is:

> **Evaluation is control-plane verification over one immutable typed subject. Deterministic evaluators execute as explicitly accounted EvaluationOperations; they are not Tasks, Runs, Attempts, or Logical Agents.**

V1 supports two concrete EvaluationRound subject types:

```text
TASK_CANDIDATE
GOAL_COMPLETION_CANDIDATE
```

Model-based rubric/reviewer evaluation is deferred from v1.

See also:

- `docs/architecture/evaluation-and-acceptance/task-acceptance-and-completion.md`
- `docs/architecture/goals-and-planning/goal-resource.md`
- `docs/architecture/goals-and-planning/goal-lifecycle-and-completion-controller.md`
- `docs/architecture/artifacts-and-workspaces/artifact-model.md`
- `docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md`
- `docs/architecture/scheduling/scheduler-resource-ledger-and-admission.md`
- `docs/architecture/operations/budget-usage-and-rate-limits.md`
- `docs/architecture/security/permissions-and-capabilities.md`
- `docs/architecture/operations/event-and-observability-model.md`

## 1. Core resources

Pantheon distinguishes:

```text
Evaluator
  trusted logical verification mechanism

EvaluatorVersion
  one immutable concrete definition of that mechanism

EvaluationRound
  one immutable judgment context for one exact typed subject

EvaluationOperation
  one concrete control-plane execution of one criterion

EvaluationAttempt
  one bounded external evaluator/process contact lineage

Evidence
  immutable verdict/provenance for one Round criterion
```

Conceptually:

```text
Task Candidate --------------------┐
                                   │
                                   ├─> EvaluationRound
                                   │       │
GoalCompletionCandidate -----------┘       ├── EvaluationOperation A
                                           ├── EvaluationOperation B
                                           └── HumanEvaluation C
                                                     ↓
                                                  Evidence
                                                     ↓
                                            Acceptance aggregation
                                                     ↓
                               Task Controller OR Goal Completion Controller
```

Evaluation shares verification machinery across Task and Goal acceptance; it does not merge their lifecycle authority.

## 2. Trusted Evaluator Registry

Tasks and Goals may reference registered evaluator names, but never embed arbitrary executable hooks.

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

Evaluator definitions are governed by trusted system/user/project configuration. Planners, workers, Tasks, Goals, models, and executor backends may select only from evaluator references permitted by the Task/Goal contract; they may not define executable evaluator code dynamically.

## 3. Logical ref versus immutable version

A logical evaluator reference such as:

```text
check://project/unit-tests
```

may advance from one definition to another over time.

Every concrete `EvaluatorVersion` is immutable and content-addressed. A material change to command, arguments, result protocol, sandbox profile, timeout, resource requirements, input projection, or other execution semantics creates a new version digest.

Historical versions are never rewritten.

## 4. Acceptance semantics pin evaluator versions before evaluation

Evaluator version selection belongs to the immutable semantic acceptance contract, not to the later EvaluationRound creation moment.

For Task acceptance:

```text
Task materialization
→ resolve permitted logical evaluator refs
→ pin exact EvaluatorVersions in immutable TaskSpec acceptance contract
```

For Goal acceptance:

```text
GoalRevision commit
→ resolve permitted logical evaluator refs
→ pin exact EvaluatorVersions in immutable GoalRevision acceptance contract
```

Both acceptance contracts also retain evaluator-resolution provenance equivalent to:

```text
ConfigurationRevision
evaluatorRegistryDigest
```

Therefore editing evaluator configuration while a Task/Goal is in progress does not silently alter its acceptance semantics.

If a pinned version later becomes forbidden or unavailable under current hard policy, Pantheon does not silently substitute a newer version. The owning Task/Goal becomes blocked/reconciliation-required and normal semantic revision/replanning rules decide the next action.

Pinned evaluator identity is semantic history, not frozen execution authority. Every EvaluationOperation still rechecks current hard/current authorization policy before execution/admission.

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

Trusted in-process structural validation such as JSON Schema validation, Candidate output shape checks, Goal deliverable-set checks, or Artifact structural requirements.

### `human`

Explicit human judgment bound to the exact EvaluationRound subject and criterion.

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

Where the evaluator needs a particular projection of a composite GoalCompletionCandidate, that projection rule is part of the trusted immutable evaluator/acceptance semantics rather than a model-generated runtime choice.

## 7. No dynamically generated shell strings

Executable evaluators use an executable plus argv vector and an explicitly controlled environment.

Pantheon does not implicitly execute evaluator definitions through `/bin/sh -c`.

Dynamic substitutions, where supported, are typed and validated. Model-authored free-form shell fragments are never interpolated into trusted evaluator commands.

## 8. EvaluationRound typed subject

When an immutable subject becomes authoritative for evaluation, the owning Acceptance/Goal Completion Controller creates one immutable EvaluationRound that freezes the exact judgment context.

V1 uses concrete subject ownership, not an opaque polymorphic reference:

```text
subjectKind = TASK_CANDIDATE
  -> taskCandidate is set
  -> goalCompletionCandidate is NULL

subjectKind = GOAL_COMPLETION_CANDIDATE
  -> goalCompletionCandidate is set
  -> taskCandidate is NULL
```

Task example:

```yaml
evaluationRound:
  id: evalround_123
  subject:
    kind: TASK_CANDIDATE
    candidate: candidate://sha256/C

  acceptanceContract:
    hash: sha256:A

  evaluators:
    - criterion: tests
      ref: check://project/unit-tests
      version: sha256:E1

  configRevision: cfgrev_43
  evaluatorRegistryDigest: sha256:ER
  createdAt: ...
```

Goal example:

```yaml
evaluationRound:
  id: evalround_456
  subject:
    kind: GOAL_COMPLETION_CANDIDATE
    candidate: goal-completion-candidate://sha256/G

  acceptanceContract:
    hash: sha256:GA

  evaluators:
    - criterion: release-check
      ref: check://project/release
      version: sha256:E7

  configRevision: cfgrev_19
  evaluatorRegistryDigest: sha256:ER2
  createdAt: ...
```

The Round copies the exact evaluator versions and evaluator-resolution provenance from the owning immutable TaskSpec/GoalRevision acceptance contract. It does **not** resolve logical refs against whichever registry happens to be current when evaluation begins.

`configRevision` and `evaluatorRegistryDigest` are immutable decision provenance; they do not freeze old authorization/security policy for later execution.

## 9. Immutable typed subject is authoritative

Evaluation always judges the exact immutable subject referenced by the Round.

For `TASK_CANDIDATE`, the subject is the immutable CandidateResult digest/ref. Individual criteria may additionally bind constituent Artifact digests.

For `GOAL_COMPLETION_CANDIDATE`, the subject is the immutable completion snapshot containing Goal/Graph revision, accepted deliverable bindings, producer Candidate digests and the pinned Goal acceptance contract. Individual criteria may project exact deliverable Artifacts from that immutable subject according to trusted evaluator/criterion semantics.

Evaluation never treats a mutable filesystem path, producing Agent session, current TaskGraph lookup, or current deliverable lookup as subject authority after the Round is created.

## 10. EvaluationOperation

A deterministic external check executes as an `EvaluationOperation`, owned by the Acceptance/Evaluation Controller.

Conceptually:

```yaml
evaluationOperation:
  id: evalop_91
  round: evalround_123
  criterion: tests

  evaluator:
    ref: check://project/unit-tests
    version: sha256:E1

  state: PENDING

  execution:
    sandboxProfile: verification-default
    timeout: 10m
```

The exact subject is derived from the immutable EvaluationRound; EvaluationOperation does not invent or override a separate Candidate/Goal subject field.

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

## 15. Typed evaluation admission transaction

Before an external EvaluationOperation starts, Pantheon atomically verifies current authority and commits durable execution intent.

Common preconditions:

```text
EvaluationRound exists/current for its owning acceptance flow
exact pinned criterion/EvaluatorVersion matches Round
current hard policy permits execution
criterion is still applicable
required immutable subject material is available
```

Then subject-specific currentness is checked:

```text
TASK_CANDIDATE
  verify parent Task is still Evaluating/current
  verify Round taskCandidate is the Task's exact current Candidate

GOAL_COMPLETION_CANDIDATE
  verify parent Goal is still Evaluating/current
  verify Round goalCompletionCandidate is the Goal's exact current completion candidate
  verify GoalRevision represented by that candidate is still current for terminalization
```

Conceptually:

```text
BEGIN IMMEDIATE

resolve concrete Round subject + owning lifecycle object
re-read common + subject-specific currentness
verify exact pinned evaluator version
verify current hard/current authorization policy
assess and reserve generic resources
create EvaluationOperation execution intent
freeze metering-source binding if backend-authored billable usage applies
create BudgetHold if applicable
append Events

COMMIT
```

External process/container execution happens only after this durable transaction.

## 16. Independent verification environment

Authoritative evaluation runs against an independently materialized representation of the immutable Round subject.

It never executes authoritatively against a producing Agent's mutable Task workspace or mutable current Goal lookup.

For a Task Candidate, verification materializes the Candidate/Artifacts.

For a GoalCompletionCandidate, verification materializes the exact immutable deliverable/Artifact projection required by the pinned criterion. It never silently substitutes newer deliverable bindings from current Goal state.

The producing worker's own test output is useful development evidence, but does not automatically become authoritative Acceptance Evidence.

## 17. Immutable input, disposable scratch

Verification environments may need writable build/test scratch space, but the Round subject identity remains immutable.

Conceptually:

```text
EvaluationRound subject material
  immutable logical input

Evaluator scratch/build output
  writable + disposable

Evaluator result artifacts
  sealed explicitly
```

Evaluator-created scratch may never mutate the authoritative subject or its content-addressed identity.

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

Network or credentials require an explicit evaluator definition plus current policy authorization; they are never inherited from the producing Run/Task/Goal.

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

Evaluation produces immutable Evidence bound to the exact EvaluationRound, concrete immutable subject, criterion, EvaluatorVersion, and evaluation provenance.

Conceptually:

```yaml
evidence:
  evaluationRound: evalround_123
  subject:
    kind: TASK_CANDIDATE
    ref: candidate://sha256/C

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

Goal Evidence uses the same shape with `kind: GOAL_COMPLETION_CANDIDATE` and the exact GoalCompletionCandidate ref.

Relationally, the authoritative subject comes from `evaluation_rounds`; any self-contained subject copy in Evidence must exactly match the Round. Evidence cannot point at a different Task Candidate or GoalCompletionCandidate than its Round.

Large logs, reports, coverage files, or scanner output become Artifacts referenced by Evidence rather than large inline database/event payloads.

## 21. Evaluation execution retries

One criterion may require more than one low-level evaluation execution attempt when infrastructure fails.

Conceptually:

```text
EvaluationOperation
  ├ EvaluationAttempt 1 → ERROR (sandbox infrastructure)
  └ EvaluationAttempt 2 → FAIL  (authoritative criterion result)
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

Where backend-authored billable usage exists, a lineage durably proven `NOT_CONTACTED` with no independent external-contact evidence cannot justify backend-authored usage. Once any EvaluationAttempt reached `CONTACT_MAY_HAVE_OCCURRED`, delayed usage remains admissible subject to the immutable metering-source provenance and normal idempotency checks; current terminal state is still not a usage-truth predicate.

The Round's typed subject has no effect on these launch-contact semantics.

## 23. Human evaluation

Human evaluation is distinct from authorization approval.

A `HumanEvaluationRequest` binds:

- EvaluationRound;
- criterion;
- exact pinned EvaluatorVersion;
- state/provenance.

Its exact immutable Task Candidate or GoalCompletionCandidate subject is obtained from and must match the referenced Round; HumanEvaluationRequest does not need a separate ambiguous subject field.

The human supplies a PASS/FAIL judgment that becomes immutable Evidence.

It does not create a Capability Grant, alter permissions, increase a budget, or authorize unrelated actions.

Likewise, a human authorization approval does not satisfy an acceptance criterion.

## 24. Evidence staleness

Human and automated Evidence apply only to the exact immutable typed subject and evaluator version they judged.

Changing Task Candidate digest, GoalCompletionCandidate digest, criterion semantics, owning acceptance contract, or evaluator version prevents stale Evidence from satisfying a new EvaluationRound.

Historical Evidence is never rewritten.

## 25. Producing Agent cannot self-grade authoritatively

Tests/checks run inside a producing Run may inform development and may be recorded as ordinary artifacts/events, but do not satisfy registered Task acceptance criteria merely because the worker reports success.

For Goal acceptance, accepted Task outputs/deliverables are still only immutable subject material; their producing workers do not gain authority to assert Goal acceptance.

Pantheon independently materializes the exact Round subject and executes the pinned trusted evaluator before producing authoritative Evidence.

## 26. Task Evaluation and producing Run finalization

For a `TASK_CANDIDATE` Round, EvaluationOperations may proceed while the producing Run is still Finalizing because they depend on the durable immutable Candidate rather than its live execution process.

However, if Task Acceptance fails, `REQUEUE_TASK` may not transition the Task to Ready until the producing Run is terminal. Until then the Task remains Evaluating with a condition such as `PriorRunFinalizing`.

This preserves the invariant that a Ready Task has no nonterminal prior Run.

This producer-Run rule does not apply to `GOAL_COMPLETION_CANDIDATE` merely because that subject references producer Candidate digests. Goal Completion Controller owns Goal lifecycle and finalization separately.

## 27. Cancellation, revision, and current authority

Current desired state always wins over applying a later evaluation result.

Evaluation result commitment resolves the Round's concrete subject and re-reads its owning lifecycle authority:

```text
TASK_CANDIDATE
  -> Task still Evaluating/current with this exact Candidate
  -> no cancellation/supersession/currentness fence won

GOAL_COMPLETION_CANDIDATE
  -> Goal still Evaluating/current with this exact completion candidate
  -> represented GoalRevision still current for completion
  -> no cancellation/new-revision/finalization fence won
```

Completed Evidence may remain immutable history after either subject becomes stale, but cannot resurrect a Task or Goal or cause success through stale authority.

## 28. Registry evolution

Publishing a new EvaluatorVersion never rewrites old Evidence, old EvaluationRounds, pinned Task acceptance contracts, or pinned GoalRevision acceptance contracts.

Logical evaluator refs may move to a new current version for newly materialized TaskSpecs or newly committed semantic GoalRevisions only.

Configuration/Policy revisioning owns atomic registry publication and reload, but this subsystem requires immutable evaluator versions and exact acceptance-contract pinning.

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
  subject_kind                  TASK_CANDIDATE|GOAL_COMPLETION_CANDIDATE
  task_candidate_digest         nullable FK -> candidates
  goal_completion_candidate_digest nullable FK -> goal_completion_candidates
  acceptance_hash
  config_revision_id
  evaluator_registry_digest
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

`evaluation_rounds` enforces a strict typed-subject XOR:

```text
subject_kind = TASK_CANDIDATE
  => task_candidate_digest IS NOT NULL
     AND goal_completion_candidate_digest IS NULL

subject_kind = GOAL_COMPLETION_CANDIDATE
  => task_candidate_digest IS NULL
     AND goal_completion_candidate_digest IS NOT NULL
```

Pantheon intentionally does not use one opaque `subject_ref` as the relational safety boundary. The concrete FK determines the immutable subject and provides the path back to the owning Task or Goal lifecycle controller.

`evaluation_rounds.config_revision_id` and `evaluation_rounds.evaluator_registry_digest` are immutable provenance copied from the semantic acceptance contract that resolved the pinned evaluator versions. They do not substitute for current hard-policy/current-authorization checks when an EvaluationOperation executes.

`evaluation_attempts.launch_contact_state` is created as `NOT_CONTACTED` and may transition only to `CONTACT_MAY_HAVE_OCCURRED`. The timestamp/incarnation are written with that transition and are provenance, not a separate evaluation ownership epoch. At most one nonterminal EvaluationAttempt exists per EvaluationOperation; the persistence layer enforces this with a partial unique constraint/index over the operation identity and nonterminal state domain.

The metering-source columns are immutable operation intent and are either absent together for a non-backend-metered operation or complete together before external contact. They are not an ExecutionBinding.

Verification Sandbox ownership is persisted in `sandbox_instances`, not duplicated on EvaluationAttempt. A control-operation Sandbox points relationally to its owning `evaluation_operations` row; its holder remains stable across bounded sequential EvaluationAttempts.

Exact DDL belongs to the implementation schema pass.

## 30. Atomic evaluation result commitment

When an evaluator establishes a result, Pantheon commits it against current authority atomically.

Conceptually:

```text
BEGIN IMMEDIATE

load EvaluationRound + concrete typed subject
verify criterion/EvaluatorVersion matches pinned Round contract
verify operation provenance

TASK_CANDIDATE:
  verify Candidate unchanged/current
  verify Task is still Evaluating/current

GOAL_COMPLETION_CANDIDATE:
  verify completion candidate unchanged/current
  verify Goal is still Evaluating/current
  verify candidate's GoalRevision remains current for completion

create immutable Evidence bound to Round + exact subject
settle operation resources/budget
update criterion evaluation state
aggregate required criteria into AcceptanceResult
append Events

COMMIT
```

The transaction may record Evidence/AcceptanceResult even when later lifecycle application is subject to an owning-controller transition, but no stale result can directly mutate Task/Goal phase.

Task lifecycle changes/rejection recovery remain owned by Task/Acceptance/Recovery controllers. Goal success/finalization remains owned by Goal Completion Controller. The evaluator itself owns neither.

## 31. Authority separation

An evaluator may report only its criterion result and structured evidence.

It may not:

- transition Task or Goal phase directly;
- choose another Task Run;
- create/revise a Goal;
- choose recovery/replanning;
- increase budget;
- broaden permissions;
- modify immutable subject content;
- change evaluator registry configuration.

Evaluation/Acceptance Controller aggregates Evidence. Task Controller or Goal Completion Controller applies acceptance to lifecycle after rechecking current subject ownership. Recovery Policy decides Task rejection/recovery behavior where applicable.

## v1 scope

Include:

- trusted Evaluator Registry;
- immutable content-addressed EvaluatorVersions;
- exact evaluator pinning in immutable TaskSpec and GoalRevision acceptance contracts;
- concrete typed EvaluationRound subjects for Task Candidate and GoalCompletionCandidate;
- `check`, `schema`, and `human` evaluator kinds;
- EvaluationRound;
- EvaluationOperation and bounded low-level EvaluationAttempts;
- generic `control-operation` resource holder scope;
- generic Resource Ledger admission;
- independent verification materialization/Sandbox with explicit EvaluationOperation ownership;
- immutable exact-subject Evidence;
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
2. **EvaluationRound has exactly one concrete immutable subject: Task Candidate xor GoalCompletionCandidate.**
3. **TaskSpec pins Task evaluator versions at Task materialization; GoalRevision pins Goal evaluator versions at GoalRevision commit.**
4. **EvaluationRound consumes the already-pinned acceptance contract and never silently resolves newer registry versions at evaluation time.**
5. **V1 evaluator kinds are check, schema, and human.**
6. **External evaluation consumes Resource Ledger capacity and cannot run as hidden work.**
7. **Authoritative checks run against immutable Round subject materialization, never a producer's live workspace or mutable Goal lookup.**
8. **Producing Agents cannot self-grade authoritatively.**
9. **Evaluator infrastructure ERROR is distinct from criterion FAIL.**
10. **Evidence is immutable and must match the exact Round subject + criterion + pinned EvaluatorVersion.**
11. **Evaluators produce Evidence; Task Controller and Goal Completion Controller retain their separate lifecycle authority.**
12. **Cancellation/current revision/current subject ownership overrides applying stale evaluation results.**
13. **Registry changes never rewrite historical Evidence/EvaluationRounds or existing Task/Goal acceptance semantics.**
14. **Model-based authoritative evaluation is deferred from v1.**
15. **Backend-authored EvaluationOperation usage is accepted only from the backend frozen in immutable operation-level metering provenance; that binding does not redefine EvaluationOperation lifecycle/execution ownership.**
16. **Every externally executing EvaluationAttempt has a durable monotonic launch-contact marker committed before its external evaluator/process call.**
17. **At most one EvaluationAttempt per EvaluationOperation is nonterminal; ambiguous contact remains on the same attempt identity and never authorizes an overlapping retry.**
18. **A verification Sandbox is durably owned by its EvaluationOperation, not by an EvaluationAttempt; at most one current/non-RELEASED verification Sandbox exists for that operation and ambiguous Sandbox existence blocks replacement.**
19. **Current hard/security policy is rechecked at execution time and is not frozen merely because evaluator semantics were pinned earlier.**
