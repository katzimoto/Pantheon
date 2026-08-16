# Scheduler Resource Ledger and Admission

## Status

Draft design — Pantheon scheduler S2 specification.

## Purpose

Pantheon admission is a generic resource-fit engine. It operates on normalized resource claims emitted by the Execution Fabric and Pantheon infrastructure/policy controllers. It does not contain provider-, harness-, runtime-, or model-specific logic.

The core rule is:

> Admission receives a normalized resource claim set and answers whether the entire set fits the current resource ledger. It does not know why the resources are required.

## Relationship to the Execution Fabric

```text
ExecutionRequest
      ↓
ExecutorBackend
      ↓
ExecutionOffer
      │
      ├─ backend-generated resource claims
      │
      ▼
Claim augmentation
      ├─ workspace claims
      ├─ sandbox claims
      └─ policy/concurrency claims
              │
              ▼
     Effective ResourceClaimSet
              │
              ▼
        Admission Engine
              │
              ▼
      AdmissionAssessment
              │
              ▼
   Atomic reservation (S5)
```

Admission evaluates an `ExecutionOffer`, not the semantic Task directly. Backend-specific resource estimation and implementation knowledge remain behind the Execution Fabric boundary.

## Resource keys

A `ResourceKey` is opaque to Admission.

Examples:

```text
resource://host/default/cpu
resource://host/default/memory
resource://workspace/default/worktree
resource://sandbox/isolated/instance
resource://backend/executor-17/concurrency
resource://limit/global/runs
resource://limit/goal/goal-123/runs
```

Pantheon core must not branch on the semantic identity of backend-owned resource keys.

## Resource descriptors

Resource meaning and accounting properties are declared by a resource descriptor rather than hard-coded into Admission.

Conceptual example:

```yaml
resource:
  key: resource://workspace/default/worktree
  owner:
    ref: controller://workspace
  quantity:
    unit: count
    granularity: 1
  allocation:
    mode: discrete
  capacity: 8
  allocatable: 8
  reserved: 5
  health: available
  revision: 42
```

A divisible resource might look like:

```yaml
resource:
  key: resource://host/default/memory
  owner:
    ref: controller://host
  quantity:
    unit: bytes
    granularity: 1Mi
  allocation:
    mode: divisible
  capacity: 48Gi
  allocatable: 36Gi
  reserved: 18Gi
  health: available
  revision: 103
```

These serializations are conceptual and are not yet frozen as public schemas.

## Allocation modes

v1 supports only two allocation modes:

```text
DIVISIBLE
DISCRETE
```

### Divisible

The resource is allocated in quantities subject to a declared granularity.

Typical examples are host memory, CPU accounting capacity, or temporary storage.

### Discrete

The resource is allocated in integer units.

Typical examples are worktree capacity, isolated-environment capacity, backend concurrency, and synthetic concurrency limits.

An exclusive resource is represented as a discrete resource with `allocatable = 1` and a claim of `1`; no separate exclusive-resource primitive is needed.

## Capacity and allocatable

Admission uses `allocatable`, never raw physical/configured `capacity`.

```text
capacity
  - owner/system reserve
  = allocatable

allocatable
  - active reservations
  = available
```

Resource owners decide how much of their underlying capacity is exposed to Pantheon.

## Resource publishers

Admission does not probe external systems itself. Resource-owning components publish normalized descriptors into the Resource Ledger.

Examples:

```text
Host controller
  → host resources

Workspace controller
  → workspace resources

Sandbox controller
  → isolation resources

ExecutorBackend
  → backend-owned resources

Scheduler/policy controller
  → synthetic concurrency-limit resources
```

The Resource Ledger is the normalized control-plane view of allocatable capacity.

## Resource claims

A normalized claim is intentionally small:

```yaml
claim:
  resource: resource://host/default/memory
  quantity: 12Gi
```

The resource-producing component is responsible for translating implementation knowledge and uncertainty into a safe concrete quantity before the claim reaches Admission.

Admission does not apply provider/model-specific estimation formulas or safety factors.

## Complete resource footprint

An `ExecutionOffer` must expose its complete resource footprint that is relevant to Pantheon allocation.

If an executor consumes both backend-owned capacity and shared host capacity, it claims both:

