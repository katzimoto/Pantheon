# Planner and Task Decomposition

## Status

Draft design — Pantheon planning subsystem specification.

## Purpose

Pantheon uses planning to convert a user Goal into bounded Tasks and TaskGraph mutations. Planning is advisory intelligence: a planner proposes work structure, while the Pantheon controller validates and materializes the proposal.

The planner never owns scheduler state, Task lifecycle, authorization, agent assignment, provider/model selection, or graph mutation authority.

## Foundational principles

1. **The planner proposes; Pantheon commits.** Planner output is never executable truth.
2. **Planning is optional.** Trivial Goals should become a direct Task without invoking a planner.
3. **Minimum useful decomposition.** Split work only when the child Tasks represent independently meaningful and verifiable outcomes.
4. **Prefer progressive planning for uncertain long-horizon work.** Avoid speculative full-project decomposition when future work depends on facts not yet discovered.
5. **Plans are structured and revision-bound.** Free-form planning prose is not graph state.
6. **Deterministic validation precedes materialization.** Schema, graph, policy, type, cycle, scope, and limit checks do not depend on an LLM.
7. **Assumptions and unknowns are explicit.** Missing information should produce discovery work rather than silent guesses.
8. **Replanning is event-driven and patch-based.** Existing completed history is never rewritten.
9. **Planner determines logical work structure, not executor allocation.** Router/Scheduler choose agents, models, harnesses, and physical concurrency.
10. **Planning itself is bounded and observable.** Replans, graph growth, unused Tasks, duplicate work, and downstream usefulness are measured.

## Planning authority

```text
User Goal
   │
   ▼
Planner / Planning Logic
   │
   ▼
Graph Proposal
   │
   ▼
Pantheon Validator
   │
   ├── schema
   ├── Task quality
   ├── acceptance contracts
   ├── dependency/binding validity
   ├── cycle/deadlock safety
   ├── authorization/scope ceilings
   ├── resource/decomposition limits
   └── graph revision
           │
           ▼
      Controller commit
           │
           ▼
      TaskGraph revision
```

The planner receives `task.graph.propose`, never `task.graph.mutate`.

## Planning modes

Pantheon supports three conceptual planning modes.

### DIRECT

Use when the Goal already corresponds to one bounded outcome.

```text
Goal → Task
```

No planner agent is required.

Examples:

- fix a typo;
- explain one file;
- make one bounded configuration change.

Avoiding unnecessary planning reduces latency, token use, coordination overhead, and failure surface.

### SHALLOW

Use for moderately structured work whose near-term decomposition is already clear.

```text
Goal
 ├── Task A
 ├── Task B
 └── Task C
```

The planner proposes a small complete graph once; after materialization, the graph belongs to Pantheon.

### PROGRESSIVE

Use for complex or uncertain Goals where later work depends on facts learned during execution.

```text
Goal
  ↓
plan current horizon
  ↓
execute
  ↓
observe evidence
  ↓
planning checkpoint
  ↓
plan next horizon
```

Pantheon should prefer rolling-horizon planning over speculative large master plans when important unknowns remain.

## Planning checkpoints

A planning checkpoint is a graph/controller concept, not a fake Task whose objective is merely `think about what to do next`.

Conceptual form:

```yaml
checkpoint:
  id: checkpoint_17
  trigger:
    after:
      - task_research_auth
  reason: unresolved-planning-horizon
```

When the trigger is satisfied, Pantheon builds a fresh planning snapshot and invokes the planner if policy allows.

## Planning context

The planner receives a compact state-derived snapshot:

- Goal contract and current revision;
- current TaskGraph revision;
- active/Pending/Ready/terminal Task summaries;
- relevant completed Artifacts and Evidence;
- rejected candidates and important failures;
- explicit assumptions and unknowns;
- project/context references;
- current policy/authority ceilings;
- remaining planning/decomposition budget.

Do not rely on an indefinitely growing conversation or private chain-of-thought history.

## Structured plan proposal

Planner output must be machine-readable. Example:

```yaml
proposalId: plan_01K...

goal:
  ref: goal_123

basedOn:
  graphRevision: 18
  goalRevision: 3

strategy: progressive

tasks:
  - localId: research-auth
    type: research.codebase
    objective: >
      Document the current authorization request flow,
      enforcement boundaries, and provider-specific policy
      compilation paths.
    outputs:
      - name: findings
        kind: architecture.report

prerequisites: []
bindings: []

checkpoints:
  - after:
      - research-auth
    reason: >
      Implementation decomposition depends on the observed
      authorization architecture.

assumptions:
  - id: auth-centralized
    statement: >
      Existing authorization logic is sufficiently centralized
      to identify from repository analysis.

unknowns:
  - >
    Whether provider adapters share a common policy compilation boundary.
```

`localId` values exist only inside the proposal. Pantheon assigns opaque durable Task IDs during materialization.

## Assumptions and unknowns

Unstated assumptions are dangerous because plausible guesses can silently become graph structure.

The planner should explicitly separate:

```text
KNOWN     supported by current Goal/state/evidence
ASSUMED   believed but not verified
UNKNOWN   unresolved and potentially planning-relevant
```

When an unknown materially blocks confident decomposition, prefer a bounded discovery Task.

## Plan validation

### Deterministic validation

Pantheon code validates:

- proposal schema;
- Task schema;
- known Task/output/input/evaluator types;
- references and bindings;
- no dependency cycles;
- no runtime wait/join deadlocks introduced;
- graph revision freshness;
- scope and authority cannot broaden;
- Task creation/decomposition limits;
- acceptance requirements;
- no fixed provider/model assignment in Task semantics;
- existing immutable history is preserved.

These checks are authoritative and never delegated to the planner.

### Semantic plan review

For significant/high-risk Goals, a separate reviewer may assess:

- whether decomposition covers the Goal;
- whether Tasks are meaningfully independent;
- whether important work is missing;
- whether Tasks duplicate one another;
- whether the plan is over-decomposed;
- whether sequencing reflects actual semantic dependencies.

Semantic review creates evidence; it does not mutate the graph directly.

The planner should not be the sole reviewer of its own plan for high-impact work.

## Minimum useful decomposition

A proposed child Task should generally have:

1. one meaningful outcome;
2. clear expected outputs;
3. verifiable acceptance;
4. enough inputs/context to execute independently;
5. a reason to exist separately from its parent/sibling work.

Do not turn an agent trajectory into a graph.

Bad decomposition:

```text
open file
read function
think
edit line
save file
run test
```

Better:

```text
diagnose root cause
implement verified fix
independent review
```

File boundaries alone do not define Task boundaries. Decomposition should follow semantic outcomes, verification boundaries, dependency structure, and conflict risk.

## Planner vs Router/Scheduler

Planner may express logical structure:

```text
A and B are independent
C requires A
```

It must not choose:

```text
agent = Atlas
provider = Claude
model = Opus
run A and B on these two workers now
```

Separation:

```text
Planner   → logical work structure
Router    → suitable logical agent/executor class
Scheduler → when and how much physical concurrency
```

## Event-driven replanning

Replanning may be requested/triggered by events such as:

- initial complex Goal;
- planning checkpoint reached;
- dependency becomes impossible;
- important assumption is invalidated;
- recovery paths are exhausted;
- user materially changes the Goal;
- new evidence changes the architecture/problem model;
- a worker requests replanning with evidence.

Workers may issue `task.replan.request`; they do not launch a planner and mutate the graph directly.

Conceptual request:

```yaml
reason: assumption-invalidated
evidence:
  - artifact://research-738
message: >
  Authorization is distributed across several provider adapters
  rather than centralized as the current plan assumed.
```

## Patch-based replanning

Do not regenerate and replace the entire graph after work has started.

A replan proposes a patch against a specific graph revision:

```yaml
baseRevision: 18

operations:
  - addTask:
      localId: inspect-opencode-policy
      ...

  - addBinding:
      ...

  - addCheckpoint:
      ...

  - supersedeTask:
      task: task_183
      replacement: local:implement-new-policy-layer
```

Pantheon validates and transactionally commits the patch to revision 19.

Completed history is never deleted. Running Task specs are never silently mutated. Work whose contract is no longer authoritative is superseded explicitly.

## Optimistic concurrency

Every plan/patch proposal binds to the Goal and TaskGraph revisions it observed.

If:

```text
proposal.graphRevision = 18
current graphRevision = 19
```

then the proposal is stale.

For v1, prefer rejecting and rerunning planning against the current snapshot rather than implementing complex semantic rebasing.

## Planning limits

Planning must be bounded. Example conceptual ceilings:

```yaml
planning:
  maxReplans: 5
  maxInitialTasks: 8
  maxPlanDepth: 3
  maxGraphTasks: 50
```

The initial horizon ceiling is not a total Goal-size ceiling. A complex Goal may eventually contain many Tasks, while only a small concrete frontier is planned at each checkpoint.

## Planner Agent Genome

The logical planner can be represented like any other persistent Pantheon agent:

```text
~/.pantheon/agents/planner/
├── SOUL.md
├── AGENT.yaml
├── BEHAVIOR.md
├── skills/
└── ...
```

Its learned behavior remains separate from its authority. Even an excellent planner still produces proposals through controller-governed interfaces.

Useful planner outcome signals include:

- proposal validation/acceptance rate;
- replan rate;
- number of Tasks proposed/materialized;
- unused/cancelled Tasks;
- duplicate or redundant Task rate;
- dependency mistakes;
- missing critical work discovered later;
- dynamic spawn expansion caused by missing decomposition;
- Goal success rate;
- cost and latency attributable to planning.

## v1 scope

Implement:

- DIRECT, SHALLOW, PROGRESSIVE planning decisions;
- structured PlanProposal;
- deterministic validation;
- explicit assumptions/unknowns;
- planning checkpoints;
- revision-bound proposals;
- patch-based replanning;
- immutable history/supersession;
- bounded graph growth/replans;
- `task.graph.propose` and `task.replan.request` controller commands;
- planner telemetry.

Defer:

- automatic semantic rebasing of stale plan patches;
- unconstrained planner-written graph expressions;
- self-modifying planning policies;
- arbitrary cross-Goal graph merging;
- autonomous agent/model assignment by the planner;
- full-project speculative planning by default.

## Key invariants

1. **Planner output is a proposal, not state.**
2. **Simple Goals bypass planning.**
3. **Progressive planning is preferred when important unknowns exist.**
4. **Deterministic constraints are enforced by code.**
5. **Replanning adds/supersedes; it does not rewrite history.**
6. **Planner owns neither authorization nor execution allocation.**
