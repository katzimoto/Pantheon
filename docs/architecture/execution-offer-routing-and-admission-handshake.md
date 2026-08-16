# Execution Offer Routing and Admission Handshake

## Status

Draft design — Pantheon scheduler/execution-fabric subsystem specification.

## Purpose

This subsystem defines the protocol that turns a scheduler claim into a concrete but provider-independent execution decision.

The core rule is:

> **Backends propose normalized execution offers; Pantheon validates, ranks, admits, and commits one immutable execution binding.**

Concrete providers, model names, runtimes, CLI harnesses, native session IDs, and backend-specific configuration remain private to `ExecutorBackend` implementations.

See also:

- `docs/architecture/execution-fabric.md`
- `docs/architecture/scheduler-resource-ledger-and-admission.md`
- `docs/architecture/scheduler-task-ordering-and-fairness.md`
- `docs/architecture/scheduler-ready-task-eligibility.md`

## Flow

```text
SchedulingClaim
      │
      ▼
ExecutionRequest
      │
      ▼
Backend Registry
      │
      ├─────────┬─────────┐
      ▼         ▼         ▼
 Backend A   Backend B   Backend C
      │         │         │
      ▼         ▼         ▼
   Offer A    Offer B    Offer C
      └─────────┼─────────┘
                ▼
          Offer Validator
                │
                ▼
       Effective Claim Builder
          │             │
          ▼             ▼
   Resource claims   Budget claims
          │             │
          ▼             ▼
      Admission      Budget check
          └──────┬──────┘
                 ▼
            Route Policy
                 │
                 ▼
          Selected Offer
                 │
                 ▼
 atomic resource reservations
       + budget holds
       + execution binding
                 │
                 ▼
               Run
```

## ExecutionRequest

A scheduling attempt produces one immutable, revision-bound `ExecutionRequest` from the Task, selected logical Agent, Goal constraints, policy, and workspace requirements.

Conceptually:

```yaml
request:
  id: exec-request_01K...
  task: task_123
  agent: coder

  requirements:
    taskCapabilities:
      - code-analysis
      - code-editing

    executionFeatures:
      - session.interactive
      - tools.structured

    context:
      minTokens: 64000

    placement:
      locality: any

    isolation:
      minimum: workspace

  snapshots:
    taskRevision: 17
    goalRevision: 8
    graphRevision: 42
    agentSpecHash: sha256:...
    policyHash: sha256:...
```

The canonical representation is hashed. Every offer must bind to the request hash.

A Goal, Graph, Agent, or policy change that invalidates the request makes dependent offers stale.

## Backend Registry prefilter

Pantheon should use normalized `BackendDescriptor` information to eliminate obviously incompatible or unhealthy backends before soliciting offers.

Prefiltering may use only portable facts such as:

- required Execution Features;
- placement/locality;
- minimum isolation support;
- context capacity;
- backend health.

No core branch may inspect a concrete provider, model, runtime, or harness name.

## Offer generation

Calling a backend to obtain offers is quotation, not execution.

Offer generation must not:

- launch an agent/model session;
- create a worktree;
- create a container/VM;
- consume a durable execution slot;
- create irreversible external state.

A backend may return zero, one, or multiple offers for the same request. Multiple offers let a backend expose different normalized execution alternatives while keeping its internal runtime/model choices opaque.

Conceptually:

```yaml
offer:
  id: offer_A1
  backend: executor://A
  requestHash: sha256:...

  execution:
    features:
      - session.interactive
      - tools.structured
    contextTokens: 128000
    placement:
      locality: local
    isolation:
      level: workspace

  resourceClaims:
    - resource: resource://host/default/memory
      quantity: 12Gi

  usageEstimate:
    - budget: budget://goal/goal-123/tokens
      expected: 80000
      maximum: 150000

  descriptorRevision: 17
  createdAt: ...
  validUntil: ...
```

## Backend trust boundary

Backends report facts they own. They do not decide their own desirability.

Portable offer facts may include:

