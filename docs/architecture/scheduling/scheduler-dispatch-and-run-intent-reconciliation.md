# Scheduler Dispatch and Run-Intent Reconciliation

## Status

Canonical Scheduler-to-Run-Controller handoff specification.

## Central rule

> **Scheduler ends at a durable atomic Run intent. It never launches the executor or builds runtime context. T3 freezes the Run's exact ExecutionBinding and ContextSourceSnapshot, charges durable Goal fairness, and is permitted only when durable dispatch desired state plus current recovery/configuration gates allow new work. Run Controller owns Workspace/Sandbox/Context preparation, Attempt creation, backend contact and reconciliation.**

## Dispatch permission

Operator dispatch intent and system readiness are separate inputs.

Durable desired state is:

```text
scheduler_state.dispatch_mode = RUNNING | PAUSED
```

Effective permission to commit a new Run is:

```text
dispatch_mode == RUNNING
AND global recovery barrier is open
AND active ConfigurationRevision is published/usable
AND normal Task/Goal/Graph/admission preconditions hold
```

`PAUSED` fences new T3 commits only. It does not cancel existing Runs/Attempts or pretend external work stopped.

The recovery barrier does not rewrite operator desired state. Ordinary daemon restart preserves `PAUSED`; a scheduler that was paused before restart remains paused after recovery completes until an authorized resume command changes the durable desired state.

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
Goal fairness service-sequence charge
Task scheduler backoff normalization
Events
```

The ContextSourceSnapshot binds the exact Task/Goal/Graph revisions, Agent/source versions, starting WorkspaceRevision/continuation inputs where applicable, `ConfigurationRevision + contextPolicyDigest`, and any stable Memory/index/Skill/input generation identities that can affect deterministic Context Builder selection.

T3 freezes **source eligibility**, not the ContextPlan. It performs no Memory retrieval, arbitrary repository exploration, prompt rendering, model call, backend call, or other context-selection side effect.


Immediately before T3 commit, Pantheon revalidates at least:

```text
scheduler_state.dispatch_mode == RUNNING
recovery barrier/current dispatch gate open
active configuration published
Task Ready + SchedulingEligible + expected Task revision
current Goal/Graph authority
SchedulingClaim current
captured ConfigurationRevision still applicable under the configuration snapshot rule
route/admission/resource/budget decisions still valid
ContextSourceSnapshot source identities valid for commit
expected scheduler/fairness state revisions current
```

Implementation status (v0.1.0): the single-writer MVP evaluates every entry above except two. The SchedulingClaim is not acquired because route attempts are in-process and side-effect-free; exclusion comes from the serialized authoritative writer plus the expected scheduler-state revision fence. The recovery barrier does not exist yet, and readiness reports it unimplemented rather than asserting it. Both entries become load-bearing with the missions that introduce expensive resolution and startup recovery.

Immutable source/version refs must still be valid/available according to their domain contract. A source whose required view cannot be named by a stable/reconstructable identity cannot participate in frozen v1 context.

Only after commit can execution preparation begin.

No external backend/process/Sandbox/Git/network/model call occurs inside T3.

## Fairness charge at T3

Goal fairness is charged only when T3 successfully commits service.

The same authoritative transaction that creates the Run performs conceptually:

```text
selected Goal.last_served_sequence
  = scheduler_state.next_service_sequence

scheduler_state.next_service_sequence += 1
```

and clears/normalizes temporary scheduler backoff for the Task whose Run was committed.

Therefore:

```text
routing/admission attempt that fails before T3
→ no fairness charge

T3 rollback/conflict
→ no fairness charge

T3 commit
→ Run + fairness charge durable together
```

A stale concurrent priority/fairness/dispatch mutation causes T3 CAS/revalidation failure and the scheduling decision is recomputed. Event ordering never substitutes for these state checks.

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

A later operator `pause dispatch` affects **new T3 commits only**; it does not invalidate preparation of a Run whose T3 already committed. Existing Run continuation is controlled by Task/Goal/security/recovery authority, not the scheduler queue switch.

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

Scheduler separately reloads durable `scheduler_state`, Goal scheduling state and Task eligibility/backoff state before rebuilding its disposable queue. The startup recovery barrier prevents T3 until pre-existing external-effect obligations are known/fenced, and durable `dispatch_mode=PAUSED` continues to block T3 even after that barrier opens.

Restart never fabricates a fairness service charge from Event history: committed Runs already have the corresponding atomic service-sequence update; pre-T3 attempts do not.

## Core invariants

1. Scheduler never directly launches executor or constructs runtime context.
2. Operator dispatch desired state is durable and separate from the recovery/configuration readiness gates; `PAUSED` survives ordinary restart and forbids new T3 commits.
3. T3 rechecks effective dispatch permission and atomically charges one Goal fairness service sequence only when the Run intent commits.
4. A routing/admission failure or rolled-back T3 never advances fairness.
5. T3 atomically binds each Run to exactly one immutable ContextSourceSnapshot before preparation begins.
6. T3 freezes source/version/generation eligibility but performs no context retrieval/rendering/external call.
7. Task becomes Active in T3 before external execution exists.
8. Preparation belongs to Run Controller and may fail with zero Attempts.
9. Context Builder preparation may retry only against the Run's frozen ContextSourceSnapshot; newer mutable source/config state requires a new Run to affect semantics.
10. ContextReady requires a one-time immutable ContextPlan attachment derived from the exact Run source snapshot.
11. Attempt + LaunchKey + AgentControlSession are durable before external backend contact.
12. `CONTACT_MAY_HAVE_OCCURRED` is committed before the launch call.
13. Lost acknowledgement after contact means UNKNOWN, not "try a new Attempt".
14. Backend launch semantics are explicit and unsafe observational routes are filtered before scheduling.
15. Same-lineage recovery reuses Attempt/LaunchKey; fresh execution uses new Attempt only after old is terminal.
16. A later dispatch pause does not retroactively cancel or invalidate an already-committed Run.
17. Run finalization/UNKNOWN retains ownership until safe release.
