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
What exact authoritative Pantheon source identities are eligible for this Run?

        ↓

CONTEXT PLAN
What exact semantic information did Pantheon select from those sources?

        ↓

BACKEND CONTEXT PACKAGE
How did this backend represent it?
```

`ContextSourceSnapshot` and ContextPlan are Pantheon-owned. Backend rendering is adapter-owned.

A ContextPlan is not a provider prompt transcript.

## ContextSourceSnapshot

Every Run freezes one immutable `ContextSourceSnapshot` **at Run-intent commitment (T3)**, before Context Builder performs retrieval/selection.

The snapshot is a canonical manifest over source identities, not a copy of all source bytes. It binds enough immutable/versioned state that later Context Builder retries for the same Run observe the same eligible semantic universe.

Conceptually:

```yaml
contextSourceSnapshot:
  digest: sha256:...

  task:
    specDigest: sha256:...
    goalRevision: 8
    graphRevision: 47

  agent:
    snapshot: sha256:...
    soul: sha256:...
    behavior: sha256:...
    permittedSkills:
      - ref: skill://git
        version: sha256:...

  configuration:
    revision: cfgrev_43
    contextPolicyDigest: sha256:CONTEXT

  workspace:
    startingRevision: workspace-rev_92

  continuation:
    ref: continuation://...        # nullable

  memory:
    corpusGeneration: memory-corpus://...
    indexGeneration: memory-index://...
    retrieverVersion: ...

  requiredInputs:
    - artifact://sha256/...
```

Exact fields depend on which sources are enabled, but every selection-affecting source must be represented by an immutable version/digest or a stable, reconstructable generation identity.

If a configured source cannot provide a stable/reconstructable identity for the view that Context Builder is supposed to select from, that source is not eligible for frozen v1 context. Pantheon does not silently query mutable "latest" state during later preparation and call the result reproducible.

The source snapshot freezes **eligibility**, not selection. It does not run Memory retrieval, render a prompt, read arbitrary repository content, invoke a model/backend, or choose preload items inside T3.

A change to ConfigurationRevision, Memory corpus/index generation, permitted Skill versions, continuation inputs, or other source semantics after T3 cannot alter the already-committed Run. Materially different source semantics require a new Run.

## Run boundary

Context Builder runs after durable Run commitment and before Attempt creation:

```text
pre-T3 source resolution
  ↓
ContextSourceSnapshot canonicalized
  ↓
T3 commits Run + exact ContextSourceSnapshot identity
  ↓
workspace/context prerequisites prepared
  ↓
Context Builder reads only that frozen source snapshot
  ↓
ContextPlan persisted
  ↓
one-time RunContextPlan attachment
  ↓
ContextReady=True
  ↓
LaunchReady=True
  ↓
Attempt created
```

Context construction may retry after daemon restart, but every retry for the same Run uses the same `ContextSourceSnapshot`.

Once a ContextPlan is attached to a Run it is immutable and cannot be replaced by another plan for that Run.

Materially changing semantic execution context requires a new ExecutionRequest, Binding, ContextSourceSnapshot, and Run.

Runtime tool calls, file reads, command output, and provider message evolution inside the Attempt do not mutate the initial ContextPlan.

## ContextPlan

Conceptually:

```yaml
contextPlan:
  digest: sha256:...
  sourceSnapshot: sha256:...

  builder:
    version: context-builder-v1
    contextPolicyDigest: sha256:...

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

The plan's `sourceSnapshot` must equal the immutable source snapshot bound to its Run. A ContextPlan cannot be attached to a different Run merely because its selected sections happen to be byte-identical.

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

The initial semantic availability of versioned Skills/Memory/reference inputs must remain within the permissions and generation identities frozen by the ContextSourceSnapshot. Runtime mutable Workspace exploration is governed separately by the Run's Workspace authority and does not rewrite the initial source snapshot.

## Agent Manifest integration

The following Agent fields become direct Context Builder inputs through the frozen source snapshot:

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
  ContextSourceSnapshot; the correct result may be zero Memory items
