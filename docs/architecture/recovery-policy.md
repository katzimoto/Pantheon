# Recovery Policy

## Status

Canonical Pantheon recovery, retry and escalation specification.

## Purpose

Recovery converts immutable failure/condition evidence into deterministic next action. Controllers/backends record facts; Recovery Policy decides what Pantheon should do.

> **Evidence records what happened. Recovery policy decides the next permitted control-plane action. UNKNOWN observation is not failure and is reconciled before replacement execution.**

## FailureRecord

A `FailureRecord` is immutable factual evidence. Initial normalized origins:

```text
PREPARATION
EXECUTION
BACKEND
RESOURCE
BUDGET
POLICY
ACCEPTANCE
SCHEDULING
WORKSPACE
ARTIFACT
SYSTEM
```

Fine-grained namespaced codes carry the exact condition. FailureRecord never embeds retry conclusions.

## Recovery levels

```text
LEVEL 0  RECONCILE
  same Attempt / same LaunchKey / same external lineage

LEVEL 1  RETRY EXECUTION
  new Attempt / same Run / same immutable Binding

LEVEL 2  RETRY STRATEGY
  Task returns Ready -> Scheduler creates new Run/new Binding

LEVEL 3  REPLAN
  Planner proposes TaskGraph changes/new or superseding Tasks
```

Human approval/request and terminal Task failure are orthogonal outcomes.

## Canonical RecoveryActions

```text
RECONCILE
RETRY_ATTEMPT
REQUEUE_TASK
REPLAN
REQUEST_APPROVAL
FAIL_TASK
```

There is intentionally no `CREATE_RUN` RecoveryAction. The Scheduler remains the only component that creates scheduled Runs.

## RecoveryDecision

A RecoveryDecision is immutable and binds at least:

- subject/failure/evidence refs;
- relevant Task/Run/Attempt revisions;
- current Goal/Graph context;
- `configRevision` and exact `recoveryPolicyDigest`;
- selected RecoveryAction;
- charged recovery counters/backoff;
- reason/fingerprint.

Recovery policy is deterministic/fail-closed. An LLM classifier may propose a category but never becomes authority.

## RECONCILE and UNKNOWN

`UNKNOWN` means Pantheon cannot establish whether an external obligation still exists. It is not a reason to create fresh work.

While UNKNOWN:

- same Attempt/LaunchKey is inspected/reattached;
- relevant reservations/holds remain fenced;
- replacement Attempt/Run is prohibited merely for progress;
- timeout alone never proves absence.

The pre-launch contact marker in `run-and-attempt.md` separates definitely-never-contacted execution from may-have-crossed-the-call-boundary ambiguity.

## RETRY_ATTEMPT

Allowed only when:

1. prior Attempt is conclusively terminal/absent;
2. exact immutable ExecutionBinding/semantic context remains valid;
3. policy authorizes another execution incarnation;
4. retry ceilings/backoff allow it.

Creates a new Attempt/new LaunchKey under the same Run. Failed Attempts retain factual usage/cost.

## REQUEUE_TASK

Used when semantic execution strategy/context must change, including normal Acceptance rejection.

Critical ownership rule:

> **REQUEUE_TASK may not move a Task to Ready until the previous responsible Run is terminal.**

If the RecoveryDecision is known while the producing Run is still Finalizing:

```text
Task remains Evaluating/Active as appropriate
condition = PriorRunFinalizing
RecoveryDecision is durable
```

Once that Run is terminal, the requeue transaction revalidates current Goal/Graph/Task/config/policy, installs Recovery/ContinuationContext where applicable, applies `notBefore`/backoff and commits `Task -> Ready`.

The Scheduler later creates the new Run. This preserves `Task Ready => zero nonterminal Runs` and avoids deadlock against the unique-live-Run invariant.

## Acceptance rejection

Acceptance rejection does not retroactively fail the producing Run. The Run normally reaches/stays `Completed` with its immutable Candidate.

Rejection Evidence becomes RecoveryContext for a later Run. If policy selects `REQUEUE_TASK`, the Task waits until the old Run is terminal before Ready.

## Blocking child continuation

