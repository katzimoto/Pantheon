# Run and Attempt

## Status

Canonical Pantheon execution lifecycle specification.

## Purpose

Pantheon separates semantic work from resolved execution strategy and concrete execution lineage:

```text
TASK
bounded semantic outcome
   ↓
RUN
one immutable resolved execution strategy
   ↓
ATTEMPT
one logical backend execution lineage
```

The core rule is:

> **A Run owns one immutable ExecutionBinding, one immutable ContextSourceSnapshot frozen at T3, and at most one immutable initial ContextPlan attached during preparation. An Attempt owns one logical backend-execution lineage identified by one immutable LaunchKey and one Attempt-scoped AgentControlSession.**

This produces four distinct recovery levels:

```text
RECONCILE
same Attempt / same LaunchKey

RETRY EXECUTION
new Attempt / same Run / same Binding / same ContextPlan

REROUTE OR SEMANTIC RETRY
new Run / new Binding / new ContextSourceSnapshot / new ContextPlan

REPLAN
new/superseding Tasks / graph mutation
```

See also:

- `task-lifecycle.md`
- `blocking-spawn-and-run-yield.md`
- `agent-control-channel.md`
- `context-builder.md`
- `sandbox-broker-and-isolation.md`
- `recovery-policy.md`

## Run identity

A Run answers:

> What exact resolved execution strategy and eligible semantic source universe has Pantheon committed to for this Task now?

Its immutable specification contains/references at least:

- Task/Goal/Graph revisions relevant to the decision;
- selected Logical Agent version;
- immutable ExecutionBinding;
- frozen configuration component digests used by the Binding;
- immutable ContextSourceSnapshot, including `configRevision + contextPolicyDigest` and other source/version/generation identities;
- SandboxPlan/security ceiling;
- Task Workspace reference and starting WorkspaceRevision where applicable.

The ContextPlan does **not** need to exist when the immutable Run row commits. It is produced later during preparation from the already-bound ContextSourceSnapshot and attached exactly once through a separate immutable Run-to-ContextPlan relation. This avoids mutating the Run specification after T3 while still making the initial ContextPlan part of the Run's durable execution history.

Material changes to these semantic/binding-level inputs require a new Run. Examples:

- a different Logical Agent;
- a different ExecutionOffer/backend;
- changed execution requirements/profile;
- Acceptance rejection feedback;
- blocking-child ContinuationContext;
- changed semantic Memory/Skill/context-source generation;
- changed ContextPolicy semantics;
- rerouting/escalation.

Two Runs remain distinct even if they later choose the same Agent/backend or happen to produce byte-identical ContextPlans.

## Context preparation boundary

Context-source freezing and ContextPlan construction are deliberately separate:

```text
pre-T3
resolve/canonicalize ContextSourceSnapshot inputs
        ↓
T3
commit immutable Run + exact ContextSourceSnapshot identity
        ↓
preparation
Context Builder reads only that source snapshot
        ↓
persist immutable ContextPlan
        ↓
one-time RunContextPlan attachment
        ↓
ContextReady=True
```

T3 performs no Memory retrieval, repository exploration, model/backend call, prompt rendering, or other external context-construction work.

If Pantheon crashes after T3 but before ContextPlan attachment, preparation retries against the same ContextSourceSnapshot. It may not query newer mutable source generations or the current active ContextPolicy and silently attach a different semantic plan to the same Run.

If the required frozen source generation is unavailable, preparation fails/reconciles the Run according to policy; substitution of a newer source generation requires a new Run.

## Attempt identity

An Attempt answers:

> What concrete external execution lineage are we controlling under this Run's exact strategy?

Attempt identity is not an OS PID. The same Attempt may survive:

- Pantheon daemon restart;
- adapter restart;
- PTY/session reconnect;
- transport loss;
- status retry;
- native session reattachment;
- persisted backend-private attachment recovery.

As long as execution continuity is the same, Pantheon reconciles the same Attempt/LaunchKey.

## LaunchKey and launch semantics

Every Attempt receives one immutable `LaunchKey` before any external execution side effect.

