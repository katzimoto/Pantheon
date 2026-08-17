# Engineering Missions

## Status

**Canonical for how work is specified.** This document defines what a Pantheon
Engineering Mission is and how missions relate to each other. It has no
authority over what Pantheon *is* — that lives in `docs/architecture/` — and
none over how an agent works, which lives in `AGENTS.md`.

The contract is the semantics below, not any particular way of recording them:

```text
Engineering Mission semantics
  -> a GitHub Issue is the mission record
    -> the Issue Form, the REST API, gh, and any future Pantheon adapter
       are ways of creating that record
```

`.github/ISSUE_TEMPLATE/engineering-mission.yml` is the default interface for a
human authoring one, and nothing more. A mission opened through the API, a CLI
or a tool is no less a mission for having skipped the form; it is judged against
this document either way.

## What a mission is

A mission is one bounded piece of engineering work, recorded as a GitHub Issue.
It answers exactly one question:

> What must become true?

The repository answers the other one. Which files change, which components are
affected, which internal APIs move, which tests are appropriate — an agent
determines these from architecture, schemas, implementation and tests, after
inspection, more accurately than an author can predict in advance. A mission
that dictates them narrows the work to the author's guess and hides the
reasoning that would have caught a wrong guess.

Each layer owns one thing and does not repeat the others:

| Where | Owns |
|---|---|
| `README.md` | What the project is |
| `AGENTS.md` | How an agent operates, validates and finishes |
| `docs/architecture/` | The contracts that must remain true |
| A mission Issue | The outcome this piece of work must achieve |
| Implementation, schemas, tests | What the system currently does |
| The pull request | The change, and evidence it satisfied the mission |

## The fields

Six fields, three required. The form explains how to fill each one; this
section says why each exists and what it must not absorb.

**Outcome** — required. The observable state the mission must produce. This is
the field that makes an Issue executable: it is the mission's stopping
condition, and an agent with no other context should be able to tell from it
whether it is finished. It states a result, so "implement reservation
persistence" and "add `restore_reservations()`" are both the wrong altitude.

**Context** — optional. Why the mission exists: the problem today, why it
matters, what triggered it, and for a defect, the observed failure and its
reproduction. It is not a place to re-explain the system; canonical
architecture already does that, and a copy here becomes a second, decaying
authority. A mission whose outcome and acceptance criteria are self-evident
does not need it, and inventing context to fill the field makes the Issue
worse.

**Acceptance criteria** — required. The executable meaning of done: what a
reviewer checks. Criteria are observable and independently understandable, and
they name behaviour rather than artifacts — "recovery cannot produce two
owners for the same capacity", not "add a table". An artifact belongs in a
criterion only when the artifact itself is the contract, because architecture
or a compatibility commitment requires it.

**Scope and constraints** — optional. The boundaries: what is deliberately
excluded, which invariants must survive, which areas are protected. It is also
where a mission grants authority it does not otherwise have, such as permission
to open follow-up Issues. It must contain only real constraints; a mission with
no meaningful boundary leaves it empty rather than writing "none".

**Architecture entry points** — optional. One or two documents that are worth
opening first, for missions where the right domain is not obvious or a second
domain is involved. This is a pointer into `docs/architecture/README.md`, not a
substitute for it, and never a transitive dependency list — the map is
maintained and checked, and an Issue body is neither.

**Evidence** — required. What will demonstrate that the acceptance criteria
hold. Acceptance criteria say what must be true; evidence says how a reviewer
will know, and keeping them separate is what stops "done" from meaning "the
author asserts it". The evidence that matters is mission-specific: a regression
test that reproduces the original failure, coverage of a recovery path, or for
an architecture mission, that the affected canonical contracts are reconciled
and no conflicting authority remains. `AGENTS.md` already requires the
repository's standard validation on every change, so restating it adds nothing.

Nothing else is a field. Implementation plans, file lists, function names,
branch names, estimates, model choice and agent instructions are all either
owned by another layer or discoverable from the repository, and each one is
wrong the moment the repository moves.

## Labels

