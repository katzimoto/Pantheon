# Context Builder

## Status

Canonical Pantheon context construction specification.

## Purpose

The Context Builder defines how Pantheon selects, freezes, and exposes the semantic inputs for a Run without binding the control plane to one provider's prompt, conversation, session, or tool representation.

The central rule is:

> **The Context Builder deterministically selects and freezes the authoritative semantic inputs for a Run. It produces a provider-neutral immutable `ContextPlan`; backend adapters render that plan into provider/harness-specific prompts, messages, files, tools, MCP resources, or sessions.**

## Three-layer model

Pantheon distinguishes:

```text
CONTEXT SOURCE SNAPSHOT
What authoritative Pantheon state is eligible?

        ↓

CONTEXT PLAN
What exact semantic information did Pantheon select?

        ↓

BACKEND CONTEXT PACKAGE
How did this backend represent it?
```

The source snapshot and ContextPlan are Pantheon-owned. Backend rendering is adapter-owned.

A ContextPlan is not a provider prompt transcript.

## Run boundary

Context Builder runs after durable Run commitment and before Attempt creation:

```text
Run committed
  ↓
workspace/context prerequisites prepared
  ↓
Context Builder
  ↓
ContextPlan attached exactly once
  ↓
ContextReady=True
  ↓
LaunchReady=True
  ↓
Attempt created
```

Once a ContextPlan is attached to a Run it is immutable.

Materially changing semantic execution context requires a new ExecutionRequest, Binding, and Run.

Runtime tool calls, file reads, command output, and provider message evolution inside the Attempt do not mutate the initial ContextPlan.

## ContextPlan

Conceptually:

```yaml
contextPlan:
  digest: sha256:...
  run: run_123

  builder:
    version: context-builder-v1
    policyDigest: sha256:...

  task:
    specDigest: sha256:...
    goalRevision: 8
    graphRevision: 47

  agent:
    version: sha256:...
    soul: sha256:...
    behavior: sha256:...

  workspace:
    startingRevision: workspace-rev_92

  sections:
    - kind: task-contract
      inclusion: mandatory
    - kind: agent-guidance
      inclusion: mandatory
    - kind: skill
      ref: skill://git
      version: sha256:...
      inclusion: preload
    - kind: memory
      ref: memory://...
      digest: sha256:...
      inclusion: preload
    - kind: continuation
      ref: continuation://...
      inclusion: mandatory
    - kind: reference
      artifact: artifact://sha256/...
      inclusion: on-demand
```

The plan stores immutable refs/digests and only small trusted instruction bodies where necessary.

## Trust and precedence strata

Context content has explicit semantic authority:

```text
PANTHEON EXECUTION PROTOCOL
trusted control-plane instructions
        ↓
GOAL / TASK CONTRACT
objective, constraints, outputs, acceptance
        ↓
AGENT GUIDANCE
SOUL, BEHAVIOR, selected Skills
        ↓
CONTINUATION / RECOVERY EVIDENCE
structured prior results
        ↓
REFERENCE DATA
Artifacts, repository files, external documents
```

Lower strata cannot override higher strata.

Reference content is always data, even if it contains text that looks like instructions.

Backend renderers must preserve this distinction using the strongest representation the execution mechanism supports.

## Inclusion classes

The initial ContextPlan uses three classes:

```text
MANDATORY
must be present at launch

PRELOAD
selected because it is likely useful

ON-DEMAND
discoverable/fetchable during execution
```

### Mandatory

Normally includes:

- Task objective;
- required Task outputs;
- Task constraints;
- Task acceptance contract;
- relevant Goal constraints;
- continuation/recovery context;
- Agent SOUL;
- Agent BEHAVIOR;
- execution protocol;
- Agent Control operation descriptions.

### Preload

Normally includes:

- Skills explicitly marked preload;
- bounded selected Memory;
- required small input Artifacts;
- critical child-result summaries;
- bounded workspace/repository orientation.

