# Change Lifecycle

## Status

**Canonical for how a candidate change is proven, reviewed, continued and
landed.** This document covers four related contracts — the pull request, the
review, the handoff, and merge and mission closure. It has no authority over
what a mission is, which is `docs/development/missions.md`, none over what
Pantheon is, which is `docs/architecture/`, and none over how an agent operates,
which is `AGENTS.md`.

As with a mission, the contract is the semantics below and not any particular
way of recording them:

```text
change lifecycle semantics
  -> a GitHub pull request is the candidate change and its evidence
    -> the Markdown template, the REST and GraphQL APIs, gh, and any future
       Pantheon adapter are ways of producing that record
```

`.github/pull_request_template.md` is the default interface for authoring one
and nothing more. A pull request opened through the API — which does not apply
the template at all — is judged against this document either way.

## The lifecycle

```text
mission -> draft pull request -> self-review -> ready -> independent review
        -> revision -> merge -> mission closure

and, when unfinished work changes hands:

interrupted work -> handoff -> successor reverifies current state -> continues
```

Every state on the top line is one GitHub already holds, except self-review,
which is a behaviour and leaves no state anywhere. Nothing here adds a second
representation of any of them.

## The pull request

A pull request is the durable candidate change and the evidence that it
satisfies a mission. That is the whole of its job.

It is not a second statement of the mission, not an architecture document, not a
session transcript, not an implementation diary, not a copy of the checks, and
not a handoff. Each of those is owned elsewhere, and a copy here is a second
authority that decays the moment the original moves.

The division is the one `docs/development/missions.md` already sets out. The
Issue is a prediction of what must become true; the pull request is the result,
plus what the work discovered along the way.

### Mission linkage

Every mission-related pull request states its relationship to its mission, in
GitHub's own linkage rather than in prose:

| Relationship | How it is written | On merge |
|---|---|---|
| Closes the mission | `Closes #123` | GitHub closes the Issue |
| Contributes to it | `Part of #123`, or any bare reference | The mission stays open |

A closing keyword is a claim that merging this change completes the mission. It
belongs on exactly one pull request per mission, and only when that claim is
true. Every other pull request for the mission references it without a keyword,
which still records the association in the Issue's timeline — the mission
therefore shows every pull request that touched it, in order, without anything
having to maintain a list.

A pull request that was expected to close its mission and turns out not to must
have the keyword removed before it merges. The body is editable to the last
moment; a stale keyword closes a mission that is not finished, and reopening it
loses nothing except the accuracy of the record.

Closing keywords are interpreted only when a pull request targets the
repository's default branch; on any other base GitHub ignores them entirely.
That makes a keyword inert on a stacked pull request *while it is stacked*, and
live the moment the layer below merges and GitHub rebases it onto the trunk. Do
not use the base branch to suppress a keyword. Omit the keyword.

### One mission, more than one pull request

One mission landing as one focused pull request is the heuristic
`docs/development/missions.md` already states, and for the same reason: review
quality falls away as a change grows, so a change that has grown past what one
review can hold is better split than reviewed badly. It is not a rule, and no
line count makes it one. Splitting a coherent change to hit a number produces
pieces that only make sense together, which is worse than one larger change.

When a mission does need more than one pull request, the supporting ones
reference it and the last one closes it. Nothing tracks the sequence in prose;
the Issue timeline is the sequence.

A pull request closes at most one mission. Two missions closed by one merge
cannot be accepted separately, reverted separately, or given separate evidence,
and a reviewer who judges one satisfied and the other not has no move that
GitHub can express. If a change genuinely satisfies a second mission, reference
that mission and leave closing it to a human — or accept that the two were one
mission, and say so.

### The body

Three sections carry the change. A fourth exists when there is something to put
in it.

**Mission** — the linkage above. A change with no mission says so and says why.

**Change** — what the change does, and why it is this shape rather than a
smaller or larger one. The diff shows what moved; this says what it means, and
gives a reviewer the reasoning they would otherwise have to reconstruct before
they could disagree with it.

**Evidence** — how the acceptance criteria were proven. Below.

**Impact and risk** — optional. What the change binds beyond its own diff: a
canonical contract, a schema, a compatibility commitment. What it risks or
leaves incomplete. And work found but deliberately not done, which `AGENTS.md`
requires be reported and which has nowhere more durable to go. When there is
none of this, the heading is deleted rather than answered with "None"; a section
that is mostly filled with nothing teaches readers to skip it, and they will
still be skipping it on the day it matters.

