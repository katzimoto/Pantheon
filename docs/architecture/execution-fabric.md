# Execution Fabric

## Status

Draft design — Pantheon execution abstraction specification.

## Purpose

Pantheon core must remain independent of concrete model providers, CLI harnesses, local runtimes, remote workers, and future execution systems.

The core schedules and routes against normalized execution contracts. Concrete executor implementations live behind an `ExecutorBackend` boundary and are opaque to core logic.

The central rule is:

> **Pantheon core may reason about semantic work, Logical Agents, execution features, policy, placement, resources, budgets, and observed performance. It must not contain business logic keyed to a concrete provider, harness, runtime, or model name.**

Concrete names may appear in backend configuration, adapter-private state, audit metadata, diagnostics and adapter-specific tests, but not in core scheduling/routing branches.

See also:

- `docs/architecture/logical-agent-resolution.md`
- `docs/architecture/execution-offer-routing-and-admission-handshake.md`
- `docs/architecture/run-and-attempt.md`
- `docs/architecture/scheduler-resource-ledger-and-admission.md`
- `docs/architecture/scheduler-dispatch-and-run-intent-reconciliation.md`

## Architectural boundary

```text
PANTHEON CORE
────────────────────────────────────
Goal
Task / TaskGraph
Logical Agent
Agent Resolver
Scheduler / Router / Admission
Policy
Run / Attempt
Artifact / Evidence

understands:
  Task types and competencies
  Logical Agent identity
  canonical actions/tools
  Execution Features
  placement/isolation requirements
  generic resource claims
  budget/usage contracts
  policy constraints
  normalized historical metrics

              │
              ▼

EXECUTION FABRIC
────────────────────────────────────
Backend Registry
ExecutionRequest
ExecutionOffer
ExecutionBinding
ExecutorBackend contract

              │
              ▼

BACKEND / ADAPTER PRIVATE WORLD
────────────────────────────────────
provider APIs
CLI harnesses
local runtimes
model identifiers
native session/process identifiers
runtime flags
provider-specific configuration
remote worker protocols
```

Dependency direction is one-way:

```text
core → abstract execution contracts
backend adapters → implement those contracts
```

A branch such as `if backend == <concrete-runtime>` inside core scheduler/router/admission should normally be treated as an architecture violation.

## Terminology

Pantheon deliberately separates:

```text
COMPETENCY
  Semantic ability required by Task / provided by Logical Agent.
  Example: code.analysis, security.analysis.

EXECUTION FEATURE
  Mechanism an ExecutorBackend can provide.
  Example: session.resume, input.image, tools.structured.

ACTION / TOOL
  Canonical operation that may be invoked.
  Example: filesystem.write, shell.execute.

CAPABILITY GRANT / TICKET
  Authorization object permitting a concrete action/resource.
```

Backends never evaluate semantic competencies as self-authorization. Agent Resolution handles competency matching before backend routing.

## ExecutorBackend

An `ExecutorBackend` is an adapter for one class of execution system. A configured backend **instance** is addressed by a stable opaque ID such as:

```text
executor://a
executor://b
executor://remote-lab
```

The implementation type behind that ID remains operator/adapter-private configuration.

One adapter implementation may expose multiple backend instances with different endpoints, credentials, native models, resource pools, security posture, or quotas.

After the Run/Attempt design, the conceptual contract is no longer a naïve `launch()` API. It needs discovery/offers plus idempotent Attempt reconciliation.

Conceptually:

```rust
trait ExecutorBackend {
    async fn descriptor(&self) -> BackendDescriptor;
    async fn health(&self) -> BackendHealth;

    async fn offers(
        &self,
        request: &ExecutionRequest,
    ) -> Vec<ExecutionOffer>;

    async fn ensure_execution(
        &self,
        attempt: &AttemptExecutionSpec,
        attachment: Option<&BackendAttachment>,
    ) -> EnsureExecutionResult;

    async fn observe(
        &self,
        attempt: &AttemptExecutionRef,
    ) -> ExecutionObservation;

    async fn interrupt(...);
    async fn resume(...);
    async fn terminate(...);
}
```

The exact Rust trait remains deferred until implementation integration. The normative semantics are:

- offer generation is side-effect free;
- Attempt creation and immutable `LaunchKey` happen durably before external creation;
- repeated `ensure_execution` for the same Attempt/LaunchKey must not create a second logical execution lineage;
- reconnect/recovery/reattachment stay under the same Attempt when continuity is preserved;
- a fresh execution incarnation uses a new Attempt/LaunchKey;
- changing Agent or ExecutionBinding creates a new Run.

## BackendDescriptor

A backend publishes normalized slowly changing discovery data.

Conceptually:

```yaml
backend:
  id: executor://a
  descriptorRevision: 17

  features:
    - session.interactive
    - session.interrupt
    - session.resume
    - tools.structured
    - output.structured

  placement:
    locality: local

  isolation:
    supported:
      - workspace
      - isolated

  health:
    state: healthy
    observedAt: ...
```

The descriptor contains facts about mechanisms, placement, isolation and health. Per-request feasibility belongs in `ExecutionOffer`.

v1 health states remain small:

```text
healthy
unhealthy
unknown
draining
```

`unknown` fails closed for new execution commitments.

## Execution Features

Execution Features are namespaced provider-neutral mechanisms.

Examples:

```text
session.interactive
session.interrupt
session.resume
session.long-running
input.image
output.structured
tools.structured
tools.streaming-events
transport.streaming
```

Pantheon should avoid a large frozen enum. Core-standard names receive Pantheon-defined semantics. Adapter-extension names may exist but cannot silently become mandatory portable Agent requirements.

## Logical Agent eligibility precedes offers

Task competencies are matched against Agent competencies before backend solicitation.

```text
Task
  ↓
Agent Resolver
  ↓
Eligible Logical Agents
  ↓
ExecutionRequest per Agent
  ↓
Backend offers
```

Backends therefore receive a request for an already-eligible Logical Agent. They do not choose which Agent is semantically qualified.

Final commitment is nevertheless joint: routing compares valid `Agent + ExecutionOffer` pairs so an eligible Agent with no feasible execution path need not block another valid Agent that can execute now.

## ExecutionRequest

An `ExecutionRequest` is a normalized provider-independent statement of what execution must support for one **Task + eligible Logical Agent** pairing.

It is generated from:

```text
immutable Task
+
selected Agent candidate
+
Goal constraints
+
project/system policy
+
workspace/sandbox requirements
+
execution policy
```

Conceptually:

```yaml
request:
  id: exec-request_01K...

  task: task_123
  agent: agent://coder

  semantic:
    taskType: code.debug
    competencies:
      - code.analysis
      - code.debugging

  requirements:
    executionFeatures:
      - session.interactive
      - tools.structured

    context:
      minTokens: 64000

    placement:
      locality: any

    isolation:
      minimum: workspace

  tools:
    actions:
      - filesystem.read
      - filesystem.write
      - shell.execute

  policy:
    envelopeHash: sha256:...

  workspace:
    requirement: worktree
```

The semantic section is retained for provenance/routing metrics; backend feasibility must not reinterpret competency eligibility.

The request does **not** name a concrete provider, harness, model, endpoint, or native session.

The request is immutable/revision-bound and hashed before offers are accepted.

## ExecutionOffer

A compatible backend may return zero or more short-lived normalized offers.

An offer says:

> This backend instance can satisfy this ExecutionRequest now under these normalized factual properties and claims.

Conceptually:

```yaml
offer:
  id: offer_01K...
  request: exec-request_01K...
  requestHash: sha256:...

  backend: executor://a
  descriptorRevision: 17

  satisfies:
    executionFeatures:
      - session.interactive
      - tools.structured

  placement:
    locality: local

  isolation:
    mode: workspace

  resources:
    claims:
      - resource: resource://host/default/memory
        quantity: 20Gi
      - resource: resource://backend/a/concurrency
        quantity: 1

  usageEstimate:
    ref: usage-estimate://...

  validUntil: ...
```

Offer generation must not start a process/session, allocate a worktree, consume a durable execution slot, or otherwise create execution side effects.

A backend may return multiple offers if its private world has several ways to satisfy the same request.

## Backends report facts, not desirability

A backend may report factual normalized properties it owns:

- supported Execution Features;
- placement/locality;
- isolation mechanism;
- resource footprint;
- expected usage;
- current availability;
- offer validity.

It must not be trusted to self-award:

```text
quality score
recommended=true
priority
best model
```

Pantheon owns historical acceptance/reliability/latency/budget-efficiency metrics and route policy.

## Backend Registry

The Backend Registry owns:

```text
backend instance identity
descriptor revision
Execution Features
placement/isolation facts
health/fingerprint
backend-owned resource namespaces
adapter configuration reference
```

It does not own Task semantics, Agent eligibility, routing policy, or authorization.

v1 may use static/configuration-based registration. Hot-pluggable process discovery can come later.

## Generic resource claims

Offers express capacity demand using normalized resource keys, for example:

```text
resource://host/default/cpu
resource://host/default/memory
resource://workspace/default/worktree
resource://sandbox/isolated/instance
resource://backend/a/concurrency
```

The Resource Ledger understands generic accounting metadata and does not need implementation semantics for every namespaced resource.

Only the owner of a resource namespace may publish its capacity/health.

