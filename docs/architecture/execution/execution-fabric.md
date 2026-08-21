# Execution Fabric

## Status

Canonical Pantheon execution-backend abstraction specification.

## Central invariant

> **Pantheon core reasons about semantic work, Logical Agents, execution features, policy, placement, resources and observed performance. It contains no business logic keyed to a concrete provider, model, harness or runtime name.**

```text
PANTHEON CORE
Goal / Task / Agent / Scheduler / Router / Admission / Run / Artifact
  understands:
    competencies
    canonical actions/tools
    execution features
    placement/isolation requirements
    generic resources
    policy/budget constraints
    normalized metrics/evidence
            ↓
EXECUTION FABRIC
Backend Registry
ExecutionRequest
ExecutionOffer
ExecutionBinding
ExecutorBackend interface
            ↓
BACKEND-PRIVATE WORLD
provider/runtime/harness/model/session/protocol identifiers and flags
```

## Terminology

```text
COMPETENCY
  semantic ability of a Logical Agent

EXECUTION FEATURE
  backend mechanism required by the Run

ACTION / TOOL
  semantic operation available through Agent Control/brokers

AUTHORIZATION / GRANT
  whether a principal may perform an action

CREDENTIAL BINDING
  which logical credential authority may satisfy an already-authorized semantic action/resource

RESOURCE
  reservable finite capacity
```

These concepts are not interchangeable.

## ExecutorBackend

An ExecutorBackend is one registered execution instance capable of describing factual features/placement/health and producing execution offers.

Core never branches on its concrete implementation name. Backend-private configuration may contain provider endpoints, model IDs, CLI/runtime flags, remote session protocols and credentials via SecretRefs.

## BackendDescriptor

Descriptor includes factual, revisioned information such as:

```text
backend identity
revision
health/draining state
placement
supported execution features
launch semantics
resource/metering contract
```

A backend does not publish its own semantic quality score, authorization or scarcity preference.

## Launch semantics

BackendDescriptor/Offer reports one factual launch class:

```text
KEYED_IDEMPOTENT
  same LaunchKey can be safely ensured/recovered as one logical execution

OBSERVATIONAL
  backend lacks trustworthy create-idempotency/lookup; after ambiguous contact only conservative observation/reconciliation is possible
```

Pantheon may implement KEYED_IDEMPOTENT semantics in an outer process/session supervisor around a harness.

`OBSERVATIONAL` offers are ineligible for Tasks/Runs where ambiguous duplicate external execution could violate the safety envelope and no outer Sandbox/process mechanism prevents duplicates.

The adapter may not label itself idempotent merely because retries usually appear harmless.

## ExecutionRequest

ExecutionRequest is provider-neutral and Agent-specific. It contains/references only semantic/operational requirements, for example:

```text
Task type/competency context
selected Logical Agent/version
required execution features
context-capacity floor
placement constraints
isolation requirements
canonical tool/action availability requirements
resource/budget compatibility facts
configuration component digests
```

It contains no provider/model allowlist in core.

## Agent Control execution features

Runtime Agent interaction is expressed as abstract execution features. The v1 set includes, for example:

```text
control.result-submit
control.artifact-seal
control.action-invoke
control.task-spawn
```

`control.graph-propose` is reserved post-v1 vocabulary and is not a v1 ExecutionRequest requirement or backend-selection feature. No v1 AgentControlSession has the corresponding `task.graph.propose` authority. Multi-node structural planning uses the PlanningOperation/PlanningRecord/GraphPatch control-plane path instead.

A backend may implement v1 Agent Control features through native structured tools, a private bridge, function calls or another adapter-private mechanism. Core cares only that the required semantic feature exists.

## ExecutionOffer

An offer is side-effect-free. It states that the backend can satisfy a specific ExecutionRequest under stated factual conditions.

It may include:

```text
backend/descriptor revision
supported required features
factual context capacity
placement
required generic resources
metering/rate-limit availability
launch semantics
backend-private opaque offer ref/hash
```