The headings are stable, in that order, because agents and future Pantheon
adapters read them. That is as much machine-readability as this needs: no
frontmatter, no schema, no field syntax, because there is no consumer today that
plain Markdown under predictable headings would not serve.

### Evidence

The mission says how success will be judged. The pull request says how it was
judged, which is not the same claim and is the one a reviewer can check.

Evidence covers each acceptance criterion that the diff does not already settle,
one line each, naming the criterion by a few words of its own text. Do not copy
the criterion — the Issue has it, and a copy is a second version to keep true.
Do not number them either: a number is a position in a list, and editing the
Issue moves it.

Evidence is proportionate to the claim, and the form follows what is being
claimed:

| The change claims | Evidence that supports it |
|---|---|
| A defect is fixed | The failure reproduced before, and a regression that fails without the fix |
| Runtime behaviour | A result that was executed, at the level the claim is made — end-to-end claims are not proven by unit tests |
| A recovery or concurrency property | The path exercised, including the branch that was previously undefined |
| An architecture or schema change | The affected canonical contracts reconciled, and no conflicting authority left |
| A documentation correction | Often the structural validator and the diff itself |

The failure this guards against is a claim proven at the wrong altitude:
offering a passing unit suite for an end-to-end claim, or
`scripts/check-docs-links.sh` for a claim about behaviour. The check that ran is
not evidence for a claim it does not reach.

`AGENTS.md` already requires the repository's standard validation on every
change, and the checks report themselves on the pull request, so "the checks
pass" is not evidence of anything and is not written here. Name a check when its
result *is* the proof: when the mission was about the thing that check verifies,
one line saying so is the whole of the evidence, and when a new check is the
change, that it fails on the input it was written to catch is a fact CI cannot
report about itself.

### What the pull request does not carry

Not because these are unimportant, but because something else already owns each
one and will stay right when the pull request does not:

| Not in the body | Already owned by |
|---|---|
| Acceptance criteria, context, scope | The mission Issue |
| Changed files, commits, diffstat | Git |
| Check results, mergeability | The checks and the merge box |
| Draft state, review state, unresolved threads | GitHub |
| Continuation state for unfinished work | A handoff comment |

## Draft and ready

Draft and ready-for-review are GitHub's states and Pantheon adds nothing to
them. A draft cannot be merged and does not request code owners; marking it
ready does. There is no `wip`, `agent-working`, `implementation-done` or
`ready-for-review` label, because each would be a second copy of a state the
platform already holds and shows.

Open the pull request as early as it is useful. A draft opened at the start of
the work is a place for evidence to accumulate and a place a handoff can attach
to, and it costs nothing. Waiting until the end so the first push looks finished
throws away the only durable record of work that is interrupted before it is.

A pull request is ready for review when the change is coherent, the author
believes the acceptance criteria are satisfied, mission-specific evidence is in
the body, the repository's validation passes or the body explains why an
exception is correct, the author has read the whole diff, and no known blocking
problem is left in it.

## Self-review

Before marking a pull request ready, its author reads the complete diff as a
reviewer would. This is a behaviour, not a form: nothing about it is recorded in
the body, there is no checklist to tick, and "self-reviewed" is not evidence of
anything.

It is looking for the things an author is uniquely placed to catch and uniquely
likely to miss — whether the change satisfies the mission rather than merely
looking finished; whether every line traces to the task, as `AGENTS.md`
requires; whether anything unintended rode along, such as debug output, a
scratch file, or reformatting of a line that had no reason to change; whether
the evidence in the body is what it says it is; and whether anything in the diff
quietly contradicts a canonical contract.

Self-review is not review. An author checking their own work is re-running the
reasoning that produced it, and reasoning that was wrong the first time is
usually wrong the second. It raises the floor on what reaches a reviewer. It
does not substitute for one, and an author approving their own pull request is
not an approval.

## Independent review

A reviewer is deciding one thing: whether this candidate change should become
authoritative. Not whether it followed a ritual, and not whether it is the
change the reviewer would have written.

Independent means the review is not the author's reasoning run again — a
different reviewer, working from context the authoring session did not shape.
Pantheon does not require a different model: there is no evidence that model
diversity is what makes review work, and a rule invented ahead of that evidence
would be enforced long after it stopped being believed. A fresh context is what
independence actually rests on.