- supported Execution Features;
- context capacity;
- placement/locality;
- isolation capability;
- normalized resource claims;
- normalized expected usage;
- current availability;
- validity period.

Backends must not be trusted to self-author canonical fields such as:

- quality score;
- historical reliability;
- global preference;
- scheduler priority;
- "recommended" status.

Pantheon owns route preference using policy and observed history.

## RouteMetrics

Observed executor performance belongs to Pantheon rather than backend-authored offers.

Future `RouteMetrics` may include dimensions such as:

- backend;
- logical Agent;
- Task class;
- Execution Features;
- project/domain where statistically justified.

Metrics may include:

- acceptance rate;
- execution failure rate;
- median/percentile latency;
- normalized token/cost usage;
- retry/escalation frequency.

These are observations, not backend assertions.

## Offer validation

An offer participates in routing only after deterministic validation.

Validation must verify at least:

- request hash matches the current request;
- backend remains registered;
- descriptor revision is acceptable/current;
- offer has not expired;
- backend health is acceptable;
- required Execution Features remain satisfied;
- placement satisfies all hard constraints;
- isolation meets the minimum;
- context requirement is satisfied;
- resource keys/quantities are valid;
- budget estimates/ceilings are structurally valid;
- current policy permits the execution configuration.

Unknown mandatory state fails closed.

## Effective resource claims

Backend claims are only one input to admission.

Pantheon controllers augment them with claims implied by infrastructure and policy:

```text
backend claims
+ workspace claims
+ sandbox claims
+ global concurrency claims
+ Goal concurrency claims
+ Agent concurrency claims
= EffectiveResourceClaimSet
```

The backend need not know about Pantheon Goal/Agent concurrency policy.

## Resource admission and budget admission

Resource capacity and consumable budgets are intentionally separate.

### Resource Ledger

Reservable/releasable capacity such as:

- CPU/memory;
- worktree slots;
- sandbox slots;
- backend concurrency;
- synthetic global/Goal/Agent concurrency resources.

### Budget Ledger

Consumable ceilings such as:

- token budget;
- monetary/cost budget;
- premium-use allowance where representable.

Context-window tokens are compatibility, not budget consumption.

Actual token counts are usage metrics.

## Budget holds

Concurrent Runs must not each assume the entire remaining Goal budget is available.

Before execution Pantheon creates a `BudgetHold` for the maximum amount that policy is willing to let a Run consume.

Conceptually:

```text
budget limit = 200k
consumed     = 60k
held         = 80k
available    = 60k
```

As a Run consumes budget:

```text
held → consumed
```

When the Run ends, unused held budget returns to `available`; consumed budget remains consumed.

A Budget Hold is not a Resource Reservation:

```text
ResourceReservation:
capacity returns when released

BudgetHold:
unused hold returns;
actual spend remains consumed
```

## Feasibility before ranking

Hard requirements are filters, never scoring terms.

Examples of hard filters:

- authorization/policy compatibility;
- locality constraints;
- minimum isolation;
- required Execution Features;
- minimum context capacity;
- hard budget ceilings;
- structural validity.

Only feasible offers participate in route ranking.

## Routing policy

Pantheon v1 should prefer an explainable ordered policy over one opaque weighted score.

Conceptually:

```yaml
routePolicy:
  prefer:
    - when:
        risk: high
      by:
        historicalAcceptance: descending
    - by:
        localityPreference
    - by:
        budgetScarcity: ascending
    - by:
        historicalLatency: ascending
```

Stable final tie-breaking must be deterministic, for example by `offerId`.

The exact policy vocabulary is a later routing-subsystem decision; this specification fixes only the separation between hard filtering and preference ranking.

## Admission feedback and deferral

Resource Admission returns normalized states such as:

```text
ADMITTABLE
TEMPORARILY_UNAVAILABLE
UNSATISFIABLE
```

A preferred offer may therefore be temporarily unavailable while a less-preferred feasible offer can run immediately.

Choosing whether to wait is a route/scheduling policy decision, not an Admission decision.

