# Task Acceptance and Completion Contracts

## Status

Draft design — Pantheon task subsystem specification.

## Purpose

Pantheon must distinguish an executor claiming that work is finished from the system determining that the Task contract has actually been satisfied.

The executor may submit a candidate result. Only Pantheon may declare a Task accepted and later complete.

## Foundational principles

1. **Workers submit results; they do not complete Tasks.**
2. **Prefer outcome verification over transcript inspection.**
3. **Acceptance evidence is independent and bound to the exact candidate/state being judged.**
4. **Deterministic checks are preferred; model and human judgment are used where deterministic verification is insufficient.**
5. **Required criteria all pass or the Task is not accepted.**
6. **Evaluator errors fail closed.**
7. **Changing the candidate invalidates stale evidence and approvals.**
8. **Acceptance determines success; retry, escalation and replanning belong to later subsystems.**

## Result submission

Workers use a semantic operation such as `task.submit_result`, never `task.complete`.

Conceptual payload:

```yaml
result:
  outputs:
    changeset: artifact://change-938
    diagnosis: artifact://report-939
  summary: >
    Timeout was caused by connection pool exhaustion.
```

The submission creates a candidate for evaluation.

## Acceptance contract

A Task may define human-readable criteria plus evaluator references:

```yaml
acceptance:
  strategy: all

  criteria:
    - id: checkout-tests
      statement: Checkout integration tests pass.
      evaluator:
        ref: check://project/checkout-integration
      severity: required

    - id: payment-regression
      statement: Existing payment tests remain green.
      evaluator:
        ref: check://project/payment-regression
      severity: required

    - id: root-cause-quality
      statement: >
        Diagnosis explains the actual root cause, supporting
        evidence, and why the fix addresses it.
      evaluator:
        ref: rubric://engineering/root-cause-analysis
      severity: required
```

The statement is understandable to humans/models. The evaluator reference identifies an executable or reviewable acceptance mechanism.

## Evaluator classes

Pantheon should normalize evaluators into a small set of classes:

```text
check       deterministic executable validation
assertion   structured environment/state assertion
schema      output-shape validation
policy      security/governance validation
rubric      qualitative model-assisted judgment
review      independent specialist review
human       explicit human authority/judgment
```

The first four should be deterministic whenever practical.

## Evidence first

Pantheon should prefer actual outcome/state verification over trusting worker narration.

Preferred order:

```text
actual environment state
        ↓
deterministic executable check
        ↓
structural/static verification
        ↓
independent model judgment
        ↓
human judgment
```

The ordering is about automation and verifiability, not the intrinsic value of human review.

## Evidence object

Acceptance results are durable evidence records bound to immutable subjects.

Conceptual form:

```yaml
kind: Evidence

metadata:
  id: evidence_01K...

subject:
  kind: artifact
  ref: artifact://changeset-938
  digest: sha256:abc...

criterion:
  task: task_123
  id: checkout-tests

evaluator:
  ref: check://project/checkout-integration
  version: sha256:9292...

result:
  verdict: pass
  details:
    testsPassed: 37
    testsFailed: 0

provenance:
  run: run_384
  startedAt: ...
  completedAt: ...
```

The evidence binds:

```text
immutable candidate/state
+
criterion
+
evaluator/policy version
+
verdict
+
provenance
```

## Staleness

Evidence applies only to the exact subject it evaluated.

If a candidate digest changes, earlier evidence and approvals become stale and may not satisfy acceptance.

```text
candidate sha256:abc
        ↓ tests pass
        ↓ candidate changes
candidate sha256:xyz
        ↓
previous PASS = STALE
```

This rule applies equally to deterministic checks, model reviews and human approvals.

## Verdicts

Criterion evaluation and evaluator health are separate facts.

Minimum v1 verdicts:

```text
PASS     criterion satisfied
FAIL     criterion not satisfied
ERROR    evaluator could not determine outcome
PENDING  evaluation not finished
```

`ERROR` must never be interpreted as `PASS`.

## Required and advisory criteria

V1 supports two blocking classes:

```text
required   failure/error blocks acceptance
advisory   recorded but does not block acceptance
```

Avoid additional severity hierarchies until a real need appears.

## Aggregation

V1 acceptance strategy is deliberately simple:

> Every required criterion must PASS.

Advisory criteria do not block acceptance.

Future versions may add `any`, threshold, weighted or quorum aggregation, but v1 should not depend on them.

