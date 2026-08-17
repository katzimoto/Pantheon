# Pantheon Second Architecture Review Instructions

## Status

Review process material from August 2026: the brief written for a second adversarial review. **Not canonical** — see `docs/reviews/README.md`. The decisions it summarizes were current when it was written; read the referenced subsystem contracts under `docs/architecture/` for the present rules.

## Mission

Perform a **second adversarial architecture review** of Pantheon after the first-review correction sweep.

Repository: `https://github.com/katzimoto/Pantheon`

Read all current files under:

- `docs/architecture/`
- `schemas/`
- `docs/reviews/2026-08-architecture-review-resolution.md`

The first review was committed on branch `claude/pantheon-architecture-review-kue3se` at `5fd665bf8af707897b63aefa21f3e7ce72fee9b6`.

Do not implement code and do not create GitHub issues yet.

## Primary objective

Verify that the four Critical and fifteen High findings from the first review are actually resolved **across the full document set**, not merely asserted resolved in the resolution ledger.

Try to find:

- contradictory old wording elsewhere in the corpus;
- lifecycle transitions that violate the new canonical invariants;
- races/crash windows introduced by the fixes;
- schema/document mismatch;
- authority that can bypass Agent Control/Sandbox/Brokers;
- duplicate execution paths;
- double reservation/accounting paths;
- external effects with no durable intent/reconciliation identity;
- v1 features that still depend on deferred machinery.

## Canonical decisions to treat as current unless you find a concrete safety/correctness failure

### Agent Control

Operator Control and Agent Control are separate. Agent identity is Attempt-scoped. Agent credential authenticates identity only; it grants no action authority. Operator socket/state are physically unreachable from untrusted Sandboxes.

### Blocking spawn

V1 dynamic spawn is blocking-only. Parent Run finalizes to terminal `Yielded`, releases Run-scoped execution capacity, then parent Task becomes Waiting with zero nonterminal Runs. Join satisfaction returns Task Ready and a **new Run** continues from immutable ContinuationContext.

### Evaluation

V1 evaluator types are `check`, `schema`, and `human`. Deterministic external checks are accounted `EvaluationOperation`s using control-operation resources; they are not Runs. Model-based authoritative rubric/review evaluation is deferred.

### Configuration

One immutable atomic ConfigurationRevision is active at a time. It contains domain-specific component digests; generic `policyHash` is obsolete. Existing Run authority is bounded by frozen ceiling intersected with current policy; policy relaxation never broadens an existing Run.

### Run/Attempt

Every Finalizing Run has `terminalTarget`. Terminal outcomes are `Completed|Failed|Cancelled|Yielded`; only Completed requires Candidate. Attempt owns LaunchKey + AgentControlSession. Pre-launch `CONTACT_MAY_HAVE_OCCURRED` is committed before backend call. Backend launch semantics are `KEYED_IDEMPOTENT|OBSERVATIONAL`.

### Reservations

Existing compatible Task-scoped reservations are reused/subtracted before incremental Run admission. New Runs get fresh Run-scoped reservations. Persistence enforces singular live Task reservation per `(task, resource key)` where appropriate.

### UNKNOWN

UNKNOWN never authorizes replacement execution. Operator force-resolution is explicit/audited lineage tombstoning and administrative settlement. It does **not** fabricate actual Usage/Charge.

### Usage

Usage identity is namespaced by backend + Attempt/control-operation + adapter key + meter. Reporting backend must own the Attempt in the immutable Binding. Delayed valid usage is **not rejected solely because controller epoch changed**; epoch is authority fencing for commands/state mutation, not proof that factual usage is false.

### Code Artifacts

`code.changeset` is CAS-complete: canonical changed-path entries reference Pantheon CAS Blobs. Git-rendered patch bytes are not identity. Git object refs/pins are optional verification/efficiency/retention, not the only payload store.

### Sandbox

Workspace/Git strategy and Sandbox security are distinct. Security classes are `TRUSTED_HOST|CONTAINER|HARDENED`. Untrusted model-driven shell requires `isolation.control-plane`. Operator socket/DB/config/raw CAS/peer workspaces/SecretProvider admin/shared Git common-dir authority/runtime sockets/host credential agents are excluded.

### Secrets

`secret.use` means Pantheon-owned brokered use. Raw secret injection into arbitrary Agent shell is equivalent to `secret.read`. Agent `secret.read` is hard-denied in v1. Long-lived secret bytes are not stored in SQLite.

### Acceptance / requeue

Cancellation/supersession committed first beats Candidate submission. Candidate committed first remains immutable history. REQUEUE_TASK may not make Task Ready until its prior responsible Run is terminal.

### Goal

Goal lifecycle is Planning -> Active -> Evaluating -> Finalizing -> Succeeded|Failed|Cancelled. Goal success is required deliverables + optional Goal acceptance, not all Tasks terminal. Superseded Tasks pass through Finalizing; no terminal Task may retain a live Run.

### V1 simplifications

