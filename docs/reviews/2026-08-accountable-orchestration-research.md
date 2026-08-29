# Research: Accountable Multi-Agent Orchestration for Pantheon

## Status

**Not canonical.** This is the research and decision record for Issue #100. It
preserves the evidence, source limitations and disposition of candidate
post-MVP movements. It does not authorize implementation and is not a runtime
contract.

The one decision adopted immediately is written into
`docs/architecture/overview.md` and
`docs/architecture/tasks/task-spawn-and-dynamic-graphs.md`: cross-agent
coordination uses controller-brokered, typed, immutable, provenance-bound
artifacts rather than ambient peer chat or shared mutable memory. Those
canonical documents are authoritative for that rule. Every other retained
movement becomes real only through a later Engineering Mission and accepted
canonical/implementation change.

## Question

What should Pantheon learn from
[@pwnh4's SMOG post](https://x.com/pwnh4/status/2093001388246663370),
and which additional movements improve Pantheon's architecture without
duplicating its existing control plane or derailing the v0.1.0 sequence?

The post describes a specialized smart-contract-audit system with deliberately
designed agents, deterministic tools and sandboxes, a graph of invariants and
hypotheses, per-hypothesis confidence, coverage tracking and an aggregate
"general feeling" about completion. Its most important unresolved question is
how the system can know that the work is done and give a human reason to trust
that conclusion.

## Method

The review used two passes:

1. reconstruct Pantheon's implemented and specified control plane from the
   current repository, v0.1.0 Milestone and active critical-path missions;
2. test each candidate movement against primary or first-party evidence on
   multi-agent scaling, evaluation, context, oversight, provenance and agentic
   security, retaining only capabilities Pantheon does not already have.

The comparison treated a result as stronger when it survived contrary or
limiting evidence. Product reports and recent preprints are identified as such
rather than treated as universal laws.

## Pantheon baseline

Pantheon already has unusually strong foundations for this problem:

- durable revisioned control-plane state and an Event Journal;
- immutable Run/Attempt lineage and provider/model/harness-neutral execution;
- deterministic, provenance-bound ContextPlan construction;
- controller-owned budgets and factual usage;
- exact immutable Candidate/Evidence subjects and independent acceptance;
- Attempt-bound Agent Control, bounded dynamic `task.spawn` and restart-safe
  reconciliation;
- evidence-gated, versioned Genome promotion as a stated post-v1 invariant.

The review therefore rejects recommendations to rebuild generic memory,
identity, evaluation, budgeting, context freezing or event replay. The missing
layer is not more infrastructure of those kinds; it is a way to evaluate and
select orchestration strategies, represent open-ended inquiry explicitly and
let an operator steer that state durably.

## Evidence synthesis

| Evidence | Useful result | Limitation carried into the decision |
|---|---|---|
| [SMOG post](https://x.com/pwnh4/status/2093001388246663370) | Specialized roles, deterministic tools, invariant discovery, hypothesis tracking and inspectable coverage are useful investigation primitives. | Expert retrospective, not a controlled comparison. Self-reported confidence, a universal coverage threshold and an aggregate feeling are not acceptance evidence. |
| [Google, *Towards a Science of Scaling Agent Systems*](https://arxiv.org/html/2512.08296v3) | Multi-agent value is conditional on task decomposability, sequential depth, tool use and verification architecture. Some configurations improve substantially while others amplify errors and regress. | Controlled benchmark distribution and fixed budgets do not establish Pantheon-specific thresholds. |
| [Anthropic multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system) | Breadth-first, independently explorable research can benefit from bounded delegation and central synthesis. | The reported gain is first-party and not cost-matched; the system used roughly fifteen times the chat tokens of ordinary conversations. |
| [Anthropic, multi-agent patterns and problems](https://www.anthropic.com/research/multiagent-systems) | Agents used as bounded invocations compose more reliably than long-lived peers; centralized validation limits error propagation. | Emerging design patterns, not a settled standard. Swarms still risk silos, bad merging, convergence and hidden information. |
| [Anthropic agent evaluations](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents) and [OpenAI's agent improvement loop](https://developers.openai.com/cookbook/examples/agents_sdk/agent_improvement_loop) | Complete traces plus environment outcomes, repeated isolated trials and mixed graders are needed to compare harness changes. Real failures should seed capability and regression suites. | Evaluation design remains workload-specific; a passing end-state grader may miss a bad reasoning or evidence process. |
| [Anthropic context engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents) | Context is a finite attention resource; high-signal just-in-time retrieval and minimal tool overlap outperform indiscriminate accumulation. | Does not prove causal attribution for an individual context item in one stochastic Run. |
| [Anthropic autonomy study](https://www.anthropic.com/research/measuring-agent-autonomy) and [Microsoft Magentic-UI](https://www.microsoft.com/en-us/research/blog/magentic-ui-an-experimental-human-centered-web-agent/) | Experienced users benefit from monitoring plus easy intervention, plan editing and co-tasking rather than approval at every tool call. | Early, platform-specific evidence; Magentic-UI includes simulated-user evaluation. |
| [OWASP Agentic Top 10](https://genai.owasp.org/resource/owasp-top-10-for-agentic-applications-for-2026/) and [SLSA provenance](https://slsa.dev/spec/v1.2/provenance) | Agent messages and memory need source attribution, typed/versioned exchange, isolation, lifecycle controls and inspectable lineage. Automatic re-ingestion creates poisoning and cascading-failure risk. | Security guidance defines controls, not Pantheon's exact data model. |
| [AI scientists produce results without reasoning scientifically](https://arxiv.org/abs/2604.18805) and [EviGraph](https://arxiv.org/html/2608.04738v2) | Outcome-only evaluation can miss evidence neglect and failed hypothesis revision. Typed question/hypothesis/finding/claim graphs make contradiction and repair more inspectable. | Both are recent preprints. EviGraph improves support but does not establish that one universal graph schema fits every Pantheon workload. |
| [Google DeepMind AlphaEvolve](https://deepmind.google/blog/alphaevolve-a-gemini-powered-coding-agent-for-designing-advanced-algorithms/) | Broad proposal generation can improve results when objective evaluators reliably measure the real target. | It does not justify autonomous policy promotion where the evaluator is incomplete or gameable. |

## Decisions

### Adopt now: brokered typed artifact exchange

Pantheon already models dynamic work as Tasks, Runs, accepted Artifacts and
controller-owned joins. That structure should remain the only cross-agent
coordination authority:

- a worker requests bounded outcomes, not an unmanaged peer process;
- cross-agent payloads have a declared type, scope/audience and provenance;
- exchanged state is immutable and versioned;
- accepted Artifact bindings, not transcripts or shared workspaces, enter a
  later Run's context;
- no ambient peer chat or shared mutable memory becomes graph, completion,
  permission or execution authority.

This is a design constraint, not a new v0.1.0 operation.

### Protect the MVP critical path

The v0.1.0 proof still depends on the existing #33 through #39 sequence. The
research movements below are post-MVP missions. None belongs inside the active
Agent Control, container, executor, evaluation, completion, recovery or release
missions.

### Retain as dependency-ordered post-MVP movements

1. **Measure orchestration before optimizing it.** Add immutable scenario
   snapshots, strategy variants, repeated isolated trials and comparable
   outcomes. This is distinct from accepting one Task Candidate.
2. **Seed capability and regression suites from real failures.** Capture
   representative recovery, budget, sandbox, evaluation and coordination
   failures so an orchestration change cannot improve a headline case while
   silently losing a load-bearing behavior.
3. **Select coordination from task structure.** A controller-owned assessment
   records decomposability, sequential depth, tool density, shared-state
   contention, evidence independence, verification locality and risk. Single
   Agent remains the default; bounded parallel or centralized-worker modes
   require measured justification.
4. **Represent inquiry separately from work.** TaskGraph says what work exists;
   an optional Inquiry Graph says what is known and why. Candidate nodes include
   Question/Gap, Invariant, Hypothesis, Experiment/ProofAttempt,
   Observation/Finding, Claim, Contradiction and Verdict. Negative results and
   supersession remain durable.
5. **Make coverage and stopping controller-owned.** A Goal-bound coverage
   contract identifies mandatory requirements and acceptable evidence.
   Terminal reasons distinguish satisfied, budget-exhausted,
   no-verified-progress, unresolved-critical, blocked, human-stop and
   risk-limit outcomes. Useful-but-incomplete is not success.
6. **Make operator steering durable and plan-level.** Focus revisions, branch
   dispositions, evidence challenges and checkpoints create inspectable
   revision/replan events rather than mutating a running model's hidden context.
7. **Turn ContextPlan into an effectiveness and provenance loop.** Record why a
   source was selected, its trust/ancestry, intended support and downstream use;
   compare context policies across repeated trials; quarantine or expire
   unvalidated agent output instead of automatically re-ingesting it.
8. **Learn orchestration policy only under governance.** Begin with transparent
   versioned heuristics. Later policies or Genome candidates run in shadow or
   replay and pass capability, regression, cost and safety gates plus explicit
   promotion authority. A generator never changes its own evaluator or
   promotion threshold.

## Metrics to keep separate

Pantheon should not compress trust into one confidence score. At minimum,
orchestration evaluation should keep these dimensions separable:

| Dimension | Examples |
|---|---|
| Outcome | acceptance rate, pass@1, pass^k, partial/blocked rate |
| Evidence | supported critical claims, independently verified findings, contradiction closure |
| Coverage | mandatory invariant coverage, unresolved critical gaps, risk-scoped coverage |
| Efficiency | verified outcome per cost, tokens, tool calls and wall time |
| Coordination | duplicate work, merge conflicts, error amplification, marginal verified novelty |
| Context | footprint, source-trust mix, retrieval failures, outcome by ContextPlan policy |
| Oversight | interventions, accepted/rejected replans, late reversals, approval burden |
| Safety | policy denials, forbidden effects, taint propagation, recovery success |

## Rejected or explicitly deferred mechanisms

- **Arbitrary swarms:** Agent count is an evaluated strategy output, not a
  product target.
- **Neo4j-first design:** typed graph semantics do not require a graph database;
  Pantheon's existing durable storage and CAS are the initial implementation
  boundary unless measured access patterns prove otherwise.
- **A "general feeling" completion Agent:** completion belongs to a
  deterministic controller over Goal-bound requirements and accepted Evidence.
- **A universal 80% threshold:** unresolved mandatory requirements dominate a
  weighted noncritical score.
- **Self-reported confidence as truth:** store claims, evidence, contradictions,
  verifier results and calibrated uncertainty separately.
- **Peer chat or shared mutable memory:** neither is durable, replayable or safe
  authority. Use the adopted artifact-exchange contract.
- **Automatic trust or re-ingestion:** agent output remains tainted input until
  provenance and validation permit promotion.
- **Hard-coded orchestration constants as architectural truth:** version
  heuristics and replace them only when replay data supports the change.
- **A promise of deterministic model output:** Pantheon can promise a
  reproducible envelope of frozen inputs, versions, tools, policy, environment
  and lineage, not identical stochastic output.
- **Self-promoting policy or Genome:** generation, evaluation, acceptance and
  promotion authority remain separate.

## Final disposition

The most important movement is not "more agents." It is more accountable
orchestration:

> **Scale verified, non-redundant work; represent what is known and unknown;
> let the operator steer durable plans; and learn orchestration policy only
> from replayable evidence.**

The accepted invariant is now canonical. The remaining movements are preserved
here and decomposed as post-MVP Engineering Missions; this review itself grants
none of their runtime authority.
