# Agent Genome

## Status

Draft design — first Pantheon subsystem.

## Purpose

The Agent Genome defines a persistent logical agent independently of the model or harness that executes it. A coder should remain the same coder whether a particular task is executed by Claude Code, OpenCode, or a local Qwen model.

Pantheon therefore owns the canonical agent representation and compiles it into provider-specific sessions.

## Canonical agent home

```text
~/.pantheon/
└── agents/
    ├── coder/
    │   ├── SOUL.md
    │   ├── AGENT.yaml
    │   ├── BEHAVIOR.md
    │   ├── MEMORY.md
    │   ├── skills/
    │   │   ├── debugging/
    │   │   │   ├── SKILL.md
    │   │   │   ├── scripts/
    │   │   │   ├── references/
    │   │   │   └── evals/
    │   │   └── testing/
    │   ├── experiences/
    │   │   └── events.jsonl
    │   ├── reflections/
    │   │   └── candidates.jsonl
    │   ├── evals/
    │   │   └── regression/
    │   └── changes/
    │       └── history.jsonl
    ├── researcher/
    └── security/
```

## Knowledge layers

### `SOUL.md` — stable identity

`SOUL.md` defines durable identity:

- role and responsibility;
- mentality and decision principles;
- engineering/research/security character;
- collaboration principles;
- communication posture.

It must not contain project commands, credentials, ports, transient frameworks, repository paths, or short-lived procedures.

Agents may propose soul changes but may not silently apply them. Soul mutation requires human approval.

### `AGENT.yaml` — machine-readable contract

The exact schema is a later design item. It will bind capabilities, allowed skills, provider preferences, permission profiles, routing policy, evaluation policy, and other machine-readable configuration.

### `BEHAVIOR.md` — learnable heuristics

Pantheon adds an explicit adaptive layer between identity and procedures.

Examples:

- Inspect logs and recent changes before editing an unfamiliar service.
- When acceptance criteria are ambiguous, derive testable criteria before implementation.
- After repeated similar failures, revisit the underlying assumption rather than attempting another variation.

Behavior may evolve through the validated learning pipeline. This prevents one-off experiences from polluting fundamental identity.

### Memory — durable facts

Memory stores facts, preferences, decisions, and stable context that should influence future work without becoming procedural instructions.

The exact Markdown/SQLite/retrieval split remains an open design item.

### Skills — procedural knowledge

Skills use portable `SKILL.md` directories with optional scripts, references, assets, and evaluations.

```text
debugging/
├── SKILL.md
├── scripts/
├── references/
└── evals/
    ├── case-01.yaml
    └── case-02.yaml
```

## Knowledge horizons

```text
SHORT TERM
experience / session state
hours

MEDIUM TERM
memory / reflections / candidate heuristics
days to weeks

LONG TERM
validated skills / promoted behavior / approved soul
months to permanent
```

The more permanent a piece of knowledge becomes, the stronger the evidence required to promote it.

## Learning pipeline

Pantheon treats reflection as a hypothesis, not truth.

```text
Task execution
   ↓
Objective outcome signals
   ↓
Experience record
   ↓
Reflector
   ↓
Candidate lesson
   ↓
Classifier
   ├─ memory
   ├─ skill
   ├─ behavior heuristic
   └─ soul proposal
   ↓
Validation / evaluation
   ↓
Promote or reject
```

An agent may learn and propose freely. Permanent self-modification requires evidence.

## Outcome signals

Learning should incorporate objective signals rather than relying only on conversation transcripts:

- exit status;
- tests;
- build and lint results;
- benchmarks;
- reviewer verdict;
- user correction and acceptance;
- number and type of retries;
- rollback occurrence;
- execution time;
- token/quota usage;
- security/policy violations.

User corrections are especially high-value signals because they represent explicit ground truth about desired behavior.

## Example learning episode

A coding agent tries to fix a generated API binding twice by directly editing generated output. Both attempts fail. On the third attempt it discovers the schema generator, updates the source schema, regenerates the binding, and all tests pass.

The raw experience records the attempts and objective test outcome. Reflection proposes:

> Before directly modifying generated API bindings, identify their source schema or generator.

The classifier identifies this as procedural knowledge and proposes a patch to `skills/code-generation/SKILL.md`, rather than modifying `SOUL.md`.

## Versioned skill mutation

Production skills are never modified directly.

```text
skill v1.4
   ↓
candidate patch
   ↓
v1.5-rc1
   ↓
regression + effectiveness evals
   ├─ regression → reject
   └─ improvement → promote to v1.5
```

