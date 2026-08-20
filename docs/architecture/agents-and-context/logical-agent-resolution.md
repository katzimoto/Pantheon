# Logical Agent Resolution

## Status

Canonical Pantheon Logical Agent eligibility specification.

## Central rule

> **Agent Resolver answers which persistent Logical Agents are semantically valid for a Task. It does not choose backend/model/provider capacity. Final routing later chooses among feasible Agent+ExecutionOffer pairs and freezes the selected Agent in the Run Binding.**

## Flow

```text
Ready Task
  ↓
Agent Resolver
  ↓
eligible Logical Agent versions
  ↓
per-Agent ExecutionRequests
  ↓
Execution Fabric offers
  ↓
Agent + Offer feasibility/routing
  ↓
ExecutionBinding freezes Agent + Offer
```

An Agent is not permanently attached to a Task before routing. Changing Agent between execution strategies means a new Run/new Binding.

## Hard eligibility

V1 eligibility is deterministic. Agent must satisfy at least:

```text
enabled/current immutable Agent version
Task type in accepts
Task required competencies subset of Agent competencies
operator/project Agent pins or exclusions
current Goal/Task/delegation policy compatibility
Agent manifest/schema validity
required revision/config compatibility
```

`accepts` and `competencies` are operator/config-controlled semantic claims; Genome learning cannot auto-expand them.

### Agent version status

Each configured Agent version declares its own status, compiled into the Agent
component and digested with it:

- `enabled` (default `true`) — whether this immutable version may be considered
  for new Tasks at all;
- `current` (default `true`) — whether this is the configured current version
  of its Logical Agent.

At most one `current` version may exist per Agent name; a candidate declaring
two is rejected before activation. The Resolver treats `enabled`/`current` as
hard filters and never infers status from declaration order or version number.

### Pins and exclusions

`routing.agentPins` and `routing.agentExclusions` name exact immutable Agent
versions (`name@version`):

- a non-empty pin list is an allowlist — every unpinned Agent version becomes
  ineligible for **every** Task while the pins are active, regardless of Task
  type;
- an exclusion removes the named version from eligibility;
- a version that is both pinned and excluded is rejected before activation.

Both lists participate in the routing component digest, so pin/exclusion
changes alter routing identity.

## Not Agent eligibility

Do not use these as semantic eligibility:

```text
concrete backend/model identity
backend current capacity
Sandbox slot availability
rate-limit state
self-reported quality
recent provider latency
```

Those belong to later offer/routing/admission.

## Ranking

If multiple Agents are eligible, v1 uses deterministic configured precedence/tie-break rules. Optional prose/description matching may provide diagnostics or future ranking signals but cannot override hard eligibility.

RoutePolicy fields are compiled into the routing component and digested with
it: `priority` (higher values preferred first, default `0`), `ordering` (the
closed v0.1.0 preference-key vocabulary, currently `contextCapacity`),
`tieBreak` (`backendId` | `agentId`), and `requiresKeyedLaunch` (default `true`;
`false` admits `OBSERVATIONAL` launch semantics only when controller-owned
outer duplicate-prevention evidence exists). Unknown preference or tie-break
keys are rejected at activation rather than silently ignored. See
`docs/architecture/execution/execution-offer-routing-and-admission-handshake.md`.

Model-based semantic Agent ranking is deferred from v1. It is not needed for correctness and would introduce another unaccounted/nondeterministic control operation.

## No eligible Agent

Return a structured resolution failure containing unmet semantic requirements/policy constraints. Do not select an incompatible Agent just to make progress.

Recovery/Planner/operator may then change Task decomposition/configuration, request human action or fail according to policy.

## Configuration

Agent registry versions are immutable ConfigurationRevision content/snapshots. Resolver captures the Scheduler's one active ConfigurationRevision and produces decisions bound to exact Agent version(s).

If config changes before T3, Scheduler aborts/re-resolves rather than mixing old Agent eligibility with new route policy.

## Delegation/spawn

A parent Agent does not choose a concrete child Agent when requesting `task.spawn`; it proposes a child Task outcome. When that child becomes Ready, normal Agent Resolution selects eligible Logical Agents under inherited ceilings.

## Run freezing

ExecutionBinding records selected exact Logical Agent version. Subsequent Agent config/Genome changes do not mutate an existing Run. Current security tightening may still reduce permitted operations according to Configuration/authorization intersection.

## Core invariants

1. Logical Agent is provider/model/backend independent.
2. Agent eligibility is deterministic semantic control-plane logic in v1.
3. Agent is selected jointly with a feasible ExecutionOffer only after offers exist; Binding freezes both.
4. Backend capacity/identity is not semantic Agent eligibility.
5. Model-based semantic ranking is deferred and could never override hard eligibility.
6. No eligible Agent produces structured failure rather than unsafe fallback.
