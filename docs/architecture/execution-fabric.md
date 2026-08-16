# Execution Fabric

## Status

Draft design — Pantheon execution abstraction specification.

## Purpose

Pantheon core must remain independent of concrete model providers, CLI harnesses, local runtimes, remote workers, and future execution systems.

The core schedules and routes against normalized execution contracts. Concrete executor implementations live behind an `ExecutorBackend` boundary and are opaque to core logic.

The central rule is:

> **Pantheon core may reason about semantic work, execution features, policy, placement, resources, and observed performance. It must not contain business logic keyed to a concrete provider, harness, runtime, or model name.**

Concrete names may appear in backend configuration, adapter-private state, audit metadata, and diagnostics, but not in core scheduling/routing branches.

## Architectural boundary

```text
PANTHEON CORE
────────────────────────────────────
Goal
Task
Logical Agent
TaskGraph
Scheduler
Router
Admission
Policy
Run
Artifact / Evidence

understands:
  semantic task capabilities
  canonical actions/tools
  execution features
  placement constraints
  isolation requirements
  generic resource claims
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
ExecutorBackend interface

              │
              ▼

BACKEND / ADAPTER PRIVATE WORLD
────────────────────────────────────
provider APIs
CLI harnesses
local model runtimes
model identifiers
provider session identifiers
runtime flags
provider-specific configuration
remote worker protocols
```

Dependency direction is one-way:

```text
core → abstract execution contracts
backend adapters → implement those contracts
```

Never:

```text
core scheduler/router → provider-specific branches
```

A practical code-review invariant is that provider/model/runtime names should not appear in core routing or scheduling decision logic.

## Terminology

Pantheon deliberately separates four terms that are easy to overload:

```text
TASK CAPABILITY
  Semantic ability needed to achieve an outcome.
  Example: code-analysis, security-analysis.

EXECUTION FEATURE
  Mechanism an executor backend can provide.
  Example: session.resume, input.image, tools.structured.

ACTION / TOOL
  Canonical operation that may be invoked.
  Example: filesystem.write, shell.execute.

CAPABILITY GRANT / TICKET
  Authorization object allowing a principal to perform an action.
```

Backend mechanics are therefore called **Execution Features**, not authorization capabilities.

## ExecutorBackend

An `ExecutorBackend` is an adapter that can convert a normalized Pantheon execution request into one or more executable offers and manage the resulting backend session.

Examples of implementations are intentionally outside the core model. A backend implementation could wrap a local runtime, a CLI harness, an API service, a remote worker, SSH, a VM-based worker, or another orchestration system.

Conceptual interface:

```rust
trait ExecutorBackend {
    async fn descriptor(&self) -> BackendDescriptor;
    async fn health(&self) -> BackendHealth;

    async fn offers(
        &self,
        request: &ExecutionRequest,
    ) -> Vec<ExecutionOffer>;

    async fn launch(
        &self,
        binding: &ExecutionBinding,
    ) -> BackendSession;

    async fn status(&self, session: &BackendSession) -> BackendStatus;
    async fn interrupt(&self, session: &BackendSession);
    async fn resume(&self, session: &BackendSession);
    async fn terminate(&self, session: &BackendSession);
}
```

The exact Rust trait is deferred until the Run/Attempt subsystem is designed. Operations that are not universally supported are advertised through Execution Features and fail closed when required but unsupported.

## Backend instance versus implementation

Core addresses a configured backend **instance** through a stable opaque reference such as:

```text
executor://local-primary
executor://premium-reasoning
executor://remote-lab
```

The implementation type behind that reference is adapter-private/operator configuration.

One adapter implementation may expose multiple backend instances with different endpoints, credentials, models, resource pools, security posture, or routing policy.

This prevents core state from depending on implementation names.

## BackendDescriptor

A backend publishes a normalized descriptor used for discovery and coarse filtering.

Conceptual shape:

```yaml
backend:
  id: executor://local-primary
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

The descriptor contains stable/fairly stable facts. Dynamic per-request decisions belong in `ExecutionOffer`.

Backends should periodically fingerprint/report health. A backend that is unhealthy or whose required dependencies are missing is not eligible to produce launchable offers.

Health states for v1 should remain small:

```text
healthy
unhealthy
unknown
draining
```

`unknown` fails closed for new launches.

## Execution Features

Execution Features are namespaced strings with provider-neutral semantics.

Initial examples:

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

Pantheon should avoid a large frozen enum. Features use a validated namespace format so future adapters can introduce additional features without modifying the core scheduler.

Core-standard feature names receive Pantheon-defined semantics. Adapter-specific features may exist under an extension namespace but cannot become required by portable Agent/Task definitions unless an operator intentionally accepts that non-portability.

## ExecutionRequest

An `ExecutionRequest` is a normalized, provider-independent statement of what an execution environment must support for one Task/Agent pairing.

It is generated by Pantheon from the immutable Task, logical Agent, current Goal constraints, policy, workspace requirements, and execution policy.

Conceptual shape:

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

  tools:
    bundles:
      - filesystem
      - git
      - shell

  policy:
    envelopeHash: sha256:...

  workspace:
    requirement: worktree
```

The request does **not** specify provider, harness, model, API endpoint, CLI flags, or provider session data.

### Request composition

The effective request is the intersection/combination of:

```text
Task requirements
+
Agent intrinsic execution requirements
+
Goal constraints
+
project/system policy
+
workspace/sandbox requirements
```

A lower layer may tighten requirements but cannot weaken enclosing hard constraints.

## ExecutionOffer

A backend receives an `ExecutionRequest` and may return zero or more short-lived normalized offers.

An offer says:

> This backend instance can satisfy this request under these concrete normalized properties and resource claims right now.

Conceptual shape:

```yaml
offer:
  id: offer_01K...
  request: exec-request_01K...
  backend: executor://local-primary
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
      - resource: resource://host/memory
        quantity: 20Gi
      - resource: resource://host/cpu
        quantity: 4
      - resource: resource://backend/local-primary/concurrency
        quantity: 1

  estimates:
    startupClass: normal
    confidence: medium

  validUntil: ...
```

The backend may keep an opaque private token/state associated with the offer. Core must not interpret that state.

### What an offer must not do

A backend must not grant itself authority or claim that policy has been satisfied merely because it produced an offer.

Authorization remains owned by Pantheon. The offer is execution feasibility information, not a security decision.

Backend self-reported qualitative claims such as "best model" or "high quality" should not directly determine routing. Quality and reliability ranking should primarily come from operator policy and Pantheon's normalized historical evidence.

## Backend Registry

The Backend Registry stores the currently configured backend instances and their descriptors/health.

Conceptually:

```text
Backend Registry
  ├── executor://local-primary
  ├── executor://premium-reasoning
  └── executor://remote-lab
```

The registry owns:

```text
backend identity
descriptor revision
execution features
placement attributes
supported isolation modes
health/fingerprint
resource namespaces supplied by the backend
adapter-private configuration reference
```

It does not own Task routing policy or authorization.

For v1, backends may be configured statically and registered when Pantheon starts. Dynamic plugin installation/discovery can be added later without changing the contract.

## Generic resource claims

Execution offers express resource demand using normalized resource identifiers.

Core-standard host/workspace resources may include:

```text
resource://host/cpu
resource://host/memory
resource://host/disk-temporary
resource://workspace/worktree
resource://sandbox/isolated
```

Backend-specific capacity is namespaced to the backend instance:

```text
resource://backend/<backend-id>/concurrency
resource://backend/<backend-id>/<resource-name>
```

The resource ledger understands generic accounting metadata such as:

```text
resource key
quantity/unit
capacity
allocatable
reserved
shareability / exclusivity
overcommit policy
health
```

It does not need to understand the implementation meaning of every backend-specific key.

This lets a future backend advertise a new constrained resource without adding provider-specific scheduler code.

## Resource ownership

Only the component that owns a resource namespace may publish its capacity/health.

Examples:

```text
host resource controller
  owns resource://host/**

workspace controller
  owns resource://workspace/**

backend executor://local-primary
  owns resource://backend/local-primary/**
```

This prevents arbitrary adapters from spoofing unrelated host capacity.

## Router and Admission responsibilities

The Router and Admission Engine remain separate.

```text
Router
  asks compatible backends for offers
  filters by request/policy
  ranks viable offers using normalized policy + history

Admission
  checks generic resource claims and concurrency/budget guards
  returns whether each offer fits current capacity
```

Neither component branches on concrete provider/model names.

A typical flow is:

```text
ExecutionRequest
      ↓
Backend Registry
      ↓
compatible backend instances
      ↓
ExecutionOffers
      ↓
Router scoring
      ↓
Admission fit
      ↓
ExecutionBinding
```

Routing and admission may iterate when a preferred offer is temporarily unavailable.

## ExecutionBinding

After routing/admission chooses an offer and resource reservation succeeds, Pantheon creates an immutable `ExecutionBinding`.

Conceptual shape:

```yaml
binding:
  id: binding_01K...

  request: exec-request_01K...
  requestHash: sha256:...

  offer: offer_01K...
  offerHash: sha256:...

  backend: executor://local-primary
  descriptorRevision: 17

  reservations:
    - reservation://...

  policyHash: sha256:...
```

The binding is the execution decision that a later Run materializes.

A stale/expired offer cannot be silently rebound. If the backend state has materially changed, Pantheon must produce/revalidate an offer and binding.

## Run Manifest

The Run Manifest records the normalized binding used by core:

```yaml
executor:
  backend: executor://local-primary
  binding: binding_01K...
  descriptorRevision: 17
```

For diagnostics and reproducibility, backend-private resolved details may also be persisted as namespaced audit metadata or an opaque backend-state reference, for example runtime/model/session identifiers.

Core routing/scheduling logic must never depend on those fields.

This gives Pantheon both abstraction and observability:

```text
control decisions use normalized contract
+
audit records preserve concrete reality
```

## Policy and security

The Execution Fabric does not replace Pantheon authorization.

Before launch, Pantheon determines whether the execution configuration can faithfully enforce the resolved policy using:

```text
Pantheon Action Broker
provider/backend permission compilation where available
OS/container/VM sandbox compensation
```

If a required security property cannot be enforced, the offer is invalid/fails closed.

Backend declarations are evidence about mechanisms, not authority.

## Logical Agent portability

A canonical Agent must not contain an allowlist of concrete harness/provider names.

Instead, the Agent expresses intrinsic execution requirements:

```yaml
execution:
  routePolicy: coding-default
  requirements:
    executionFeatures:
      - session.interactive
      - tools.structured
    minContextTokens: 64000
```

The Backend Registry and Router determine which currently configured backend instances can satisfy those requirements.

Backend-specific tuning does not belong in the portable Agent manifest. It belongs in backend configuration or routing-policy configuration.

This allows replacing an executor implementation without modifying logical agent identity.

## Relationship to A2A

A2A interoperability remains separate from the internal Execution Fabric.

A2A's discovery model is useful conceptually because clients discover declared capabilities/interfaces while treating a remote agent as an opaque system. Pantheon may later expose logical Agents through A2A or wrap an A2A remote system as an ExecutorBackend, but A2A is not the canonical internal execution contract.

## v1 scope

Include:

- `ExecutorBackend` abstraction;
- backend instance identifiers (`executor://...`);
- Backend Registry;
- descriptor revision and health/fingerprint;
- provider-neutral Execution Features;
- `ExecutionRequest`;
- short-lived `ExecutionOffer`;
- generic namespaced resource claims;
- immutable `ExecutionBinding`;
- normalized Run Manifest backend reference;
- fail-closed policy compatibility checks.

Defer:

- hot-installable backend plugins;
- remote backend federation;
- marketplace/discovery protocols;
- semantic negotiation across arbitrary vendor extensions;
- ML-driven offer scoring;
- cross-host distributed resource allocation.

## Key decisions

1. **Provider/model/runtime names do not participate in Pantheon core scheduling or routing logic.**
2. **Concrete execution is hidden behind `ExecutorBackend` instances.**
3. **Logical Agents express requirements, never concrete compatible harness allowlists.**
4. **Backend mechanics are called Execution Features to avoid confusion with authorization capability grants/tickets.**
5. **Backend discovery uses normalized descriptors plus health/fingerprint.**
6. **Per-request feasibility is represented by short-lived `ExecutionOffer`s.**
7. **Backend-specific resource capacity uses namespaced generic resources.**
8. **Router ranks normalized offers; Admission evaluates generic resource fit.**
9. **Selected execution becomes an immutable `ExecutionBinding`.**
10. **Concrete runtime/model/session details remain available for audit but are opaque to core decision logic.**
11. **Backend declarations never grant authority; Pantheon authorization remains canonical and fail-closed.**