## Trusted evaluator registry

Tasks must not embed arbitrary executable verification hooks such as:

```yaml
acceptance:
  command: npm test
```

Instead Tasks reference trusted, versioned evaluators:

```yaml
evaluator:
  ref: check://project/test-suite
```

A separately governed evaluator definition may describe the command or verifier:

```yaml
kind: Evaluator
metadata:
  name: project/test-suite
spec:
  type: command
  command:
    executable: cargo
    args: [test]
  sandbox: verification
  timeout: 5m
```

This prevents dynamically generated Tasks from becoming a shell-execution injection surface.

## Independent verification context

Executor-run tests are useful development feedback but are not automatically authoritative acceptance evidence.

Preferred flow:

```text
executor produces candidate
        ↓
Pantheon freezes candidate
        ↓
verification workspace/container
        ↓
Pantheon-controlled evaluator
        ↓
Evidence
```

Acceptance evaluators should run under their own permissions and verification sandbox where practical.

## Model graders

Model-assisted rubrics are appropriate for criteria that require qualitative judgment.

They should:

- use explicit structured rubrics;
- produce structured findings and verdicts;
- be independent from the executor where practical;
- never override deterministic failure;
- be calibrated against human/expert judgments for high-impact uses.

Self-certification by the same executor is not authoritative acceptance evidence.

## Structured rubric example

```yaml
rubric:
  dimensions:
    - id: correctness
      required: true
      definition: >
        Design satisfies the stated architectural constraints.

    - id: maintainability
      definition: >
        Responsibilities are separated and interfaces are explicit.

    - id: unnecessary-complexity
      inverse: true
      definition: >
        Penalize abstractions without a current architectural need.
```

The grader returns structured scores/findings rather than a free-form impression.

## Human approval

Human decisions are represented as evidence and are bound to the exact candidate/state being approved.

```yaml
- id: production-approval
  statement: User approves production deployment.
  evaluator:
    type: human
    authority: owner
  severity: required
```

Changing the candidate after approval invalidates the approval unless policy explicitly says otherwise.

## Rejection feedback

Acceptance failure should produce structured feedback that later lifecycle/recovery logic can consume.

```yaml
acceptance:
  verdict: rejected
  failures:
    - criterion: root-cause
      evidence: evidence_738
      feedback: >
        The proposed fix does not address the diagnosed
        connection-pool exhaustion.
```

Acceptance does not decide whether to continue the same Run, create a new Attempt, escalate, replan or spawn more Tasks.

## Acceptance versus completion

Pantheon should distinguish successful acceptance from final terminal completion.

```text
execution finished
      ↓
acceptance satisfied
      ↓
finalization
  ├─ seal artifacts
  ├─ persist audit trail
  ├─ release workspace/resources
  ├─ notify graph joiners
  └─ perform authorized final integration actions
      ↓
Task complete
```

Exact lifecycle states are defined by the Task lifecycle subsystem.

## Immutability

The acceptance contract is part of the immutable Task specification.

Runs bind to at least:

```text
taskSpecHash
acceptanceSpecHash
```

Changing acceptance requirements creates a new Task revision or explicit superseding Task rather than mutating the criteria under an active Run.

## Learning integration

Agent Genome learning should consume objective acceptance evidence rather than worker self-report.

Useful outcome signals include:

```text
required criteria pass/fail
review scores/findings
number of attempts
evaluator errors
user acceptance/rejection
rollback occurrence
stale evidence events
```

This gives the self-improvement subsystem evidence tied to actual outcomes.

## v1 scope

V1 should implement:

- result submission rather than self-completion;
- immutable acceptance contracts;
- evaluator references;
- evaluator classes: check, assertion, schema, policy, rubric, review, human;
- required/advisory criteria;
- `all required must pass` aggregation;
- PASS/FAIL/ERROR/PENDING verdicts;
- immutable Evidence objects with subject/evaluator digests;
- automatic evidence staleness when the subject changes;
- Pantheon-controlled verification context;
- structured rejection feedback.

Defer:

- weighted scoring;
- quorum/threshold acceptance;
- probabilistic aggregation;
- automatic model-grader calibration;
- sophisticated partial-credit semantics.

## Core invariant

> An executor may claim that it is finished. Only Pantheon, using evidence bound to the exact candidate and acceptance contract, may determine that the Task succeeded.