```yaml
resourceClaims:
  - resource: resource://backend/executor-17/concurrency
    quantity: 1
  - resource: resource://host/default/memory
    quantity: 12Gi
```

Pantheon core must never infer hidden backend resource requirements from backend identity.

## Claim augmentation

Backend claims are only part of the effective allocation requirement.

Pantheon controllers may add generic claims for infrastructure and policy requirements:

```text
backend claims
+ workspace claims
+ sandbox claims
+ scheduler/policy claims
= Effective ResourceClaimSet
```

Example:

```yaml
claims:
  - resource: resource://backend/executor-17/concurrency
    quantity: 1
  - resource: resource://workspace/default/worktree
    quantity: 1
  - resource: resource://limit/global/runs
    quantity: 1
  - resource: resource://limit/goal/goal-123/runs
    quantity: 1
```

This preserves boundaries: a backend does not need to know Goal concurrency policy, and the scheduler does not need to understand backend internals.

## Concurrency limits as synthetic resources

Concurrency limits reuse the generic resource ledger and reservation mechanism.

Examples:

```text
resource://limit/global/runs
  allocatable = 8

resource://limit/goal/goal-123/runs
  allocatable = 3

resource://limit/agent/researcher/runs
  allocatable = 2
```

Each matching Run claims `1` unit.

These remain semantically policy limits, not physical resources; only the accounting/reservation machinery is shared.

## No overcommit in v1

Pantheon v1 does not model strict/soft/burstable/overcommit admission classes.

For every claimed resource:

```text
reserved + requested <= allocatable
```

must hold before a candidate can be admitted.

Runtime enforcement and actual usage are separate from admission accounting.

## Admission is pure

Admission does not mutate the Resource Ledger and does not reserve resources.

Conceptually:

```text
assess(ResourceSnapshot, ResourceClaimSet)
  → AdmissionAssessment
```

This makes admission deterministic and easy to test.

The later reservation subsystem is the authority that atomically commits capacity.

## Resource snapshot

Admission operates against an immutable snapshot:

```yaml
snapshot:
  revision: 731
  resources:
    resource://host/default/memory:
      allocatable: 36Gi
      reserved: 18Gi
    resource://workspace/default/worktree:
      allocatable: 8
      reserved: 5
    resource://limit/global/runs:
      allocatable: 8
      reserved: 6
```

The assessment records the snapshot revision it observed.

## All-or-nothing assessment

All required claims must fit together.

If one claim fails, the ExecutionOffer is not admissible. Admission never performs partial reservations.

## Admission outcomes

v1 has three semantic outcomes:

```text
ADMITTABLE
TEMPORARILY_UNAVAILABLE
UNSATISFIABLE
```

### ADMITTABLE

Every claim fits the current snapshot.

### TEMPORARILY_UNAVAILABLE

The resource exists and its total allocatable capacity can satisfy the claim, but current reservations/availability prevent admission now.

### UNSATISFIABLE

The claim cannot fit even if competing reservations disappear, or a required resource is structurally unavailable for this offer.

The Router/scheduling policy may immediately discard an unsatisfiable offer and consider another one.

## Structured failure details

Admission results include resource-level reasons rather than only a boolean.

Example:

```yaml
result: temporarily-unavailable
failures:
  - resource: resource://workspace/default/worktree
    required: 1
    available: 0
    allocatable: 8
    reason: capacity-reserved
```

or:

```yaml
result: unsatisfiable
failures:
  - resource: resource://sandbox/isolated/instance
    required: 2
    allocatable: 1
    reason: exceeds-allocatable-capacity
```

These details drive routing feedback, scheduler wakeups, diagnostics, and observability.

## Assessment versus reservation

An AdmissionAssessment is advisory and snapshot-bound.

A candidate can fit at snapshot revision 731 and lose the race before reservation.

The reservation subsystem therefore rechecks current ledger state and atomically commits all claims or none:

```text
AdmissionAssessment: ADMITTABLE
        ↓
reservation transaction
        ├─ verify current state/revisions
        ├─ verify all claims still fit
        ├─ reserve all claims
        └─ commit
```

On conflict, the candidate is reassessed.

## Reservation versus runtime enforcement

A reservation means Pantheon will not allocate the same accounted capacity to another Run.

It does not inherently guarantee that the OS/runtime prevents a Run from exceeding its reservation.

