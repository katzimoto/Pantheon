# Recovery Policy

## Status

Draft design — Pantheon recovery, retry, and escalation specification.

## Purpose

This subsystem defines how Pantheon turns immutable failure/condition evidence into a deterministic recovery decision without allowing individual controllers or backends to invent independent retry behavior.

The central rule is:

> **Evidence records what happened. Recovery policy decides what Pantheon should do next.**

Not every recovery trigger is a failure. Acceptance rejection, budget exhaustion, rate limiting, policy revocation, and unavailable execution routes can all require recovery decisions while representing different underlying facts.

See also:

- `docs/architecture/run-and-attempt.md`
- `docs/architecture/task-lifecycle.md`
- `docs/architecture/task-acceptance-and-completion.md`
- `docs/architecture/budget-usage-and-rate-limits.md`
- `docs/architecture/execution-offer-routing-and-admission-handshake.md`
- `docs/architecture/scheduler-dispatch-and-run-intent-reconciliation.md`

## 1. Recovery levels

Pantheon has four increasingly broad execution-recovery scopes:

```text
RECONCILE
same Attempt / same LaunchKey / same execution lineage

RETRY EXECUTION
new Attempt / same Run / same ExecutionBinding

RETRY STRATEGY
Task returns to Ready / Scheduler creates a new Run and Binding

REPLAN
Planner mutates or extends the TaskGraph through validated GraphPatch
```

Two terminal/control outcomes sit alongside them:

```text
REQUEST HUMAN / APPROVAL
FAIL TASK
```

Recovery policy should select the smallest safe recovery scope that can plausibly resolve the condition.

## 2. Failure and condition evidence are immutable facts

A backend, controller, evaluator, or accounting subsystem may produce normalized evidence, but must not authoritatively decide retryability.

Conceptual failure record:

```yaml
failure:
  id: failure_123

  subject:
    kind: attempt
    ref: attempt_456

  origin: execution
  code: execution.process-exit
  certainty: definitive

  evidence:
    exitCode: 137
    observation: EXITED

  source:
    backend: executor://A

  occurredAt: ...
  fingerprint: sha256:...
```

The record intentionally does not contain `retryable`, retry count, or recovery action.

## 3. Normalized origins

Use a small stable origin vocabulary plus namespaced codes.

v1 origins:

```text
preparation
execution
backend
resource
budget
policy
acceptance
scheduling
workspace
artifact
system
```

Examples of namespaced codes:

```text
preparation.workspace-failed
execution.process-exit
execution.timeout
backend.transport-unavailable
backend.execution-lost
resource.memory-exhausted
budget.hard-limit-reached
budget.extension-required
policy.authority-revoked
acceptance.required-check-failed
scheduling.no-compatible-route
workspace.corrupt
artifact.missing
system.invariant-violation
```

The code namespace remains extensible without teaching core policy concrete provider/runtime names.

## 4. UNKNOWN execution is not failure

`Attempt.observedExecution = UNKNOWN` means Pantheon lacks sufficient evidence to establish whether execution exists or terminated.

It must therefore remain in:

```text
RECONCILE
same Attempt
same LaunchKey
```

A new Attempt is forbidden while continuity is unresolved.

Only a definitive terminal observation/evidence may permit retry policy to create another Attempt.

## 5. Canonical recovery actions

v1 actions:

```text
RECONCILE
RETRY_ATTEMPT
REQUEUE_TASK
REPLAN
REQUEST_APPROVAL
FAIL_TASK
```

### RECONCILE

Continue the same Attempt/LaunchKey. Used for uncertainty, reconnect, adapter restart, transient status failure, and similar continuity-preserving cases.

May include a `notBefore` time.

### RETRY_ATTEMPT

Requires the current Attempt to be definitively terminal.

Creates a fresh Attempt and LaunchKey under the same Run/ExecutionBinding.

Used only when repeating the exact resolved execution strategy remains appropriate.

