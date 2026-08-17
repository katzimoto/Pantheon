# Pantheon Architecture Overview

## Status

Canonical high-level Pantheon architecture. Establishes the system model, resource hierarchy and cross-cutting invariants. Detailed subsystem contracts under `docs/architecture/<domain>/` are authoritative on their own subjects; see `docs/architecture/README.md` for the subsystem map.

## Purpose

Pantheon is a **local-first deterministic control plane for heterogeneous agent execution**. Models/agents provide reasoning and semantic work; Pantheon owns durable state, execution truth, scheduling, authorization, accounting, verification and external-effect reconciliation.

## Core invariant

> **Pantheon core never contains semantic routing/scheduling/authorization business logic keyed to a concrete provider, model, CLI harness or runtime name. Concrete execution details live behind backend/adapter interfaces.**

## Top-level flow

```text
User / Operator
      ↓
Goal
      ↓
Planner / TaskGraph
      ↓
Task lifecycle
      ↓
Logical Agent Resolution
      ↓
Agent-specific ExecutionRequests
      ↓
ExecutionOffers
      ↓
Agent + Offer candidate
      ↓
Sandbox / Resource / Budget / Policy feasibility
      ↓
ExecutionBinding + Run
      ↓
Attempt + LaunchKey + AgentControlSession
      ↓
CandidateResult / Artifacts
      ↓
Independent Evaluation / Evidence
      ↓
Task Acceptance
      ↓
Goal Completion Candidate / Goal Acceptance
      ↓
optional separately-authorized Integration
```

## Control-plane ownership

Pantheon owns:

- Goal/Task/TaskGraph state;
- Scheduler, resource admission and fairness;
- Logical Agent eligibility and execution routing;
- Run/Attempt/LaunchKey lifecycle;
- Workspaces, Sandboxes and process/session reconciliation;
- authorization, Grants and brokered privileged actions;
- SecretRef/Credential brokering without exposing raw secrets to Agents;
- BudgetHolds, factual Usage/Charges and rate-limit state;
- immutable Artifacts/Candidates/Evidence;
- Acceptance/Evaluation;
- configuration/policy revisions;
- crash recovery/fencing/tombstones;
- durable Event Journal, audit and diagnostics;
- operator API/CLI and restricted Agent Control.

## Major resource hierarchy

```text
Goal
  └─ TaskGraph
      └─ Task
          └─ Task-owned Workspace
          └─ Run
              └─ Run-owned SandboxInstance
              └─ Attempt
                  └─ LaunchKey
                  └─ AgentControlSession
```

`Attempt failure != Run failure != Task failure != Goal failure`.

## Run boundary

A Run is one immutable resolved execution strategy: Binding + frozen semantic ContextPlan + security/config snapshots. Changing Agent/backend/offer/material semantic context creates a new Run.

An Attempt is one logical backend execution lineage under that Run. Daemon/adapter reconnect of the same lineage keeps the same Attempt/LaunchKey; a fresh execution under the same Binding requires a new Attempt only after the old lineage is definitively terminal.

UNKNOWN execution never authorizes duplicate replacement work.

## Dynamic blocking work

V1 runtime Task spawn is blocking-only. Blocking spawn yields/terminalizes the current Run, releases Run-scoped capacity, leaves the parent Task Waiting with zero live Runs, and resumes later through a **new Run** after accepted child Artifact bindings satisfy the Join.

## Security boundary

Authorization and Sandbox containment are separate.

```text
semantic authority
  = hard policy
    ∩ frozen Run ceiling
    ∩ current policy
    ∩ Task scope
    ∩ valid scoped Grants

physical possibility
  = Sandbox ambient authority
```

Untrusted model-driven shell execution requires control-plane isolation. Agent Sandboxes cannot reach Operator Control, Pantheon DB/config/raw CAS, peer workspaces, host credential agents, authoritative Git common-dir state or host container-runtime sockets.