A normal blocking spawn/yield is **not Recovery**. It creates `ContinuationContext`, the old Run terminalizes `Yielded`, Task becomes Waiting, and join satisfaction later returns it Ready without charging recovery retry counters.

## REPLAN

Planner proposes a GraphPatch against current Goal/Graph revisions. Running/completed Task specs are never rewritten in place. Supersession follows Task `Finalizing/terminalTarget=Superseded` and must safely close any responsible Run before terminal Task state.

## REQUEST_APPROVAL

Used when a trusted human decision is required. Approval creates the appropriate scoped Grant/configuration decision and the original operation is re-evaluated; approval is not authorization by itself.

## FAIL_TASK

Moves Task toward `Finalizing/terminalTarget=Failed`. Task reaches terminal Failed only after execution/accounting/finalizer obligations are safe.

## Counters and fingerprints

Factual counts and policy-charged retry counters are separate. Recovery Policy may fingerprint repeated equivalent failures so repeated same-failure retries escalate instead of looping indefinitely.

Counter examples:

```text
attempts observed
attempt retries charged
strategy retries charged
same-fingerprint repeats
```

Reconciliation loops do not consume retry count merely because inspection repeated.

## Backoff

V1 uses deterministic bounded/capped exponential backoff and honors authoritative/provider `retryAfter` where applicable. Random jitter is unnecessary for single-daemon local-first v1.

Rate-limit waiting that preserves the same external execution continuity remains reconciliation/waiting and does not create an Attempt by itself.

## Authority overrides recovery

Cancellation, supersession, Goal terminalization, current hard policy and current authorization ceilings override a previously proposed recovery path. RecoveryDecision application rechecks current authority before mutation/external effect.

## Operator force-resolution of UNKNOWN

Permanently unrecoverable UNKNOWN obligations need an escape hatch, but this is **administrative override**, not an ordinary automatic RecoveryAction.

Operator Control may issue an audited `force-resolve` command against an exact unresolved obligation such as:

```text
Attempt/LaunchKey
SandboxInstance/SandboxKey
ResourceReservation
BudgetHold linkage
external broker operation
```

Force resolution must:

1. identify the exact subject and expected revision;
2. require explicit operator reason/risk acknowledgement;
3. tombstone/fence the old external lineage so later callbacks cannot reacquire authority;
4. rotate/revoke relevant AgentControlSession/control lease authority;
5. transition Reservations/Holds through explicit administrative settlement states;
6. append high-severity Audit Events/RecoveryFinding resolution;
7. never fabricate factual Usage or Charge to make the ledger look settled.

A `LaunchKeyTombstone` (or equivalent durable lineage tombstone) records that Pantheon will never again treat callbacks/attachments for that old LaunchKey as current execution authority.

### Late observations after force resolution

A late callback may still be retained as historical/anomaly evidence, but it cannot mutate current Task/Run authority. Late legitimate Usage can still be ingested under immutable backend+Attempt provenance and may create overdraw; force resolution does not make factual usage disappear.

### No automatic timeout force-release

Pantheon may surface age/escalation diagnostics, but v1 does not automatically force-release UNKNOWN capacity after N minutes. That would reintroduce duplicate execution risk.

## Recovery application transaction

Before any external recovery effect, Pantheon durably applies/revalidates the RecoveryDecision/ownership transition in SQLite. External calls/process/Git operations remain outside the database transaction and are subsequently reconciled.

## Core invariants

1. Evidence is factual; RecoveryDecision is policy.
2. UNKNOWN => RECONCILE same lineage; UNKNOWN never creates replacement execution automatically.
3. RETRY_ATTEMPT => new Attempt only after old Attempt definitively ended and Binding unchanged.
4. Binding/semantic-context change => REQUEUE_TASK/new Scheduler-created Run.
5. REQUEUE_TASK cannot make Ready until previous Run terminal.
6. Acceptance rejection does not rewrite producing Run outcome.
7. Blocking yield/continuation is orchestration, not retry failure.
8. Recovery decisions bind exact ConfigurationRevision/recovery policy digest and revalidate current authority before application.
9. Operator force-resolution is explicit/audited lineage tombstoning, not timeout-based automatic recovery.
10. Administrative settlement never fabricates factual Usage/Charge.