Labels are orthogonal query/release metadata, not another work-state model.
GitHub-native state remains authoritative for completion, Version membership,
dependencies, hierarchy, assignees, and review/check status. Pantheon does not
use `status:*`, `priority:*`, `version:*`, `kind:*`, blocker, parent/epic, or
readiness labels to mirror those facts.

Every Engineering Mission has one primary `area:*` label. A mission whose
observable outcome genuinely spans two domains may carry one additional
secondary area; more than two is a sign that the mission is too broad or the
labels are being used as a dependency list. Area labels are broad discovery
facets, not a replacement for Architecture entry points:

```text
area:goals-planning
area:tasks
area:scheduling
area:execution
area:agents-context
area:evaluation-acceptance
area:artifacts-workspaces
area:security
area:persistence-recovery
area:operations
area:repository
```

`area:repository` is for repository tooling, CI, documentation process, and
development infrastructure rather than a Pantheon runtime subsystem.

A Mission assigned to a Version Milestone additionally carries exactly one
`changelog:*` label under `docs/development/versions.md`. Changelog
classification belongs to the Mission outcome and is not copied onto its pull
requests. `good first issue` and `help wanted` are optional contributor-discovery
signals; they do not change mission semantics.

## Relationships

Hierarchy and dependency are represented in GitHub's issue graph, never in
prose:

- **Sub-issue and parent** express decomposition: this mission is part of that
  larger outcome.
- **Blocked by and blocking** express execution order: this mission cannot
  start, or cannot land, until that one does.

These are different relations. A sub-issue does not necessarily block its
parent, and a blocking mission is usually not a child. Modelling both as a
checklist in the body loses the distinction.

A textual `Blocked by: #12` is a second source of truth that nothing updates
when the graph changes, and it is invisible to every tool that reads the graph.
Use the native relationships and leave the body to the mission itself.

## Decomposition

Most work is one mission. Decompose when a single outcome genuinely contains
work that can be completed and reviewed independently — not to enumerate the
steps of one change.

Each child is a mission in its own right, with its own outcome, acceptance
criteria, boundaries and evidence, and each one leaves Pantheon meaningfully
further along on its own. "Create the table", "add the method", "write the
test" fail that test: they are one change cut into three, and none of them is
independently reviewable.

One mission landing as one focused pull request is a good heuristic and not a
rule. Splitting a coherent change to satisfy it produces missions whose
acceptance criteria only make sense together, which is worse than a slightly
larger mission.

## Readiness

A mission is ready for an agent when its outcome is observable, its acceptance
criteria define success, its boundaries are meaningful, its known hard
constraints are stated, any entry point that is not discoverable from the
architecture map is named, its blockers are represented in the graph, and its
evidence is enough for a reviewer to judge the result.

That is a judgement, not a checklist to copy into the Issue. There is no
readiness label, score or bot.

## Completion

A mission is complete when its acceptance criteria are satisfied, its evidence
supports them, the validation `AGENTS.md` requires passes, out-of-scope
discoveries are reported rather than absorbed, and the change is reviewed and
merged.

The Issue owns what success means. `AGENTS.md` owns how the agent gets there.
Neither redefines the other.

## Work discovered during a mission

`AGENTS.md` already settles this: work that is not required to satisfy the
current acceptance criteria is reported, not done, and no new Issue is opened
for it unless the task asked for one. A mission does not expand because
adjacent work became visible.

A mission may grant that authority explicitly under scope and constraints when
decomposition is part of the outcome. Absent that grant, deciding whether
discovered work becomes another mission is a human decision.

## Why there is one mission form

Defects, features, refactors, architecture changes and documentation
corrections are all bounded work with an outcome, a definition of done and a
boundary. They differ in what gets written into those fields, not in which
fields they need. A bug's reproduction is context; its expected behaviour is
the outcome; its regression test is evidence. An architecture mission states
the design problem to resolve and what must hold once it is; the resolution
itself belongs in a canonical contract under `docs/architecture/`, and the
Issue remains only the record of the mission.

Separate templates per work type would duplicate most of their content,
drift apart, and force authors to classify work before describing it. If a
category ever needs materially different fields, that is the evidence for
adding a form — not the expectation that it might. A route for something that
is not a mission at all, such as support or vulnerability reporting, is a
separate question this contract does not answer.
