# Agent Manifest

## Status

Draft design — Pantheon subsystem specification.

## Purpose

`AGENT.yaml` is Pantheon's declarative, machine-readable contract for a persistent logical agent. It describes desired agent configuration independently of the concrete backend, runtime, model, or harness that executes it.

The manifest complements the Agent Genome:

- `SOUL.md` — stable identity;
- `BEHAVIOR.md` — validated adaptive heuristics;
- memory — durable facts and context;
- skills — procedural knowledge;
- `AGENT.yaml` — specialties, portable execution requirements, authorization policy, delegation, limits, review, and learning policy.

Runtime state, learned content, sessions, capability grants/tickets, resource reservations, backend health, provider quota, and concrete executor details are deliberately excluded.

See also:

- `docs/architecture/agent-genome.md`
- `docs/architecture/permissions-and-capabilities.md`
- `docs/architecture/execution-fabric.md`
- `docs/architecture/run-and-attempt.md`

## Foundational principles

### 1. Logical identity is execution-independent

An Agent is not a model, provider, CLI harness, endpoint, or local runtime configuration.

Pantheon owns the canonical logical Agent. The Execution Fabric resolves the Agent onto a compatible `ExecutorBackend` for each Run.

### 2. Declarative desired state

`AGENT.yaml` describes configuration, not observed runtime state.

Runtime state belongs in Pantheon's state store and event log.

### 3. Pantheon owns authorization and delegation

Backend-native permissions and subagent mechanisms are implementation details. Pantheon is the canonical authority for:

- what an Agent may do;
- which resources it may access;
- which Agents it may delegate to;
- delegation depth/concurrency limits;
- sandbox/workspace requirements;
- review gates.

Backend adapters may tighten these constraints but may never broaden them.

### 4. Fail closed

If an execution configuration cannot faithfully enforce a required policy, Pantheon must reject it rather than silently weaken policy.

## Proposed v1alpha1 manifest

```yaml
apiVersion: pantheon/v1alpha1
kind: Agent

metadata:
  name: coder
  displayName: Atlas
  labels:
    domain: software-engineering
    tier: specialist

spec:
  description: >
    Implements, debugs, refactors, and reviews software.
    Use for tasks that require modifying or understanding code.

  accepts:
    - code.implement
    - code.debug
    - code.refactor
    - code.review

  genome:
    soul: SOUL.md
    behavior: BEHAVIOR.md

    memory:
      namespaces:
        - agent
        - project
      retrieval:
        mode: adaptive
        maxTokens: 4000

    skills:
      available:
        - git
        - debugging
        - testing
        - code-review
      preload:
        - git

  execution:
    routePolicy: coding-default
    requirements:
      executionFeatures:
        - session.interactive
        - tools.structured
      minContextTokens: 64000

  tools:
    bundles:
      - filesystem
      - git
      - shell
      - lsp

  permissions:
    profile: developer
    rules:
      - action: filesystem.write
        resource: workspace://**
        effect: permit

      - action: git.push
        resource: repo://**
        approval: required

      - action: secret.read
        resource: secret://**
        effect: forbid

  workspace:
    isolation: worktree
    sandbox: developer

  delegation:
    enabled: true
    allow:
      - researcher
      - reviewer
    maxDepth: 1
    maxConcurrent: 2

  limits:
    maxTurns: 40
    maxRetries: 2
    wallTime: 30m

  review:
    policy: code-standard
    requiredBeforeMerge: true

  learning:
    reflection: after-task
    promotion:
      memory: automatic
      skill: eval-gated
      behavior: eval-gated
      soul: human-approval
```

## Field model

### `metadata`

Stable identification and non-behavioral labels.

Required:

- `name` — unique machine identifier.

Optional:

- `displayName` — human-facing name;
- `labels` — routing/discovery metadata.

### `spec.description`

Human-readable summary used for selection and discovery. It describes what the Agent is for, not how it is implemented.

### `spec.accepts`

Task classes for which the Agent is a semantic candidate.

Examples:

```text
code.implement
code.debug
code.review
research.web
security.ctf
security.reverse-engineering
```

Task classes express specialty, not authorization or executor compatibility.

## Specialty, skills, tools, execution features, and permissions are distinct

Pantheon deliberately separates these concepts:

```text
SPECIALTY
  What kind of Task is this Agent appropriate for?

SKILL
  What reusable procedure does the Agent know?

TOOL / ACTION
  What canonical mechanism may be invoked?

EXECUTION FEATURE
  What mechanism must an ExecutorBackend provide?

PERMISSION
  What action is the Agent authorized to perform?
```

These must not be collapsed into provider-specific tool or model definitions.

`Execution Feature` is intentionally distinct from authorization capability grants/tickets.

## Genome references

