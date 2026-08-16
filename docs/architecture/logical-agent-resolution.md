# Logical Agent Resolution

## Status

Draft design — Pantheon agent selection and execution-candidate formation specification.

## Purpose

This document defines how Pantheon determines which persistent logical Agents are semantically eligible for a Task, how those candidates interact with the Execution Fabric, and where final Agent choice becomes durable.

The core rule is:

> **Pantheon determines Agent eligibility before execution routing, but does not permanently select a Logical Agent until it can evaluate feasible Agent + ExecutionOffer pairs.**

This preserves the distinction between semantic ownership and execution placement while avoiding premature Agent assignment that ignores current execution feasibility.

## Terminology

Pantheon uses the following distinct concepts:

```text
TASK TYPE
What class of work is this?

COMPETENCY
What semantic ability is required?

SKILL
What reusable procedure does an Agent know?

TOOL / ACTION
What canonical operation may be invoked?

EXECUTION FEATURE
What mechanism must an ExecutorBackend provide?

PERMISSION
What action is authorized?

CAPABILITY GRANT / TICKET
A concrete temporary authorization instrument.
```

Semantic Task requirements use `competencies`, not `capabilities`, to avoid collision with authorization capability grants/tickets and Execution Features.

## Agent Manifest additions

A logical Agent declares both Task applicability and competencies.

Conceptually:

```yaml
spec:
  accepts:
    - code.implement
    - code.debug
    - code.review

  competencies:
    - code.analysis
    - code.debugging
    - code.editing
    - test.execution
```

`accepts` answers:

> For which Task types may this Agent be considered?

`competencies` answers:

> Which semantic abilities is this Agent configured and trusted to provide?

These are not self-reported runtime quality scores.

## Control of applicability and competencies

`accepts` and `competencies` are control-plane configuration.

Agent Genome learning may improve memory, skills, and validated behavior, but it must not silently expand the Agent's trusted applicability or competency set.

An Agent may propose a competency/applicability change as a configuration candidate, but promotion must pass explicit validation/operator policy.

This prevents self-promotion such as:

```text
I learned a security skill
→ therefore I may now own every security.audit Task
```

## Agent Registry

Pantheon maintains normalized discovery state derived from canonical Agent configuration.

Conceptually:

```yaml
agent:
  id: agent://coder
  specHash: sha256:...
  enabled: true

  description: >
    Implements, debugs, refactors and reviews software.

  accepts:
    - code.implement
    - code.debug

  competencies:
    - code.analysis
    - code.debugging
    - code.editing
    - test.execution

  skills:
    - debugging
    - testing
    - git

  execution:
    requirements:
      features:
        - tools.structured
```

The Registry is a discovery/indexing layer, not another source of Agent identity. Canonical identity remains the Agent Manifest plus Genome.

## Eligibility versus ranking

Pantheon separates two questions:

```text
ELIGIBILITY
Could this Agent legitimately own this Task?

SELECTION
Which eligible Agent + execution configuration should Pantheon actually use?
```

Eligibility is deterministic.

Ranking may use semantic hints and historical evidence, but only after eligibility has been established.

## Deterministic Agent eligibility

Input:

```text
Task
Goal constraints
current policy
Agent Registry
explicit execution overrides
```

Output:

```text
AgentCandidateSet
```

Hard eligibility checks include, where applicable:

- Agent is enabled;
- Task type is accepted;
- every required Task competency is satisfied;
- intrinsic Agent execution requirements do not contradict hard Task/Goal/policy constraints;
- Agent hard policy does not make the mandatory Task envelope impossible;
- an explicit required Agent pin matches;
- enclosing hard policy permits this Agent to own the work.

Conceptual result:

```yaml
eligible:
  - agent://coder

ineligible:
  agent://security:
    - missing-competency: code.debugging

  agent://researcher:
    - unsupported-task-type: code.debug
```

No LLM or backend may override a hard ineligibility result.

## Descriptions and skills are discovery hints

Agent descriptions, skill metadata, tags, examples, Task labels, project affinity, and domain hints may improve ranking among already-eligible Agents.

They do not create eligibility.

Example:

```text
Task requires security.reverse-engineering

Agent prose says it is excellent at reverse engineering
but Agent competencies do not include it

→ INELIGIBLE
```

Prose may rank candidates. Prose cannot authorize them.

## Skills are not Task requirements

Tasks specify competencies, not Agent implementation details.

A Planner should not normally emit:

```yaml
requiresSkill: postgres-debugging
```

Instead it may emit:

```yaml
requirements:
  competencies:
    - database.analysis
    - code.debugging
```

A matching Agent may happen to possess a `postgres-debugging` skill, which can improve affinity/ranking and later context compilation.

Thus:

```text
competencies → hard semantic matching
skills       → procedure/affinity/context
```

## Do not commit Agent selection before execution feasibility

After deterministic eligibility, Pantheon should not normally freeze one Agent immediately.

For each eligible Agent, Pantheon constructs an Agent-specific ExecutionRequest because Agent execution requirements, context construction, route policy, and policy intersection may differ.

```text
Task
  ↓
Agent Resolver
  ↓
Eligible Agents
  ├──────────────┐
  ▼              ▼
Agent A         Agent B
  │              │
  ▼              ▼
Execution      Execution
Request A      Request B
  │              │
  ▼              ▼
Offers         Offers
```

This avoids selecting an excellent semantic candidate that currently has no feasible execution path when another valid Agent can execute immediately.

## Agent + ExecutionOffer is the routable candidate

The final route-candidate unit is conceptually:

```text
ExecutionCandidate
=
AgentCandidate
+
ExecutionOffer
```

Example:

```yaml
candidate:
  agent:
    ref: agent://coder
    specHash: sha256:...

  execution:
    request: exec-request://...
    offer: offer://...

  semanticFit:
    taskType: exact
    competencyCoverage: complete
```

Hard Agent eligibility has already been established. S4 routing/admission/budget logic then operates over valid pair candidates.

## ExecutionBinding freezes Agent and execution offer together

The immutable ExecutionBinding records the selected pair.

Conceptually:

```yaml
binding:
  agent:
    ref: agent://coder
    specHash: sha256:...

  execution:
    request: exec-request://...
    offer: offer://...
    backend: executor://...
```

Changing either the Logical Agent or the execution offer/configuration changes the binding and therefore requires a new Run.

The Task itself never gains an Agent assignment.

## Selection modes

Pantheon should avoid invoking a model selector for obvious cases.

v1 uses three conceptual modes:

```text
DIRECT
Exactly one eligible Agent remains.
Use it without a selector model.

POLICY
Deterministic route/selection policy resolves the candidates.
No semantic selector is required.

SEMANTIC
Multiple genuinely ambiguous eligible candidates remain.
A semantic ranker may propose ordering.
```

A semantic ranker is advisory only.

It receives only already-valid Agent candidates and returns a ranking/proposal. Pantheon validates every referenced Agent against the candidate set before any decision is used.

## Ranking inputs

Useful advisory/routing evidence may include:

### Static discovery hints

- Agent description;
- skill metadata/tags/examples;
- Task labels;
- project/domain affinity.

### Pantheon-observed history

- acceptance rate by Task type/competency;
- rejection rate;
- Run/Attempt counts per successful Task;
- latency;
- budget efficiency;
- user corrections;
- project-specific outcomes;
- Agent + backend pair outcomes.

Historical quality data belongs to Pantheon evidence/metrics, never to self-advertised Agent scores.

Do not add manifest fields such as:

```yaml
quality: 9.9
intelligence: expert
preferred: true
```

## Explicit Agent pins and preferences

Human/operator execution policy may express either a hard requirement or a preference.

Conceptually:

```yaml
agent:
  require: agent://security
```

means no other Agent is acceptable.

```yaml
agent:
  prefer:
    - agent://security
```

means prefer it when valid/feasible but allow alternatives.

These are execution-policy constraints, not immutable TaskSpec fields.

Even a human pin does not bypass hard compatibility or security policy.

## Permissions during Agent resolution

Agent selection does not grant authority.

Effective runtime authority remains the intersection of:

```text
system
∩ user
∩ project
∩ Agent
∩ Task
∩ temporary grants
```

Agent eligibility may reject an Agent whose hard policy makes the mandatory Task envelope impossible, but detailed action authorization remains the responsibility of Pantheon's PDP/capability-ticket system.

## No eligible Agent

Pantheon must not silently fall back to an omnipotent generalist.

Return a structured resolution result such as:

```yaml
status: no-eligible-agent
reasons:
  agent://coder:
    - missing-competency: security.reverse-engineering
  agent://researcher:
    - unsupported-task-type: security.ctf
  agent://security:
    - disabled
```

Higher-level policy may then replan, request operator intervention, enable/create an Agent, or ultimately fail the Task.

A generalist Agent is valid only if explicitly configured with broad applicability and competencies.

## Learning and provenance

Because Agent choice is frozen in the ExecutionBinding/Run, Pantheon can separately evaluate:

```text
Task type
required competencies
selected Agent
selected backend
Attempts
usage
candidate
acceptance evidence
user corrections
```

This preserves the ability to distinguish:

```text
Agent quality
backend quality
Agent + backend pair quality
```

without collapsing them into one score.

## v1 non-goals

Defer:

- opaque global quality scores;
- learned automatic expansion of Agent competencies;
- unrestricted LLM selection authority;
- speculative parallel execution by several Agents;
- embedding/ML retrieval as a hard dependency for Agent discovery;
- automatic creation of new Agents when no candidate exists.

## Key decisions

1. Semantic Task requirements are called `competencies`, not `capabilities`.
2. Agent Manifest contains explicit `accepts` and `competencies`.
3. `accepts` and `competencies` are control-plane configuration and cannot be silently self-expanded by Genome learning.
4. Agent Registry is normalized discovery state derived from canonical Agent configuration.
5. Agent eligibility is deterministic and based on hard semantic/policy facts.
6. Descriptions, skills, tags and examples are ranking/discovery hints, never eligibility authority.
7. Tasks require competencies, not Agent skills.
8. Agent Resolver outputs an eligible candidate set rather than prematurely assigning one Agent.
9. Pantheon constructs an Agent-specific ExecutionRequest for every eligible Agent that remains under consideration.
10. The routable candidate is an Agent + ExecutionOffer pair.
11. ExecutionBinding freezes selected Agent and execution configuration together.
12. Changing Agent creates a new Binding and therefore a new Run.
13. DIRECT, POLICY, and SEMANTIC selection modes avoid unnecessary selector-model calls.
14. Semantic rankers may rank only already-valid candidates and cannot introduce or authorize candidates.
15. Quality/performance belongs to Pantheon-observed evidence, not Agent self-advertisement.
16. Agent pins/preferences belong to execution policy/overrides, not TaskSpec.
17. No eligible Agent produces a structured failure; there is no implicit fallback Agent.
18. Agent selection and executor routing remain conceptually distinct, while final commitment is deliberately joint so execution feasibility may influence which valid Agent is chosen.
