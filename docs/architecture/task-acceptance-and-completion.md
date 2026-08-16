# Task Acceptance and Completion Contracts

## Status

Canonical Pantheon Task acceptance specification.

## Purpose

> **Workers submit immutable CandidateResults. Only Pantheon may determine that the Task contract is accepted and later terminalize the Task.**

See also:

- `artifact-model.md`
- `evaluation-and-evaluator-registry.md`
- `task-lifecycle.md`
- `recovery-policy.md`

## Foundational principles

1. Workers submit results; they never complete Tasks directly.
2. Prefer outcome/state verification over transcript narration.
3. Evidence is bound to the exact immutable subject + criterion + EvaluatorVersion.
4. Deterministic verification is preferred where practical.
5. V1 acceptance requires every required criterion to PASS.
6. Evaluator ERROR never counts as PASS.
7. Candidate/evaluator-version change makes prior evidence stale for the new subject/contract.
8. Acceptance decides satisfaction; Recovery decides retry/requeue/replan.
9. Producing Agent self-checks are development signals, not authoritative Acceptance evidence.

## CandidateResult

`task.submit_result` creates one immutable content-addressed Candidate per Run.

Example:

```yaml
candidate:
  task: task_123
  run: run_17
  outputs:
    changeset: artifact://sha256/...
    diagnosis: artifact://sha256/...
  summary: ...
```

Candidate identity is `candidate://sha256/...` over canonical content. Old opaque examples such as `artifact://change-938` are obsolete.

Submission/cancellation race semantics are defined by `task-lifecycle.md`: authoritative cancellation/supersession committed first causes submission conflict; Candidate committed first remains immutable history even if cancellation follows.

## Acceptance contract

TaskSpec defines human-readable criteria plus trusted evaluator refs/versions, conceptually:

```yaml
acceptance:
  strategy: all
  criteria:
    - id: tests
      statement: Required tests pass.
      evaluator:
        ref: check://project/tests
        version: sha256:...
      severity: required
```

Task materialization resolves logical Evaluator refs through the operator-governed Evaluator Registry and pins exact immutable EvaluatorVersions. A Task may not embed arbitrary executable commands.

## V1 evaluator kinds

Authoritative v1 evaluator kinds are:

```text
check
schema
human
```

`check` is a deterministic registered executable validation in an independent verification Sandbox. `schema` is trusted structural validation. `human` is explicit human judgment bound to the Candidate.

Model-based `rubric`/`review` evaluators are architecture-reserved but implementation-deferred from v1. They are not silently executed through an unaccounted model path.

Assertions/policy-style deterministic criteria compile to `check`/`schema` or current control-plane policy validation rather than requiring separate evaluator machinery.

## EvaluationRound

When Candidate becomes current, Acceptance Controller creates/reuses an EvaluationRound freezing:

```text
Task/Candidate digest
acceptance contract digest
criterion set
exact EvaluatorVersions
relevant ConfigurationRevision
```

Each external deterministic criterion creates an accounted `EvaluationOperation` using control-operation ResourceReservations/BudgetHolds where required. Human criteria create HumanEvaluationRequests.

## Independent verification

Executor-run tests are useful feedback but not authoritative acceptance proof.

Preferred path:

```text
Agent edits Task Workspace
  ↓
Pantheon seals immutable Candidate/code.changeset
  ↓
independent verification Sandbox
  ↓
registered EvaluatorVersion
  ↓
Evidence
```

Verification receives immutable Candidate input + disposable writable scratch. It does not operate authoritatively on the producer's mutable Task Workspace.

## Evidence

Evidence is immutable and binds at least:

```text
Candidate/Artifact subject digest
Task criterion
Evaluator ref + immutable version digest
EvaluationOperation/HumanEvaluation provenance
verdict
bounded structured details
output Artifact refs where needed
```

Large logs/reports become Artifacts referenced by Evidence.

## Verdicts

```text
PASS
FAIL
ERROR
PENDING
```

`FAIL` means the evaluator ran correctly and the criterion was not satisfied. `ERROR` means authoritative evaluation could not determine the criterion (sandbox failure, timeout without valid result, parser failure, etc.).

Required FAIL/ERROR prevents PASS aggregation.

## Required versus advisory

V1 supports:

```text
required
advisory
```

All required criteria must PASS. Advisory results are recorded but do not block acceptance.

No weighted/quorum/threshold strategy in v1.

## Human acceptance versus authorization approval

Human Task evaluation and permission Approval are different authority domains.

```text
HumanEvaluationRequest -> PASS/FAIL Evidence
ApprovalRequest -> scoped capability Grant -> authorization re-evaluation
```

Neither substitutes for the other.

## Staleness

Evidence applies only to the exact immutable Candidate/criterion/EvaluatorVersion it judged.

Changing Candidate or semantic Goal/Task acceptance revision creates a new subject/contract; old Evidence remains historical but cannot be reused as current PASS unless an explicit compatibility rule says it evaluates the same immutable subject/criterion/version.

## Acceptance aggregation

Acceptance Controller, not Evaluator, owns aggregation. Evaluators only return evidence/verdict.

When every required criterion PASSes:

```text
Task Evaluating -> Finalizing / terminalTarget=Succeeded
```

through the Task lifecycle transaction, after rechecking current Task/Goal authority and cancellation/supersession fences.

Evaluator cannot directly set Task phase.

## Rejection

A definitive required FAIL produces AcceptanceResult FAIL and immutable Evidence. It does not retroactively mark producing Run Failed.

Recovery Policy decides whether to:

```text
REQUEUE_TASK
REPLAN
REQUEST_APPROVAL/HUMAN where appropriate
FAIL_TASK
```

If REQUEUE is selected while the producing Run still Finalizing, Task remains Evaluating with `PriorRunFinalizing` until that Run terminalizes. Only then may T9 move Task Ready.

## Evaluator infrastructure error

EvaluationOperation may have bounded infrastructure retry/reconciliation separate from Task execution retry. Genuine criterion FAIL is not retried as though it were sandbox noise.

UNKNOWN evaluator process/sandbox state follows the same durable external-effect reconciliation discipline; duplicate verification execution is avoided while prior state is ambiguous.

## Cancellation/current authority

Cancellation/supersession can stop pending EvaluationOperations. Completed Evidence remains immutable history, but Acceptance cannot transition a cancelled/superseded Task to Succeeded because the aggregation commit re-reads current Task authority/revision.

## Goal acceptance reuse

GoalCompletionCandidate uses the same EvaluationRound/Evidence machinery with a different immutable subject type. There is no separate Goal evaluator execution architecture.

## Core invariants

1. Worker submits Candidate; Pantheon declares acceptance/success.
2. Candidate/Artifact refs are content-addressed canonical identifiers.
3. Task pins registered immutable EvaluatorVersions; arbitrary Task commands are forbidden.
4. V1 authoritative evaluator kinds are check/schema/human; model review is deferred.
5. Evaluation consumes accounted Resources/Budget where applicable.
6. Authoritative checks run independently against immutable Candidate materialization.
7. Evidence is immutable and exact-subject/evaluator-version bound.
8. ERROR never means PASS; all required criteria must PASS.
9. Producing Agent self-checks are never authoritative acceptance evidence.
10. Acceptance rejection does not fail the producing Run and cannot requeue until that Run is terminal.
11. Evaluators produce Evidence only; Task Controller owns lifecycle.
