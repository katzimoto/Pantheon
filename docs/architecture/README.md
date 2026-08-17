# Pantheon Architecture Map

This is a routing map, not a specification. It tells you which documents exist,
what each one owns, and which small set to read for a given change. The
contracts themselves are the source of truth.

Read `docs/architecture/overview.md` first if you have not already. It is the
canonical high-level system model and is short by design.

Every document listed below is canonical for the subsystem it names. Each one
opens with a `## Status` section stating how far its authority extends.

## Domains at a glance

| Domain | Owns |
|---|---|
| `docs/architecture/goals-and-planning/` | The user-outcome contract, its revisions, and decomposition into a TaskGraph |
| `docs/architecture/tasks/` | The Task contract, its lifecycle, graph edges, and dynamic spawn |
| `docs/architecture/scheduling/` | Which Task runs next, resource admission, reservations, and dispatch |
| `docs/architecture/execution/` | Backend abstraction, routing to a Binding, Run/Attempt lifecycle, worker control surface |
| `docs/architecture/agents-and-context/` | Logical Agent identity, eligibility, and the frozen semantic inputs for a Run |
| `docs/architecture/evaluation-and-acceptance/` | Verification of results and the authority to accept a Task |
| `docs/architecture/artifacts-and-workspaces/` | Immutable results, mutable workspaces, and Git integration |
| `docs/architecture/security/` | Authorization, physical isolation, and secret brokering |
| `docs/architecture/persistence-and-recovery/` | Durable state, crash reconciliation, and retry/escalation policy |
| `docs/architecture/operations/` | Configuration, accounting, observability, and the operator interface |

---

## goals-and-planning/

The durable user-outcome contract and how it becomes work.

| Document | Owns |
|---|---|
| `docs/architecture/goals-and-planning/goal-resource.md` | The `Goal` resource: desired outcome, revisions, acceptance criteria |
| `docs/architecture/goals-and-planning/goal-lifecycle-and-completion-controller.md` | Goal phases, completion proof, and finalization before terminal state |
| `docs/architecture/goals-and-planning/goal-revision-reconciliation.md` | What happens to existing work when a Goal revision changes desired state |
| `docs/architecture/goals-and-planning/planner-and-task-decomposition.md` | How a Goal revision becomes a proposed TaskGraph/GraphPatch, and the limits on Planner authority |

Goal completion is proved through the shared evaluation machinery in
`docs/architecture/evaluation-and-acceptance/evaluation-and-evaluator-registry.md`;
this domain owns Goal lifecycle authority, not a separate evaluation path.

Read when: changing what a Goal means, when Pantheon may believe a Goal is
done, how replanning works, or what the Planner is allowed to decide.

## tasks/

The bounded unit of semantic work and its state machine.

| Document | Owns |
|---|---|
| `docs/architecture/tasks/task-object.md` | The immutable `TaskSpec`: outcome, inputs, acceptance criteria, competencies, scope |
| `docs/architecture/tasks/task-lifecycle.md` | Controller-owned `TaskStatus` transitions across Run failure, waiting, acceptance, cancellation and recovery |
| `docs/architecture/tasks/taskgraph-dependencies.md` | Graph edges, dependency semantics and join conditions |
| `docs/architecture/tasks/task-spawn-and-dynamic-graphs.md` | How a running worker proposes new Tasks and how Pantheon materializes them |
| `docs/architecture/tasks/blocking-spawn-and-run-yield.md` | The v1 blocking-spawn contract: parent Run yields, capacity is released, a new Run resumes |

Read when: changing Task semantics, Task states, graph structure, or dynamic
work discovery.

## scheduling/

Which eligible Task gets the next claim, and whether capacity exists for it.

| Document | Owns |
|---|---|
| `docs/architecture/scheduling/scheduler-ready-task-eligibility.md` | Which Tasks are logically eligible to be considered — nothing about capacity |
| `docs/architecture/scheduling/scheduler-task-ordering-and-fairness.md` | Goal-level fairness, then Task selection within the chosen Goal |
| `docs/architecture/scheduling/scheduler-resource-ledger-and-admission.md` | Generic namespaced resource keys, quantities, and whether a workload may be admitted now |
| `docs/architecture/scheduling/scheduler-reservations-ownership-and-leases.md` | Reservation ownership, leases, and when capacity may safely be released |
| `docs/architecture/scheduling/scheduler-dispatch-and-run-intent-reconciliation.md` | The atomic handoff from Scheduler to Run Controller and the gates on committing new work |

The four stages are distinct and are deliberately separate documents:
eligibility → ordering → admission → dispatch. Most changes touch one.

Read when: changing what may be scheduled, in what order, against what
capacity, or how scheduling intent becomes a Run.

## execution/

How resolved work reaches a backend and how its lifecycle is tracked.