Pantheon v1 should default to **prefer progress**:

> If an acceptable feasible offer can execute now, do not idle indefinitely solely for a more-preferred temporarily unavailable offer.

## Distinct no-route outcomes

Do not collapse all failures into `routing failed`.

At minimum distinguish:

- no compatible backend;
- compatible offers but all temporarily unavailable;
- all offers unsatisfiable;
- all valid offers blocked by budget;
- candidate backends unhealthy/unknown;
- all offers rejected by policy.

These outcomes drive different wakeup/retry/escalation behavior later.

## Offer validity

Offers are short-lived snapshots.

Each offer records at least:

- offer ID;
- request hash;
- backend reference;
- backend descriptor revision;
- creation time;
- validity deadline.

A stale offer is discarded and regenerated rather than silently reused.

## Atomic commit boundary

Admission assessments are advisory because capacity can change between assessment and commit.

The authoritative decision is a durable transaction that revalidates current state and atomically creates:

- all Resource Reservations;
- all Budget Holds;
- the immutable `ExecutionBinding`;
- the associated Run intent (defined by the Run subsystem).

Conceptually:

```text
BEGIN

verify SchedulingClaim current
verify Task/Goal/Graph/Policy snapshots current
verify selected offer still valid
verify all resource claims still fit
verify all budget holds still fit

create resource reservations
create budget holds
create ExecutionBinding
create Run intent

COMMIT
```

Any conflict rolls the entire transaction back.

External backend launch happens only after this durable commit.

## ExecutionBinding

The binding freezes the selected execution decision.

Conceptually:

```yaml
binding:
  id: binding_123
  task: task_456

  request:
    id: request_789
    hash: sha256:...

  selectedOffer:
    id: offer_321
    hash: sha256:...

  backend:
    ref: executor://A
    descriptorRevision: 17

  resources:
    reservations:
      - reservation://...

  budgets:
    holds:
      - budget-hold://...

  routePolicyHash: sha256:...
  policyHash: sha256:...
  decidedAt: ...
```

Once committed, routing is finished for that Run.

If Pantheon later chooses a different execution configuration after failure/retry/escalation, it creates a new Run with a new request/offers/binding rather than mutating the old binding.

## Backend-private resolved execution

After binding, the backend may resolve private implementation details such as:

- runtime/model identifier;
- native provider session ID;
- CLI flags;
- endpoint details.

Pantheon may record these as audit/diagnostic/learning metadata, but core scheduling semantics remain keyed to the abstract backend and immutable binding.

## Concurrency and races

An `ADMITTABLE` assessment does not guarantee reservation success.

Another scheduler attempt may reserve capacity first. Reservation conflicts are expected concurrency outcomes and should trigger refresh/reassessment rather than corrupting state.

## Key decisions

1. **ExecutionRequest is immutable, revision-bound, and hashed.**
2. **Backend descriptors provide cheap provider-independent prefiltering.**
3. **Offer generation is side-effect free.**
4. **Backends may return multiple normalized offers while internal execution choices remain opaque.**
5. **Backends report execution facts; Pantheon owns route preference and historical quality/reliability metrics.**
6. **Offers are deterministically validated before routing.**
7. **Infrastructure and policy claims augment backend resource claims.**
8. **Resource Admission and Budget Admission are separate.**
9. **Tokens/cost use Budget Holds; context-window capacity is execution compatibility; actual tokens are usage.**
10. **Resource Reservations, Budget Holds, ExecutionBinding, and Run intent are committed atomically.**
11. **Hard constraints filter; preferences rank only feasible offers.**
12. **v1 uses explainable ordered routing policy rather than an opaque weighted score.**
13. **v1 prefers immediate acceptable progress over indefinite waiting for a merely preferred unavailable offer.**
14. **No-route outcomes remain structured and distinct.**
15. **Offers are short-lived and bound to request/backend descriptor revisions.**
16. **ExecutionBinding is immutable; changing execution configuration requires a new Run.**