### REQUEUE_TASK

Concludes the current strategy and returns the Task to `Ready`.

The normal Scheduler then performs Logical Agent Resolution, offer routing, admission, reservation, and creates a new Run/ExecutionBinding.

Recovery policy never directly creates a scheduled Run.

### REPLAN

Hands a recovery snapshot to the Planner so it can propose a validated GraphPatch. Used when changing execution strategy alone is insufficient.

### REQUEST_APPROVAL

Creates a durable human/operator approval request, for example to increase a recovery allowance or budget.

The Agent cannot approve its own recovery authority.

### FAIL_TASK

Marks the Task terminal `Failed` after all allowed recovery paths are exhausted or policy determines recovery is inappropriate.

## 6. RecoveryDecision

Policy output is a durable, revision-bound decision.

Conceptually:

```yaml
decision:
  id: recovery_123

  subject:
    task: task_42
    run: run_17
    attempt: attempt_2

  inputs:
    evidence:
      - failure://91
    policyHash: sha256:...
    stateRevision: 144

  action: RETRY_ATTEMPT
  notBefore: ...

  accounting:
    charge:
      attemptRetry: 1

  reason:
    code: transient-execution-failure

  createdAt: ...
```

Applying a decision rechecks all bound state/revisions. Stale decisions do not resurrect or mutate newer work.

## 7. Recovery never bypasses the Scheduler

For strategy replacement:

```text
Run 1 concludes
   ↓
Task → Ready
   ↓
Scheduler
   ↓
Logical Agent eligibility
   ↓
Agent + ExecutionOffer routing
   ↓
Admission / budget
   ↓
new ExecutionBinding
   ↓
Run 2
```

This preserves fairness, resource accounting, routing policy, and the invariant that scheduled Runs are created only by the normal scheduler commitment path.

## 8. Acceptance rejection is not Run failure

A Run that durably submits one valid candidate can complete successfully as a Run even if Task acceptance later rejects that candidate.

```text
Run 1 → Completed
Task → Evaluating
Acceptance → FAIL
```

Recovery may choose `REQUEUE_TASK`, producing a later Run with the acceptance evidence in recovery context.

Run 1 remains `Completed`; it is not retroactively changed to `Failed`.

This separates execution reliability from candidate quality.

## 9. RecoveryContext

TaskSpec remains immutable across retries.

When recovery requeues work, Pantheon attaches a separate recovery context containing references to prior evidence.

Conceptually:

```yaml
recoveryContext:
  previousRun: run_17

  evidence:
    - failure://...
    - acceptance://...

  candidate:
    ref: artifact://...

  summary:
    reason: acceptance-rejected
```

Agent Resolution, Context Builder, and ExecutionRequest construction may consume this context for the next Run.

## 10. Factual counts and recovery charges are separate

Observed execution history is immutable fact:

```text
Attempts created
Runs created
acceptance rejections
```

Recovery quotas count policy-charged retries:

```text
attempt retries charged
strategy retries charged
semantic retries charged
```

Some infrastructure/disruption failures may be configured not to consume recovery quota even though a new Attempt was factually created.

Never falsify Attempt or Run history to implement retry accounting.

## 11. Recovery limits

Recovery limits are control-plane policy, not Resource Ledger or Budget Ledger capacity.

Conceptually:

```yaml
recoveryPolicy:
  limits:
    attemptRetriesPerRun: 2
    runRetriesPerTask: 3
    elapsedRecoveryTime: 30m
```

Exact defaults remain configuration, not architecture.

The older ambiguous generic `maxRetries` concept should not be used as canonical recovery semantics.

## 12. Ordered deterministic policy

Recovery policy is evaluated as deterministic ordered rules with a fail-closed default.

Conceptually:

