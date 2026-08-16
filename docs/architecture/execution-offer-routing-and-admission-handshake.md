# Execution Offer Routing and Admission Handshake

## Status

Canonical Pantheon Agent+Offer routing/admission handshake.

## Central rule

> **Pantheon jointly chooses a semantically eligible Logical Agent and a side-effect-free compatible ExecutionOffer, then proves Sandbox/Resource/Budget/Rate feasibility before atomically freezing one immutable ExecutionBinding.**

## Flow

```text
Ready Task
  ↓
Agent Resolver
  ↓
eligible Logical Agents
  ↓
per-Agent ExecutionRequests
  ↓
Execution Fabric
  ↓
side-effect-free ExecutionOffers
  ↓
ExecutionCandidate = Agent + Offer
  ↓
validate captured ConfigurationRevision
  ↓
Sandbox Planner feasibility
  ↓
Resource incremental-claim feasibility
  ↓
BudgetHold feasibility
  ↓
rate-limit/freshness feasibility
  ↓
RoutePolicy score among feasible candidates
  ↓
selected Agent + Offer + SandboxPlan
  ↓
T3 atomic Run-intent commit
```

## Agent eligibility comes first

Execution routing does not permanently assign an Agent before offers exist, but concrete offers are only requested for Logical Agents that satisfy deterministic semantic eligibility.

Hard Agent eligibility includes configured/current Agent version, Task type, competencies, operator pins/constraints, delegation/policy compatibility and immutable revision validity.

Backend/model identity/capacity/self-reported quality are never Agent semantic eligibility.

## ExecutionRequest

Each eligible Agent receives its own provider-neutral ExecutionRequest frozen from the Task/Agent/current configuration context. It may contain:

```text
required execution features
context-capacity floor
placement/isolation requirements
canonical Agent Control feature requirements
resource requirement declarations
Sandbox profile requirement
configuration component digests
```

It never contains core branches/allowlists for concrete provider/model/harness names.

## ExecutionOffer

Offer creation is side-effect-free. It states factual compatibility and required resources/availability for one specific Request.

Offer can include:

```text
backend + descriptor revision
feature compatibility
factual context capacity
placement
launch semantics (KEYED_IDEMPOTENT|OBSERVATIONAL)
generic resource requirements
metering contract
rate-limit availability/freshness
opaque backend-private offer ref/hash
```

Offer does not:

```text
reserve capacity
create external session
launch execution
grant authorization
choose Agent
self-award semantic quality/scarcity score
```

## Candidate validation

Before scoring/admission Pantheon rejects stale/incompatible candidates if:

- Agent version no longer eligible/current for captured ConfigurationRevision;
- backend descriptor/offer revision invalidated;
- required execution features missing;
- launch semantics unsafe for the Task and no outer idempotent supervisor exists;
- SandboxPlan cannot establish required isolation guarantees;
- current hard authority invalidates the requested strategy.

## SandboxPlan

Sandbox Planner is controller-owned. It converts the Agent/Task/Offer isolation requirements into a concrete immutable SandboxPlan using current SandboxBackend facts.

A backend cannot make itself feasible by claiming security. SandboxPlan feasibility is part of candidate validation before T3.

## Desired versus incremental resources

For a candidate, Pantheon constructs desired effective resource claims from:

```text
Task/Agent/Offer
Workspace
SandboxPlan
scheduler/policy synthetic limits
```

Before Resource Ledger admission it subtracts compatible Task-scoped Reservations already held by the Task. New Run therefore requests only incremental capacity; Task Workspace reservation is not re-created on every Run.

Run-scoped execution/backend/Sandbox/concurrency resources are normally fresh.

## BudgetHold

Feasible candidate must obtain authority for an initial bounded Run BudgetHold tranche across all applicable accounts. This is prospective spend authority, not factual usage.

The Hold is created only in T3 with the selected Binding/Run, not during offer generation.

## Rate limits

Rate-limit state is temporary replenishing availability. A temporarily exhausted otherwise-compatible offer is not selected now; Task remains Ready for future consideration unless other Recovery logic applies.

Rate-limit state is not cumulative budget.

## RoutePolicy

RoutePolicy ranks only already valid/feasible Agent+Offer candidates. Backend supplies facts; Pantheon-owned policy/metrics determine preference.

V1 ranking should remain deterministic/simple. Adaptive quality/scarcity learning is deferred unless required by an implementation issue.

RoutePolicy is an immutable ConfigurationRevision component. Binding stores exact `routePolicyDigest`; it never uses an ambiguous generic `policyHash`.

## Configuration fence

Entire routing cycle captures one `configRevision` from Scheduler eligibility through T3.

Immediately before T3:

```text
active ConfigurationRevision still captured revision?
```

If not, abort/re-run selection. Pantheon never commits Agent eligibility under one config and routing/Sandbox policy under another.

## Atomic T3 commit

Selected candidate does not become execution truth until T3 transaction revalidates and atomically commits:

```text
SchedulingClaim
Task/Goal/Graph/config revisions
Agent+Offer validity
SandboxPlan validity
Resource descriptor revisions
existing Task reservations + incremental resource fit
Budget headroom
hard policy/dispatch fences

create only required incremental Reservations
create initial BudgetHolds
create immutable ExecutionBinding
create Run
Task Ready -> Active
consume SchedulingClaim
append Events/fairness service point
```

External backend/Sandbox/process calls happen later under Run Controller.

## Binding

Binding freezes:

```text
selected Logical Agent/version
ExecutionRequest hash
ExecutionOffer hash
backend/descriptor revision
SandboxPlan digest
ConfigurationRevision
routePolicyDigest
executionProfileDigest
frozen authorization ceiling digest
reservation/initial Hold refs
resolved backend-private execution metadata for audit where appropriate
```

Changing selected Agent/Offer/backend/Sandbox strategy/material context means another Run/new Binding.

## No side effects before commit

Agent Resolution, offer generation, Sandbox feasibility planning, Resource assessment, Budget feasibility and scoring are pure/side-effect-free with respect to external execution. T3 is the durable Run-intent boundary.

## Core invariants

1. Agent semantic eligibility precedes backend offer routing.
2. Offer is side-effect-free and factual; it cannot authorize/launch/score itself.
3. Final selection unit is Agent+Offer, with controller-owned SandboxPlan feasibility.
4. Desired Resource claims are reduced by compatible existing Task-scoped reservations before incremental admission.
5. Budget/Resource/Rate Limit remain distinct feasibility dimensions.
6. One captured ConfigurationRevision covers the whole routing/T3 decision; stale config aborts commit.
7. Binding stores domain-specific config digests, not generic `policyHash`.
8. T3 atomically freezes selected strategy and ownership before any external execution side effect.