```text
Resource Ledger / Reservations
  → allocation correctness

Sandbox / ExecutorBackend
  → runtime enforcement
```

The two are intentionally separate, analogous to Pantheon's authorization-versus-sandbox separation.

## Resource health

Resource health is separate from quantity/capacity.

v1 health states:

```text
Available
Degraded
Unavailable
Unknown
```

`Unavailable` and `Unknown` fail closed for new reservations.

A degraded resource may remain usable according to owner policy, but the health state must be visible in the ledger and assessment.

## Capacity shrinkage

Allocatable capacity may decrease after Runs are already reserved.

If:

```text
reserved > allocatable
```

Pantheon marks the resource oversubscribed and blocks new admission.

v1 does not automatically preempt or terminate existing Runs. Recovery/resource-policy logic handles the condition separately.

## No automatic preemption in v1

Higher-priority pending Tasks do not kill already-admitted Runs merely to free capacity.

Priority affects which pending Task is considered next. Existing Runs continue unless another explicit lifecycle/policy mechanism cancels them.

Preemption may be investigated later after Run checkpoint/resume semantics are mature.

## Token accounting is deliberately separate

Model-token concepts do not belong in the Resource Ledger because ordinary resource reservations are released when a Run finishes, while token consumption is permanently charged to a budget/accounting period.

Pantheon distinguishes three token concepts:

```text
CONTEXT CAPACITY
  execution compatibility

TOKEN BUDGET
  spending ceiling

TOKEN USAGE
  metering/accounting
```

### Context capacity

Context window requirements are ExecutionRequest compatibility constraints, for example:

```yaml
context:
  minTokens: 64000
```

They are not reservable resources.

### Token budget

Goal/Task/Run token ceilings belong to a future Budget Ledger rather than the Resource Ledger.

Conceptually:

```text
Goal budget
  ↓
Run spending
  ↓
remaining budget decreases permanently
```

Budgets may later include normalized token units, monetary cost units, premium/scarce execution allowance, or other non-releasable consumption ceilings.

### Token usage

ExecutorBackends normalize native usage into Pantheon usage telemetry:

```yaml
usage:
  inputTokens: 42000
  outputTokens: 8700
  cachedInputTokens: 18000
  accuracy: exact
```

When exact counts are unavailable, a backend may report estimated usage with provenance/accuracy metadata.

Pantheon core does not need to know the concrete tokenizer/model internals.

### Backend scarcity/quota

Backend/provider-specific rate limits or subscription scarcity are not exposed as concrete provider concepts to core policy. Backends normalize their current state into generic availability/scarcity signals or ExecutionOffer attributes.

Routing may use those normalized signals without provider-specific branches.

## Separate ledgers

The architecture intentionally separates:

```text
RESOURCE LEDGER
  releasable allocation
  CPU / memory / workspace / execution capacity / concurrency

BUDGET LEDGER
  consumptive ceilings
  tokens / cost units / premium usage / other spending

USAGE TELEMETRY
  actual input/output/cache tokens
  wall time
  calls
  resource observations
```

These systems interact at Run admission and runtime, but their accounting semantics remain distinct.

## Key invariants

1. Admission understands only opaque resource keys, normalized quantities, descriptors, health, and revisions.
2. Concrete execution backends translate private requirements into normalized claims.
3. Infrastructure and policy controllers can augment backend claims.
4. Resource owners publish normalized capacity; Admission never probes implementation-specific systems directly.
5. v1 supports divisible and discrete allocation modes only.
6. v1 performs conservative allocation accounting without overcommit.
7. Admission uses allocatable capacity, not raw total capacity or instantaneous utilization.
8. All claims for one ExecutionOffer must fit as a unit.
9. Admission is a pure snapshot-based assessment; reservation is a later atomic mutation.
10. Admission distinguishes temporary shortage from structural unsatisfiability.
11. Concurrency limits reuse the resource ledger as synthetic discrete resources.
12. Estimation uncertainty is resolved by the resource-producing component before normalization.
13. Reservations provide allocation correctness; runtime enforcement remains separate.
14. Capacity shrinkage blocks new admission but causes no automatic preemption in v1.
15. Context tokens are compatibility, token ceilings are Budget Ledger concerns, and token counts are usage telemetry; none are ordinary Resource Ledger reservations.
