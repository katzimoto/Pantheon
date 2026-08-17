<!--
Pantheon pull request. `docs/development/change-lifecycle.md` is the contract;
this file is the default way to author one, not the contract itself.

Write only what the repository does not already know. The mission Issue owns the
outcome, acceptance criteria and scope. Git owns commits and changed files.
GitHub/checks own generic workflow state. Copies here decay.
-->

## Mission

<!--
Use `Closes #123` only when merging this pull request completes that mission.
GitHub then creates the closing relationship and closes the Issue when the PR
merges to the default branch.

For a supporting PR, use a non-closing cross-reference such as `Part of #123`.
Do not manually link a supporting PR through GitHub's Development control,
because that is also a closing relationship on merge to the default branch.

Exactly one PR per mission is mission-closing, and one PR closes at most one
Engineering Mission. Reconcile overlapping missions before merge rather than
inventing a second manual completion path.

A rare change with no mission says so and explains why no Engineering Mission
was appropriate.
-->

## Change

<!--
What this change does, and why it is this shape rather than a smaller or larger
one. Enough that a reviewer can judge the diff against the mission without
reconstructing your reasoning first.
-->

## Evidence

<!--
Account for every acceptance criterion without copying it. Name each with a few
stable words and state the actual proof, for example:

  - expired reservations -> regression reproduces pre-fix failure and passes
  - ownership after restart -> integration run over both recovery branches
  - broken reference -> directly shown by diff; docs validator confirms

Use evidence at the altitude of the claim. `AGENTS.md` already requires standard
repository validation and GitHub shows generic check state, so do not repeat it
unless that check's result is itself proof of a mission criterion.
-->

## Impact and risk

<!--
Optional. Meaningful architecture/schema/API/security/compatibility impact,
risks, incomplete areas, and work discovered but deliberately left out of
scope.

Delete this heading when there is nothing meaningful to say. Do not write
"None".
-->
