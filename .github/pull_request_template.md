<!--
Pantheon pull request. `docs/development/change-lifecycle.md` is the contract;
this file is the default way to author one, not the contract itself.

Write only what the repository does not already know. The mission Issue owns the
outcome, the acceptance criteria and the scope. Git owns the commits and the
changed files. The checks own their own results. A copy of any of them here is a
second version that decays.
-->

## Mission

<!--
`Closes #123` only when merging this pull request completes the mission —
GitHub then closes the Issue on merge into the default branch. Otherwise
reference the mission without a keyword, for example `Part of #123`, so it
stays open. Exactly one pull request per mission carries the keyword, and a
pull request closes at most one mission.

A change with no mission says so, and says why it did not need one.
-->

## Change

<!--
What this change does, and why it is this shape rather than a smaller or a
larger one. Enough that a reviewer can judge the diff against the mission
without reconstructing your reasoning first.
-->

## Evidence

<!--
How the acceptance criteria that the diff does not already settle were actually
proven. One line each, naming the criterion by a few words of its own text:

  - expired reservations -> regression test reproducing the pre-fix failure
  - ownership after restart -> integration run over both recovery branches

Proportionate to the claim. A claim about behaviour needs a result that was
executed; an architecture change needs the reconciliation showing no
conflicting authority is left. `AGENTS.md` already requires the repository's
standard validation on every change and the checks report it themselves, so do
not restate it here.
-->

## Impact and risk

<!--
Optional. What this change binds beyond its own diff — a canonical contract, a
schema, a compatibility commitment — what it risks or leaves incomplete, and
work you found but deliberately did not do, which `AGENTS.md` requires you to
report.

Delete this heading when there is nothing to say. Do not write "None".
-->
