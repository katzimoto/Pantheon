# Execution Offer Routing and Admission Handshake

## Status

Draft design — Pantheon scheduler/execution-fabric subsystem specification.

## Purpose

This subsystem defines the protocol that turns a scheduler claim into a concrete but provider-independent execution decision.

The core rule is:

> **Pantheon first determines eligible Logical Agents, solicits normalized execution offers for each eligible Agent-specific request, then validates and commits one immutable Agent + ExecutionOffer binding.**

Concrete providers, model names, runtimes, CLI harnesses, native session IDs, and backend-specific configuration remain private to `ExecutorBackend` implementations.

See also:

- `docs/architecture/logical-agent-resolution.md`
- `docs/architecture/execution-fabric.md`
- `docs/architecture/budget-usage-and-rate-limits.md`
- `docs/architecture/scheduler-resource-ledger-and-admission.md`
- `docs/architecture/scheduler-task-ordering-and-fairness.md`
- `docs/architecture/scheduler-ready-task-eligibility.md`

## Flow

```text
SchedulingClaim
      │
      ▼
Logical Agent Eligibility
      │
      ├───────────────┐
      ▼               ▼
 Agent A            Agent B
      │               │
      ▼               ▼
ExecutionRequest A  ExecutionRequest B
      │               │
      ▼               ▼
Backend Registry / normalized offers
      │               │
      └───────┬───────┘
              ▼
      Agent + Offer candidates
              │
              ▼
        Offer Validator
              │
              ▼
       Hard feasibility
        ├─ policy
        ├─ metering/budget
        ├─ rate-limit facts
        └─ resource admission
              │
              ▼
          Route Policy
              │
              ▼
    Selected Agent + Offer
              │
              ▼
 atomic resource reservations
       + initial budget holds
       + execution binding
              │
              ▼
             Run
```

## Logical Agent candidate set

The scheduler does not permanently assign one Agent before execution feasibility is known.

The Logical Agent Resolver deterministically produces an eligible candidate set from:

- Task type;
- required Task competencies;
- Agent `accepts` classes;
- Agent competencies;
- hard policy/authorization compatibility;
- enabled/disabled state;
- explicit operator requirements.

Descriptions, skills, tags, examples, and observed Agent history may help ranking but cannot introduce an ineligible Agent.

Each eligible Agent produces its own normalized `ExecutionRequest`.

## ExecutionRequest

An Agent-specific scheduling candidate produces one immutable, revision-bound `ExecutionRequest` from the Task, that Logical Agent, Goal constraints, policy, and workspace requirements.

Conceptually:

```yaml
request:
  id: exec-request_01K...
  task: task_123
  agent: agent://coder

  requirements:
    competencies:
      - code.analysis
      - code.debugging

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

Competencies have already been evaluated by Logical Agent Resolution. Executor backends do not claim semantic competence; they only evaluate execution feasibility for the Agent-specific request.

A Goal, Graph, Agent, or policy change that invalidates the request makes dependent offers stale.

## Backend Registry prefilter

Pantheon uses normalized `BackendDescriptor` information to eliminate obviously incompatible or unhealthy backends before soliciting offers.

Prefiltering may use only portable execution facts such as:

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

A backend may return zero, one, or multiple offers for the same request. Multiple offers let a backend expose different normalized execution alternatives while keeping internal runtime/model choices opaque.

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
    - meter: genai.input_tokens
      expected: 65000
    - meter: genai.output_tokens
      expected: 15000

  metering:
    genai.input_tokens:
      precision: provider-reported
      enforcement: guarded
    genai.output_tokens:
      precision: provider-reported
      enforcement: guarded

  allowanceState:
    state: available
    observedAt: ...

  rateLimits:
    - key: limiter://backend/A/requests
      state: available
      retryAfter: null

  descriptorRevision: 17
  createdAt: ...
  validUntil: ...
```

The exact serialization remains draft. The semantic boundaries are normative.

## Backend trust boundary

Backends report facts they own. They do not decide their own desirability.

Portable offer facts may include:

- supported Execution Features;
- context capacity;
- placement/locality;
- isolation capability;
- normalized resource claims;
- normalized expected usage;
- metering precision/enforcement capability;
- external allowance state and freshness where observable;
- rate-limit state/retry timing where observable;
- current availability;
- validity period.

Backends must not be trusted to self-author canonical fields such as:

- quality score;
- historical reliability;
- global preference;
- scheduler priority;
- scarcity score;
- `recommended` status.

