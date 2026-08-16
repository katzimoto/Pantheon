# Agent Manifest

## Status

Draft design — Pantheon Agent subsystem specification.

## Purpose

`AGENT.yaml` is Pantheon's declarative, machine-readable contract for a persistent Logical Agent. It describes desired Agent configuration independently of the concrete backend, runtime, model, or harness that executes it.

The manifest complements the Agent Genome:

- `SOUL.md` — stable identity;
- `BEHAVIOR.md` — validated adaptive heuristics;
- memory — durable facts/context;
- skills — procedural knowledge;
- `AGENT.yaml` — applicability, competencies, execution requirements, authorization policy, delegation, limits, review, and learning policy.

Runtime state, learned content, sessions, capability grants/tickets, reservations, budget consumption, recovery counters, backend health, and concrete executor details are excluded.

See also:

- `docs/architecture/agent-genome.md`
- `docs/architecture/logical-agent-resolution.md`
- `docs/architecture/permissions-and-capabilities.md`
- `docs/architecture/execution-fabric.md`
- `docs/architecture/run-and-attempt.md`
- `docs/architecture/recovery-policy.md`

## Foundational principles

### Logical identity is execution-independent

An Agent is not a model, provider, CLI harness, endpoint, or local runtime configuration.

Pantheon owns the canonical Logical Agent. Agent Resolution determines semantic eligibility; the Execution Fabric later resolves eligible Agent + execution pairs.

### Declarative desired state

`AGENT.yaml` describes configured intent, not observed runtime state.

Runtime state belongs in Pantheon's state store and event log.

### Applicability is not self-promoted

`accepts` and `competencies` define which work the Agent is configured/trusted to own. Genome learning may propose changes, but it must not silently broaden either set.

### Pantheon owns authorization and delegation

Backend-native permissions and subagent mechanisms are implementation details. Pantheon remains canonical for actions, resources, delegation, sandbox/workspace policy, approvals, and review gates.

### Fail closed

If an execution configuration cannot faithfully enforce mandatory policy, Pantheon rejects it rather than weakening policy.

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
    Use for Tasks that require modifying or understanding code.

  accepts:
    - code.implement
    - code.debug
    - code.refactor
    - code.review

  competencies:
    - code.analysis
    - code.debugging
    - code.editing
    - test.execution

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

    actions:
      - filesystem.read
      - filesystem.write
      - shell.execute
      - git.commit

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

## Applicability: `accepts`

`spec.accepts` lists Task types for which this Agent may be considered.

Examples:

```text
code.implement
code.debug
code.review
research.web
security.ctf
security.reverse-engineering
```

Task type expresses work class, not authorization or executor compatibility.

## Competencies

`spec.competencies` lists semantic abilities this Agent is configured/trusted to provide.

Examples:

```text
code.analysis
code.debugging
code.editing
test.execution
security.analysis
reverse-engineering
web.research
```

A competency is distinct from a Skill, Tool/Action, Execution Feature, or authorization capability grant.

```text
TASK TYPE
  What class of Task may this Agent own?

COMPETENCY
  What semantic ability can this Agent provide?

SKILL
  What reusable procedure does the Agent know?

TOOL / ACTION
  What canonical mechanism may be invoked?

EXECUTION FEATURE
  What mechanism must an ExecutorBackend provide?

PERMISSION
  What action is authorized?

CAPABILITY GRANT / TICKET
  What concrete temporary authorization exists?
```

Agent Resolution first performs deterministic eligibility using `accepts`, Task-required `competencies`, enabled state and hard policy. Descriptions/skills may influence ranking only among valid candidates.

`accepts` and `competencies` are control-plane configuration. They cannot be silently expanded by Agent self-reflection or Skill promotion.

## Genome references

`spec.genome` references persistent identity/knowledge layers but does not inline them.

### Memory policy

The manifest configures retrieval policy, not memory contents.

### Skills

Pantheon uses progressive disclosure:

```yaml
skills:
  available:
    - git
    - debugging
    - postgres
  preload:
    - git
```

`available` means a Skill may be activated when relevant. `preload` means its instructions are loaded at execution-context construction time.

Skills may improve semantic affinity/ranking but are not hard Task requirements; Tasks require competencies.

## Execution policy

Agents reference a provider-independent route policy:

```yaml
execution:
  routePolicy: coding-default
```

An Agent may declare intrinsic portable requirements:

```yaml
execution:
  requirements:
    executionFeatures:
      - session.interactive
      - tools.structured
    minContextTokens: 64000
```

Execution Features use Pantheon-defined provider-neutral semantics, for example:

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

The Agent does **not** maintain a concrete provider/harness/model allowlist. Backend compatibility is discovered dynamically.

Backend-specific tuning belongs in backend or route-policy configuration, never portable Agent identity.

## Canonical tools and actions

Pantheon exposes canonical actions rather than backend-native tool names.

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

Tool bundles provide ergonomic groups. `tools.actions` may explicitly enumerate canonical actions exposed through the Agent's execution context.

Tool availability still does not imply authorization.

## Authorization model

The full security design is defined in `permissions-and-capabilities.md`.

Canonical authorization outcomes are binary:

```text
PERMIT
DENY
```

Manifest rules use either `effect: permit|forbid` or `approval: required`, never both.

Approval creates a scoped capability grant; the request is then re-evaluated.

Effective authority remains:

```text
system hard policy
      ∩
user policy
      ∩
project policy
      ∩
Agent policy
      ∩
Task restrictions
      ∩
temporary grants
```

The model's interpretation of its prompt is never an authorization boundary.

## Workspace and sandbox

Worktree isolation and sandboxing are separate:

```yaml
workspace:
  isolation: worktree
  sandbox: developer
```

Worktree isolation protects concurrent repository modification. Sandbox policy protects host/environment execution.

The Execution Fabric may return only offers that can satisfy resolved workspace/isolation requirements, natively or with Pantheon-managed compensation.

## Delegation

Delegation is a Pantheon control-plane operation, not a backend-native authority mechanism.

```yaml
delegation:
  enabled: true
  allow:
    - researcher
    - reviewer
  maxDepth: 1
  maxConcurrent: 2
```

Pantheon checks authorization, depth, concurrency, Task creation limits, Goal ownership, inherited scope ceilings and graph semantics before materializing child work.

Backend-native subagent functionality may later be an execution optimization; Pantheon remains source of truth for Task identity/lineage/policy.

## Limits

`spec.limits` defines intrinsic execution ceilings that are meaningful for a Logical Agent, not consumed runtime quota or recovery policy.

```yaml
limits:
  maxTurns: 40
  wallTime: 30m
```

Retry/recovery ceilings belong to `RecoveryPolicy`, because reconnects, Attempt retries, strategy retries, and acceptance retries have different semantics. Tokens/cost, ResourceReservations, and backend quota state belong to their dedicated runtime subsystems.

## Review and learning

Review is a first-class policy gate.

Learning policy configures reflection and promotion behavior but never stores learned content.

Reflection remains a hypothesis; permanent self-modification requires the configured evidence/promotion process.

## Immutable Run execution snapshot

Execution reproducibility data is system-generated and is not part of `AGENT.yaml`.

Pantheon does not maintain a separate authoritative `RunManifest`. The canonical Run resource has an immutable strategy/snapshot portion recording or referencing the exact resolved execution strategy.

Conceptually:

```yaml
run:
  id: run-01J...
  task: task-...

  spec:
    agent: agent://coder
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
      backend: executor://a
      descriptorRevision: 17

    policyHash: sha256:...

    workspace:
      commit: ef8191...
      ref: workspace://task-291
```

Concrete runtime/model/session information may be retained as backend-namespaced audit metadata or Attempt-scoped opaque state. Core routing/scheduling logic never depends on it.

Attempt-specific LaunchKey and backend attachment state belong to Attempt, not Run.

## A2A interoperability

Pantheon may eventually derive an A2A Agent Card from canonical Agent/Skill metadata instead of duplicating configuration. A2A is an interoperability surface, not Pantheon's internal execution abstraction.

A remote A2A system could later be wrapped behind `ExecutorBackend` if useful.

## v1 scope

Include:

- metadata;
- `description`;
- explicit Task applicability (`accepts`);
- explicit semantic `competencies`;
- Genome references;
- Skill availability/preload;
- route policy and portable Execution Features;
- canonical tool bundles/actions;
- permission profile/rules;
- workspace and sandbox profile;
- delegation controls;
- intrinsic execution ceilings such as turns/wall time;
- review policy;
- learning policy;
- immutable Agent/Genome snapshots inside canonical Run strategy state.

Defer:

- complex manifest inheritance;
- dynamically generated Agent manifests;
- arbitrary inline shell hooks;
- A2A export;
- backend-specific execution optimizations;
- opaque Agent quality scoring.

Never allow:

- credentials or secrets inside the manifest;
- runtime quota/usage inside the manifest;
- retry/recovery counters or policy inside generic Agent limits;
- concrete provider/model/harness allowlists inside portable Agent identity;
- backend-specific tuning fields inside portable Agent identity;
- capability grants/tickets inside the manifest;
- Agents silently expanding their own `accepts` or `competencies`;
- backend adapters weakening mandatory security policy.

## Key decisions

1. Agent identity is backend/provider/model independent.
2. `accepts` defines Task-type applicability; `competencies` defines trusted semantic abilities.
3. Agent applicability/competencies are control-plane configuration and cannot be silently self-promoted.
4. Skills, competencies, actions/tools, Execution Features, and permissions remain separate abstractions.
5. Agent Resolver determines semantic eligibility before final joint Agent + ExecutionOffer selection.
6. Agent manifests describe portable Execution Features, not compatible harness names.
7. Pantheon owns authorization and delegation.
8. `AGENT.yaml` is desired configuration; runtime and learned state live elsewhere.
9. Retry/recovery semantics live in RecoveryPolicy, not ambiguous generic Agent limits.
10. Backend compatibility is dynamically discovered through Execution Fabric and fails closed.
11. Backend-specific tuning lives outside Logical Agent identity.
12. Each Run carries one immutable resolved strategy snapshot; Attempt-specific LaunchKey/attachment state lives under Attempt.
