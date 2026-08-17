# Pantheon Agent Contract

How to work in this repository. What Pantheon *is* lives in `docs/`; this file
does not repeat it. See `README.md` for the project summary.

## Repository state

Pantheon is in the design phase. The repository holds canonical architecture
documents, JSON Schemas, and the script that validates them. There is no
implementation code, build system, package manager or test framework yet. Do
not go looking for a source tree, and do not introduce a language or toolchain
unless the task says to.

## Validation

Run `scripts/check-docs-links.sh` from the repository root after your final
change, before reporting the task complete or handing it off. CI runs the same
script on every pull request.

It verifies that every documentation reference resolves, that the architecture
map lists every canonical contract exactly once, and that `CLAUDE.md` still
imports this contract. It is the only check that exists today. As implementation
lands, the canonical commands for build, test and lint belong in this section.

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
6. Decide the smallest correct change before editing anything.

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
- `scripts/check-docs-links.sh` passes;
- every line of the diff traces to the task;
- work you found but did not do is reported.

A change reaches `main` through a pull request carrying the evidence for those
claims, reviewed by someone other than its author.
`docs/development/change-lifecycle.md` is that contract, and it also covers
taking over work someone else left unfinished.

If you stop before the work is finished — the session ends, ownership changes,
or you are blocked — leave a handoff on the pull request, as that contract
describes. It is the one part of the lifecycle that nothing else in the
repository will do for you. What you know and do not write down is lost with
the session.

## About this file

`AGENTS.md` is the canonical contract for every coding agent. `CLAUDE.md`
imports it so Claude Code loads the same text; there is no second copy. Keep
repository knowledge in `docs/` and operating rules here.

Scoped `AGENTS.md` files in subdirectories are for subtrees that genuinely need
different commands, constraints or workflow. None exist yet, and one root
contract is easier to keep true.