| Document | Owns |
|---|---|
| `docs/architecture/execution/execution-fabric.md` | The provider-neutral backend abstraction: Backend Registry, ExecutionRequest/Offer/Binding, launch classes |
| `docs/architecture/execution/execution-offer-routing-and-admission-handshake.md` | Jointly choosing an eligible Agent and a compatible Offer, proving feasibility, freezing one Binding |
| `docs/architecture/execution/run-and-attempt.md` | Run and Attempt lifecycle, terminal outcomes, LaunchKey, contact state |
| `docs/architecture/execution/agent-control-channel.md` | The restricted Attempt-authenticated surface a worker uses to call back into Pantheon |

Read when: changing backend integration, routing, Run/Attempt state, or what a
running worker is allowed to ask for.

## agents-and-context/

Who does the work and what semantic inputs they get.

| Document | Owns |
|---|---|
| `docs/architecture/agents-and-context/agent-genome.md` | Persistent Logical Agent identity, memory, skills and learning history, independent of any model |
| `docs/architecture/agents-and-context/agent-manifest.md` | The declarative `AGENT.yaml` contract (see `schemas/agent-v1alpha1.schema.json`) |
| `docs/architecture/agents-and-context/logical-agent-resolution.md` | Which Logical Agents are semantically eligible for a Task — not which backend runs them |
| `docs/architecture/agents-and-context/context-builder.md` | Deterministic selection and freezing of a Run's semantic inputs into a provider-neutral `ContextPlan` |

Read when: changing Agent identity or declaration, Agent eligibility, or how
prompts/context are assembled.

## evaluation-and-acceptance/

Whether a result is good, and who may say the contract is met.

| Document | Owns |
|---|---|
| `docs/architecture/evaluation-and-acceptance/evaluation-and-evaluator-registry.md` | Evaluator versions, EvaluationRound/Operation, accounting and isolation for verification |
| `docs/architecture/evaluation-and-acceptance/task-acceptance-and-completion.md` | CandidateResult submission and Pantheon's exclusive authority to accept and terminalize a Task |

Goal-level acceptance *authority* is owned elsewhere —
`docs/architecture/goals-and-planning/goal-lifecycle-and-completion-controller.md`
— but it is not a separate mechanism. Goal acceptance reuses this domain's
`EvaluationRound`/`Evidence` machinery with a different immutable subject:
`GOAL_COMPLETION_CANDIDATE` is one of the two v1 EvaluationRound subject types,
alongside `TASK_CANDIDATE`. Changing evaluator semantics therefore affects Goal
acceptance as well as Task acceptance.

Read when: changing how results are verified, what an evaluator may do, or how
a Task becomes accepted.

## artifacts-and-workspaces/

Immutable results versus mutable working state.

| Document | Owns |
|---|---|
| `docs/architecture/artifacts-and-workspaces/artifact-model.md` | Immutable content-addressed Artifacts and Candidates, and what is explicitly not an Artifact |
| `docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md` | Task-owned mutable Workspaces, worktree isolation, changeset sealing, and the Integration Controller's exclusive right to move shared refs |

Read when: changing result representation, workspace handling, or Git
integration.

## security/

Two separate boundaries: what is authorized, and what is physically reachable.

| Document | Owns |
|---|---|
| `docs/architecture/security/permissions-and-capabilities.md` | The authorization model: principals, actions, Grants, and `PERMIT`/`DENY` |
| `docs/architecture/security/sandbox-broker-and-isolation.md` | Physical containment and the ceiling on ambient authority inside a Sandbox |
| `docs/architecture/security/secret-store-and-credential-brokering.md` | `secret.use` brokering, SecretProvider mutation/lease lifecycle, and provider-state recovery without disclosing material to principals |

Authorization and containment are independent and must both be satisfied.
Changing one without reading the other is a common source of error. The
worker-side trust boundary is defined in
`docs/architecture/execution/agent-control-channel.md`.

Read when: changing who may do what, what a Sandbox can reach, how credentials
are used, or how SecretProvider mutations/leases are reconciled after crash or
restore.

## persistence-and-recovery/

Durable truth and how it is restored after things go wrong.

| Document | Owns |
|---|---|
| `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md` | The relational model, transaction boundaries, and the rule that external effects never occur inside a transaction |
| `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md` | Reconstructing safe control after crashes, partial external effects and divergence; fencing and recovery barriers |
| `docs/architecture/persistence-and-recovery/recovery-policy.md` | Converting failure/condition evidence into the next permitted action: retry, escalate, or reconcile UNKNOWN first |

These three are frequently consulted together: persistence defines what is
durable, global recovery defines what must be reconciled or fenced, and
recovery policy decides what happens next. External-domain contracts may add
mandatory domain-specific reconciliation semantics; SecretProvider mutations
and CredentialLeases are defined in
`docs/architecture/security/secret-store-and-credential-brokering.md` and
participate in the same global recovery barrier/fencing model.

