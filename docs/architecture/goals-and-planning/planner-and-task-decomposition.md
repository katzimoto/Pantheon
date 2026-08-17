# Planner and Task Decomposition

## Status

Canonical Pantheon Planner architecture; progressive autonomous planning is post-v1.

## Purpose

Planner converts a Goal revision into a proposed bounded TaskGraph/GraphPatch. It supplies semantic decomposition, not control-plane authority.

> **Planner proposes structure; Pantheon validates/materializes it. Planner never assigns concrete execution backends/models, grants permissions, creates Runs or directly mutates lifecycle state.**

Planner execution is also not allowed to become an untracked second execution plane. Every authoritative planning invocation has one durable `PlanningOperation`; an invocation that crosses an external Planner/backend boundary additionally uses bounded `PlanningAttempt` lineage/contact state before that external call.

## Inputs

Planner invocation receives a bounded immutable snapshot including:

```text
Goal revision
current TaskGraph revision/summary
reconciliation/trigger reason
relevant accepted Artifacts/Evidence
hard decomposition/security/budget ceilings
Planner ContextPolicy/Agent snapshot where applicable
```

It does not rely on hidden chain-of-thought from previous planners/workers.

The exact authoritative planning input is canonicalized/frozen before external contact. A `planning_input_digest` or equivalent immutable reference binds the operation to the Goal/Graph/configuration/input snapshot that the Planner actually observed.

## Output

Planner returns a structured proposal/GraphPatch describing bounded Tasks, dependencies/input bindings and rationale/provenance.

Pantheon validates:

```text
Goal/Graph revision fence
Task schemas/types
boundedness
cycle/dependency legality
security ceiling inheritance
output/input compatibility
policy/decomposition limits
idempotency
```

Only then are Tasks/edges materialized transactionally.

A backend response is never itself Graph authority. Pantheon first parses/normalizes it into an immutable `PlanningRecord` bound to the exact `PlanningOperation`, then Graph Controller re-reads current Goal/Graph/policy state before any GraphPatch commit. A late valid Planner response may remain historical provenance while its proposed patch is rejected as stale.

## V1 planning modes

V1 supports two practical modes:

```text
DIRECT
  Goal is already one bounded Task; Planner proposes one Task/minimal graph.

SHALLOW
  Planner proposes a small useful DAG of bounded Tasks up front.
```

`PROGRESSIVE` autonomous long-horizon decomposition/continuous graph optimization is architecture-reserved but implementation-deferred. Runtime discovery in v1 is handled by the explicitly bounded blocking `task.spawn` protocol rather than a general self-expanding Planner loop.

A purely local/deterministic DIRECT implementation still creates a durable `PlanningOperation`/`PlanningRecord` boundary for revision fencing and audit, but it does not invent a `PlanningAttempt` when no external execution/contact exists.

## Minimum useful decomposition

Planner should create the smallest TaskGraph that exposes real independence/dependency/verification value.

Avoid one Task per trivial implementation step and avoid a single giant Task that hides separable outcomes required for acceptance.

Task is a bounded outcome, not an instruction-by-instruction transcript segment.

## Task requirements

Planner specifies semantic Task fields such as:

```text
type/objective
inputs/output slots
competency requirements
scope/effect constraints
acceptance/evaluator refs
```

Planner does not specify:

```text
concrete provider/model/backend
physical executor slot
actual ResourceReservation
BudgetHold
Run/Attempt
secret credential material
```

The Planner Controller may resolve/configure the concrete Planner Agent/backend for the `PlanningOperation`; that resolution is control-plane state, not Planner output.

## PlanningOperation

`PlanningOperation` is the durable control-plane intent for one exact planning decision. It is not a Run and does not create Task execution authority.

Immutable operation identity/provenance binds at least:

