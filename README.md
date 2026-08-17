# Pantheon

Pantheon is a local-first multi-agent orchestration system designed to coordinate heterogeneous agent runtimes while keeping task state, routing, permissions, evaluation, and learning under a deterministic control plane.

## Design goals

- Treat Claude Code, OpenCode, and local models as first-class execution backends.
- Keep orchestration state deterministic and inspectable rather than hidden inside an LLM conversation.
- Give each persistent agent a portable identity, memory, skill set, and learning history independent of the model executing it.
- Route work based on capability, cost/quota, privacy, and security constraints.
- Separate Task coding state from authoritative Git refs, and contain risky execution with sandbox profiles.
- Make agent self-improvement evidence-driven, versioned, testable, and reversible.

## Documentation

Start at [docs/README.md](docs/README.md). It is the documentation entry point:
it states which material is canonical and routes you to the relevant subsystem
contracts.

- [Documentation entry point](docs/README.md) — where to start, source-of-truth rules
- [Architecture overview](docs/architecture/overview.md) — the system model and cross-cutting invariants
- [Architecture subsystem map](docs/architecture/README.md) — which documents to read for a given change
- [Agent Genome: identity, memory, skills, and self-improvement](docs/architecture/agents-and-context/agent-genome.md)

Material under [docs/reviews/](docs/reviews/README.md) is historical review
analysis and is not canonical architecture.

## Working in this repository

Contributors and coding agents should read [AGENTS.md](AGENTS.md), the
repository operating contract: how to navigate to authoritative material, what
may be changed, and how to validate a change. It is deliberately short and
routes into the documentation above rather than restating it.

## Status

Early design phase. The architecture is being specified subsystem-by-subsystem before implementation.
