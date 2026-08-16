# Agent Manifest

## Status

Draft design — Pantheon subsystem specification.

## Purpose

`AGENT.yaml` is Pantheon's declarative, machine-readable contract for a persistent logical agent. It describes desired agent configuration independently of the model or harness that executes it.

The manifest complements the Agent Genome:

- `SOUL.md` — stable identity;
- `BEHAVIOR.md` — validated adaptive heuristics;
- memory — durable facts and context;
- skills — procedural knowledge;
- `AGENT.yaml` — specialties, execution compatibility, authorization policy, delegation, limits, review, and learning policy.

Runtime state, learned content, current sessions, capability grants, capability tickets, usage counters, and provider quota are deliberately excluded.

See also:

- `docs/architecture/agent-genome.md`
- `docs/architecture/permissions-and-capabilities.md`

## Foundational principles

### 1. Provider- and model-independent identity

An agent is not a Claude, OpenCode, Qwen, or other provider-specific configuration. Pantheon owns the canonical agent definition and compiles it into provider-specific sessions.

### 2. Declarative desired state

`AGENT.yaml` describes configuration, not observed runtime state.

Runtime state belongs in Pantheon's state store and event log.

### 3. Pantheon owns authorization and delegation

Provider-native permissions and subagent mechanisms are implementation details. Pantheon is the canonical authority for:

- what an agent may do;
- which resources it may access;
- which agents it may delegate to;
- depth and concurrency limits;
- sandbox/workspace requirements;
- review gates.

Provider adapters may tighten these constraints but may never broaden them.

### 4. Fail closed

If an executor cannot faithfully enforce a required policy, Pantheon must fail rather than silently weaken the policy.

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
    compatibleHarnesses:
      - claude-code
      - opencode
      - openai-compatible
    requirements:
      toolUse: true

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

  extensions:
    claude-code:
      effort: high
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

Human-readable summary used for selection and discovery. It describes what the agent is for, not how it is implemented.

### `spec.accepts`

Task classes for which the agent is a valid candidate.

Examples:

```text
code.implement
code.debug
code.review
research.web
security.ctf
security.reverse-engineering
```

Task classes express specialty, not authorization.

## Specialty, skills, tools, and permissions are distinct

Pantheon deliberately separates four concepts:

```text
SPECIALTY   What kind of task is this agent appropriate for?
SKILL       What reusable procedure does the agent know?
TOOL        What mechanism can the agent invoke?
PERMISSION  What action is the agent authorized to perform?
```

These must not be collapsed into a provider-specific tool list.

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

### Route policy, not a fixed model

Agents should normally reference a route policy:

```yaml
execution:
  routePolicy: coding-default
```

A separate routing subsystem resolves the concrete executor based on task requirements, privacy, availability, quality, cost/quota, and local resources.

Concrete provider/model selection belongs to the router and immutable execution record, not agent identity.

### Compatible harnesses

```yaml
compatibleHarnesses:
  - claude-code
  - opencode
  - openai-compatible
```

This is an allowlist of harnesses that can faithfully run the agent.

### Requirements

Capabilities an executor must provide before it can be selected:

```yaml
requirements:
  toolUse: true
```

Future requirements may include vision, structured output, minimum context size, MCP, browser control, or local-only execution.

## Canonical tools and capabilities

Pantheon does not expose Claude/OpenCode-specific tool names as its canonical API.

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

Provider adapters translate canonical capabilities into native mechanisms.

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

Human approval is not a third effect. An approval-gated rule uses:

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
provider/harness permission layer
    ↓
OS/container/VM sandbox
    ↓
resource
```

The LLM's interpretation of its prompt is never an authorization boundary.

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

Protects agents from conflicting code modifications.

```yaml
workspace:
  isolation: worktree
```

### Sandbox profile

Protects the host from the agent.

```yaml
workspace:
  sandbox: developer
```

Pantheon should support progressively stronger profiles such as native/read-only, development/workspace, and isolated container/VM execution for CTF or untrusted code.

## Delegation

Delegation is a Pantheon control-plane operation, not a provider-native primitive.

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

Before creating a child task Pantheon must check authorization, depth, concurrency, resource/quota policy, workspace requirements, executor availability, and record task lineage.

Provider-native subagents may be used as an optimization, but Pantheon remains the source of truth.

## Limits

`spec.limits` defines ceilings, not live consumption:

```yaml
limits:
  maxTurns: 40
  maxRetries: 2
  wallTime: 30m
```

Live provider usage and quota belong to the scheduler/resource manager.

## Review policy

Review is a first-class gate:

```yaml
review:
  policy: code-standard
  requiredBeforeMerge: true
```

The review subsystem may later combine automated tests, static checks, reviewer agents, and human approval.

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

This implements the Agent Genome rule that reflection is a hypothesis and permanent self-modification requires evidence.

## Provider extensions

Provider-specific settings are allowed only as an explicit non-portable escape hatch:

```yaml
extensions:
  claude-code:
    effort: high
  opencode:
    temperature: 0.1
```

Portable fields must not gradually become provider-specific options.

## Immutable Run Manifest

Every execution produces a separate immutable `RunManifest`. This is not authored by the user and is not part of `AGENT.yaml`.

It should capture at least:

```yaml
runId: run-01J...
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

executor:
  harness: claude-code
  model: concrete-model-id

policyHash: sha256:...

workspace:
  commit: ef8191...
  worktree: task-291
```

The Run Manifest enables reproducibility, auditing, regression analysis, and meaningful evaluation of self-improvement.

Capability grants and tickets used during the run are referenced from runtime audit events rather than embedded into the static Agent Manifest.

## A2A interoperability

Pantheon should eventually derive an A2A Agent Card from canonical agent metadata, skill metadata, and exposed runtime interfaces instead of maintaining duplicate configuration.

A2A export is not required for v1.

## v1 scope

Include:

- `apiVersion`, `kind`, and metadata;
- task specialties (`accepts`);
- Genome references;
- skill availability/preload;
- route policy and compatible harnesses;
- canonical tool bundles/capabilities;
- permission profile and `permit`/`forbid`/approval-gated rules;
- workspace and sandbox profiles;
- delegation allowlist/limits;
- execution ceilings;
- review policy;
- learning policy;
- provider extension escape hatch;
- immutable Run Manifest generation.

Defer:

- complex manifest inheritance;
- dynamically generated agent manifests;
- arbitrary inline shell hooks;
- A2A export;
- advanced provider-specific compilation optimizations;
- risk scoring.

Never allow:

- credentials or secrets inside the manifest;
- runtime quota/state inside the manifest;
- capability grants/tickets inside the manifest;
- agents silently granting themselves additional authority;
- provider adapters weakening mandatory security policy.

## Key decisions

1. **Agent identity is provider/model independent.**
2. **Pantheon owns authorization and delegation.**
3. **Authorization is binary; approval creates a scoped grant.**
4. **`AGENT.yaml` is stable desired configuration; runtime and learned state live elsewhere.**
5. **Skills, tools, specialty, and permissions are distinct abstractions.**
6. **Provider compilation is fail-closed.**
7. **Every execution is captured by an immutable Run Manifest.**