```text
id
Goal ID + GoalRevision
expected GraphRevision
planning trigger / reconciliation identity
planning input digest/reference
Planner Agent snapshot/version where applicable
ConfigurationRevision
resolved external Planner backend + descriptor revision when external
metering contract digest when backend-authored usage is accepted
createdAt
```

The operation owns its bounded control-operation ResourceReservations/BudgetHolds where planning needs them. If backend-authored factual usage is possible, the operation freezes the immutable metering-source binding before external contact exactly as required for other billable control operations.

`PlanningOperation` and `PlanningRecord` are intentionally different:

```text
PlanningOperation
  durable intent/lifecycle/accounting/recovery identity

PlanningRecord
  immutable normalized proposal/result/provenance produced by that operation
```

A crash therefore cannot turn "Pantheon intended to ask the Planner" into "Pantheon has a committed Planner proposal".

## PlanningAttempt and external-contact boundary

An external Planner/model/backend invocation uses `PlanningAttempt` as one bounded external-contact lineage under the operation.

Conceptually:

```text
planningAttempt:
  id
  operation
  ordinal
  state
  contactState: NOT_CONTACTED | CONTACT_MAY_HAVE_OCCURRED
  contactInitiatedAt
  contactDaemonIncarnation
  externalAttachment/correlation
```

At most one nonterminal PlanningAttempt may exist per PlanningOperation.

Correct ordering is:

```text
PlanningOperation committed
        ↓
create PlanningAttempt / NOT_CONTACTED
        ↓
prepare exact immutable planning request
        ↓
T16 commit CONTACT_MAY_HAVE_OCCURRED
        ↓
external Planner/backend invocation
        ↓
reconcile/normalize result
        ↓
PlanningRecord
```

`PlanningAttempt.id` is the provider-neutral correlation/idempotency identity where the external mechanism can preserve/inspect one. Provider-private request IDs or attachments may be stored behind the adapter but do not become Pantheon semantic identity.

Crash semantics are conservative:

```text
NOT_CONTACTED + no independent external evidence
→ Pantheon knows its external Planner call boundary was not crossed

CONTACT_MAY_HAVE_OCCURRED
→ external execution/charge/result may exist
→ reconcile the same PlanningAttempt identity
→ never blindly overlap another Planner invocation
```

If the external mechanism cannot establish the result after ambiguous contact, the PlanningAttempt remains `UNKNOWN`/nonterminal for accounting/recovery purposes until policy/operator resolution can safely conclude it. A replacement PlanningAttempt is allowed only after the prior lineage is definitively terminal/absent and bounded planning retry policy deliberately retries the same operation.

## Planner execution authority

The v1 external Planner path is a structured control-plane invocation, not a normal worker execution session.

By default it receives no:

```text
AgentControlSession
task.spawn / task.graph.propose worker authority
action.invoke
Task Workspace ownership
operator API
ambient shell/host authority
```

The Planner's only semantic output is the bounded proposal consumed by Planner/Graph controllers.

If a future Planner implementation genuinely requires arbitrary executable/shell behavior or a verification Sandbox, that is an explicit new control-operation Sandbox/authority contract with a concrete durable holder edge. It is not inherited implicitly merely because the Planner happens to use a model or backend.

## Replanning triggers

Replanning is event/state-driven, not periodic model polling. Triggers can include:

```text
Goal revision reconciliation
unrecoverable Task failure
Acceptance evidence requiring different work
Join/child failure making plan impossible
structured discovery requiring replacement work
operator request
```

Planner proposes a **patch** against current immutable history. Running/completed TaskSpecs are never edited in place.

A new trigger/revision that materially changes the authoritative planning input creates a new PlanningOperation. It does not rewrite an in-flight operation to mean something else.

## Supersession

If changed Goal/plan makes an existing Task obsolete, Planner may propose a replacement relationship, but controller applies Task supersession through `Finalizing/terminalTarget=Superseded`; Planner never terminalizes a live Task directly.

## Planning budget/resource

Planner execution is a control operation/approved planning mechanism and remains bounded. Planning does not receive unlimited recursive budget merely because it can create more Tasks.