The reviewer reads the whole conceptual change against its mission, not only the
lines that moved, widening into contracts, schemas, tests and surrounding
implementation where evidence says they matter. Selective retrieval still
applies: `AGENTS.md` asks for the domain the change touches and only what a
concrete reason widens it to, and that binds a reviewer exactly as it binds an
author.

### Priority

Findings are ranked by what they threaten, so that what matters is not filed
behind what does not:

1. The change does not satisfy its mission, or satisfies a different one.
2. Correctness, data loss, or a security or safety failure.
3. A canonical contract or invariant is violated.
4. The evidence does not support what is claimed.
5. Recovery, concurrency or compatibility risk.
6. Maintainability that will materially raise the risk of future change.
7. Everything else.

A reviewer who has found something at 1 through 4 says so before anything else,
and may reasonably say nothing else at all.

### Findings

Three kinds, carried by GitHub's own review states. No labels, no severity
prefixes, no second status system:

| Finding | Submitted as | Means |
|---|---|---|
| Blocking | Request changes | Must be resolved before merge |
| Non-blocking | Comment | Worth doing; the author may decline with a reason |
| Question | Comment | The reviewer cannot judge until this is answered |

Approve means the change should become authoritative, and an approval carrying
non-blocking findings is normal.

A question is not a soft blocking finding. If the answer could change the
verdict, the review says that plainly, so the author knows whether they are
answering or fixing.

### Signal discipline

The failure mode of an agent reviewer is not missing a defect. It is producing
so much that the defects it did find are read past. Where AI review has been
measured, the large majority of comments are noise, and sustained noise does not
make reviewers more careful — it makes them stop reading, including the findings
that mattered.

So: say the fewest things that change the outcome. Three real findings are a
better review than twenty findings containing the same three.

Do not spend a finding on:

- what a check already reports, including lint and formatting output;
- style, naming or wording that no check enforces and that does not impede
  understanding — a preference stated as a correctness problem is the most
  expensive kind of noise, because it cannot be dismissed cheaply;
- a failure mode with no plausible path to it;
- the same problem in several places, when one thread would carry it;
- cleanup of code the change did not touch — scope discipline binds reviewers
  too;
- generated files, absent a specific reason;
- demonstrating that the review happened. A review that finds nothing and
  approves is a complete review.

Every finding says what is wrong and how it is known — the contract it
contradicts, the input that breaks it, the line that shows it. A finding that
cannot be traced cannot be acted on.

When uncertain, ask precisely, or say what is uncertain and why. Do not
manufacture a blocking finding to be safe. A blocking finding that turns out to
be nothing costs more than the doubt it was meant to cover, because it teaches
the next author that blocking findings can be waited out.

### The loop

```text
review -> findings -> revision -> revalidation -> re-review -> approval
```

A first review is not expected to be the last. Re-review is scoped to what
changed and to what the change could have disturbed, not a fresh pass over the
whole diff; a mechanical correction whose result a check already covers does not
need one at all.

Blocking findings are resolved rather than outlasted. An author who disagrees
says why, on the thread, and a disagreement that stays unresolved is a decision
for a human, not something either party settles by merging or by waiting.

## Handoff

A handoff exists for one situation: unfinished work has to be continued by
another agent, person or context.

That is ownership changing, a session being discarded, an agent blocked or
unable to continue, or review fixes being delegated. It is not the end of a
session that finished what it started, and it is not a status update. A finished
pull request needs no handoff — its continuation state is empty, and its
evidence is already in the body. Handoffs written for every session end are
noise of the same kind as a noisy review, and cost the same way.

The gain is measurable, and its shape decides what a handoff is for. Where
takeovers have been measured, a successor handed structured notes resumed with
substantially fewer steps and substantially fewer tokens than one given the
repository alone — but the one given the repository alone still finished the
work. A handoff buys rediscovery, and only rediscovery. That is what it should
be written to hold, and why nothing depends on it being right.

### Where

A handoff is a comment on the pull request.

It stays attached to the artifact the successor will actually work on; it is
visible to anyone who opens the pull request; it is one API call to fetch; it
leaves the body alone, so durable review state and continuation state do not
overwrite each other; repeated handoffs stack in chronological order rather than
replacing one another; and each one carries the timestamp that lets a successor
judge how much has happened since.

If no pull request exists yet, the mission Issue takes it, and the pull request
picks the thread up when it opens. Pantheon does not store handoffs itself.
There is no store, and inventing one to hold something GitHub already holds
would create the second authority this contract exists to avoid.

