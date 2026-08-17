# Agent Genome

## Status

Canonical Logical Agent knowledge/identity architecture. Autonomous mutation/promotion pipeline is post-v1.

## Purpose

The Agent Genome defines persistent Logical Agent identity and reusable guidance independently of concrete execution providers/models/harnesses.

> **Backend choice does not define Agent identity. Pantheon owns the Logical Agent; execution adapters compile the frozen Agent inputs into a backend-specific context package.**

## Canonical Agent home

Conceptually:

```text
~/.pantheon/agents/<agent>/
  SOUL.md
  AGENT.yaml
  BEHAVIOR.md
  memory/
  skills/
  experiences/      # future learning inputs
  reflections/      # future candidates
  evals/
  changes/
```

Filesystem layout is operator storage/configuration representation; active runtime authority comes from immutable Agent/configuration snapshots, not whatever files happen to contain during a Run.

## Knowledge layers

### SOUL

Stable human-governed identity/responsibility/decision principles. It excludes credentials, transient repository paths, provider flags and short-lived procedures.

SOUL mutation requires human approval and is not automatic in v1.

### AGENT.yaml

Machine-readable applicability/competencies/execution requirements/tool/action declarations/permission ceiling/delegation/limits and Genome references. See `docs/architecture/agents-and-context/agent-manifest.md`.

### BEHAVIOR

Validated cross-task heuristics describing how the Agent should approach work. V1 treats configured BEHAVIOR as static approved input during a Run.

### Memory

Durable facts/context/preference-like knowledge that may influence work without becoming procedural authority. Context Builder selects a bounded immutable set of Memory items for each Run and freezes their versions/digests in ContextPlan.

### Skills

Portable procedural knowledge with progressive disclosure. Skills may contain `SKILL.md` plus references/scripts/assets/evals. `skills.preload` is initial ContextPlan content; other allowed Skills are on-demand.

A Skill is not a competency, permission, tool/action or execution feature.

## Agent snapshot and Run freezing

Configuration/Agent registry publishes immutable Agent versions/snapshots. Agent Resolution selects a current eligible Logical Agent version; the Run freezes that identity.

Context Builder freezes exact selected:

```text
Agent version
SOUL version/digest
BEHAVIOR version/digest
preloaded Skill versions
selected Memory item versions
ContextPolicy/retrieval provenance
```

Later file/memory/Skill changes do not mutate an existing Run.

## V1 scope

V1 includes:

```text
SOUL
AGENT.yaml
BEHAVIOR
approved Skills
bounded selected Memory
immutable Agent snapshots
Context Builder integration
```

V1 does **not** automatically promote or mutate Genome content based on execution outcomes.

This keeps implementation focused on deterministic orchestration and avoids making unvalidated model reflection part of authoritative Run configuration.

## Post-v1 learning architecture

Pantheon may later support a staged learning pipeline:

```text
Task/Event/Evidence outcomes
  ↓
Experience
  ↓
Reflection proposal
  ↓
classification
  ↓
candidate Memory/Skill/Behavior/Soul change
  ↓
validation/evaluation
  ↓
promotion or rejection
```

The invariant remains:

> **Reflection is a hypothesis, not truth. Production Genome state changes only through versioned governed promotion.**

Potential promotion strength increases with permanence:

```text
Memory candidate        lower bar, still validated/governed
Skill/Behavior change   evaluation-gated
SOUL change             explicit human approval
```

Learning may never silently expand operator-controlled `accepts` or `competencies`.

## Experiences and reflections

Future raw experiences/reflections are not automatically supplied to normal Run context. They are learning-system inputs until validated/promoted.

This prevents transient failure narration, prompt injection or one-off model conclusions from becoming long-lived Agent instruction authority.

## Skill mutation

If future skill learning is enabled, production Skill versions remain immutable. Candidate change produces a new version/candidate evaluated against regression/effectiveness cases before activation.

Old Runs continue referencing old frozen Skill versions.

## Cross-Agent promotion

Future sharing may follow:

```text
private Agent candidate
  ↓ evidence/evals
shared candidate
  ↓ stronger validation/review
published shared Skill/Behavior version
```

A curator/librarian may organize/deduplicate candidate knowledge but is not trusted to manufacture semantic truth without evidence.

## Outcome signals for future learning

Useful objective signals include:

```text
Acceptance Evidence
build/test results
recovery/failure fingerprints
retries
rollbacks
resource/usage cost
policy violations
user corrections
```

Events/Evidence are preferred over hidden model reasoning/transcripts as learning ground truth.

## Security

Genome content cannot grant authority. Permissions/actions remain governed by Agent Manifest ceiling, current policy, Grants, Agent Control and Sandbox.

Memory/Skill/reference text is context data/instruction guidance, never a Capability Grant or secret container.

Secrets/Agent Control credentials must never be stored in Genome content.

## Core invariants

1. Logical Agent identity is backend/provider/model independent.
2. Genome inputs are versioned/frozen per Run through Agent snapshot + ContextPlan.
3. SOUL/BEHAVIOR/Skills/Memory are distinct semantic layers.
4. Skill is not competency/tool/execution feature/permission.
5. V1 uses static approved Genome state; autonomous reflection/promotion is deferred.
6. Future learning is staged/evidence-gated/versioned and may never directly mutate current Runs.
7. Genome learning may not silently broaden `accepts`, competencies or security authority.
