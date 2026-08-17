# Pantheon Agent Contract

How to work in this repository. What Pantheon *is* lives in `docs/`; this file
does not repeat it. See `README.md` for the project summary.

## Repository state

Pantheon holds canonical architecture documents, JSON Schemas, and a Rust
workspace under `crates/`. The workspace is scaffolding: every crate compiles and
states its own boundary, and none implements Pantheon behaviour yet.

Rust is the implementation language, and the toolchain is pinned in
`rust-toolchain.toml`; no other language or toolchain enters unless the task says
so. `docs/development/implementation.md` is the map of where code belongs and
which crate may depend on which. Read it before adding code, a crate or a
dependency — the dependency boundaries are mechanically enforced, so guessing
costs a failed verification.

## Validation

Run `./scripts/verify.sh` after your final change, before reporting the task
complete or handing it off. It works from anywhere in the repository, and CI runs
the same command on Linux.

It runs the GitHub Actions pin check, the documentation checks, the internal
crate dependency check, formatting, Clippy with warnings denied, the tests, and
rustdoc with warnings denied — all against the committed lockfile. It needs the
pinned Rust toolchain and ordinary OS build prerequisites, and nothing else.

`docs/development/implementation.md` states what each stage proves and what is
deliberately excluded. A new check belongs in `scripts/verify.sh`, so that local
and CI verification never drift into two different commands.

## Agent skills and hooks

Pantheon-specific procedural guidance lives once, at
`.agents/skills/<name>/SKILL.md`; `docs/development/agent-skills-and-hooks.md`
is canonical for that mechanism, the four MVP skills, and what a lifecycle
hook may and may not do. A skill never outranks this file, canonical
architecture, or `./scripts/verify.sh` — it only operationalizes them.

Four triggers are mandatory for every coding agent working in this
repository, regardless of which agent surface is running:

- before planning or implementing an Engineering Mission, use
  `pantheon-mission-planning`;
- before reporting repository work complete, use
  `pantheon-change-verification`;
- before creating or updating durable pull request evidence, use
  `pantheon-pr-evidence`;
- independent review of a Pantheon change must use
  `pantheon-independent-review`, and only from a principal/context distinct
  from whoever authored the change.

A local hook may block a completion attempt when the working tree has
changed since the last successful `./scripts/verify.sh` run; the fix is to
verify again, or, for genuinely unfinished work, write a proper `## Handoff`
(`docs/development/change-lifecycle.md`) instead of stopping as if the work
were done.

## Start here

1. Establish the task: the requested outcome and its acceptance criteria. When
   the task comes from a GitHub Issue, the Issue defines the requested outcome
   and the intended scope; repository evidence determines which changes are
   actually required to satisfy it. Do not silently broaden the mission because
   adjacent work is visible.
2. Read `docs/README.md`. It states what is canonical and how to navigate.
3. Read `docs/architecture/overview.md` if you need the system model and do not
   already have it.
4. Use `docs/architecture/README.md` to find the domain the task touches. It
   carries reading recipes for common kinds of change; follow the matching one
   instead of reading the tree.
5. Read only the contracts it names, then the relevant schemas, then
   implementation and tests where they exist.
6. For a code change, read `docs/development/implementation.md` to place it:
   which crate owns the concern, and what that crate is allowed to depend on.
7. Decide the smallest correct change before editing anything.

Retrieval is selective. Start with the domain the map points to, and widen only
when a dependency, invariant, schema or implementation path gives you a
concrete reason to. Avoid rereading what you have already established this
session, but re-read when you need a detail exactly rather than from memory.

## Sources of truth

| Source | Authority |
|---|---|
| Task or Issue | The requested outcome and the bounds of the work |
| `docs/architecture/` | System contracts and invariants |
| `schemas/` | Machine-readable contracts |
| `docs/development/implementation.md` | Where code belongs, and the allowed crate dependencies |
| Implementation | What the system currently does |
| Tests | Executable evidence of behaviour |
| `docs/reviews/` | History and analysis. Binding only once written into a contract |

Precedence among documents is defined in `docs/README.md`. Follow it rather
than re-deriving it. Two disagreements need a decision it does not make for
you:

- **Two canonical documents conflict.** `docs/README.md` calls this a defect.
  Report it; do not pick a side.
- **Implementation conflicts with canonical architecture.** This is not routine
  implementation work. Determine whether the task authorises changing the
  architecture, the implementation, or both. If it authorises neither, stop and
  report the discrepancy.

## Scope

Solve the requested problem completely, and change almost nothing else.

- Make the smallest change that fully satisfies the acceptance criteria. Small
  means narrow, not shallow: fix causes rather than symptoms, within the scope
  you were given.
- Do not refactor, reformat, rename or tidy anything you were not asked to
  change.
- Do not redesign an adjacent subsystem because a different design looks
  better.
- Do not add a dependency without a stated need.
- Do not broaden the acceptance criteria you were handed.

### Work discovered along the way

Ask one question: is fixing this required to satisfy the current acceptance
criteria?

- **Yes.** It is in scope. Do it, and say that you did.
- **No.** Leave it, and report it in your final summary so it can be triaged.

Do not open issues or pull requests for discovered work unless the task asks
for them.

## Changing architecture

Architecture documents are deliberate subsystem contracts, not descriptions of
whatever the code happens to do.

- **Implementation task.** The canonical contract constrains the
  implementation. Do not edit a contract to make an implementation choice
  legal.
- **Architecture task.** Change the contract deliberately, and keep its
  cross-references and the architecture map consistent, per the rules in
  `docs/README.md`.
- **Unexpected conflict.** Surface it, as described under Sources of truth.

## Finishing

Before reporting a change complete:

- each acceptance criterion is met, and you can say how;
- `./scripts/verify.sh` passes;
- every line of the diff traces to the task;
- work you found but did not do is reported.

A change reaches `main` through a pull request carrying the evidence for those
claims and an independent review. `docs/development/change-lifecycle.md` is that
contract, including the distinction between a logical reviewer and the GitHub
credential that records the review.

If you stop before the work is finished — the session ends, ownership changes,
or you are blocked — leave a handoff as that contract describes: on the pull
request when one exists, otherwise on the mission Issue. What you know and do
not record is lost with the session.

## About this file

`AGENTS.md` is the canonical contract for every coding agent. `CLAUDE.md`
imports it so Claude Code loads the same text; there is no second copy. Keep
repository knowledge in `docs/` and operating rules here.

Scoped `AGENTS.md` files in subdirectories are for subtrees that genuinely need
different commands, constraints or workflow. None exist yet, and one root
contract is easier to keep true.
