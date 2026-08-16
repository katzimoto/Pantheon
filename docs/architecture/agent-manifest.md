# Agent Manifest

## Status

Draft design — Pantheon subsystem specification.

## Purpose

`AGENT.yaml` is Pantheon's declarative, machine-readable contract for a persistent logical agent. It describes the desired configuration of an agent independently of the model or harness that executes it.

The manifest complements the Agent Genome:

- `SOUL.md` — stable identity;
- `BEHAVIOR.md` — validated adaptive heuristics;
- memory — durable facts and context;
- skills — procedural knowledge;
- `AGENT.yaml` — capabilities, execution compatibility, authorization, delegation, limits, review, and learning policy.

Runtime state, learned content, usage counters, current sessions, and provider quota are deliberately excluded from the manifest.

## Foundational principles

### 1. Provider- and model-independent identity

An agent is not a Claude, OpenCode, Qwen, or other provider-specific configuration. Pantheon owns the agent definition and compiles it into provider-specific sessions.

A coder remains the same logical agent regardless of whether a task is executed by Claude Code, OpenCode, or a local OpenAI-compatible endpoint.

### 2. Declarative desired state

`AGENT.yaml` describes desired configuration, not observed state.

Runtime state belongs in Pantheon's state store and event log. Examples that must not be stored in the manifest:

- current session ID;
- active task;
- selected model for a particular run;
- tokens consumed;
- remaining provider quota;
- last error;
- learned memories;
- pending reflections;
- current health/status.

### 3. Pantheon owns authorization and delegation

Provider-native permissions and subagent mechanisms are implementation details. Pantheon is the canonical authority for:

- what an agent may do;
- which resources it may access;
- which agents it may delegate to;
- depth/concurrency limits;
- sandbox/workspace requirements;
- review gates.

Provider adapters may enforce these constraints, but may never broaden them.

### 4. Fail closed

If an executor cannot faithfully enforce a required policy, compilation must fail rather than silently weakening the policy.

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
      - action: git.push
        effect: ask
      - action: filesystem.external
        effect: deny
      - action: secrets.read
        effect: deny

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

Human-readable summary used for agent selection and discovery. It should describe what the agent is for, not how it is implemented.

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

Task classes represent specialty, not authorization.

## Specialty, skills, tools, and permissions are distinct

Pantheon deliberately separates four concepts:

```text
SPECIALTY   What kind of task is this agent appropriate for?
SKILL       What reusable procedure does the agent know?
TOOL        What mechanism can the agent invoke?
PERMISSION  What action is the agent authorized to perform?
```

For example:

```yaml
accepts:
  - code.debug

skills:
  available:
    - debugging

tools:
  bundles:
    - shell
    - filesystem

permissions:
  profile: developer
```

These must not be collapsed into a single provider-specific `tools` list.

## Genome references

`spec.genome` points to the persistent knowledge and identity layers defined by the Agent Genome subsystem.

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

`available` means the skill may be activated when relevant. `preload` means the full skill instructions are loaded at session start.

Preloading should be uncommon because context is a scarce resource.

## Execution policy

### Route policy, not a fixed model

Agents should normally reference a route policy:

```yaml
execution:
  routePolicy: coding-default
```

A separate routing configuration resolves the actual executor based on task requirements, privacy, availability, quality, cost/quota, and local resources.

Example conceptually:

```yaml
routes:
  coding-default:
    premium:
      harness: claude-code
    hosted:
      harness: opencode
      pool: opencode-go
    local:
      harness: openai-compatible
      pool: local-mlx
```

Concrete provider/model selection belongs to the router and immutable execution record, not agent identity.

### Compatible harnesses

```yaml
compatibleHarnesses:
  - claude-code
  - opencode
  - openai-compatible
```

This is an allowlist of execution harnesses that can faithfully run the agent.

### Requirements

Capabilities an executor must provide before it can be selected:

```yaml
requirements:
  toolUse: true
```

Future requirements may include vision, structured output, minimum context size, MCP, browser control, or local-only execution.

## Canonical tools

Pantheon must not expose Claude/OpenCode-specific names as its canonical API.

Recommended capability names include:

```text
filesystem.read
filesystem.write
filesystem.external.read
filesystem.external.write
shell.execute
network.http
network.raw
git.read
git.commit
git.push
git.merge
secrets.read
browser.navigate
process.spawn
container.run
delegate.spawn
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

Canonical permission effects:

```text
allow
ask
deny
```

`deny` is always strongest.

Example:

```yaml
permissions:
  profile: developer
  rules:
    - action: git.push
      effect: ask
    - action: filesystem.external
      effect: deny
```

Authorization should be enforced in layers:

```text
agent request
    ↓
Pantheon policy engine
    ↓
