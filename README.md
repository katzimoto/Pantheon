# Pantheon

Pantheon is a local-first multi-agent orchestration system designed to coordinate heterogeneous agent runtimes while keeping task state, routing, permissions, evaluation, and learning under a deterministic control plane.

## Design goals

- Treat Claude Code, OpenCode, and local models as first-class execution backends.
- Keep orchestration state deterministic and inspectable rather than hidden inside an LLM conversation.
- Give each persistent agent a portable identity, memory, skill set, and learning history independent of the model executing it.
- Route work based on capability, cost/quota, privacy, and security constraints.
- Isolate coding work with Git worktrees and risky execution with sandbox profiles.
- Make agent self-improvement evidence-driven, versioned, testable, and reversible.

## Design documents

- [Architecture overview](docs/architecture/overview.md)
- [Agent Genome: identity, memory, skills, and self-improvement](docs/architecture/agent-genome.md)

## Status

Early design phase. The architecture is being specified subsystem-by-subsystem before implementation.