### On-demand

Normally includes references to:

- repository files;
- large Artifacts;
- non-preloaded Skills;
- documentation;
- historical Events;
- additional memory.

## Agent Manifest integration

The following Agent fields become direct Context Builder inputs:

- `genome.skills.available`;
- `genome.skills.preload`;
- memory namespaces/retrieval/token ceilings;
- `execution.requirements.minContextTokens`;
- immutable Agent/SOUL/BEHAVIOR versions.

V1 uses static approved SOUL, BEHAVIOR, Skills, and bounded Memory. Autonomous Genome learning/reflection/promotion is post-v1.

## Memory retrieval modes

`genome.memory.retrieval.mode` controls only Pantheon-owned initial Memory preload selection. It never delegates semantic context selection to the Agent/model.

V1 meanings are:

```text
disabled
  do not run initial Memory retrieval for preload

always
  run the configured deterministic Memory retriever for every Run

adaptive
  run deterministic relevance-based retrieval against the frozen
  Context source snapshot; the correct result may be zero Memory items
```

`adaptive` means the retriever adapts its result to the authoritative Task/Goal/Agent/source inputs, not that a model decides what it wants to remember. Given the same frozen source snapshot, ContextPolicy, retriever implementation/version, retriever parameters/index identity, and token ceiling, selection and ordering must be deterministic.

The ContextPlan records enough retrieval provenance to reconstruct the decision, including the retriever/policy version and any versioned index/corpus identity that affects ranking, plus the exact selected Memory item digests. BM25, vector, hybrid, or another retrieval technique is an implementation choice only if it satisfies this deterministic/frozen-provenance contract.

## Repository/workspace context

Pantheon does not preload an entire repository.

The initial package should normally contain:

- starting WorkspaceRevision;
- a small deterministic repository orientation;
- explicitly required paths/inputs;
- authorized tools for just-in-time inspection.

The Task-owned workspace remains the source for runtime exploration.

## Context capacity

Context capacity remains a factual execution compatibility property, separate from resource and budget accounting.

After Binding, the selected backend may report factual rendered/token size.

Conceptually:

```text
maximum input context
- required output reserve
- protocol/tool overhead
- safety margin
= usable initial context budget
```

The backend may measure the rendered package. It may not decide semantic importance.

## Mandatory-content overflow

Mandatory content is never silently truncated.

If mandatory content does not fit the selected execution strategy, preparation fails before an Attempt exists with a normalized error such as:

```text
context.required-content-too-large
```

Recovery may choose a different execution strategy, replan/decompose, or request human input.

## Deterministic trimming

Optional/preload trimming follows the frozen ContextPolicy.

For example:

```text
mandatory       always keep
preload tier 1  keep
preload tier 2  keep if capacity
preload tier 3  drop first
```

V1 does not use an LLM to decide what pre-launch requirements or context to remove.

## Memory freezing

If the memory retriever selects items A and C, the ContextPlan records their exact digests/versions and the retriever/policy/index provenance that affected selection.

Later memory, index, or retriever changes do not alter the Run.

The exact storage and retrieval implementation is not fixed here; the selected inputs and the provenance needed to reproduce their deterministic selection are.

## Blocking continuation

A new Run after blocking yield receives immutable continuation inputs, not an old provider conversation.

Conceptually:

```yaml
continuation:
  priorRun: run_17
  reason: blocking-child-completed
  startingWorkspaceRevision: workspace-rev_82
  resolvedInputs:
    findings: artifact://sha256/...
  join: join_44
```

The same pattern applies to semantic retries after Acceptance rejection: prior Candidate/Evidence and structured rejection information become frozen new-Run context.

## Provider/session boundary

Same-Attempt reconciliation may preserve/recover a provider session because it is still the same LaunchKey/execution lineage.

A new Run does not depend on reusing an opaque provider conversation/session from a previous Run.