Every change records at least:

- author agent;
- source session/task;
- reason;
- supporting evidence;
- previous/new content hash;
- evaluation results;
- timestamp;
- rollback metadata.

## Skill evaluation

A skill change should be evaluated against task-like cases rather than only reviewed textually.

Example:

```yaml
task: >
  Investigate a failing generated TypeScript API client.

expected:
  must:
    - identify generator
    - inspect source schema
    - avoid editing generated output first
  must_not:
    - immediately patch generated client
```

The candidate is promoted only when it improves or preserves evaluation performance without critical regression.

## Agent-specific skill views

A global skill library may exist, but agents receive capability-scoped views.

Example:

```text
GLOBAL
├── git
├── research
├── browser
└── documentation

CODER
├── debugging
├── testing
├── rust
└── web-development

RESEARCHER
├── source-evaluation
├── web
└── synthesis

SECURITY
├── reverse-engineering
├── web-security
├── binary-analysis
└── ctf
```

One agent cannot silently mutate another agent's private skill set.

## Skill promotion

Private knowledge may become broadly useful. Pantheon supports an explicit promotion pipeline:

```text
agent-private skill
      ↓
proven repeatedly
      ↓
shared candidate
      ↓
cross-agent review/eval
      ↓
global skill
```

Cross-agent/global promotion requires stronger evidence than local skill improvement.

## Soul evolution

`SOUL.md` is protected but not absolutely frozen.

After substantial evidence, Pantheon may produce a human-reviewable proposal such as:

```text
SOUL CHANGE PROPOSAL

Current:
Seek robust and extensible solutions.

Proposed:
Prefer the simplest solution that satisfies current requirements;
add extensibility only when justified.

Evidence:
37 tasks
12 user corrections
18 reviewer findings

Confidence: 0.94
```

Only the human may approve the mutation.

## Reflection economics

Execution and learning are separate roles. The executor does not need to perform its own routine reflection.

Preferred pattern:

```text
Claude / OpenCode / local executor
              ↓
        telemetry + result
              ↓
        cheap/local reflector
              ↓
       candidate learning
              ↓
             evals
              ↓
       routine reviewer
              ↓
 high-impact? → premium reviewer / human
```

This preserves scarce premium-model usage for difficult reasoning and important validation boundaries.

## Curator

The curator is a librarian, not an unrestricted teacher.

It may automatically:

- detect stale skills;
- detect likely duplicates;
- run regression/effectiveness evaluations;
- archive unused material;
- identify degraded skills;
- propose merges or splits;
- propose promotion from private to shared knowledge.

Production rewrites still pass through the candidate/evaluation pipeline.

## Negative learning

Failures are first-class learning material.

The reflection system asks:

- What failed?
- Why did it fail?
- Was the failure caused by a bad assumption, procedure, tool use, or missing knowledge?
- Is the lesson reusable?
- Which knowledge layer should own it?

Repeated failure patterns can become explicit pitfalls in skills.

## Observability

Self-modification must be inspectable.

Expected CLI concepts:

```text
pantheon agent inspect coder
pantheon agent diff coder
pantheon learning pending coder
pantheon learning history coder
pantheon learning rollback <change-id>
```

Inspection should expose:

- soul version and last change;
- active memory count;
- active/candidate/stale skills;
- recent performance metrics;
- recent learning promotions/rejections;
- pending high-impact proposals.

## Provider compilation

Pantheon compiles the canonical genome into provider-specific sessions.

### Claude Code

Compile identity, active behavior, selected memory, selected skills, model/tool constraints, and permission policy into the Claude Code agent/session representation.

### OpenCode

Compile the same canonical identity into an OpenCode agent definition with selected model/provider, skills, and permissions.

### Local OpenAI-compatible provider

Construct the appropriate system/context prompt and retrieval package for oMLX or another local endpoint.

The provider is an executor, not the owner of agent identity.

## Guiding principle

> An agent may learn freely and propose changes freely. Permanent self-modification requires evidence. Fundamental identity remains under human control.

## Open design questions

1. Exact `AGENT.yaml` schema.
2. Markdown vs SQLite vs retrieval index for memory.
3. Minimum evidence thresholds for each mutation class.
4. Evaluation isolation and anti-overfitting strategy.
5. Cross-agent skill promotion criteria.
6. Soul-change approval UX.
7. How context limits affect genome compilation and retrieval.
8. How to detect contradictory or obsolete memories/heuristics.
9. Whether high-impact policy changes require multi-reviewer consensus.