### What it holds

Only what is expensive for a successor to reconstruct:

- where the work stands against the mission — what is left, not what is done;
- why the current approach was chosen, and what was tried and abandoned, with
  the reason;
- what failed, and how it actually failed;
- what is unverified or still uncertain, including hypotheses that were
  disproven, which are what a successor would otherwise pay to disprove again;
- anything in the branch or working tree that is partial or unsafe — a
  half-applied change, a test that passes for the wrong reason, a decision
  encoded in the diff but not yet argued anywhere;
- the single next action the predecessor would take.

Not: changed files, commits, diffs, test names, check results, review comments,
or a restatement of the mission or the pull request. Git and GitHub hold every
one of those, hold them correctly, and will still be right about them when the
handoff is not.

### Trust

**A handoff is historical evidence. It is never authority.**

Before acting on one, a successor establishes current state from the repository
and from GitHub: the branch and its diff against the base, the pull request and
its review threads, the state of the checks, and the mission as it now reads.
Where the handoff and current state disagree, current state wins, and the
handoff is stale from that point onward — including the parts that still look
right.

Read a handoff as what the previous owner believed when they wrote it, never as
how things are. In particular, do not carry out a recommended next action that
cannot be re-derived from current evidence: the recommendation was made against
a repository state that may no longer exist, and it is the part of a handoff
that goes wrong most quietly.

## Merge and mission closure

Four states, none of which is the one before it:

| State | Means |
|---|---|
| Ready | The author asserts the change is reviewable |
| Approved | A reviewer judges it should become authoritative |
| Mergeable | GitHub finds nothing blocking the merge |
| Complete | The mission is satisfied |

A mission is complete when the pull request carrying its closing keyword merges
into the default branch, having satisfied the acceptance criteria on evidence a
reviewer accepted, under independent review, with no blocking finding
outstanding. GitHub closes the Issue at that moment, from the linkage, without
anything else being updated.

A supporting pull request merging changes nothing about the mission, which is
true by construction: it carries no keyword, so there is no state for anyone to
get wrong.

Closure records that the mission was believed satisfied, and is not proof that
it was. A mission found unsatisfied after its pull request merged is reopened,
or replaced by a new one — the repository, not the Issue's state, is what says
whether the outcome holds.

### What stays native

Draft and ready, review decisions, unresolved threads, check results,
mergeability, the linked Issue and its closure are all GitHub's, and Pantheon
mirrors none of them. There is no `agent-working`, `agent-review`,
`ready-to-merge`, `validation-passed` or `mission-complete` label. A label that
restates a state the platform already holds is wrong as soon as the two diverge,
and nothing keeps them together.

If some future automation genuinely needs a state GitHub does not represent,
that is the moment to add it, and the justification belongs here.

### What is deferred

The merge method is not chosen. Merge commit, squash and rebase differ in the
history they leave, and the history worth having depends on the commits, the
bisect and revert workflows, and the stacking patterns of a codebase that does
not exist yet. Choosing now would be picking a history model for code nobody has
written. Mission linkage is unaffected either way: the link lives on the pull
request, not in a commit message, so all three methods close the Issue.

Enforcement is likewise deferred. Required reviews, required checks, required
conversation resolution, signed commits, linear history, merge queue,
auto-merge, CODEOWNERS and reviewer automation are all deliberately not
configured. The semantics above are the contract; a repository rule is how a
contract is enforced against people who have not read it, and Pantheon today has
one check and no implementation code, so most of those rules would gate on
nothing. Each has its own trigger to reconsider:

| Add | When |
|---|---|
| Required checks | There is a test suite whose result should gate merge |
| Required reviews, CODEOWNERS | Merges happen without a human in the loop, or ownership stops being obvious |
| Required conversation resolution | Review threads are observed being lost rather than answered |
| Merge queue, linear history | Concurrent merges start conflicting, or bisect becomes something anyone does |

Nothing in this section is a plan to add them. It is the evidence that would
justify revisiting each, recorded so the question is answered from the
repository rather than from habit.

## What is checked

`scripts/check-docs-links.sh` verifies that `.github/pull_request_template.md`
exists and still carries its stable headings, since those are the part of this
contract that other tools read.

Nothing checks the content of a pull request body. A check that graded evidence
or judged review quality would be enforcing a judgement it cannot make, and the
cost of it being wrong is paid by every change that follows. Review is what
holds this contract, and review is a judgement.
