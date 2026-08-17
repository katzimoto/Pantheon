# Pantheon Documentation

This is the entry point for Pantheon documentation. Start here, then descend
into exactly the documents your task needs.

## What exists

| Area | Path | Authority |
|---|---|---|
| Architecture overview | `docs/architecture/overview.md` | **Canonical.** System model, top-level flow, cross-cutting invariants. |
| Subsystem map | `docs/architecture/README.md` | Navigation only. Routes you to the right contracts. |
| Detailed architecture contracts | `docs/architecture/<domain>/` | **Canonical.** One document per subsystem contract. |
| Reviews and review process | `docs/reviews/` | **Not canonical.** Historical analysis and review ledgers. |
| Schemas | `schemas/` | **Canonical** for the artifacts they describe. |

## Where to start

1. Read `docs/architecture/overview.md` once. It is short and establishes
   Pantheon's system model, resource hierarchy and cross-cutting invariants.
2. Open `docs/architecture/README.md` and find the domain your task touches.
3. Read the two to five documents that domain names for your kind of change.
   Do not read the whole architecture tree; it is large and mostly irrelevant
   to any single task.
4. Check `schemas/` and, once code exists, the implementation and tests.

## How architecture documentation is organized

Detailed contracts live in one directory per architectural domain under
`docs/architecture/`. Domains follow Pantheon's own control-plane structure
(goals → tasks → scheduling → execution → evaluation/acceptance) plus the
cross-cutting concerns (security, persistence/recovery, operations,
artifacts/workspaces, agents/context).

The hierarchy is deliberately one level deep. A document's domain is its
directory; nothing else encodes it.

Each contract begins with a `## Status` section stating what the document is
and how far its authority extends (for example, which parts are deferred past
v1). That line is the document's authority statement — read it first.

## Source of truth and precedence

1. A subsystem contract under `docs/architecture/<domain>/` is authoritative
   for the subsystem it names.
2. `docs/architecture/overview.md` is authoritative for the system model and
   cross-cutting invariants, but it does not override a detailed contract on
   that contract's own subject. Where older overview wording conflicts with a
   newer subsystem contract, the subsystem contract wins.
3. `docs/reviews/` is never authoritative. A review may diagnose a problem
   correctly and still propose a resolution that was not adopted. If a review
   conflicts with canonical architecture, canonical architecture wins.
4. Where two canonical documents appear to conflict, treat it as a defect.
   Report it rather than choosing a side.

## How reviews differ from architecture

`docs/architecture/` states what Pantheon *is*. `docs/reviews/` records what
reviewers *found* and how findings were dispositioned. Review conclusions
become architecture only by being written into a canonical contract. Until
then they are not implementation requirements. See `docs/reviews/README.md`.

## Adding new documentation

| You are writing | It belongs in |
|---|---|
| A new canonical contract for a subsystem | `docs/architecture/<existing-domain>/` |
| A contract for a subsystem no domain covers | A new `docs/architecture/<domain>/`, added to the subsystem map |
| A system-wide invariant or model change | `docs/architecture/overview.md` |
| A review, audit or external analysis | `docs/reviews/` |
| Superseded material worth keeping | `docs/reviews/`, with a `## Status` line saying what superseded it |

Rules for any new document:

- Give it a `## Status` section saying what it is and how far it is binding.
- Give it a filename that describes its subject, so its relevance can be
  judged without opening it.
- Reference other documents by repository-root-relative path in inline code,
  for example `docs/architecture/tasks/task-lifecycle.md`. Do not use bare
  filenames; they break when files move.
- Add it to the domain listing in `docs/architecture/README.md`. That map is
  the only place the full inventory is maintained.
- Do not restate another document's contract. Link to it.

Run `scripts/check-docs-links.sh` after moving or adding documents. It verifies
that every referenced path exists, and that the architecture map lists every
canonical contract on disk exactly once — so a new contract that is never added
to the map fails the check rather than becoming invisible to navigation.