Pantheon classifies each concrete backend's launch semantics factually:

```text
KEYED_IDEMPOTENT
  repeated ensure/recover for the same LaunchKey is guaranteed to address one execution lineage

OBSERVATIONAL
  backend lacks a trustworthy create-idempotency/lookup primitive; after ambiguous contact Pantheon can only observe/reconcile conservatively
```

Pantheon may provide `KEYED_IDEMPOTENT` semantics itself when it owns the lower-level process/session supervisor even if the wrapped harness has no native idempotency token.

An `OBSERVATIONAL` offer is ineligible where an ambiguous duplicate execution could violate the Task/Run safety envelope and no outer mechanism can prevent the duplicate. Pantheon never pretends an adapter is idempotent merely because retry usually works.

## Durable pre-launch contact marker

Pantheon must distinguish:

```text
Attempt exists, backend definitely never contacted
```

from:

```text
Pantheon may have crossed the external-call boundary
```

Before the first `ensureExecution`/create contact, Run Controller commits a durable contact marker on the Attempt, conceptually:

```text
launchContactState = NOT_CONTACTED | CONTACT_MAY_HAVE_OCCURRED
launchContactInitiatedAt
launchContactControllerEpoch/incarnation
```

Correct ordering:

```text
T4 create Attempt + LaunchKey + AgentControlSession
COMMIT

if the original raw Agent Control bearer is unavailable after restart:
  T4a pre-contact Agent Control rekey
  rebuild exact credential delivery/projection

prepare exact launch request

T4b BEGIN IMMEDIATE
verify same current Attempt/Run/control authority
verify AgentControlSession current generation/revision
set CONTACT_MAY_HAVE_OCCURRED
append Event
COMMIT

external ensureExecution(...)
```

A crash before the marker proves Pantheon never crossed the launch-call boundary for that Attempt. A crash after the marker is conservatively ambiguous: the backend may or may not have received the request. Pantheon reuses the same LaunchKey and reconciles; it does not create a replacement Attempt merely because no acknowledgement exists.

The marker is deliberately conservative: it can produce UNKNOWN even when the call was never actually delivered, but it cannot falsely prove absence after a potentially delivered launch.

## Pre-contact Agent Control credential rekey

The raw Agent Control bearer is intentionally not durable. Therefore a daemon may restart after T4 committed but before T4b while the Attempt is still safely `NOT_CONTACTED` and the original bearer exists only in lost process memory.

Pantheon may recover this exact pre-launch Attempt by rekeying the **existing** AgentControlSession, not by creating another Attempt/session.

T4a is permitted only when all of the following are true in one authoritative transaction:

```text
same current RestoreGeneration
AgentControlSession == ACTIVE
AgentControlSession.restoreGeneration == current RestoreGeneration
Attempt is current and nonterminal
Attempt.launchContactState == NOT_CONTACTED
no independent evidence that launch-capable external execution received the bearer
current Run/ControlLease authority is valid
```

Pantheon generates a new high-entropy bearer, atomically replaces the persisted verifier, increments the session `credentialRevision`, and records non-secret rekey provenance. The raw new bearer exists only in protected transient launch state.

A pre-contact rekey invalidates every previously prepared credential projection/package for that Attempt. Before T4b, Pantheon must rebuild or replace any sandbox-local credential file, inherited descriptor setup, adapter bootstrap object, or equivalent delivery material so the exact launch request carries only the current credential revision. Stale prepared credential material is never considered execution authority.

The transition boundary is strict:

```text
NOT_CONTACTED
  → current-generation pre-launch rekey is allowed

CONTACT_MAY_HAVE_OCCURRED
  → AgentControlSession credential verifier/revision freezes
  → no same-Attempt rekey to recover lost bearer material
```

If contact may have occurred, Pantheon reconciles the existing external lineage. If that lineage is later proven absent/terminal and fresh execution is desired, Recovery Policy creates the normal new Attempt with a new LaunchKey and AgentControlSession; it does not rotate the old contacted Attempt's credential to relaunch it.

Disaster restore does not use T4a to promote an old-generation session. A session whose `restoreGeneration` differs from the current installation generation remains fenced as defined by the restore architecture.