Adapters may use provider caching/session facilities as optimizations, but those are not Pantheon semantic state.

## Tools and authorization

ContextPlan may describe available canonical operations and schemas.

Knowledge that an operation exists does not authorize it.

All consequential actions still pass through:

```text
Agent Control authentication
  ↓
current Task/Run authority
  ↓
current authorization policy
  ↓
PDP / Broker
```

No Capability Grant, Capability Ticket, Agent Control credential, or operator credential belongs in ContextPlan.

## Secrets

ContextPlan may contain a semantic SecretRef such as `secret://github/pat` where necessary, but never raw credential material.

Raw secrets never belong in:

- prompts/context plans;
- Run snapshots;
- Events;
- Artifacts;
- backend attachment metadata;
- logs.

## Content addressing

ContextPlan is canonical/content-addressed internal control-plane state, conceptually:

```text
context-plan://sha256/<digest>
```

It is not automatically exposed under ordinary Artifact visibility or retention semantics because it may contain sensitive control-plane metadata.

## Backend rendering

The adapter compiles:

```text
ContextPlan + ExecutionBinding
```

into a backend-specific context package.

Possible representations include:

- message hierarchies;
- bootstrap files;
- native tool definitions;
- MCP resources;
- CLI configuration;
- provider session bootstrap state.

The adapter records at least:

- ContextPlan digest;
- renderer version;
- rendered-package digest where representable.

Pantheon does not claim to know hidden provider system prompts or proprietary internal state.

## Reproducibility boundary

Pantheon promises to reconstruct the semantic inputs and control-plane decisions of a Run:

- Task/Goal/Graph revisions;
- Agent/SOUL/BEHAVIOR/Skill/Memory versions selected;
- starting WorkspaceRevision;
- continuation/recovery evidence;
- ContextPolicy/Builder/retriever provenance;
- backend renderer/version provenance where available.

Pantheon does not promise that a stochastic model rerun yields identical output tokens.

## Configuration integration

ConfigurationRevision includes a versioned `ContextPolicy` component controlling at least:

- mandatory section definitions;
- preload priority;
- memory limits;
- workspace orientation limits;
- context safety margin;
- deterministic optional drop order.

ContextPolicy changes affect future Runs only.

## Failure taxonomy

Context build failures occur before Attempt creation and are Preparation failures.

Initial codes may include:

```text
context.required-content-too-large
context.required-input-missing
context.memory-selection-failed
context.skill-version-unavailable
context.workspace-revision-unavailable
context.rendering-unsupported
context.measurement-failed
```

## Core invariants

1. Context Builder is deterministic control-plane logic, not an Agent/model.
2. Exactly one immutable provider-neutral ContextPlan is attached to each Run before Attempt creation.
3. Context source snapshot, ContextPlan, and backend rendering are distinct layers.
4. Provider conversation/session state is never durable context authority.
5. Material semantic context changes require a new Run.
6. Runtime tool/results evolution does not mutate the initial ContextPlan.
7. Context has explicit trust/precedence strata.
8. Initial context uses mandatory/preload/on-demand inclusion classes.
9. Large repositories and Artifacts are primarily just-in-time resources.
10. Backends report factual context capacity/measurement but do not decide semantic relevance.
11. Mandatory context is never silently truncated.
12. V1 context selection/trimming, including `adaptive` Memory retrieval, is deterministic; model-generated pre-launch selection/compaction is deferred.
13. Selected Memory/Skill inputs are frozen by exact version/digest, with retrieval provenance frozen where Memory ranking is used.
14. Blocking continuation and Acceptance rejection use immutable structured context rather than previous opaque provider sessions.
15. Tool visibility is distinct from authorization.
16. Secrets and security credentials never enter ContextPlan.
17. ContextPlan is content-addressed internal control-plane state.
18. Reproducibility means reconstructable semantic inputs and decisions, not identical model output.
