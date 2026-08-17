---
name: pantheon-mission-planning
description: Establish a Pantheon Engineering Mission's outcome and find the smallest correct change before editing anything. Use when starting work on a GitHub Issue in this repository, before creating or modifying any file, or when the right architecture/implementation entry point is not yet known.
metadata:
  pantheon-authority: procedural-guidance-only
---

# Pantheon mission planning

This skill operationalizes `AGENTS.md`'s "Start here" sequence and
`docs/development/missions.md`. It does not restate either document; read
them for the actual contract. This skill is the checklist that gets you
through them in the right order without missing a step.

## When this applies

Use this before making any repository change driven by a mission Issue:
at the start of a session working an Issue, before the first Read/Edit/Write
of a source or documentation file, and any time you are unsure which
crate or architecture domain a change belongs to.

Do not use this for a change with no mission (rare; `AGENTS.md` and
`docs/development/change-lifecycle.md` describe when that is legitimate) or
for pure exploration/question-answering that will not produce a change.

## Procedure

1. **Read the mission Issue completely.** Identify the required fields:
   Outcome (what must become true) and Acceptance criteria (what a reviewer
   checks). Treat Context and Scope and constraints as guidance, not as
   additional requirements to satisfy beyond the outcome. If the Issue names
   Architecture entry points, note them — read those first once you reach
   step 4, not before.

2. **Do not broaden the mission.** If related work becomes visible while
   reading the Issue or the code, that is not yet in scope. `AGENTS.md`
   settles what to do with it: fix it only if the current acceptance
   criteria require it; otherwise leave it and report it in your final
   summary.

3. **Read `docs/README.md` if you do not already have the navigation model
   for this session**, then `docs/architecture/overview.md` if you need the
   system model. Skip both if you already have them from earlier in the
   session — do not re-read what you have already established.

4. **Use `docs/architecture/README.md` to find the domain.** It carries
   reading recipes for common kinds of change. Follow the matching recipe
   instead of reading the architecture tree freely. If the Issue named
   entry points, start there and widen only when a dependency, invariant,
   schema, or implementation path gives a concrete reason to.

5. **For a code change, read `docs/development/implementation.md` before
   touching `crates/`.** It states which crate owns the concern and what
   that crate may depend on. Architecture domains are not one-for-one Rust
   crates — the domain from step 4 does not tell you where the code goes.
   Placing code in the wrong crate, or adding a dependency edge the
   allowlist does not permit, fails `scripts/check-crate-deps.sh` inside
   `./scripts/verify.sh`; get the placement right before writing code
   rather than discovering the failure afterward.

6. **Decide the smallest correct change before editing anything.** Smallest
   means narrow, not shallow: fix the actual cause within the mission's
   scope. Do not refactor, rename, or "improve" anything the mission did
   not ask for.

7. **If two canonical documents conflict, or implementation conflicts with
   canonical architecture and the mission does not authorize changing
   either, stop and report the discrepancy** rather than picking a side or
   quietly implementing around it.

## What this skill must not do

- It does not decide the mission's outcome or acceptance criteria — the
  Issue does.
- It does not restate architecture, implementation, or crate-dependency
  rules — it points at `docs/architecture/README.md` and
  `docs/development/implementation.md` and stops there. Read the current
  documents; do not treat this skill's summary of them as authoritative.
- It does not grant permission to open follow-up Issues or expand scope;
  `docs/development/missions.md` reserves that to an explicit grant in the
  mission itself or a human decision.

## Non-triggers

- A change with no driving mission Issue and an explicit, stated reason —
  judge that against `docs/development/change-lifecycle.md` directly.
- Answering a question about the codebase with no intent to change it.
- Continuing work already scoped earlier in the same session where nothing
  about the mission or the relevant domain has changed.
