# Scheduler Dispatch and Run-Intent Reconciliation

## Status

Canonical Scheduler-to-Run-Controller handoff specification.

## Central rule

> **Scheduler ends at a durable atomic Run intent. It never launches the executor or builds runtime context. T3 freezes the Run's exact ExecutionBinding and ContextSourceSnapshot; Run Controller owns Workspace/Sandbox/Context preparation, Attempt creation, backend contact and reconciliation.**

## Handoff

After Agent/Offer/Sandbox/Resource/Budget selection, the scheduler/control path resolves and canonicalizes the immutable source identities needed for the Run's `ContextSourceSnapshot` under the same captured ConfigurationRevision used by the scheduling decision.

T3 then commits:

```text
required incremental ResourceReservations
initial BudgetHolds
immutable ExecutionBinding
immutable ContextSourceSnapshot identity
new Run / Run Active
Task Ready -> Active
SchedulingClaim consumed
Events/fairness state
```

The ContextSourceSnapshot binds the exact Task/Goal/Graph revisions, Agent/source versions, starting WorkspaceRevision/continuation inputs where applicable, `ConfigurationRevision + contextPolicyDigest`, and any stable Memory/index/Skill/input generation identities that can affect deterministic Context Builder selection.

T3 freezes **source eligibility**, not the ContextPlan. It performs no Memory retrieval, arbitrary repository exploration, prompt rendering, model call, backend call, or other context-selection side effect.

Immediately before T3 commit, Pantheon revalidates the Task/Goal/Graph/admission authority and the captured ConfigurationRevision according to the configuration snapshot rule. Immutable source/version refs must still be valid/available according to their domain contract. A source whose required view cannot be named by a stable/reconstructable identity cannot participate in frozen v1 context.

Only after commit can execution preparation begin.

No external backend/process/Sandbox/Git/network/model call occurs inside T3.

## Task ownership

Task becomes Active at Run-intent commit, before any backend process exists. Active means the Run is responsible for making progress; it does not mean execution is already running.

V1 allows at most one nonterminal Run per Task.

## Context-source ownership

Every committed Run has exactly one immutable ContextSourceSnapshot identity from T3.

A change after T3 to active ContextPolicy, Memory/index generation, permitted Skill versions, continuation/current source registries, or other selection-affecting mutable state does not rewrite that Run. If such a change must affect execution semantics, Pantheon creates a new Run through the normal lifecycle/recovery path.

If Run Controller restarts before context construction finishes, it retries Context Builder using the same source snapshot. It never substitutes current "latest" source state merely because the ContextPlan has not yet been attached.

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

SandboxReady includes factual verification against SandboxPlan, not merely runtime existence.

ContextReady means:

```text
immutable ContextPlan exists
+
its sourceSnapshot matches the Run's exact T3 ContextSourceSnapshot
+
it has been attached through the one-time RunContextPlan relation
```

Context Builder may deterministically read/select from the frozen source snapshot during preparation. The Run row itself is not mutated to insert a later ContextPlan reference.

## Attempt creation

When LaunchReady:

```text
BEGIN IMMEDIATE
revalidate Run/Task/control/config authority
verify exact one-time ContextPlan attachment is present
create Attempt
assign Run-local ordinal
assign immutable LaunchKey
create Attempt AgentControlSession
set Run.currentAttempt
append Event
COMMIT
```

The active configuration may have advanced since T3; that does not replace the Run's frozen ContextPolicy semantics. Current hard/security authority is still rechecked independently and may block/stop execution.

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

Recovery may create another Attempt under the same Run only after the previous Attempt is conclusively terminal/absent and the same immutable Binding, ContextSourceSnapshot and attached ContextPlan remain valid.

New Attempt gets a new LaunchKey and new AgentControlSession.

## Binding/semantic change

Different Agent/Offer/backend/Sandbox strategy, ContextSourceSnapshot semantics, ContextPlan semantics, or new semantic continuation/rejection context requires Task return to Ready and a **new Run/new Binding/new ContextSourceSnapshot**, not a modified old Run.

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

Scheduler is not involved until the join later returns Task Ready. The subsequent scheduled Run receives a new ContextSourceSnapshot containing the ContinuationContext/current permitted source generations selected for that new Run.

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

A preparing Run also reloads its exact ContextSourceSnapshot and any existing RunContextPlan attachment. Missing ContextPlan is rebuilt against that same snapshot; a newer active source/config generation never silently replaces it.

The startup recovery barrier prevents new Scheduler dispatch until pre-existing external-effect obligations are either known or fenced according to Global Recovery.

## Core invariants

1. Scheduler never directly launches executor or constructs runtime context.
2. T3 atomically binds each Run to exactly one immutable ContextSourceSnapshot before preparation begins.
3. T3 freezes source/version/generation eligibility but performs no context retrieval/rendering/external call.
4. Task becomes Active in T3 before external execution exists.
5. Preparation belongs to Run Controller and may fail with zero Attempts.
6. Context Builder preparation may retry only against the Run's frozen ContextSourceSnapshot; newer mutable source/config state requires a new Run to affect semantics.
7. ContextReady requires a one-time immutable ContextPlan attachment derived from the exact Run source snapshot.
8. Attempt + LaunchKey + AgentControlSession are durable before external backend contact.
9. `CONTACT_MAY_HAVE_OCCURRED` is committed before the launch call.
10. Lost acknowledgement after contact means UNKNOWN, not "try a new Attempt".
11. Backend launch semantics are explicit and unsafe observational routes are filtered before scheduling.
12. Same-lineage recovery reuses Attempt/LaunchKey; fresh execution uses new Attempt only after old is terminal.
13. Run finalization/UNKNOWN retains ownership until safe release.