The boundary is bidirectional: Agent-writable repository/configuration state is untrusted input and may not cause Pantheon/controller processes to execute repository-configurable behavior with ambient control-plane authority. Controller operations use controller-owned sterile control state when possible, or equivalently confined helpers when hostile repository metadata must be interpreted.

Privileged operations are brokered. Raw `secret.read` is hard-denied for Agent principals in v1.

## Persistence and external effects

One authoritative SQLite database stores relational control-plane state and Event Journal rows. External network/process/Git/container/secret-store calls never occur inside SQLite transactions.

General external-effect pattern:

```text
durable intent/idempotency identity
  ↓ COMMIT
external effect
  ↓
inspect/reconcile
  ↓
persist result
```

Every Pantheon identity that is transmitted to, or later matched against, an external system to create, ensure, deduplicate, inspect, reconcile, terminate or otherwise address one external-effect lineage is **rewind-resistant**. This includes the external identities named by Global Recovery's durable external-operation rule, such as `LaunchKey`, `SandboxKey`, Planning/Evaluation attempt correlation identities and broker-operation idempotency identities.

```text
existing/restored external lineage
→ preserve its exact original external identity for reconciliation

genuinely new external lineage/effect
→ mint a fresh globally non-reused identity before external contact
```

For v1, a newly minted external-effect identity uses cryptographically strong fresh randomness. It must never be derived solely from SQLite row IDs, attempt ordinals, monotonic counters, resource IDs or other state that a supported disaster restore can rewind and later reproduce. A `UNIQUE` constraint inside the current database is necessary where specified but is not proof of non-reuse across database rewind. Ordinary internal resource IDs that are never used to address an external effect do not acquire this requirement merely by being durable identifiers.

Ordinary content-addressed CAS is the deliberate inverse: durable immutable bytes are completed before SQLite references them because orphan CAS bytes are harmless while DB references to missing bytes are unsafe.

## Code Workspaces / Artifacts

Task Workspace is mutable execution state; it is not an Artifact. Sandboxed coding Agents normally use Task-local isolated Git state rather than writable authoritative shared Git metadata.

Task-local Git state remains hostile to privileged controller execution. WorkspaceRevision/candidate capture must not use Agent-writable Git control state as the controller's repository configuration/execution authority.

Pantheon seals actual Workspace state into a CAS-complete content-addressed `code.changeset`. Worker commits/staging are workflow/provenance, not Candidate identity. Only Integration Controller may mutate shared repository refs, with durable intent and Git compare-and-swap.

## Configuration

Operator-controlled source files compile into an immutable atomic ConfigurationRevision. Controllers never independently reload arbitrary files in place. Decisions bind exact domain-specific component digests instead of one ambiguous `policyHash`.

## Interfaces

Operator:

```text
CLI / UI / automation
  -> local Operator HTTP/JSON API
  -> pantheond
```

Worker:

```text
Attempt workload
  -> restricted Attempt-authenticated Agent Control
  -> authorization/controllers/brokers
```

The CLI never opens SQLite or external execution infrastructure directly.

## v1 scope philosophy

Keep mechanisms required for:

```text
crash safety
duplicate prevention
authorization/isolation
accounting correctness
reproducible semantic inputs
independent acceptance
```

Defer machinery that mainly anticipates distributed scale or autonomous self-modification, including distributed scheduler/fleet design, semantic duplicate detection, speculative Attempts, model-based authoritative reviewers, complex spawn joins, A2A export and automatic Agent Genome promotion.

## Source of truth

Detailed contracts live under `docs/architecture/<domain>/`; `docs/architecture/README.md` maps subsystems to their canonical documents. When older wording conflicts with newer canonical subsystem documents, the explicit canonical invariants in the current subsystem contracts take precedence. The review and correction sweep that reconciled this overview against those contracts is complete, so implementation follows the current detailed contracts directly; a surviving conflict between two canonical documents is a defect to report, not a stale example awaiting a future sweep.