## Attempt creation

A Run exists before any Attempt. Preparation occurs first:

```text
Run committed with ContextSourceSnapshot
  ↓
WorkspaceReady
SandboxReady
ContextPlan attached exactly once / ContextReady
PolicyReady
  ↓
LaunchReady
  ↓
T4 create Attempt + ordinal + LaunchKey + AgentControlSession
COMMIT
  ↓
optional T4a pre-contact rekey after crash/restart
  ↓
T4b commit launch-contact marker
  ↓
ensure/reconcile external execution
```

Preparation failure may conclude/recover the Run without any Attempt.

## Agent Control identity

Each Attempt owns one AgentControlSession. A new Attempt gets a new session identity and initial credential revision. Ordinary reconciliation of the same Attempt across daemon/adapter restart retains the same session identity.

Before external contact, loss of the raw bearer may rotate only the session credential verifier/revision under T4a; this is not a new execution lineage. After contact may have occurred, the credential revision is frozen and reconciliation preserves the credential verifier expected by any potentially running worker.

When an Attempt terminalizes or authority is revoked, its AgentControlSession is revoked. Late requests from an old Attempt cannot mutate current Task/Run state.

## UNKNOWN execution

Normalized observations include:

```text
ABSENT
STARTING
RUNNING
EXITED
UNKNOWN
```

`UNKNOWN` means Pantheon cannot establish whether execution still exists. While UNKNOWN:

- Attempt remains nonterminal;
- Run remains responsible/finalizing as appropriate;
- relevant ResourceReservations remain consumed/UNCERTAIN;
- unresolved Budget authority remains fenced conservatively;
- Pantheon retries inspection/reattachment/recovery;
- no replacement Attempt/Run is created merely to make progress.

A launch contact marker plus backend-specific observation determines whether Pantheon may conclude `ABSENT`; lack of acknowledgement alone never proves absence after contact may have occurred.

## New Attempt versus new Run

Mechanical boundary:

```text
same immutable Binding + same ContextSourceSnapshot + same attached ContextPlan?
  yes + definitively fresh execution needed -> new Attempt
  no -> new Run
```

New Attempt is allowed only after the prior Attempt is conclusively terminal/absent and Recovery Policy intentionally retries the same Binding.

New Run is used for rerouting, Agent change, Acceptance feedback, blocking continuation, changed semantic evidence/source generation or other material execution-strategy changes.

## One nonterminal Attempt

V1 permits at most one nonterminal Attempt per Run. There is no speculative/hedged duplicate execution.

## Run lifecycle

Nonterminal:

```text
Active
Finalizing
```

Terminal:

```text
Completed
Failed
Cancelled
Yielded
```

### Active

The Run owns semantic execution responsibility. It may be preparing, LaunchReady, reconciling/executing an Attempt, or between sequential Attempts while Recovery Policy decides.

### Finalizing

The Run has stopped producing new semantic work and is converging to one durable `terminalTarget`:

```text
Completed
Failed
Cancelled
Yielded
```

`run_status.terminalTarget` is written durably before/with the transition to Finalizing. Finalization then stops/reconciles execution, revokes semantic Agent Control authority, settles usage/budget, releases eligible Run-scoped resources and preserves required Workspace/Candidate state.

Crash-mid-Finalizing is therefore deterministic: the controller continues toward the recorded target.

### Completed

The Run successfully submitted its one immutable Candidate and finished execution responsibility.

**Invariant: `Run Completed => exactly one Candidate`.**

Task Acceptance may still reject that Candidate; Run completion is not Task success.

### Failed

The Run ended without a usable Candidate and Recovery Policy concluded that no further Attempt under this exact Binding should occur.

### Cancelled

The Run was intentionally stopped because enclosing authority cancelled/superseded/invalidated it.

### Yielded

The Run intentionally returned execution capacity because its Task depends on a durable blocking child/join. Yielded is not failure, consumes no recovery retry quota and has **zero Candidate**.

After safe yield finalization, Task becomes Waiting with zero nonterminal Runs. Join satisfaction later returns the Task to Ready and the Scheduler creates a new Run with ContinuationContext.