- deterministic Agent resolution; no model semantic ranker;
- Planner DIRECT/SHALLOW; progressive planning deferred;
- blocking spawn only; joined/detached deferred;
- TaskGraph `requires_success` only; `after_terminal` deferred;
- static SOUL/BEHAVIOR/approved Skills/bounded Memory; automatic Genome promotion deferred;
- no model-based authoritative evaluator review;
- local single-daemon; no distributed scheduler/fleet machinery.

## Required checks

### 1. Original-finding closure matrix

For each original finding #1–#19 mark:

- **CLOSED**
- **PARTIALLY CLOSED**
- **OPEN**
- **FIX INTRODUCED A NEW ISSUE**

Cite exact current files/sections and show a concrete failure scenario for anything not CLOSED.

### 2. Cross-document consistency

Search the entire current corpus for stale contradictions, especially:

```text
policyHash/policyRevision ambiguity
Waiting with activeRun
Run Finalizing requiring Candidate
resuming a yielded Run/provider conversation
linked worktree described as a security boundary
native/workspace/isolated sandbox security-class terminology
model rubric/review described as v1 authoritative execution
joined/detached spawn described as v1
PROGRESSIVE Planner described as v1
Task Ready before old Run terminal
Task directly becoming Superseded around a live Run
Agent receiving raw secret material under secret.use
usage rejected only due control epoch
Git ODB as sole changeset payload
```

### 3. Crash/race retest

Re-run at least:

1. crash before/after T3 Run-intent commit;
2. crash after Attempt creation before contact marker;
3. crash after contact marker before/during backend call;
4. lost launch acknowledgement on KEYED_IDEMPOTENT backend;
5. lost launch acknowledgement on OBSERVATIONAL backend;
6. daemon restart with live Attempt/Sandbox;
7. UNKNOWN + operator force-resolution + late callback;
8. UNKNOWN force-resolution + late Usage record;
9. two concurrent requests consuming the last one-use Grant;
10. cancellation vs Candidate submission;
11. Acceptance rejection while producing Run Finalizing;
12. blocking child yield with all global Run slots occupied;
13. Task retry/new Run while Task Workspace reservation already exists;
14. Git GC after accepted `code.changeset` but before integration;
15. Git target-ref CAS race during integration;
16. configuration activation during routing/T3;
17. policy tightening while Sandbox cannot dynamically tighten;
18. Goal revision while Task Active and while Goal Evaluating;
19. restore older SQLite backup while old external execution/secret-store state survives.

For each classify **SAFE / NEEDS CLARIFICATION / UNSAFE**.

### 4. Persistence/schema enforcement

Verify the persistence design can actually express/enforce:

```text
one live Run per Task
Ready/Waiting => zero live Run
Finalizing Run => terminalTarget
Completed Run => Candidate
one nonterminal Attempt per Run
one AgentControlSession per Attempt
one live singular Task reservation per resource key
Attempt launch contact state
backend+Attempt usage provenance uniqueness
Grant use-count + broker operation CAS
external lineage tombstones
control-operation resource holder
evaluation tables
Sandbox identity/status
configuration revisions/components
secret metadata without secret bytes
```

Flag anything left only as prose where a relational constraint should exist.

### 5. Security bypass review

Try to bypass Pantheon by:

- arbitrary Agent shell;
- direct Git metadata manipulation;
- access to host sockets/credential agents;
- raw CAS enumeration/mutation;
- Agent Control impersonation/replay;
- stale Attempt requests;
- stale capability ticket/Grant;
- malicious backend usage claims;
- malicious evaluator definition/result;
- malicious Artifact/reference data prompt injection;
- configuration relaxation/tightening races.

### 6. V1 feasibility

Answer whether one strong Rust engineer + coding agents can implement coherent v1 without inventing major semantics.

Do not request another architecture subsystem merely because implementation details remain. Separate:

- genuine missing semantic decision;
- Rust/module/API implementation design;
- tests/fault injection;
- post-v1 feature.

## Required output

### A. Updated executive verdict

Maximum ~15 lines.

### B. Original finding closure matrix

| # | Status | Evidence/current docs | Remaining issue |
|---|---|---|---|

### C. New Critical/High findings

Only include genuinely new/unresolved issues with concrete scenarios and smallest corrections.

### D. Remaining Medium/Low inconsistencies

Concise table.

### E. Crash/race retest matrix

| Scenario | Safe / Clarify / Unsafe | Why |
|---|---|---|

### F. Security retest

### G. Persistence/schema retest

### H. V1 simplification audit

Identify any deferred feature that still leaks into mandatory v1 semantics/schema.

### I. Exact patch list

Only files that still need changes.

### J. Final verdict

Choose exactly one:

- **READY FOR IMPLEMENTATION**
- **READY AFTER SMALL ARCHITECTURE PATCHES**
- **NOT READY — MAJOR ARCHITECTURE WORK REMAINS**

Do not implement or create GitHub issues yet.