provider/harness permission layer
    ↓
OS/container sandbox
    ↓
resource
```

An LLM's own interpretation of a prompt is never an authorization boundary.

## Policy merging

Pantheon will combine policy from multiple scopes:

```text
global
  ↓
project
  ↓
agent
  ↓
task
  ↓
temporary human grant
```

### Permissions

Default rule:

```text
deny > ask > allow
```

More local configuration may tighten authority but must not silently broaden authority beyond an enclosing policy.

### Limits

The effective value is normally the lowest applicable ceiling.

Example:

```text
global maxTurns = 100
agent  maxTurns = 40
task   maxTurns = 20

effective = 20
```

### Skills

Effective skill availability should be the intersection of:

- project-allowed skills;
- agent-available skills;
- runtime compatibility;
- task policy.

## Workspace and sandbox

These are separate concepts.

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

Examples of future sandbox profiles:

```text
read-only
developer
trusted-developer
research
ctf
admin
```

A security/CTF agent might use a Git worktree for repository isolation and a disposable container/VM for process/network isolation.

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

When an agent requests delegation, Pantheon must:

1. authorize the target agent;
2. check depth and concurrency;
3. check resource/quota policy;
4. create a child task;
5. create/select a workspace;
6. select an executor;
7. record task lineage;
8. monitor/reconcile the child task.

Provider-native subagents may be used as an optimization, but Pantheon remains the source of truth.

Delegation is allowlisted by default.

## Limits

`spec.limits` defines execution ceilings, not live consumption:

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

The review policy may later select automated tests, static checks, reviewer agents, human approval, or combinations thereof.

## Learning policy

The manifest configures how learning occurs but never stores learned content.

```yaml
learning:
  reflection: after-task
  promotion:
    memory: automatic
    skill: eval-gated
    behavior: eval-gated
    soul: human-approval
```

This directly implements the Agent Genome rule that reflection is a hypothesis and permanent self-modification requires evidence.

## Provider extensions

Provider-specific settings are permitted only as an explicit non-portable escape hatch:

```yaml
extensions:
  claude-code:
    effort: high
  opencode:
    temperature: 0.1
```

Portable fields must never gradually become a collection of provider-specific options. Most agents should work without `extensions`.

## Immutable Run Manifest

Every execution produces a separate immutable `RunManifest`. This is not authored by the user and is not part of `AGENT.yaml`.

Example:

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

The Run Manifest enables reproducibility, auditability, regression analysis, and meaningful evaluation of self-improvement.

## A2A interoperability

Pantheon should eventually derive an A2A Agent Card from canonical agent metadata, skill metadata, and exposed runtime interfaces rather than requiring duplicate configuration.

Conceptually:

```text
AGENT.yaml
   +
skill metadata
   +
runtime interface
   ↓
A2A Agent Card
```

A2A export is not required for v1.

## v1 scope

Include:

- `apiVersion`, `kind`, and metadata;
- task specialties (`accepts`);
- Genome references;
- skill availability/preload;
- route policy and compatible harnesses;
- canonical tool bundles;
- permission profile/rules;
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
- advanced provider-specific compilation optimizations.

Never allow:

- credentials or secrets inside the manifest;
- runtime quota/state inside the manifest;
- agents silently granting themselves additional authority;
- provider adapters weakening mandatory security policy.

## Object model

```text
                         AGENT
                           │
        ┌──────────────────┼───────────────────┐
        │                  │                   │
        ▼                  ▼                   ▼
     IDENTITY          EXECUTION            POLICY
        │                  │                   │
     SOUL.md          compatible           permissions
   BEHAVIOR.md         harnesses             sandbox
        │              routing               limits
        │                  │                   │
        ▼                  ▼                   ▼
     KNOWLEDGE          TOOLS            DELEGATION
        │
     memory
     skills
        │
        ▼
     LEARNING
        │
    experiences
    reflection
    evaluation
```

## Controller flow

```text
Pantheon Controller
       ↓
resolve Agent manifest
       ↓
resolve Task specification
       ↓
resolve effective policy
       ↓
select executor
       ↓
select/retrieve context
       ↓
create workspace/sandbox
       ↓
generate immutable Run Manifest
       ↓
┌─────────────┬──────────────┐
▼             ▼              ▼
Claude Code   OpenCode       Local/OpenAI-compatible
```

## Key decisions

1. **Agent identity is provider/model independent.**
2. **Pantheon owns authorization and delegation.**
3. **`AGENT.yaml` is stable desired configuration; runtime and learned state live elsewhere.**
4. **Skills, tools, specialty, and permissions are distinct abstractions.**
5. **Provider compilation is fail-closed.**
6. **Every execution is captured by an immutable Run Manifest.**