Offer creation does not reserve capacity, launch execution, choose authorization or award itself quality.

## Routing candidate

Final routing unit remains:

```text
ExecutionCandidate = Logical Agent + ExecutionOffer
```

Flow:

```text
Task
  ↓ Agent Resolver
eligible Logical Agents
  ↓ per-Agent ExecutionRequests
side-effect-free offers
  ↓
Agent + Offer candidates
  ↓ Sandbox feasibility / resource-budget-rate feasibility
RoutePolicy scoring
  ↓
selected pair
  ↓ T3 atomic scheduler commit
ExecutionBinding + Reservations + Holds + Run
```

## Sandbox integration

Sandbox isolation is not owned by ExecutorBackend self-assertion. Pantheon Sandbox Planner validates a provider-neutral SandboxPlan against controller-known SandboxBackend facts/guarantees before Binding/Run execution.

V1 keeps `ExecutionCandidate = Agent + Offer`; SandboxPlan is controller-generated feasibility/binding state rather than a third self-scoring marketplace actor.

A backend cannot satisfy `isolation.control-plane` by merely claiming it is secure.

## ExecutionBinding

Binding is immutable and freezes the selected Agent+Offer strategy and relevant configuration:

```text
Task/Agent
ExecutionRequest hash
ExecutionOffer hash
backend + descriptor revision
SandboxPlan digest
ConfigurationRevision
routePolicyDigest
executionProfileDigest
frozen authorization ceiling digest
credentialBindingRegistryDigest
reservations/budget refs created by scheduling commit
resolved backend-private model/runtime audit metadata where appropriate
```

`credentialBindingRegistryDigest` identifies the immutable CredentialBindingRegistry from the ConfigurationRevision captured at T3. It freezes the Run's logical credential-mapping authority without freezing SecretVersionId or secret bytes.

Implementation status (v0.1.0): no compiled `credentialBindings` component exists yet, so committed Bindings freeze the six component digests the configuration schema defines today and omit `credentialBindingRegistryDigest`. This is a recorded lag against invariant 6, to be closed by the mission that introduces the component; it is not permission to invent a placeholder digest.

Changing Agent/backend/offer/material execution configuration creates a new Run/new Binding. Binding is never edited in place.

A later credential-binding configuration change does not mutate the Binding. For an exact credential-bearing semantic operation, Pantheon resolves the action/resource against both the Run's frozen registry and the current active registry and requires equality of the exact resolved `credentialBindingAuthorityDigest`. Whole-registry equality is deliberately unnecessary: changing an unrelated binding must not invalidate this Run.

## ExecutorBackend operational interface

Conceptual operations include:

```text
describe/health
offer(request)
prepare/validate backend-private details
ensureExecution(binding, LaunchKey, ...)
inspect/recover execution lineage
request termination
collect factual usage/termination evidence
```

Exact Rust trait surface is deferred to implementation-boundary design.

## Durable launch boundary

The Fabric does not treat an adapter call as the durable source of truth. Attempt/LaunchKey and the pre-launch contact marker are persisted by Run Controller before external `ensureExecution` contact, as defined by `docs/architecture/execution/run-and-attempt.md`.

If launch semantics are KEYED_IDEMPOTENT, repeated reconciliation addresses the same lineage. If OBSERVATIONAL and contact may have occurred, Pantheon stays UNKNOWN until the adapter/outer environment can prove a safe state; it does not blindly create a new Attempt.

## Backend attachment

Attempt may persist opaque versioned backend-private attachment needed to reattach/reconcile. Core persists but does not interpret provider session IDs, PTY handles or runtime internals.

Attachment is not authority by itself; it is validated in the context of current Attempt/Binding/backend identity.

## Metering

Backend reports factual usage under its declared metering contract. Pantheon namespaces source identity with backend + Attempt/control-operation + adapter key + meter.