V1 may use a configured Planner Logical Agent/backend path, but the semantic Planner proposal remains provider-neutral and validated before materialization.

PlanningOperation uses the existing control-operation accounting model:

```text
ResourceReservation -> holder PlanningOperation where required
BudgetHold           -> holder PlanningOperation where required
UsageRecord          -> explicit PlanningOperation control-operation provenance
```

There is no Planner-specific quota ledger and no normal Run/ExecutionBinding solely for planning.

## Context and snapshots

Persist Planner proposal/decision summaries and structured rationale/provenance required for audit. Pantheon does not store/require hidden model chain-of-thought.

Large/reference inputs use Artifact/Context Builder mechanisms rather than embedding the entire project into the planning prompt.

The immutable PlanningOperation input identity must be sufficient to determine which Goal/Graph/config/input snapshot the resulting PlanningRecord was derived from.

## Dynamic spawn relationship

Normal workers use `task.spawn` for one bounded blocking child where required. Multi-node structural graph proposals in v1 enter only through the Planner result path:

```text
PlanningOperation
        ↓
PlanningAttempt when external
        ↓
PlanningRecord
        ↓
Graph Controller
        ↓
GraphPatch
```

The external Planner backend itself does not receive worker `task.graph.propose` authority in v1. `task.graph.propose` is reserved post-v1 worker/coordinator vocabulary and is not an alias or alternate transport for a PlanningOperation. If a future executing coordinator receives that worker verb, its lifecycle, yield/join behavior, authority and recovery semantics require a separate explicit architecture correction.

In v1, runtime worker spawn is blocking/yielding only. Joined/detached/semantic-dedup/autonomous graph optimization are deferred.

## Failure and recovery

Planner execution failure is not Goal failure. Recovery may retry/reroute planning, request human input or eventually fail the Goal only when no permitted path remains.

A malformed/stale Planner proposal is rejected without partially mutating Graph state.

Startup/reconciliation inventories nonterminal PlanningOperations/PlanningAttempts together with their unresolved control-operation Reservations/Holds. `CONTACT_MAY_HAVE_OCCURRED` is reconciled against the same external attempt identity where possible; uncertainty retains the relevant accounting/resource fence and never authorizes an overlapping Planner call merely to make progress.

A late successful response may create immutable historical PlanningRecord provenance, but Graph Controller still rejects its GraphPatch if the operation's GoalRevision/GraphRevision/preconditions are stale.

## Core invariants

1. Planner proposes; controller validates/materializes transactionally.
2. Every authoritative planning invocation has one durable PlanningOperation; external planning uses bounded PlanningAttempt contact lineage rather than an untracked backend call.
3. Planner never chooses concrete backend/model, creates Run/Attempt or grants authority; Planner Controller owns external planning resolution.
4. PlanningOperation and PlanningRecord are distinct: intent/lifecycle versus immutable proposal/result.
5. At most one nonterminal PlanningAttempt exists per PlanningOperation; ambiguous contact never authorizes an overlapping external planning call.
6. Backend-authored planning usage is accepted only under immutable PlanningOperation metering-source provenance frozen before contact.
7. External Planner output has zero direct Graph authority; Goal/Graph/policy preconditions are re-read before GraphPatch materialization.
8. V1 external Planner invocation receives no normal AgentControlSession/worker operation authority by default.
9. V1 multi-node structural graph planning enters through PlanningOperation -> PlanningRecord -> GraphPatch; `task.graph.propose` is reserved post-v1 and is not an alternate v1 planning path.
10. V1 planning is DIRECT or SHALLOW; progressive autonomous planning is deferred.
11. Replanning is event-driven and patch-based; immutable Task history is preserved.
12. Runtime dynamic discovery uses bounded blocking spawn rather than unrestricted self-expanding planning.
13. Planner output is structured/auditable without storing hidden chain-of-thought.
