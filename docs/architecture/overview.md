# Pantheon Architecture Overview

## Status

Draft design. Pantheon is being specified subsystem-by-subsystem before implementation.

## Problem

Existing agent frameworks each optimize for a particular execution model: Claude-centric swarms, generic API agents, terminal-agent orchestration, or autonomous personal agents. Pantheon instead targets a heterogeneous local-first environment in which Claude Code, OpenCode, and local models are all first-class workers.

The control plane must not depend on any one provider or model.

## Core architecture

```text
                         User
                          │
                          ▼
                ┌──────────────────┐
                │     Pantheon     │
                │ deterministic    │
                │ control plane    │
                └────────┬─────────┘
                         │
                     Task DAG
                         │
          ┌──────────────┼──────────────┐
          │              │              │
          ▼              ▼              ▼
   Claude Code        OpenCode       Local Agent
          │              │              │
      Claude Pro     OpenCode Go     OpenAI-compatible
                                        │
                                      oMLX
```

Pantheon owns reality:

- task and dependency state;
- provider routing;
- process/session state;
- Git/worktree state;
- permissions and sandbox profiles;
- tests and acceptance criteria;
- quotas/budgets;
- artifacts and structured events;
- persistent agent identity and learning state.

LLMs provide intelligence, not authoritative orchestration state.

## Initial subsystems

1. **Agent Genome** — identity, memory, skills, experience, reflection, evaluation, and controlled self-improvement.
2. **Agent/provider schema** — canonical agent object plus compilation into Claude Code, OpenCode, and local-provider sessions.
3. **Task graph and scheduler** — deterministic DAG execution, dependencies, retries, cancellation, and recovery.
4. **Router** — capability/cost/privacy/security-aware provider selection and escalation.
5. **Workspace manager** — Git worktrees, commits, integration, conflict handling, and artifact ownership.
6. **Policy/sandbox engine** — filesystem, shell, network, Docker/VM and approval boundaries.
7. **Review/evaluation system** — objective checks plus optional independent model review.
8. **Event bus** — structured inter-agent communication rather than unconstrained agent chat.
9. **Observability/UI** — tasks, agents, diffs, quotas, learning proposals, logs, and recoverability.

## Design principles

- One control plane; do not stack independent orchestrators.
- Provider-independent agents; model/provider is execution infrastructure, not identity.
- Structured state and events over conversational coordination.
- Task DAGs over vague swarm topologies.
- Worktrees for coding isolation; sandboxes/VMs for risky execution.
- Cheap/local compute for routine work and reflection; scarce premium compute for hard reasoning and validation.
- Self-improvement is versioned, evaluated, observable, and reversible.
- Human authority remains above permanent identity and high-impact policy changes.
