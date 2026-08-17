---
name: pantheon-change-verification
description: Run and interpret Pantheon's canonical ./scripts/verify.sh before claiming a change is done, and understand why a completion attempt gets blocked after the working tree changes. Use before finishing a mission or pull request, after any edit to tracked files, and whenever a Stop-hook message says verification is stale.
metadata:
  pantheon-authority: procedural-guidance-only
---

# Pantheon change verification

`./scripts/verify.sh` is Pantheon's one canonical verification command
(`AGENTS.md`, `docs/development/implementation.md`). This skill is the
procedure for running it correctly and reacting to what it reports — it is
not a second copy of what it checks, and it never substitutes for it.

## When this applies

- Before telling the user or writing in a pull request that a change is
  ready, complete, or verified.
- After any edit to a tracked file, before the next claim of completeness.
- When a completion attempt is blocked by the local Stop hook
  (`scripts/hooks/check-stale-verification.sh`) reporting that verification
  is stale for the current working tree.

Do not use this for exploratory read-only work that changes nothing, and do
not treat a passing narrow post-edit check (see below) as a substitute for a
full run — it is early feedback for one file, not the gate.

## Procedure

1. **Run `./scripts/verify.sh` from anywhere in the repository.** It needs
   only the pinned toolchain and ordinary build prerequisites. Do not invent
   a different command, flag combination, or partial run and treat it as
   equivalent.

2. **Read failures in the order they were designed to appear**: structural
   checks (action pins, doc links/structure, crate dependency boundaries)
   report in about a second, before the slower `cargo fmt`, Clippy, test,
   and rustdoc stages run. Fix the earliest failure first; a later stage may
   not even be reached until it is resolved.

3. **Fix the root cause, not the check.** A failing check is evidence of a
   real problem — a forbidden crate edge, a broken documentation reference,
   a Clippy lint, a failing test. Do not weaken the check, silence the lint
   inline without justification, or restructure code merely to route around
   a check while leaving the underlying issue.

4. **If a failure is pre-existing and unrelated to your change** (reproduce
   it on a clean checkout of the target branch before concluding this), it
   is discovered work: report it per `AGENTS.md`, and say so explicitly
   rather than silently claiming a clean run. Do not fix unrelated
   pre-existing failures as a side effect unless the mission's acceptance
   criteria require it.

5. **Environment limits are not a pass.** If a required tool (the pinned
   Rust toolchain, `cargo`) is unavailable in your execution environment,
   you have not verified the change — say exactly which stages ran and
   which did not, rather than reporting `verify.sh` as passing.

## The stale-verification guardrail

`./scripts/verify.sh` records a fingerprint of the working tree on a
successful run, in a local, transient, uncommitted location under `.git/`
(never repository authority — see
`docs/development/agent-skills-and-hooks.md`). A local Claude Code Stop hook
compares that fingerprint against the current tree whenever a turn is about
to end with uncommitted changes present:

- if the tree matches the last successful `verify.sh` run, nothing happens;
- if the tree has uncommitted changes with no matching successful run
  recorded, and the turn does not contain a `## Handoff` per
  `docs/development/change-lifecycle.md`'s handoff contract, the hook blocks
  the stop and asks for one of: run `./scripts/verify.sh` again, or write a
  proper handoff instead of an implicit completion claim.

This is a local guardrail, not the gate itself, and it can only run inside
Claude Code today (see the portability boundary in
`docs/development/agent-skills-and-hooks.md`). Its absence — a different
agent surface, or a fresh clone without prior hook state — does not relax
the requirement in step 1: verify before claiming done, every time.

## What this skill must not do

- It does not replace `./scripts/verify.sh`, CI, or GitHub check state as
  the actual gate.
- It does not grade evidence quality in a pull request — that is
  independent review (`pantheon-independent-review`).
- It does not authorize skipping a stage because it is slow or
  inconvenient.

## Non-triggers

- Read-only inspection with no working-tree change.
- A change already verified with nothing modified since (the stale-tree
  guardrail will not fire, and re-running adds nothing).