Budget/usage concepts such as tokens or monetary spend are **not** ordinary releasable ResourceReservations and are modeled separately.

## Router and Admission responsibilities

The high-level path is:

```text
Agent Resolver
      ↓
eligible Agents
      ↓
ExecutionRequest per Agent
      ↓
Backend Registry prefilter
      ↓
ExecutionOffers
      ↓
Agent + Offer candidates
      ↓
hard validation / policy / budget feasibility
      ↓
route ordering
      ↓
resource admission
      ↓
atomic commitment
      ↓
ExecutionBinding
```

Admission evaluates normalized resource fit. Budget admission evaluates spend/usage constraints. Route policy ranks only candidates that pass hard constraints.

Neither routing nor admission branches on concrete provider/model names.

## ExecutionBinding

After final selection and atomic commitment, Pantheon creates an immutable `ExecutionBinding` that freezes both the Logical Agent and execution offer.

Conceptually:

```yaml
binding:
  id: binding_01K...

  agent:
    ref: agent://coder
    specHash: sha256:...

  request:
    ref: exec-request_01K...
    hash: sha256:...

  offer:
    ref: offer_01K...
    hash: sha256:...

  backend:
    ref: executor://a
    descriptorRevision: 17

  reservations:
    - reservation://...

  budgetHolds:
    - budget-hold://...

  policyHash: sha256:...
```

The Binding is one immutable resolved strategy and belongs to exactly one Run.

Changing the Agent or material execution strategy produces another Binding and therefore another Run.

## Run and Attempt boundary

The Run records/references the immutable Binding and Agent/Genome/policy/workspace snapshots required for reproducibility.

Concrete execution identity belongs to Attempt:

```text
Run
  └─ immutable ExecutionBinding
       │
       ├─ Attempt 1
       │    └─ LaunchKey A
       │
       └─ Attempt 2
            └─ LaunchKey B
```

Backend-private process/session/runtime identifiers live in Attempt-scoped opaque attachment/audit state.

Core logic may store them but does not interpret them.

## Execution reconciliation

Minimal normalized external observations are:

```text
ABSENT
STARTING
RUNNING
EXITED
UNKNOWN
```

`UNKNOWN` means insufficient evidence and never authorizes duplicate execution. `ABSENT` means the backend can positively establish that the Attempt execution does not exist.

Backend events are hints that trigger reconciliation; they are not authoritative lifecycle commands.

## Policy and security

The Execution Fabric does not replace Pantheon authorization.

Backend declarations are evidence about mechanisms, not authority.

Before consequential external actions Pantheon rechecks current authority and must ensure mandatory policy can be enforced by the combination of:

```text
Pantheon Action Broker
backend-native enforcement where available
OS/container/VM sandbox compensation
```

If required security cannot be enforced, execution fails closed.

## v1 scope

Include:

- `ExecutorBackend` abstraction;
- opaque backend instance IDs;
- Backend Registry with descriptor revision/health;
- provider-neutral Execution Features;
- Agent-specific immutable `ExecutionRequest`;
- short-lived side-effect-free `ExecutionOffer`;
- `Agent + ExecutionOffer` candidate formation;
- generic namespaced resource claims;
- budget/usage estimate references;
- immutable `ExecutionBinding` freezing Agent + execution strategy;
- Attempt-scoped idempotent `LaunchKey` and opaque attachment state;
- normalized execution reconciliation observations;
- fail-closed policy compatibility.

Defer:

- hot-installable backend plugins;
- remote backend federation;
- marketplace/discovery protocols;
- arbitrary semantic negotiation across vendor extensions;
- opaque ML route scoring;
- cross-host distributed resource allocation;
- speculative duplicate execution.

## Key decisions

1. Provider/model/runtime names do not participate in Pantheon core scheduling/routing branches.
2. Concrete execution is hidden behind configured `ExecutorBackend` instances.
3. Task semantic requirements are competencies; backend mechanics are Execution Features.
4. Agent eligibility is resolved before backend solicitation, while final commitment is joint over valid Agent + Offer pairs.
5. Logical Agents express portable requirements, never concrete provider/harness/model allowlists.
6. Backend discovery uses normalized descriptors plus health/fingerprint.
7. Per-request feasibility is represented by short-lived side-effect-free `ExecutionOffer`s.
8. Backend-specific capacity uses namespaced generic resources; token/cost budgets are separate.
9. Backends report factual execution properties, not self-awarded quality.
10. Selected Agent and execution configuration become one immutable `ExecutionBinding` owned by a Run.
11. Attempt owns the idempotent LaunchKey and concrete execution lineage.
12. Concrete runtime/model/session details remain available for audit but opaque to core decisions.
13. Backend declarations never grant authority; Pantheon authorization remains canonical and fail-closed.