```yaml
recoveryPolicy:
  rules:
    - match:
        certainty: unknown
      action:
        reconcile: {}

    - match:
        origin: backend
        code: backend.transport-unavailable
      action:
        reconcile:
          backoff: transient

    - match:
        origin: execution
        code: execution.transient
      action:
        retryAttempt: {}

    - match:
        origin: acceptance
      action:
        requeueTask: {}

    - match:
        origin: budget
        code: budget.extension-required
      action:
        requestApproval: {}

  default:
    action: failTask
```

The exact configuration language is deferred, but first-match deterministic semantics are preferred for v1.

## 13. Semantic classification is advisory only

A model may analyze ambiguous diagnostics and produce semantic evidence such as a proposed failure class.

That output is not authoritative recovery control.

Pantheon validates the classification, records it as evidence where appropriate, and the deterministic RecoveryPolicy still chooses the action.

No model directly commands retry, reroute, replan, or Task failure.

## 14. Failure fingerprints

Normalized failure evidence should include a fingerprint derived from stable relevant dimensions such as:

```text
origin
normalized code
ExecutionBinding
structured evidence subset
```

This lets policy detect repeated equivalent failures:

```text
same Binding
same fingerprint
repeated N times
```

and escalate from Attempt retry to Task requeue or replanning.

Sophisticated similarity/ML clustering is deferred.

## 15. Backoff

RecoveryDecision owns retry timing.

v1 uses capped deterministic exponential backoff:

```text
baseDelay × factor^retryIndex
capped at maxDelay
```

An authoritative external `retryAfter`/reset time acts as a minimum when applicable.

Random jitter is deferred for the local single-daemon v1 because deterministic timing improves reproducibility and debugging.

## 16. Rate limits do not consume retry quota when continuity remains

A rate-limit/throughput condition generally produces:

```text
RECONCILE
notBefore = retryAfter/resetAt
```

when the same Attempt remains valid.

It does not create a new Attempt merely because the backend asks Pantheon to wait, and it does not consume retry quota in that case.

Rate-limit state remains separate from BudgetAccount semantics.

## 17. Budget exhaustion is a recovery condition

Hard budget exhaustion may lead to:

- `REQUEST_APPROVAL` for additional authority;
- `REQUEUE_TASK` if policy allows a different lower-cost strategy;
- `FAIL_TASK` if no continuation is authorized.

Actual prior usage is never refunded or rewritten by recovery.

## 18. Resource failures may require strategy replacement

A failure such as `resource.memory-exhausted` may indicate that repeating the same Binding would simply reproduce the same condition.

Policy may therefore choose `REQUEUE_TASK` rather than `RETRY_ATTEMPT` so routing/admission can produce a new Binding with a different resource footprint.

The rule belongs to policy rather than hard-coded resource-name branches.

## 19. Backend failure feeds observations, not provider branches

Repeated backend failures may update normalized BackendHealth/RouteMetrics so future routing naturally deprioritizes or excludes that backend.

Recovery policy operates on normalized evidence and never branches on provider/model/runtime names.

## 20. Backend-internal retries are bounded by Attempt continuity

Adapters may internally retry transport, polling, reattachment, native session inspection, or other continuity-preserving operations without creating a new Attempt.

They must not secretly create a fresh logical execution after the prior execution is definitively gone.

A fresh execution lineage requires Pantheon to create a new Attempt/LaunchKey so retry limits, usage, provenance, and recovery history remain visible.

## 21. Current authority precedes retry policy

Before applying recovery Pantheon rechecks, in order:

1. cancellation/supersession;
2. current hard authority/policy;
3. execution certainty;
4. RecoveryPolicy;
5. recovery limits;
6. backoff/timing.

A stale retry decision cannot override cancellation or newly tightened hard policy.

## 22. Recovery application is transactional

### RETRY_ATTEMPT

```text
BEGIN

recheck Run still Active
recheck prior Attempt definitively terminal
recheck current policy/authority
recheck recovery limits and budget authority
record recovery charge
create Attempt N+1
create immutable LaunchKey

COMMIT

then ExecutorBackend.ensureExecution(...)
```