`spec.genome` references the persistent identity and knowledge layers defined by the Agent Genome subsystem.

### Identity

```yaml
genome:
  soul: SOUL.md
  behavior: BEHAVIOR.md
```

The manifest references these files but does not inline them.

### Memory policy

The manifest configures retrieval policy, not memory contents.

```yaml
memory:
  namespaces:
    - agent
    - project
  retrieval:
    mode: adaptive
    maxTokens: 4000
```

### Skills

Pantheon follows progressive disclosure:

```yaml
skills:
  available:
    - git
    - debugging
    - postgres
  preload:
    - git
```

`available` means the skill may be activated when relevant. `preload` means its full instructions are loaded at session start.

## Execution policy

### Route policy, not a fixed backend

Agents normally reference a provider-independent route policy:

```yaml
execution:
  routePolicy: coding-default
```

The routing subsystem combines the Task, Agent, Goal constraints, project/system policy, Backend Registry, historical evidence, and current resource state to select an execution binding.

Concrete backend/runtime/model selection belongs to the Execution Fabric and immutable Run record, not Agent identity.

### Portable execution requirements

An Agent may declare intrinsic execution requirements:

```yaml
execution:
  requirements:
    executionFeatures:
      - session.interactive
      - tools.structured
    minContextTokens: 64000
```

Execution Features are portable mechanisms with Pantheon-defined semantics, such as:

```text
session.interactive
session.interrupt
session.resume
session.long-running
input.image
output.structured
tools.structured
transport.streaming
```

The Agent does **not** maintain a concrete `compatibleHarnesses` or provider allowlist. Compatibility is discovered dynamically from `BackendDescriptor` and `ExecutionOffer` data.

Task/Goal/policy constraints may further tighten the final `ExecutionRequest`.

## Backend-specific tuning

Provider/runtime-specific Agent fields are deliberately excluded from the portable manifest.

If an operator needs executor-specific tuning, it belongs in:

```text
backend configuration
or
route-policy configuration
```

not in logical Agent identity.

This preserves the ability to replace one backend implementation with another without editing the Agent.

## Canonical tools and actions

Pantheon does not expose backend-specific tool names as its canonical API.

Examples:

```text
filesystem.read
filesystem.write
filesystem.delete
shell.execute
process.spawn
network.connect
git.read
git.commit
git.push
git.merge
secret.use
secret.read
container.run
mcp.call
agent.delegate
browser.navigate
service.read
service.mutate
```

Tool bundles provide ergonomic groups:

```yaml
tools:
  bundles:
    - filesystem
    - git
    - shell
    - lsp
```

Execution backends translate canonical mechanisms into their native implementation where possible. If a required mechanism cannot be supported or safely compensated by Pantheon's sandbox/execution brokers, routing fails closed.

## Authorization model

The full security design is defined in `permissions-and-capabilities.md`.

Canonical authorization outcomes are binary:

```text
PERMIT
DENY
```

Manifest rule effects are:

```text
permit
forbid
```

Human approval is not a third authorization outcome. An approval-gated rule uses:

```yaml
- action: git.push
  resource: repo://**
  approval: required
```

The base request is denied as approvable. If the user approves it, Pantheon creates a scoped capability grant and evaluates authorization again.

A permission rule specifies either `effect` or `approval`, never both.

Authorization is enforced in layers:

```text
agent request
    ↓
Pantheon policy engine
    ↓
capability ticket / execution broker
    ↓
backend-native enforcement where available
    ↓
OS/container/VM sandbox
    ↓
resource
```

The model's interpretation of its prompt is never an authorization boundary.

## Policy hierarchy

Effective policy is resolved from:

```text
system hard policy
      ↓
user policy
      ↓
project policy
      ↓
agent policy
      ↓
task restrictions
      ↓
temporary capability grants
```

Rules:

- default deny;
- explicit hard forbid wins;
- lower scopes may restrict authority;
- lower scopes may not bypass enclosing forbids;
- temporary grants may satisfy approvable actions but not hard forbids.

## Workspace and sandbox

Worktree isolation and sandboxing are separate concepts.

### Worktree isolation

Protects concurrent coding Tasks from conflicting filesystem/Git modifications.

```yaml
workspace:
  isolation: worktree
```

### Sandbox profile

Protects the host/environment from execution.

```yaml
workspace:
  sandbox: developer
```

Pantheon should support progressively stronger profiles such as native/read-only, development/workspace, and isolated container/VM execution for CTF or untrusted code.

The Execution Fabric must only return offers that can satisfy the resolved workspace/isolation requirement, either natively or with Pantheon-managed compensation.

## Delegation

Delegation is a Pantheon control-plane operation, not a backend-native primitive.

```yaml
delegation:
  enabled: true
  allow:
    - researcher
    - reviewer
  maxDepth: 1
  maxConcurrent: 2
```

