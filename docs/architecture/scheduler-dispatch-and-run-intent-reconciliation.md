# Scheduler Dispatch and Run-Intent Reconciliation

## Status

Canonical Scheduler-to-Run-Controller handoff specification.

## Central rule

> **Scheduler ends at a durable atomic Run intent. It never launches the executor. Run Controller owns preparation, Sandbox/Context readiness, Attempt creation, backend contact and reconciliation.**

## Handoff

After Agent/Offer/Sandbox/Resource/Budget selection, T3 commits:

```text
required incremental ResourceReservations
initial BudgetHolds
immutable ExecutionBinding
new Run / Run Active
Task Ready -> Active
SchedulingClaim consumed
Events/fairness state
```

Only after commit can execution preparation begin.

No external backend/process/Sandbox/Git/network call occurs inside T3.

## Task ownership

Task becomes Active at Run-intent commit, before any backend process exists. Active means the Run is responsible for making progress; it does not mean execution is already running.

V1 allows at most one nonterminal Run per Task.

## Run Controller preparation

Run Controller reconciles conditions such as:

```text
WorkspaceReady
SandboxReady
ContextReady
PolicyReady
LaunchReady
```

Preparation is idempotent/restart-safe and follows durable-intent-before-external-effect for Workspace/Sandbox provisioning.

SandboxReady includes factual verification against SandboxPlan, not merely runtime existence. ContextReady means immutable ContextPlan is attached to the Run.

## Attempt creation

When LaunchReady:

```text
BEGIN IMMEDIATE
revalidate Run/Task/control/config authority
create Attempt
assign Run-local ordinal
assign immutable LaunchKey
create Attempt AgentControlSession
set Run.currentAttempt
append Event
COMMIT
```

A Run may fail/recover during preparation without ever creating an Attempt.

## Pre-launch contact marker

Immediately before the first external `ensureExecution` contact for that Attempt, Run Controller commits the conservative contact marker:

```text
NOT_CONTACTED -> CONTACT_MAY_HAVE_OCCURRED
launchContactInitiatedAt
controller epoch/incarnation
```

Then it makes the external backend call.

Crash interpretation:

```text
Attempt exists + NOT_CONTACTED
  -> Pantheon's launch path definitely never crossed the external call boundary

CONTACT_MAY_HAVE_OCCURRED + lost acknowledgement
  -> execution state UNKNOWN until adapter/outer supervisor proves otherwise
```

This marker can conservatively classify a never-delivered call as UNKNOWN after a crash, but cannot falsely prove absence after a call may have been delivered.

## Launch semantics

Backend/outer supervisor advertises factual launch semantics:

```text
KEYED_IDEMPOTENT
OBSERVATIONAL
```

KEYED_IDEMPOTENT permits retry/reconciliation with the same LaunchKey addressing one execution lineage.

OBSERVATIONAL means ambiguous contact cannot be safely replayed as a create. Pantheon observes/reconciles the same Attempt and such offers are rejected during routing where duplicate-sensitive execution cannot be safely bounded.

## Backend attachment

Opaque backend-private attachment belongs to Attempt and may be persisted/updated after successful observations. It cannot replace LaunchKey/contact state as control-plane truth and cannot authorize a new lineage.

## Execution observations

Normalized observation:

```text
ABSENT
STARTING
RUNNING
EXITED
UNKNOWN
```

Observation is not a required monotonic phase sequence. Pantheon may first observe an execution after it already exited.

UNKNOWN never creates a replacement Attempt.

## Same Attempt reconciliation

The same Attempt/LaunchKey persists across:

```text
daemon restart
adapter restart
transport/PTy disconnect
session reattach
status retry
backend attachment recovery
```

provided external lineage continuity remains the same.

## New Attempt

Recovery may create another Attempt under the same Run only after the previous Attempt is conclusively terminal/absent and the same immutable Binding/Context remains valid.

New Attempt gets a new LaunchKey and new AgentControlSession.

## Binding/semantic change

Different Agent/Offer/backend/Sandbox strategy or new semantic continuation/rejection context requires Task return to Ready and a **new Run/new Binding**, not a modified old Run.

## Cancellation/finalization

If Task/Goal/security state requests stop while Run is preparing/executing:

```text
Run -> Finalizing
terminalTarget = Cancelled (or Yielded/Failed/Completed as appropriate)
desiredExecution = stopped
```

Run Controller reconciles current Attempt/Sandbox toward safe terminal/release. UNKNOWN preserves/fences capacity.

## Blocking yield

Blocking spawn commits parent Run `Finalizing/terminalTarget=Yielded`. Run Controller stops/reconciles execution, settles Run-scoped Hold/resources, captures WorkspaceRevision and only then transactionally commits:

```text
Run -> Yielded
Task Active -> Waiting
```

Scheduler is not involved until the join later returns Task Ready.

## Candidate

`task.submit_result` comes through Agent Control. T6 performs current Attempt/Run/Task revision checks; cancellation/supersession fence committed first causes conflict. Successful T6:

```text
Candidate durable
Run Active -> Finalizing / terminalTarget=Completed
Task Active -> Evaluating
```

Run Controller then stops/reconciles the producing execution independently of Acceptance evaluation.

## Restart

On daemon restart, Scheduler does not recreate active Runs. Run Controller inventories all nonterminal Runs/Attempts/Sandboxes and reconciles them under rotated control fencing.

The startup recovery barrier prevents new Scheduler dispatch until pre-existing external-effect obligations are either known or fenced according to Global Recovery.

## Core invariants

1. Scheduler never directly launches executor.
2. Task becomes Active in T3 before external execution exists.
3. Preparation belongs to Run Controller and may fail with zero Attempts.
4. Attempt + LaunchKey + AgentControlSession are durable before external backend contact.
5. `CONTACT_MAY_HAVE_OCCURRED` is committed before the launch call.
6. Lost acknowledgement after contact means UNKNOWN, not "try a new Attempt".
7. Backend launch semantics are explicit and unsafe observational routes are filtered before scheduling.
8. Same-lineage recovery reuses Attempt/LaunchKey; fresh execution uses new Attempt only after old is terminal.
9. Run finalization/UNKNOWN retains ownership until safe release.
