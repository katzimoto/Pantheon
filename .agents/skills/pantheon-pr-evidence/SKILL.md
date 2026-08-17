---
name: pantheon-pr-evidence
description: Prepare a Pantheon pull request body that carries durable Mission/Change/Evidence records instead of restating the mission or claiming success without proof. Use when opening or updating a pull request for an Engineering Mission in this repository.
metadata:
  pantheon-authority: procedural-guidance-only
---

# Pantheon PR evidence preparation

This operationalizes the pull request contract in
`docs/development/change-lifecycle.md`. Read that document for the actual
rules; this skill is the authoring checklist.

## When this applies

Opening a new pull request, or updating an existing one, for a change
driven by a Pantheon Engineering Mission Issue.

Does not apply to a rare change with no mission — state and justify that
directly per `docs/development/change-lifecycle.md` instead of forcing this
skill's shape onto it.

## Procedure

1. **Start from `.github/pull_request_template.md`.** It carries the three
   stable required headings (`## Mission`, `## Change`, `## Evidence`) in
   that order, plus an optional `## Impact and risk`. A PR authored without
   the template is still judged against the same contract.

2. **Mission**: state the relationship, not a restatement of the Issue.
   - `Closes #123` only when merging this PR completes that mission.
   - `Part of #123` (or another bare reference) for a supporting PR that
     does not complete the mission.
   - Exactly one PR closes a given mission. If this PR appears to close a
     second mission too, reconcile the mission model first
     (`docs/development/change-lifecycle.md`, "One pull request, more than
     one mission") rather than adding a second closing keyword.

3. **Change**: say what changed and why this is the smallest coherent
   solution — the reasoning a reviewer would otherwise have to
   reconstruct from the diff alone. Do not copy the mission's acceptance
   criteria text here; that belongs in Evidence, mapped to proof.

4. **Evidence**: account for every acceptance criterion, named with a few
   stable words, each paired with the actual proof — not the criterion text
   repeated back. Match evidence to the altitude of the claim:

   | Claim | Appropriate evidence |
   |---|---|
   | Defect fixed | Reproduction before + regression after |
   | Runtime behaviour | Executed result at the same altitude as the claim |
   | Recovery/concurrency property | Relevant path and edge branch exercised or otherwise convincingly established |
   | Architecture/schema change | Affected canonical contracts reconciled, no conflicting authority |
   | Documentation correction | Often the diff plus the structural validator |

   Do not cite a passing unit suite as proof of an end-to-end claim, and do
   not repeat `checks pass` merely because GitHub already shows it — only
   name a check here when its result is itself proof of a specific
   criterion. Do not claim verification that `pantheon-change-verification`
   was not actually run to establish on the current tree.

5. **Impact and risk** (optional): meaningful architecture/schema/API/
   security/compatibility implications, risks, incomplete areas, and
   out-of-scope work discovered but deliberately not absorbed. Delete the
   heading rather than writing "None" when there is nothing meaningful.

6. **Before marking ready**: self-review the complete diff as a reviewer
   would (mission satisfaction, scope discipline, contract conflicts,
   evidence validity, accidental debug/scratch noise, anything worth
   surfacing). This is behavior, not an artifact to paste into the PR body.

## What this skill must not do

- It does not decide whether the mission is actually satisfied — that is a
  judgment the author makes via self-review and the independent reviewer
  confirms; this skill only shapes how the claim is recorded.
- It does not substitute for `pantheon-change-verification`; evidence
  claims must be backed by an actual verification run, not asserted because
  this skill was followed.
- It does not perform independent review — see `pantheon-independent-review`
  for that, which must not be the same principal/session as this one.

## Non-triggers

- Drafting the mission Issue itself (`pantheon-mission-planning` and
  `docs/development/missions.md` instead).
- A handoff comment for unfinished work — use the `## Handoff` shape in
  `docs/development/change-lifecycle.md` directly; a handoff is not a PR
  evidence record.
