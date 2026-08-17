# Agent Manifest

## Status

Canonical Pantheon Logical Agent declarative contract (`AGENT.yaml`).

## Purpose

`AGENT.yaml` describes a persistent Logical Agent independently of concrete execution provider/model/harness/runtime.

It complements the static v1 Genome inputs:

```text
SOUL.md
BEHAVIOR.md
approved Skills
bounded Memory
```

Runtime state, sessions, Attempts, capability Grants/tickets, Reservations, Budget usage, backend health and concrete execution identifiers are excluded.

## Foundational distinctions

```text
TASK TYPE (`accepts`)
  what class of Task may this Agent own?

COMPETENCY
  what semantic ability is the Agent trusted to provide?

SKILL
  what reusable procedure/guidance is available?

ACTION / TOOL
  what semantic operation may be requested?

EXECUTION FEATURE
  what backend mechanism is required?

PERMISSION / GRANT
  may the principal perform an action now?

WORKSPACE STRATEGY
  how Task mutable repository state is materialized

SANDBOX PROFILE / GUARANTEE
  what physical isolation the Run requires
```

These are not aliases.

## Example

```yaml
apiVersion: pantheon/v1alpha1
kind: Agent

metadata:
  name: coder
  displayName: Atlas
  labels:
    domain: software-engineering

spec:
  description: >
    Implements, debugs, refactors, and reviews software.

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
      namespaces: [agent, project]
      retrieval:
        mode: adaptive
        maxTokens: 4000
    skills:
      available: [git, debugging, testing, code-review]
      preload: [git]

  execution:
    routePolicy: coding-default
    requirements:
      executionFeatures:
        - session.interactive
        - tools.structured
        - control.result-submit
        - control.artifact-seal
        - control.action-invoke
      minContextTokens: 64000

  tools:
    bundles: [filesystem, git, shell, lsp]
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
    strategy: isolated-clone

  sandbox:
    profile: developer-default
    requirements:
      - isolation.control-plane
      - isolation.peer-workspaces
      - process.no-privilege-escalation

  delegation:
    enabled: true
    allow: [researcher]
    maxDepth: 1
    maxConcurrent: 2

  limits:
    maxTurns: 40
    wallTime: 30m

  learning:
    reflection: disabled
    promotion:
      memory: disabled
      skill: disabled
      behavior: disabled
      soul: disabled
```

## Applicability and competencies

`accepts` and `competencies` are hard semantic eligibility inputs. Agent Resolver uses them deterministically before execution routing.

They are operator/config controlled and cannot silently expand through Agent Genome learning.

## Genome

Manifest references approved/versioned Genome sources. Context Builder resolves selected immutable versions into the Run ContextPlan.

For `genome.memory.retrieval.mode`, `adaptive` means Pantheon-controlled deterministic relevance selection against the frozen Run inputs; it does **not** mean model/Agent judgment. `always` runs the configured deterministic retriever for every Run, while `disabled` skips initial Memory preload retrieval. The Context Builder freezes exact selected Memory versions plus the retriever/policy/index provenance needed to reconstruct the selection.

V1 runtime uses static approved SOUL/BEHAVIOR/Skills/Memory. Automatic reflection/promotion is implementation-deferred; manifests should configure learning disabled for v1 deployments.

## Execution requirements

`execution.routePolicy` names a logical configured route policy resolved through ConfigurationRevision. Concrete providers/models/backends do not appear here.

`execution.requirements.executionFeatures` names factual mechanisms an offer must support, including Agent Control semantic features when needed.

`minContextTokens` is compatibility/capacity requirement, not a Budget.

## Tools/actions

`tools.actions` is the canonical semantic operation surface the Agent may need to request. Availability is not authorization.

Actual operation flows through Agent Control/current policy/Grant redemption and the appropriate broker/controller.

## Permissions

Manifest permissions define part of the Agent's frozen authority ceiling/configured policy input. Current hard/config policy may further restrict it; temporary Grants may satisfy approvable actions but cannot bypass hard/frozen forbids.

Agent `secret.read` is hard-denied by v1 built-in policy even if a malformed manifest attempted to permit it.

## Workspace strategy is not Sandbox isolation

`workspace.strategy` chooses Task mutable repository materialization:

```text
none
isolated-clone
linked-worktree
copy
```

It does **not** claim security isolation.

For untrusted model-driven shell coding, `isolated-clone` is the preferred v1 repository strategy so the worker can use local Git without writable access to authoritative shared `GIT_COMMON_DIR` state.

`linked-worktree` may be valid for trusted/safely projected contexts but cannot by itself satisfy `isolation.control-plane`.

Task-local Git metadata remains Agent-writable/untrusted even under `isolated-clone`. The Workspace strategy does not authorize a privileged Pantheon controller to execute Git against that repository state. Controller-side Git capture/recovery must satisfy the hostile-repository boundary in `workspace-and-git-integration.md`: use controller-owned sterile Git control state when logical content is sufficient, or an equally confined helper when Agent-owned Git metadata must be interpreted.

## Sandbox

`sandbox.profile` names a logical SandboxProfile from ConfigurationRevision. `sandbox.requirements` expresses hard physical guarantees the Sandbox Planner must prove, such as:

```text
isolation.control-plane
isolation.peer-workspaces
isolation.host-credentials
process.no-privilege-escalation
```

A provider/ExecutorBackend cannot self-award these guarantees. If Sandbox Planner cannot establish them, the Agent+Offer strategy is incompatible/fails closed.

## Delegation

Delegation means authority to request bounded child Tasks, not to spawn unmanaged subagents/processes outside Pantheon.

V1 runtime spawn is blocking/yielding only. Child Agent is chosen later by normal Agent Resolution. `allow` constrains permitted Logical Agent/task delegation policy where used; it does not pick a concrete backend/model.

## Limits

Agent limits are ceilings such as turns/wall time. Recovery retry counters are not Agent manifest limits and belong to RecoveryPolicy.

## Review

`review` describes desired integration/review policy references where project configuration uses them; authoritative Task Acceptance is defined by Task acceptance criteria/EvaluatorVersions, not by a provider-native reviewer feature.

## Configuration and freezing

Agent source config compiles into immutable Agent snapshots within ConfigurationRevision. A Run freezes the selected Agent version and ContextPlan; later manifest/Genome edits affect future Runs only except current security policy may tighten live authority.

## Core invariants

1. Agent identity/config is provider/model/backend independent.
2. `accepts`, competencies, Skills, actions, execution features, permissions, Workspace strategy and Sandbox guarantees are distinct.
3. Workspace strategy never claims security isolation.
4. Untrusted shell requires a SandboxProfile proving control-plane isolation independently of Git/worktree strategy, and Agent-writable repository state may not induce privileged Pantheon/controller execution outside that containment boundary.
5. Agent Control operation availability does not grant authorization.
6. Recovery retry state is not stored in Agent manifest.
7. Run freezes exact Agent/Genome/config inputs; later edits do not mutate it.
8. V1 automatic Genome mutation/promotion is disabled/deferred.