Pantheon owns route preference using policy and observed history.

## RouteMetrics

Observed Agent/executor performance belongs to Pantheon rather than backend-authored offers.

Future RouteMetrics may include dimensions such as:

- logical Agent;
- backend;
- Agent + backend pair;
- Task class/competencies;
- project/domain where statistically justified.

Metrics may include:

- acceptance rate;
- execution failure rate;
- median/percentile latency;
- normalized usage/charge;
- retry/escalation frequency.

These are observations, not self-advertised Agent/backend assertions.

## Offer validation

An Agent + Offer candidate participates in routing only after deterministic validation.

Validation verifies at least:

- Agent remains eligible/current;
- request hash matches the current Agent-specific request;
- backend remains registered;
- descriptor revision is acceptable/current;
- offer has not expired;
- backend health is acceptable;
- required Execution Features remain satisfied;
- placement satisfies hard constraints;
- isolation meets the minimum;
- context requirement is satisfied;
- resource keys/quantities are valid;
- usage estimates are structurally valid where present;
- required budget meters meet configured precision/enforcement requirements;
- externally authoritative allowance state is sufficiently fresh where policy requires it;
- current rate-limit state does not make the offer temporarily unavailable;
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

## Resource, budget, usage, and rate-limit separation

These concepts must remain separate.

### Resource Ledger

Reservable/releasable capacity such as CPU/memory, worktree slots, sandbox slots, backend concurrency, and synthetic global/Goal/Agent concurrency.

### Budget Ledger

Finite cumulative allowances such as token ceilings, currency ceilings, or premium/credit pools where representable.

A Pantheon-authoritative BudgetAccount tracks at least:

```text
limit
consumed
held
available = limit - consumed - held
```

### Usage Ledger

Append-only factual measurements such as normalized token counts, request counts, or wall-time usage. Actual usage is never rewritten or clamped to a configured budget.

### Rate-limit state

Replenishing throughput constraints such as requests/minute or tokens/minute. These are temporary availability signals, not cumulative BudgetAccounts.

Context-window tokens remain execution compatibility, not usage/budget capacity.

## Metering compatibility

A hard budget requirement is itself a hard execution constraint.

Pantheon classifies execution-budget enforcement capability as:

```text
HARD
GUARDED
OBSERVATIONAL
```

If policy requires `HARD`, an offer that can only provide guarded/observational accounting is invalid unless the user/operator explicitly relaxes that requirement.

Missing usage/allowance state is `UNKNOWN`, never zero/free.

## Initial BudgetHolds

The final Run-intent transaction creates **initial budget tranches**, not an entire Goal/project allowance.

Conceptually:

```text
Goal limit = 2m tokens
consumed = 620k
held = 180k

new Run initial hold = 100k
```

The initial hold is sized from policy and normalized usage estimates where useful.

Runs may later request controller-approved hold extensions as described by `budget-usage-and-rate-limits.md`.

The worker/model cannot extend its own allowance.

BudgetHold is not a ResourceReservation:

```text
ResourceReservation
  all released capacity returns after safe release.

BudgetHold
  actual spend remains consumed;
  only unused headroom returns.
```

## Feasibility before ranking

Hard requirements are filters, never scoring terms.

Examples:

- Agent semantic eligibility;
- authorization/policy compatibility;
- locality constraints;
- minimum isolation;
- required Execution Features;
- minimum context capacity;
- required metering precision/enforcement;
- hard BudgetAccount headroom;
- external allowance hard state/freshness where configured;
- structural validity.

Only feasible Agent + Offer candidates participate in route ranking.

Rate-limit exhaustion generally marks a candidate `TEMPORARILY_UNAVAILABLE` rather than permanently infeasible when a retry/reset condition exists.

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
        semanticAgentFit
    - by:
        localityPreference
    - by:
        derivedBudgetScarcity: ascending
    - by:
        historicalLatency: ascending
