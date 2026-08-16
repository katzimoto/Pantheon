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