## Candidate submission

A Run may submit at most one Candidate. `task.submit_result` must pass the Agent Control/current-authority preconditions defined in `task-lifecycle.md`.

Atomic submission performs/rechecks at least:

```text
current Attempt/AgentControlSession
Run Active + current responsible Run
Task Active + expected Task revision
no committed cancellation/supersession/finalization fence
Candidate/Artifact structural validity
```

Then transactionally:

```text
create immutable Candidate
Run Active -> Finalizing / terminalTarget=Completed
Task Active -> Evaluating
append Events
```

Cancellation/supersession wins if its authoritative fence committed first. A later submit request fails conflict/stale-authority. If Candidate submission committed first, the Candidate remains immutable history even if cancellation follows.

## Acceptance rejection

Acceptance rejection never retroactively changes `Run Completed` to Failed. The Candidate/Evidence remain history.

Recovery may choose `REQUEUE_TASK`, but Task cannot become Ready until the producing Run is terminal. The next execution is a new Run/new Binding/new ContextSourceSnapshot because rejection evidence changes semantic context.

## Blocking yield

Blocking spawn first commits child materialization/join plus parent Run `Finalizing/terminalTarget=Yielded`. Task stays Active until current execution is safely stopped and Run-scoped resources are settled.

Only then does the final yield transaction commit:

```text
Run -> Yielded
Task Active -> Waiting
Workspace checkpoint/freeze established
```

UNKNOWN parent execution blocks yield completion.

## Workspace and Sandbox

Task Workspace normally survives Runs/Attempts. SandboxInstance is normally Run-scoped; sequential Attempts may reuse it only while its state is known and policy permits.

A new Run normally creates a fresh SandboxInstance, ContextSourceSnapshot and ContextPlan even if it keeps the Task Workspace.

## Usage

Concrete execution usage originates at Attempt level and aggregates upward. Failed Attempts consume real usage. Reconciliation loops are not retries and do not erase/duplicate factual usage.

Usage provenance/idempotency rules are defined by `budget-usage-and-rate-limits.md`; a late valid usage observation may remain factual even after controller ownership changes.

## Cancellation, supersession and current authority

When enclosing state invalidates a Run:

```text
Run desiredExecution -> stopped
Run -> Finalizing / terminalTarget=Cancelled
```

Current Attempt is reconciled toward termination. UNKNOWN remains nonterminal/fenced. A terminal Run never reopens.

## Core invariants

1. One immutable ExecutionBinding per Run.
2. Exactly one immutable ContextSourceSnapshot is bound to each Run at T3 before Context Builder selection.
3. At most one immutable initial ContextPlan may attach to a Run; when present it must derive from that Run's exact ContextSourceSnapshot and can never be replaced.
4. A preparation retry for the same Run cannot silently switch to a newer ContextPolicy, Memory/index generation, Skill version set or other semantic source generation.
5. At most one Candidate per Run.
6. `Run Completed => exactly one Candidate`.
7. `Run Yielded => zero Candidate` and no retry charge.
8. Every Attempt has one immutable LaunchKey and one Attempt-scoped AgentControlSession identity.
9. At most one nonterminal Attempt per Run.
10. Launch-contact intent is durable before the first external launch call.
11. A current-generation AgentControlSession may rekey only while its Attempt is durably `NOT_CONTACTED` and no independent launch-capable external contact evidence exists.
12. T4a keeps the same Attempt/session identity, increments only the credential revision, and invalidates/rebuilds all prepared bearer delivery before T4b.
13. After `CONTACT_MAY_HAVE_OCCURRED`, the Agent Control credential verifier/revision is frozen; lost bearer material never authorizes same-Attempt rekey/relaunch.
14. UNKNOWN external execution never creates replacement execution.
15. Fresh execution under the same Binding creates a new Attempt only after the prior Attempt is definitively terminal and the same ContextSourceSnapshot/ContextPlan remain valid.
16. Binding/semantic context-source change creates a new Run.
17. Every Finalizing Run has a durable terminalTarget.