Delegation itself is authorized as a canonical action such as:

```text
agent.delegate
resource: agent://researcher
```

Before materializing child work Pantheon checks authorization, depth, concurrency, task-creation limits, Goal ownership, and inherited scope ceilings.

A backend-native subagent mechanism may later be used as an execution optimization, but Pantheon remains the source of truth for Task identity, lineage and policy.

## Limits

`spec.limits` defines ceilings, not live consumption:

```yaml
limits:
  maxTurns: 40
  maxRetries: 2
  wallTime: 30m
```

Live execution usage, resource reservations, and backend quotas belong to runtime scheduler/resource state.

## Review policy

Review is a first-class gate:

```yaml
review:
  policy: code-standard
  requiredBeforeMerge: true
```

Acceptance/review may combine deterministic checks, policy evaluation, independent reviewer agents, rubrics, and human approval as defined by the Acceptance Engine.

## Learning policy

The manifest configures learning behavior but never stores learned content.

```yaml
learning:
  reflection: after-task
  promotion:
    memory: automatic
    skill: eval-gated
    behavior: eval-gated
    soul: human-approval
```

Reflection remains a hypothesis; permanent self-modification requires the configured evidence/promotion process.

## Immutable Run execution snapshot

Execution reproducibility data is system-generated and is **not** part of `AGENT.yaml`.

Pantheon does not maintain a separate authoritative `RunManifest` object. The canonical Run resource contains an immutable specification/snapshot portion that records or references the exact resolved execution strategy.

It should capture at least:

```yaml
run:
  id: run-01J...
  task: task-...

  spec:
    agent: coder
    agentSpecHash: sha256:...
    soulHash: sha256:...
    behaviorHash: sha256:...

    skills:
      debugging: sha256:...
      testing: sha256:...

    memorySnapshot:
      - mem-123
      - mem-198

    executionBinding:
      ref: binding_01K...
      hash: sha256:...
      backend: executor://local-primary
      descriptorRevision: 17

    policyHash: sha256:...

    workspace:
      commit: ef8191...
      ref: workspace://task-291
```

Concrete runtime/model/session information may be retained as backend-namespaced audit metadata or Attempt-scoped adapter-private state for reproducibility and diagnostics. Core scheduling/routing logic must not depend on those values.

The immutable Run snapshot enables auditing, regression analysis and meaningful evaluation of self-improvement while preserving executor abstraction.

Capability grants/tickets used during the Run are referenced from runtime audit events rather than embedded into the static Agent Manifest.

Attempt-specific execution identity and LaunchKey belong to the Attempt, not this Run snapshot.

## A2A interoperability

Pantheon should eventually derive an A2A Agent Card from canonical Agent metadata, skill metadata, and exposed runtime interfaces instead of maintaining duplicate configuration.

A2A is an interoperability surface, not Pantheon's internal executor abstraction. An A2A remote system could later be wrapped behind an `ExecutorBackend` when useful.

A2A export is not required for v1.

## v1 scope

Include:

- `apiVersion`, `kind`, and metadata;
- Task specialties (`accepts`);
- Genome references;
- skill availability/preload;
- route policy;
- portable Execution Feature requirements;
- canonical tool bundles/actions;
- permission profile and permit/forbid/approval-gated rules;
- workspace and sandbox profiles;
- delegation allowlist/limits;
- execution ceilings;
- review policy;
- learning policy;
- immutable execution snapshot as part of the canonical Run resource.

Defer:

- complex manifest inheritance;
- dynamically generated Agent manifests;
- arbitrary inline shell hooks;
- A2A export;
- backend-specific execution optimizations;
- risk scoring.

Never allow:

- credentials or secrets inside the manifest;
- runtime quota/state inside the manifest;
- concrete provider/model/harness allowlists inside portable Agent identity;
- backend-specific tuning fields inside portable Agent identity;
- capability grants/tickets inside the manifest;
- Agents silently granting themselves additional authority;
- backend adapters weakening mandatory security policy.

## Key decisions

1. **Agent identity is backend/provider/model independent.**
2. **Agent manifests describe portable Execution Features, not compatible harness names.**
3. **Pantheon owns authorization and delegation.**
4. **Authorization is binary; approval creates a scoped grant.**
5. **`AGENT.yaml` is stable desired configuration; runtime and learned state live elsewhere.**
6. **Skills, tools/actions, specialty, Execution Features, and permissions are distinct abstractions.**
7. **Backend compatibility is dynamically discovered through the Execution Fabric and fails closed.**
8. **Backend-specific tuning lives outside logical Agent identity.**
9. **Each Run carries the immutable execution snapshot for one resolved strategy; Attempt-specific LaunchKey/attachment state lives under Attempt.**