Attempt usage is accepted only when the immutable ExecutionBinding names the reporting backend and frozen metering contract for that Attempt lineage.

A billable control operation that accepts backend-authored usage instead freezes an immutable metering-source binding in its own durable intent before external contact. That binding identifies the reporting backend, descriptor/revision and metering contract/digest. It is accounting provenance only: the control operation remains owned by its controller and is not converted into a Run, Attempt, ExecutionOffer or ExecutionBinding merely because a backend reports metering facts for it.

A backend cannot report usage for another backend's Attempt or for a control operation whose immutable metering-source binding names another backend. A control operation without such a binding cannot accept backend-authored usage.

Current lifecycle state is not a substitute for immutable provenance: delayed valid usage may arrive after terminalization. Durable launch/contact evidence may independently prove that an external lineage was never contacted, but that is an execution-reconciliation fact rather than backend ownership.

Controller lease epoch is provenance, not by itself a reason to discard delayed factual usage.

## Backend lifecycle

Registration/configuration publishes stable backend IDs and descriptor revisions. Backend health/draining is runtime state.

Removing/disabling a backend from active configuration means no new offers. Existing Attempts/obligations retain enough adapter/recovery support until they are safely terminal; configuration removal never makes an external execution disappear.

## Authorization and credential authority

ExecutionBackend is not authorization authority. It receives only what Pantheon has authorized/compiled. Backend-native permission controls are defense in depth and may tighten but never broaden the canonical policy.

Agent Control authenticates Attempt identity; Broker/PDP controls semantic actions; Sandbox limits ambient physical authority.

Credential binding is a separate gate after semantic authorization. For a credential-bearing Run operation, Pantheon requires:

```text
semantic action/resource authorized now
AND frozen Run authorization ceiling permits it
AND exact binding exists in Run's frozen CredentialBindingRegistry
AND exact binding exists in current active CredentialBindingRegistry
AND frozen credentialBindingAuthorityDigest == current credentialBindingAuthorityDigest
AND secret.use is authorized
AND the resolved SecretDescriptor is currently usable/reconciled
```

If the current exact binding is removed or changes logical SecretRef/mechanism/credential-use constraints, the operation fails closed for that existing Run. A new Run may later freeze the new mapping if otherwise authorized. Rotation of material behind the same logical SecretRef is not a Binding change and may use the current SecretVersion after SecretProvider reconciliation.

## Core review invariant

Any core Scheduler/Router/Admission branch equivalent to:

```text
if backend == ConcreteX ...
if model == ConcreteY ...
```

for semantic business logic is an architecture violation. Concrete-specific behavior belongs behind descriptor/features/adapter-private implementation.

## Core invariants

1. Core never knows provider/model/harness/runtime business names.
2. Logical Agent eligibility is semantic and precedes execution routing.
3. Offers are side-effect-free factual compatibility descriptions.
4. Final candidate is Agent+Offer; backend does not choose Agent or self-award quality/authorization.
5. Binding is immutable and provider-neutral at core boundary.
6. ExecutionBinding freezes `credentialBindingRegistryDigest`; later credential-bearing actions require exact frozen/current `credentialBindingAuthorityDigest` equality rather than whole-registry equality.
7. Credential material rotation behind the same logical SecretRef does not mutate ExecutionBinding or broaden credential authority.
8. Launch semantics are explicit `KEYED_IDEMPOTENT|OBSERVATIONAL`; unsafe observational offers are filtered before Run commitment.
9. Attempt/LaunchKey/contact marker, not adapter memory, are durable launch truth.
10. Sandbox guarantees are controller-validated, not backend self-authorization.
11. V1 Agent Control execution features cover only operations exposed by the v1 worker surface; `control.graph-propose` is reserved post-v1 and cannot affect v1 routing/admission.
12. Backend usage is validated/namespaced against immutable Attempt ExecutionBinding ownership or immutable control-operation metering-source ownership; metering provenance never reclassifies a control operation as a Run.