```

`adaptive` means the retriever adapts its result to the authoritative Task/Goal/Agent/source inputs, not that a model decides what it wants to remember. Given the same ContextSourceSnapshot, `contextPolicyDigest`, retriever implementation/version, retriever parameters/index identity, and token ceiling, selection and ordering must be deterministic.

The ContextPlan records enough retrieval provenance to reconstruct the decision, including the exact source snapshot, retriever/version, parameters/index/corpus identity that affected ranking, plus the exact selected Memory item digests. BM25, vector, hybrid, or another retrieval technique is an implementation choice only if it satisfies this deterministic/frozen-provenance contract.

## Repository/workspace context

Pantheon does not preload an entire repository.

The initial package should normally contain:

- starting WorkspaceRevision frozen by the ContextSourceSnapshot;
- a small deterministic repository orientation;
- explicitly required paths/inputs;
- authorized tools for just-in-time inspection.

The Task-owned workspace remains the source for runtime exploration. Mutable filesystem state observed after launch is runtime data, not a mutation of the initial ContextPlan or ContextSourceSnapshot.

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

Optional/preload trimming follows the frozen ContextPolicy identified by `contextPolicyDigest` in the source snapshot.

For example:

```text
mandatory       always keep
preload tier 1  keep
preload tier 2  keep if capacity
preload tier 3  drop first
```

V1 does not use an LLM to decide what pre-launch requirements or context to remove.

Implementation status (v0.1.0): no backend renderer exists yet, so preparation performs no token measurement and applies no capacity budget — it would otherwise be claiming a token count nothing measured. The drop machinery is deterministic over the frozen policy's priority/drop order and is proven by pure-domain tests with synthetic measurements; capacity evaluation arrives with backend rendering, and until then a Run's plan contains only mandatory content plus bounded references. In the MVP the static approved SOUL/BEHAVIOR guidance of the selected Agent version is carried as bounded text inside the immutable agents configuration component; its digests are frozen into the ContextSourceSnapshot at T3 and validated against that stored component both at commit time and at every later preparation. A plan's section order is a deterministic function of the sections alone — inclusion class, then authority stratum, then a fixed kind ordinal, then key — so walking the list in order never presents lower-authority content above higher-authority content; the frozen policy contributes through mandatory-section satisfiability and the optional drop order, not through ordering.

## Memory freezing

If the memory retriever selects items A and C, the ContextPlan records their exact digests/versions and the retriever/policy/index provenance that affected selection.

Later memory, index, retriever, or active-configuration changes do not alter the Run because retrieval is constrained to the Run's frozen ContextSourceSnapshot.

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

That continuation identity is frozen into the new Run's ContextSourceSnapshot before T3.

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

No Capability Grant, Capability Ticket, Agent Control credential, or operator credential belongs in ContextPlan or ContextSourceSnapshot.

## Secrets

ContextPlan may contain a semantic SecretRef such as `secret://github/pat` where necessary, but never raw credential material.

Raw secrets never belong in:

- ContextSourceSnapshots;
- prompts/context plans;
- Run snapshots;
- Events;
- Artifacts;
- backend attachment metadata;
- logs.

## Content addressing

ContextSourceSnapshot and ContextPlan are canonical/content-addressed internal control-plane state, conceptually:

```text
context-source://sha256/<digest>
context-plan://sha256/<digest>
```

They are not automatically exposed under ordinary Artifact visibility or retention semantics because they may contain sensitive control-plane metadata.

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

- exact ContextSourceSnapshot;
- Task/Goal/Graph revisions;
- Agent/SOUL/BEHAVIOR/Skill/Memory source versions eligible and selected;
- starting WorkspaceRevision;
- continuation/recovery evidence;
- ContextPolicy/Builder/retriever provenance;
- backend renderer/version provenance where available.

Pantheon does not promise that a stochastic model rerun yields identical output tokens.

## Configuration integration

ConfigurationRevision includes a versioned `context` component whose canonical digest is exposed to context decisions as `contextPolicyDigest`.

ContextPolicy controls at least:

- mandatory section definitions;
- preload priority;
- memory limits;
- workspace orientation limits;
- context safety margin;
- deterministic optional drop order.

The active ConfigurationRevision/context component is resolved while preparing the ContextSourceSnapshot and the exact `configRevision + contextPolicyDigest` is frozen into the Run at T3. Later ContextPolicy activation affects future Runs only; it never changes Context Builder behavior for a Run whose source snapshot is already committed.

Current hard/security policy remains independently enforceable at execution/action time and may stop a Run; freezing ContextPolicy is reproducibility/semantic selection provenance, not a way to retain obsolete security authority.

## Failure taxonomy

Context build failures occur before Attempt creation and are Preparation failures.

Initial codes may include:

```text
context.required-content-too-large
context.required-input-missing
context.memory-selection-failed
context.skill-version-unavailable
context.workspace-revision-unavailable
context.source-snapshot-unavailable
context.source-generation-unavailable
context.rendering-unsupported
context.measurement-failed
```

If an immutable source/version referenced by the ContextSourceSnapshot is unavailable, Pantheon fails/reconciles preparation; it does not substitute a newer source generation inside the existing Run.

## Core invariants

1. Context Builder is deterministic control-plane logic, not an Agent/model.
2. Every Run durably binds exactly one immutable ContextSourceSnapshot at T3 before context selection begins.
3. A ContextSourceSnapshot freezes eligible semantic source/version/generation identities; it does not perform retrieval or rendering.
4. At most one immutable provider-neutral ContextPlan is attached to each Run, and its source snapshot must equal the Run's frozen source snapshot.
5. Context construction may retry after restart only against that same source snapshot; a different semantic source universe requires a new Run.
6. Context source snapshot, ContextPlan, and backend rendering are distinct layers.
7. Provider conversation/session state is never durable context authority.
8. Material semantic context changes require a new Run.
9. Runtime tool/results evolution does not mutate the initial ContextPlan.
10. Context has explicit trust/precedence strata.
11. Initial context uses mandatory/preload/on-demand inclusion classes.
12. Large repositories and Artifacts are primarily just-in-time resources.
13. Backends report factual context capacity/measurement but do not decide semantic relevance.
14. Mandatory context is never silently truncated.
15. V1 context selection/trimming, including `adaptive` Memory retrieval, is deterministic; model-generated pre-launch selection/compaction is deferred.
16. Selected Memory/Skill inputs are frozen by exact version/digest, with retrieval provenance and source generation frozen where ranking/eligibility is used.
17. Blocking continuation and Acceptance rejection use immutable structured context rather than previous opaque provider sessions.
18. Tool visibility is distinct from authorization.
19. Secrets and security credentials never enter ContextSourceSnapshot or ContextPlan.
20. ContextSourceSnapshot and ContextPlan are content-addressed internal control-plane state.
21. Reproducibility means reconstructable semantic inputs and decisions, not identical model output.
