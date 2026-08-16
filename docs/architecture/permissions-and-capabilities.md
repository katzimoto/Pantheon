# Permissions and Capabilities

## Status

Draft design — Pantheon security subsystem specification.

## Purpose

Pantheon must remain the canonical authorization authority regardless of which model or harness executes an agent. Claude Code, OpenCode, local models, MCP servers, shells, containers, and future providers are execution mechanisms; none of them owns the security model.

The subsystem answers one question:

> May principal X perform action Y on resource Z under context C?

Authorization is binary: **PERMIT** or **DENY**.

Human approval is not a third authorization outcome. An approval creates a scoped, temporary capability grant and the request is evaluated again.

## Foundational principles

1. **Pantheon owns authorization.** Models and harnesses may never broaden authority.
2. **Default deny.** An action is denied unless an applicable policy or capability grant permits it.
3. **Explicit forbid wins.** Hard forbids cannot be bypassed by lower-level policy or agent requests.
4. **Approval creates a capability grant.** It never silently changes broad trust.
5. **Authorization and sandboxing are separate.** Policy determines what an agent may do; the sandbox limits what the process can physically do.
6. **Fail closed.** If Pantheon cannot enforce a required policy, the task does not start.
7. **Every privileged action is auditable.** Requests, decisions, grants, execution, and outcomes are recorded.

## Enforcement architecture

```text
Agent / model
    │ action request
    ▼
Action Normalizer
    │ canonical principal/action/resource/context
    ▼
Policy Decision Point
    │ PERMIT / DENY
    ├────────────── DENY, non-approvable ──> stop
    │
    └────────────── DENY, approvable
                         │
                         ▼
                   Approval Broker
                         │ human grants scope
                         ▼
                   Capability Grant
                         │
                         └──── re-evaluate

PERMIT
    │
    ▼
Capability Ticket
    │
    ▼
Execution Broker
    │
    ▼
Harness permissions
    │
    ▼
OS / container / VM sandbox
    │
    ▼
Resource
```

The LLM never decides whether its own request is authorized.

## Policy decision point

Pantheon should embed Cedar as the authorization engine. Cedar maps directly to Pantheon's domain:

```text
principal
  action
resource
 context
```

and provides the desired semantics:

- implicit deny when no permit applies;
- explicit `forbid` overrides `permit`;
- schema validation for authorization entities and actions;
- deterministic authorization independent of the LLM.

Pantheon users should not need to write Cedar for normal use. `AGENT.yaml`, project policy, user policy, and built-in profiles compile into Cedar policies. Native Cedar files may be supported later as an advanced escape hatch.

## Canonical action model

Provider-specific tool names are not canonical permissions. Pantheon defines semantic actions and adapters translate them.

Initial action namespaces:

```text
filesystem.read
filesystem.write
filesystem.delete

shell.execute
process.spawn

network.connect
network.listen

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

Actions may become more specific over time, but should describe effects rather than provider APIs.

## Resource namespace

Resources use typed URI-like identifiers so the policy engine can reason about them consistently.

Initial families:

```text
file://
workspace://
repo://
net://
secret://
process://
container://
mcp://
agent://
browser://
service://
device://
```

Examples:

```text
workspace://src/auth/login.rs
file:///Users/example/.ssh/config
repo://Pantheon/origin/main
net://github.com:443
secret://github/pat
mcp://github/create_issue
agent://security
container://kali
service://production/postgres
```

## Permission rules in `AGENT.yaml`

Canonical rule effects are only:

```text
permit
forbid
```

Approval is policy metadata, not an authorization effect.

Example:

```yaml
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
```

An `approval: required` rule means the base request is denied as approvable. If the user approves, Pantheon creates a scoped grant; the policy is then evaluated again.

A rule must specify either `effect` or `approval`, not both.

## Why `ask` is not an authorization result

Interactive harnesses often expose `allow`, `ask`, and `deny`. Pantheon is a control plane rather than an interactive tool wrapper.

The security state for a push should be:

```text
DENY
reason: approval-required
approvable: true
```

not `ASK`.

If the human chooses **Allow once**, Pantheon creates something conceptually equivalent to:

```yaml
grant:
  principal:
    agent: coder
    run: run-921

  capability:
    action: git.push
    resource: repo://Pantheon/origin/feature/auth

  constraints:
    task: task-483
    uses: 1
    expiresIn: 5m
```

Authorization is evaluated again and may then return PERMIT.

## Capability grants

A capability grant is an explicit delegation of authority from a trusted principal, normally the human user or a higher-level policy authority.

Grants should be scoped by as many dimensions as practical:

- principal / agent;
- run;
- task;
- action;
- resource;
- expiration;
- maximum uses;
- optional argument constraints.

Expected approval scopes:

```text
allow once
allow for this task
allow for this run
allow this action on this resource
persist for this project
```

Persistent approvals become explicit project/user policy. Short-lived approvals remain runtime grants.

## Capability tickets

After authorization, Pantheon issues a short-lived internal capability ticket to the execution broker.

Example:

```yaml
ticket:
  id: cap-938
  run: run-124
  task: task-83
  agent: coder
  action: filesystem.write
  resource: workspace://src/auth.rs
  argsHash: sha256:18aa...
  uses: 1
  expires: 2026-08-16T04:00:00+03:00
```

The ticket binds authorization to the exact operation. Authorization for one command or argument set cannot be reused for a materially different operation.

Tickets are runtime state and never belong in `AGENT.yaml`.

## Secrets

Pantheon distinguishes using a credential from revealing it to a model.

```text
secret.use   authorize a broker to use a credential on the agent's behalf
secret.read  reveal credential material to the caller
```

The preferred pattern is:

```text
agent requests git.push
       ↓