Read when: changing schema, transaction boundaries, external-effect ordering,
crash behaviour, or retry/escalation.

## operations/

Configuration, accounting, observability, and the operator interface.

| Document | Owns |
|---|---|
| `docs/architecture/operations/configuration-and-policy-revisions.md` | Compiling source files into one immutable atomic ConfigurationRevision with domain-specific component digests |
| `docs/architecture/operations/budget-usage-and-rate-limits.md` | BudgetHolds, factual usage, charges and replenishing rate limits, kept distinct from reservable capacity |
| `docs/architecture/operations/event-and-observability-model.md` | The append-only Event Journal, audit, tracing, metrics and diagnostics — and why Pantheon is not event-sourced |
| `docs/architecture/operations/public-daemon-api-and-cli.md` | The Operator Control Surface: `pantheond` as sole control-plane authority, and what the CLI may never do |

The worker-facing counterpart to the operator API is
`docs/architecture/execution/agent-control-channel.md`. Reservable resource
capacity is separate from budget and lives in
`docs/architecture/scheduling/scheduler-resource-ledger-and-admission.md`.

Read when: changing configuration publication, accounting, telemetry, or the
operator-facing interface.

---

## Reading recipes

Selective retrieval for common kinds of change. Read the listed documents in
order; consult the conditional ones only if the condition applies.

**Changing Task spawning**

1. `docs/architecture/tasks/task-spawn-and-dynamic-graphs.md`
2. `docs/architecture/tasks/blocking-spawn-and-run-yield.md`
3. `docs/architecture/tasks/task-lifecycle.md`

Consult `docs/architecture/execution/agent-control-channel.md` if the worker's
request surface changes, and
`docs/architecture/tasks/taskgraph-dependencies.md` if join semantics change.

**Changing scheduler resource admission**

1. `docs/architecture/scheduling/scheduler-resource-ledger-and-admission.md`
2. `docs/architecture/scheduling/scheduler-reservations-ownership-and-leases.md`

Consult
`docs/architecture/execution/execution-offer-routing-and-admission-handshake.md`
only if the feasibility handshake itself changes,
`docs/architecture/operations/budget-usage-and-rate-limits.md` only if budget
or rate limits are involved, and
`docs/architecture/scheduling/scheduler-dispatch-and-run-intent-reconciliation.md`
only if dispatch commit conditions change.

**Changing persistence or recovery behaviour**

1. `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`
2. `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`
3. `docs/architecture/persistence-and-recovery/recovery-policy.md`

Consult `docs/architecture/operations/event-and-observability-model.md` if the
Event Journal or transactional outbox is involved,
`docs/architecture/execution/run-and-attempt.md` if Attempt contact state or
fencing is involved, and
`docs/architecture/security/secret-store-and-credential-brokering.md` if a
SecretProvider, `SecretMutationIntent`, `SecretDescriptor`, credential use, or
`CredentialLease` is involved.

**Changing execution routing or backend integration**

1. `docs/architecture/execution/execution-fabric.md`
2. `docs/architecture/execution/execution-offer-routing-and-admission-handshake.md`
3. `docs/architecture/execution/run-and-attempt.md`

Consult `docs/architecture/agents-and-context/logical-agent-resolution.md` if
Agent eligibility changes.

**Changing authorization or isolation**

1. `docs/architecture/security/permissions-and-capabilities.md`
2. `docs/architecture/security/sandbox-broker-and-isolation.md`

Consult `docs/architecture/security/secret-store-and-credential-brokering.md`
for credential paths and
`docs/architecture/execution/agent-control-channel.md` for the worker-facing
boundary.

**Changing Goal semantics or completion**

1. `docs/architecture/goals-and-planning/goal-resource.md`
2. `docs/architecture/goals-and-planning/goal-lifecycle-and-completion-controller.md`

Also read
`docs/architecture/evaluation-and-acceptance/evaluation-and-evaluator-registry.md`
whenever Goal acceptance criteria, EvaluationRound/Evidence or evaluator
execution are involved: Goal acceptance runs on the shared evaluation machinery
under the `GOAL_COMPLETION_CANDIDATE` subject type, not on a Goal-private path.

Consult `docs/architecture/goals-and-planning/goal-revision-reconciliation.md`
if revisions are involved and
`docs/architecture/goals-and-planning/planner-and-task-decomposition.md` if
decomposition is involved.

**Changing how results are judged**

1. `docs/architecture/evaluation-and-acceptance/evaluation-and-evaluator-registry.md`
2. `docs/architecture/evaluation-and-acceptance/task-acceptance-and-completion.md`
3. `docs/architecture/artifacts-and-workspaces/artifact-model.md`

## Adding a document here

See the placement rules in `docs/README.md`. When you add a canonical
contract, add a row to the domain table above; this map is the only place the
inventory is maintained.