```

`derivedBudgetScarcity` is a Pantheon policy interpretation of factual accounting/allowance state; it is not backend-authored.

Stable final tie-breaking must be deterministic.

## Admission feedback and deferral

Resource Admission returns normalized states such as:

```text
ADMITTABLE
TEMPORARILY_UNAVAILABLE
UNSATISFIABLE
```

Budget/rate-limit feasibility also returns structured reasons rather than being collapsed into generic routing failure.

Pantheon v1 defaults to **prefer progress**:

> If an acceptable Agent + Offer candidate can execute now, do not idle indefinitely solely for a more-preferred temporarily unavailable candidate.

## Distinct no-route outcomes

At minimum distinguish:

- no eligible Logical Agent;
- eligible Agent but no compatible backend;
- compatible offers but all temporarily resource unavailable;
- all offers resource-unsatisfiable;
- all offers blocked by hard cumulative budget;
- required hard budget cannot be enforced by available offers;
- external allowance exhausted/unknown under hard policy;
- all candidates temporarily rate-limited;
- candidate backends unhealthy/unknown;
- all candidates rejected by policy.

These drive different wakeup/retry/escalation behavior.

## Offer validity

Offers are short-lived snapshots.

Each offer records at least:

- offer ID;
- request hash;
- backend reference;
- backend descriptor revision;
- creation time;
- validity deadline.

Allowance/rate-limit observations may have their own freshness metadata and must be revalidated as policy requires.

A stale offer is regenerated rather than silently reused.

## Atomic commit boundary

Admission assessments are advisory because resource and budget state can change between assessment and commit.

The authoritative decision is a durable transaction that revalidates current state and atomically creates:

- all ResourceReservations;
- initial BudgetHolds;
- immutable ExecutionBinding;
- associated Run intent.

Conceptually:

```text
BEGIN

verify SchedulingClaim current
verify Task/Goal/Graph/Policy snapshots current
verify selected Agent remains eligible
verify selected offer still valid
verify all resource claims still fit
verify all initial BudgetHolds still fit
verify hard metering/budget requirements still satisfied

create resource reservations
create initial budget holds
create ExecutionBinding
create Run intent

COMMIT
```

Any conflict rolls the entire transaction back. There are no network calls inside this transaction.

External backend execution happens only after this durable commit and later Attempt creation.

## ExecutionBinding

The binding freezes the selected **Logical Agent + execution offer** strategy.

Conceptually:

```yaml
binding:
  id: binding_123
  task: task_456

  agent:
    ref: agent://coder
    specHash: sha256:...

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
    initialHolds:
      - budget-hold://...

  routePolicyHash: sha256:...
  policyHash: sha256:...
  decidedAt: ...
```

Once committed, routing is finished for that Run.

Changing Logical Agent or any material execution binding decision requires a new Run.

BudgetHold extensions and factual UsageRecords do not mutate the ExecutionBinding; they are runtime accounting under the already-bound Run.

## Backend-private resolved execution

After binding, the backend may resolve private implementation details such as runtime/model identifier, native session ID, CLI flags, endpoint details, or provider-native billing identifiers.

Pantheon may record these as audit/diagnostic/accounting/learning metadata, but core scheduling semantics remain keyed to the abstract Agent, backend, request, offer and immutable binding.

## Concurrency and races

An `ADMITTABLE` assessment does not guarantee reservation/hold success.

Another scheduler operation may reserve capacity or budget first. Such conflicts are expected and trigger refresh/reassessment rather than corrupting state.

## Key decisions

1. **Logical Agent eligibility is resolved before offer solicitation, but final Agent selection is committed jointly with the ExecutionOffer.**
2. **ExecutionRequest is Agent-specific, immutable, revision-bound, and hashed.**
3. **Backend descriptors provide cheap provider-independent prefiltering.**
4. **Offer generation is side-effect free.**
5. **Backends may return multiple normalized offers while internal execution choices remain opaque.**
6. **Backends report execution/metering/availability facts; Pantheon owns route preference and historical quality metrics.**
7. **Offers are deterministically validated before routing.**
8. **Infrastructure and policy claims augment backend resource claims.**
9. **Resource capacity, cumulative budgets, factual Usage, rate limits, and context capacity remain separate.**
10. **Initial BudgetHolds prevent concurrent overspend; later hold extensions are controller-owned runtime accounting.**
11. **A requested hard budget requires a compatible metering/enforcement path.**
12. **ResourceReservations, initial BudgetHolds, ExecutionBinding, and Run intent are committed atomically.**
13. **Hard constraints filter; preferences rank only feasible Agent + Offer candidates.**
14. **v1 uses explainable ordered routing policy rather than an opaque weighted score.**
15. **v1 prefers immediate acceptable progress over indefinite waiting for a merely preferred unavailable candidate.**
16. **No-route outcomes remain structured and distinguish semantic, resource, budget, rate-limit, health and policy causes.**
17. **Offers are short-lived and bound to request/backend descriptor revisions.**
18. **ExecutionBinding freezes both Agent and execution strategy; changing either requires a new Run.**