secret.use permitted
       ↓
credential broker authenticates operation
       ↓
secret value never enters model context
```

`secret.read` should be forbidden by default and reserved for exceptional workflows.

## Policy hierarchy

Effective policy is resolved from multiple scopes:

```text
SYSTEM HARD POLICY
        ↓
USER POLICY
        ↓
PROJECT POLICY
        ↓
AGENT POLICY
        ↓
TASK RESTRICTIONS
        ↓
TEMPORARY GRANTS
```

Rules:

1. any applicable hard forbid wins;
2. lower scopes may restrict authority;
3. lower scopes may not bypass enclosing forbids;
4. temporary grants can satisfy approvable actions but not hard forbids;
5. absence of an applicable permit results in DENY.

## Hard policy vs normal policy

Hard policy protects boundaries that agents must never be able to negotiate away.

Examples:

```text
forbid secret.read secret://**
forbid filesystem.write file://~/.ssh/**
forbid filesystem.write file://~/.pantheon/policies/**
```

A hard-policy denial is non-approvable during the run.

Normal policy may mark an action approval-gated. For example, pushing to a protected branch can be denied but approvable.

## Authorization vs sandboxing

Authorization is the decision layer:

```text
What may this agent do?
```

Sandboxing is the containment layer:

```text
What can this process physically do if it misbehaves?
```

Both are required.

```text
Agent request
    ↓
Pantheon authorization
    ↓
Harness-native restrictions
    ↓
OS/container/VM sandbox
    ↓
Resource
```

Provider-native security settings are defense in depth, not Pantheon's source of truth.

## Sandbox classes

Pantheon should begin with three conceptual classes.

### `native`

For low-risk or read-only work. Runs on the host with policy enforcement and minimal additional isolation.

### `workspace`

For routine development. Combines:

- isolated Git worktree;
- filesystem boundary;
- network policy;
- process restrictions;
- harness-native sandbox where available.

### `isolated`

For CTFs, untrusted code, unknown binaries, dependency experiments, or other higher-risk work. Prefer a disposable Linux container or VM with:

- non-root execution;
- no host runtime socket;
- restricted mounts;
- seccomp / platform sandboxing;
- explicit network policy;
- ephemeral writable state where practical.

Worktree isolation and sandbox isolation are separate and can be combined.

## Broker privileged infrastructure

Agents do not receive direct control of privileged infrastructure such as Docker sockets, SSH agents, cloud control planes, Kubernetes admin credentials, or production databases.

Instead:

```text
agent request
    ↓
canonical action
    ↓
Pantheon authorization
    ↓
privileged broker
    ↓
external system
```

For example, `container.run` is handled by a Container Broker. The agent does not get `/var/run/docker.sock`.

## Provider compilation

Pantheon compiles effective policy into the strongest native controls each harness supports.

```text
Canonical Pantheon policy
         ↓
Provider compiler
         ↓
Claude/OpenCode/local restrictions
         ↓
Pantheon/OS sandbox
```

Compilation rules:

```text
Can provider enforce policy directly?
    yes → use native enforcement
    no  → can Pantheon sandbox/broker compensate?
              yes → launch constrained
              no  → fail closed
```

Provider adapters may tighten policy but may never broaden it.

## Delegation authorization

Delegation is itself a privileged action:

```text
agent.delegate
resource: agent://researcher
```

Before creating a child task Pantheon checks:

- delegation allowlist;
- authorization policy;
- depth;
- concurrency;
- task/run budget;
- workspace/sandbox requirements;
- executor availability.

The parent model cannot directly create an unmanaged child process that escapes Pantheon accounting.

## Audit model

Every authorization lifecycle transition is recorded as a structured event.

Examples:

```text
authorization.requested
authorization.denied
approval.requested
grant.created
authorization.permitted
capability.issued
action.started
action.completed
action.failed
grant.expired
```

An audit record should identify at least:

- run and task;
- principal/agent;
- canonical action;
- canonical resource;
- decision;
- reason;
- policy identifiers/hash;
- grant/ticket identifiers where applicable;
- timestamps;
- outcome.

This enables a command such as:

```text
pantheon audit task-123
```

and provides ground truth for debugging and the Agent Genome learning pipeline.

## Risk engine — deferred

Authorization and risk are not the same thing. A permitted delete of `workspace://node_modules/**` and a permitted delete of `workspace://src/**` have different operational risk.

A future Risk Engine may combine:

- authorization;
- reversibility;
- task intent;
- affected resource class;
- blast radius;
- environment criticality.

Risk scoring is deliberately deferred from v1 so the initial security core remains deterministic.

## v1 implementation surface

The policy decision interface should remain small:

```rust
authorize(
    principal: Principal,
    action: Action,
    resource: Resource,
    context: Context,
) -> Decision

enum Decision {
    Permit,
    Deny(DenyReason),
}
```

Core components:

```text
Action Normalizer
Policy Decision Point (Cedar)
Approval Broker
Capability Grant Store
Capability Ticket Issuer
Execution Broker
Sandbox Adapter
Append-only Audit Log
```

## Non-negotiable invariants

1. Pantheon, not the model or harness, is the authorization authority.
2. Authorization is binary: PERMIT or DENY.
3. Default deny and explicit forbid precedence are mandatory.
4. Human approvals become narrowly scoped grants.
5. Credentials should be used through brokers rather than revealed whenever possible.
6. Provider permissions and OS sandboxing are defense-in-depth layers.
7. Privileged host control planes are brokered, never handed directly to agents.
8. Enforcement failure causes task startup/execution failure rather than policy weakening.
9. Every privileged operation is auditable.