External side effects occur only after durable Attempt creation.

### REQUEUE_TASK after execution failure

```text
BEGIN

verify old execution safely terminal
verify RecoveryDecision current
finalize current Run as Failed
Task Active → Ready
attach RecoveryContext
record strategy-retry charge if applicable
set scheduler notBefore if needed
settle/release Run-scoped state when safe
retain Task-scoped workspace/state as allowed

COMMIT
```

### REQUEUE_TASK after acceptance rejection

```text
BEGIN

record Acceptance FAIL
verify RecoveryDecision current
Task Evaluating → Ready
attach RecoveryContext
record semantic-retry charge

COMMIT
```

The already-Completed Run is not mutated.

## 23. Replanning is escalation

Ordinary execution failures should not automatically invoke the Planner.

Preferred escalation ladder:

```text
uncertainty
  → RECONCILE

transient execution failure
  → RETRY_ATTEMPT

resolved strategy unsuitable
  → REQUEUE_TASK

repeated semantic/structural failure
  → REPLAN

no valid authorized path
  → REQUEST_APPROVAL or FAIL_TASK
```

## 24. Human recovery overrides are additive and scoped

A human may explicitly authorize extra recovery allowance.

Conceptually:

```yaml
recoveryOverride:
  task: task_123
  allowance:
    additionalRunRetries: 1
  uses: 1
  createdBy: human
```

Historical counters are never reset or erased.

Budget increases remain Budget authority changes, not recovery-counter overrides.

## v1 scope

Include:

- immutable normalized failure/condition evidence;
- RecoveryDecision;
- six canonical recovery actions;
- deterministic ordered RecoveryPolicy;
- separate factual versus charged retry counters;
- deterministic capped exponential backoff;
- failure fingerprints;
- RecoveryContext;
- transactional application;
- scheduler-mediated strategy retry;
- human/operator recovery overrides.

Defer:

- ML-based failure similarity clustering;
- speculative retries/racing multiple Attempts;
- distributed recovery coordinators;
- randomized jitter;
- automatic policy synthesis from learned behavior;
- provider-specific recovery branches in core.

## Key decisions

1. Recovery Policy is distinct from raw failure handling.
2. Evidence is immutable fact; retryability/action is policy.
3. Backends never author authoritative recovery actions.
4. UNKNOWN execution remains reconciliation, not failure.
5. Recovery scopes are reconcile, new Attempt, Task requeue/new Run, and replan.
6. Recovery never creates scheduled Runs directly; the Scheduler remains authoritative.
7. Canonical actions are `RECONCILE`, `RETRY_ATTEMPT`, `REQUEUE_TASK`, `REPLAN`, `REQUEST_APPROVAL`, `FAIL_TASK`.
8. Acceptance rejection does not retroactively fail a Completed Run.
9. RecoveryContext carries prior evidence without mutating TaskSpec.
10. Factual Attempt/Run counts and policy-charged retries are separate.
11. Recovery limits are not Resource/Budget Ledger units.
12. Recovery policy is deterministic, ordered, and fail-closed.
13. Semantic model classification is advisory evidence only.
14. Failure fingerprints support repeated-failure escalation.
15. v1 uses deterministic capped exponential backoff and respects authoritative retry-after times.
16. Rate-limit waiting does not create Attempts or consume retry quota while continuity remains.
17. Budget exhaustion is a recovery condition, not automatically terminal failure.
18. Backend-internal retries are allowed only while preserving Attempt/LaunchKey continuity and truthful usage accounting.
19. Cancellation/current hard authority overrides retry policy.
20. Recovery decisions are durably and transactionally applied before external side effects.
21. Replanning is escalation, not the default response to ordinary execution failure.
22. Human recovery overrides are explicit, scoped, additive, and never erase history.
