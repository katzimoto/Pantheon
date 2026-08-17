# Change Lifecycle

## Status

**Canonical for how a candidate change is proven, reviewed, continued and
landed.** This document covers four related contracts: the pull request, review,
handoff, and merge/mission closure. It has no authority over what a mission is,
which is defined in `docs/development/missions.md`; over what Pantheon is, which
lives in `docs/architecture/`; or over how an agent operates generally, which
lives in `AGENTS.md`.

As with a mission, the contract is the semantics below and not any particular
way of recording them:

```text
change lifecycle semantics
  -> a GitHub pull request is the candidate change and its evidence
    -> the Markdown template, GitHub APIs, gh, and future Pantheon adapters
       are authoring/transport mechanisms
```

`.github/pull_request_template.md` is the default authoring interface and
nothing more. Pull requests created without the template are judged against the
same contract.

## Lifecycle

```text
mission -> draft pull request -> self-review -> ready -> independent review
        -> revision -> merge -> mission closure

unfinished work changing hands:
interrupted work -> handoff -> successor reverifies current state -> continues
```

Where GitHub has native state — Draft/Ready, review state, merge state, and Issue
closure — Pantheon uses it directly. Self-review and revision are behaviours,
not mirrored statuses.

## Pull request contract

A pull request is the durable candidate change plus the evidence that it
satisfies its mission. It is not a second statement of the mission, an
architecture document, a session transcript, an implementation diary, a copy
of CI state, or a handoff.

The Issue says what must become true. The pull request says what changed, what
the work discovered, and why the evidence is sufficient.

### Mission relationship

Every mission-related pull request states which mission it serves.

There are two deliberately different relationships:

| Relationship | Representation | Result on merge |
|---|---|---|
| Mission-closing | `Closes #123` | GitHub creates a closing link and closes the Issue when the PR merges to the default branch |
| Supporting | `Part of #123` or another bare Issue reference | GitHub records a cross-reference; the mission stays open |

A supporting reference is **not** GitHub's native linked-Issue relationship.
Do not manually link a supporting PR through GitHub's Development control,
because a manually linked PR is also a closing relationship when merged to the
default branch.

A closing keyword is a claim that merging this PR completes the mission. Use it
only when that claim is true. If it stops being true, remove the keyword before
merge.

GitHub interprets closing keywords only for PRs targeting the default branch.
Do not rely on a non-default base to make a closing keyword temporarily inert in
a stack; omit the keyword until the PR is genuinely mission-closing.

### One mission, more than one pull request

One mission landing as one focused PR is a strong heuristic, not an invariant.
Split when the result becomes too large to review coherently; do not split one
conceptual change merely to hit a size target.

When one mission needs several PRs, supporting PRs cross-reference the mission
without a closing relationship. Exactly one PR is mission-closing, and only
when its merge means the whole mission is satisfied.

### One pull request, more than one mission

A PR closes at most one Engineering Mission. This keeps mission -> evidence ->
merge -> closure unambiguous and independently reviewable.

If a PR appears to complete another mission, reconcile the mission model before
merge rather than inventing a second manual completion path:

- if the second mission is the same conceptual outcome, mark it duplicate or
  superseded and point to the primary mission;
- if it is independently meaningful, split the candidate change so each mission
  has its own reviewable completion path.

A PR may cross-reference other missions for context or discovered relationships,
but the cross-reference does not complete them.

### Pull request body

Three stable sections carry the durable record. A fourth is optional:

**Mission** — the mission-closing relationship or supporting cross-reference.
A rare change with no mission says so and explains why no Engineering Mission
was appropriate.

**Change** — what changed and why this is the smallest coherent solution. The
diff shows what moved; this gives the reasoning a reviewer would otherwise have
to reconstruct.

**Evidence** — how mission acceptance criteria were actually established.

**Impact and risk** — optional. Meaningful architecture/schema/API/security/
compatibility implications, risks, incomplete areas, and out-of-scope work that
was discovered but deliberately not absorbed. Delete the heading when there is
nothing meaningful to record; do not write `None`.

The stable headings exist so humans and future Pantheon adapters can retrieve
the durable parts reliably without adding frontmatter or another schema.

### Evidence

Every acceptance criterion is accounted for, but the PR does not copy the full
criterion text. Name each criterion with a few stable words and state the proof.
If the diff itself directly settles a criterion and no separate execution is
needed, say that concisely.

Examples:

```text
expired reservations -> regression reproduces the pre-fix failure and passes
ownership after restart -> integration run exercises valid and expired paths
document reference resolves -> directly demonstrated by diff; docs validator confirms
```

Evidence is proportionate to the claim:

| Claim | Appropriate evidence |
|---|---|
| Defect fixed | Reproduction before + regression after |
| Runtime behaviour | Executed result at the same altitude as the claim |
| Recovery/concurrency property | Relevant path and edge branch exercised or otherwise convincingly established |
| Architecture/schema change | Affected canonical contracts reconciled with no conflicting authority |
| Documentation correction | Often the diff plus the structural validator |

Do not use a passing unit suite as proof of an end-to-end claim. Do not repeat
`checks pass` merely because GitHub already displays it. A check belongs in the
Evidence section only when the check's result is itself proof of a mission
criterion.

### State not copied into the body

| Not copied | Owner |
|---|---|
| Outcome, context, scope, acceptance criteria | Mission Issue |
| Changed files, commits, diffstat | Git |
| Generic CI/check state and mergeability | GitHub/checks |
| Draft/Ready and native review state | GitHub |
| Unfinished continuation state | Handoff comment |

## Draft and ready

Draft and Ready-for-review are GitHub-native states. Pantheon does not mirror
them with labels.

Open a Draft PR whenever it becomes useful for collaboration, evidence, or
resumability; it does not need to wait until implementation is finished.

A PR is ready for independent review when:

- the candidate change is coherent;
- the author believes the mission acceptance criteria are satisfied;
- the PR body accounts for the evidence;
- repository validation passes, or a justified exception is explicit;
- the author has self-reviewed the complete diff;
- no known blocking defect remains.

## Self-review

Before marking a PR ready, the author reads the complete diff as a reviewer
would. This is behaviour, not a checkbox or evidence artifact.

Self-review checks at least:

- mission satisfaction rather than superficial task completion;
- scope discipline and unrelated changes;
- canonical architecture/schema conflicts;
- validity of the evidence claimed in the PR body;
- accidental debug/scratch/generated noise;
- risks or discovered work that need to be surfaced.

Self-review raises the quality floor. It never substitutes for independent
review.

## Independent review

A reviewer decides whether the candidate change should become authoritative.
The reviewer evaluates the conceptual change against its mission, relevant
contracts, implementation/tests, and evidence, widening context only when there
is a concrete reason.

### Review principal versus GitHub actor

Pantheon distinguishes the **logical review principal** from the **GitHub
credential used to record the review**.

A review is independent when it is performed by a principal other than the
authoring principal, from fresh context not inherited from the authoring
session. The reviewer may be another human, another agent, or another fresh
agent context. It need not use a different model merely for diversity.

Different principals may temporarily share one GitHub account or credential.
GitHub cannot record an approval from the PR author account, even when a truly
independent agent performed the review through that credential.

When the independent reviewer has a distinct GitHub actor, use GitHub's native
review states. When the reviewer shares the author's GitHub actor, record the
independent verdict as a top-level PR comment with the stable marker:

```markdown
## Independent review

Verdict: approve | request changes | comment

<high-signal findings or concise approval rationale>
```

This fallback records the logical review without pretending GitHub has a native
approval it cannot represent. Do not create separate bot accounts solely to
satisfy the representation.

### Review priority

Review in this order:

1. Mission mismatch or incomplete mission satisfaction.
2. Correctness, data loss, security, or safety failure.
3. Canonical contract/invariant violation.
4. Evidence that does not support the claim.
5. Recovery, concurrency, persistence, or compatibility risk.
6. Maintainability problems that materially increase future risk.
7. Non-blocking improvements.

A reviewer who finds a high-priority blocker may stop rather than bury it under
lower-value commentary.

### Findings and verdicts

With a distinct GitHub actor:

| Semantic result | GitHub representation | Meaning |
|---|---|---|
| Blocking | Request changes | Pantheon requires resolution before merge |
| Non-blocking | Comment | Worth considering; may be declined with reason |
| Question | Comment | More information is needed to judge |
| Accepted | Approve | Reviewer judges the change ready to become authoritative |

With a shared GitHub actor, use the `## Independent review` comment marker and
the same semantic verdicts.

`Request changes` is currently a **Pantheon semantic blocker**, not necessarily
a mechanical GitHub merge blocker. Until a ruleset/branch-protection rule
requires reviews, GitHub may still permit a maintainer to merge. Semantic
requirements and mechanical enforcement are separate decisions.

A question is not automatically a blocker. If the answer could change the
verdict, say that explicitly.

### Signal discipline

Prefer a few actionable findings over exhaustive noise. Do not spend review
attention on:

- lint/format/style issues already reported mechanically;
- personal naming/wording preferences that do not impair correctness or
  understanding;
- speculative failures without a plausible path;
- duplicate comments for the same root problem;
- unrelated nearby cleanup;
- generated files without a specific reason;
- comments whose only purpose is to demonstrate that review occurred.

Every finding should say what is wrong and how it is known: the violated
contract, failing input/path, or concrete evidence. State uncertainty when it
exists instead of manufacturing a blocker.

### Review loop

```text
review -> findings -> revision -> revalidation -> targeted re-review -> verdict
```

Re-review focuses on what changed and what those changes could have disturbed.
A mechanical correction already covered by deterministic checks may not require
another full pass.

Blocking findings are resolved, not outlasted. If author and reviewer disagree
and cannot resolve the disagreement from repository evidence, a human decision
is required.

## Handoff contract

A handoff exists only when unfinished work must be continued by another agent,
person, or fresh context: ownership changes, a session is being discarded, the
current owner is blocked/unable to continue, or review fixes are delegated.

A completed PR needs no handoff.

### Location

When a PR exists, a handoff is a top-level PR comment. If no PR exists yet, put
the handoff on the Engineering Mission Issue. Repeated handoffs remain separate
chronological comments; they do not overwrite one another or the durable PR
body.

### Stable handoff shape

Every handoff starts with the stable marker `## Handoff`. Use only the
subsections that contain meaningful information:

```markdown
## Handoff

### Remaining
What still has to become true for the mission.

### Attempts / failures
Approaches tried, what failed, and what the failure established.

### Uncertainty / unsafe state
Anything unverified, partial, misleading, or unsafe to assume.

### Next action
The single concrete action the predecessor would take next.
```

A brief handoff may omit empty subsections. The stable marker is sufficient for
future Pantheon adapters to identify handoff records without a custom schema.

### What belongs in a handoff

Capture information expensive to reconstruct:

- what remains against the mission;
- why the current approach was chosen;
- approaches tried and abandoned, with reasons;
- failures and what they established;
- disproven hypotheses and unresolved uncertainty;
- partial/unsafe state;
- the recommended next action.

Do not copy data already durable and current in Git/GitHub: changed-file lists,
commits, diffs, review threads, or CI/check state already recorded there.

**Do include local-only validation/failure evidence when it materially reduces
rediscovery**, especially an exact command/input/result that disproved an
approach, exercised uncommitted state, or never reached GitHub CI.

### Trust model

**A handoff is historical evidence, never authority.**

Before acting, the successor re-establishes current state from the repository
and GitHub: mission, branch/diff, PR, reviews, and current checks. Where current
state and handoff disagree, current state wins.

Do not execute a recommended next action merely because the predecessor wrote
it. Re-derive that action from current evidence first.

## Merge and mission closure

These concepts are distinct:

| Concept | Meaning |
|---|---|
| Ready | Author claims the PR is reviewable |
| Accepted review | Independent reviewer judges it should become authoritative |
| Mergeable | GitHub currently permits the merge |
| Complete mission | The mission-closing change has landed and the mission outcome is satisfied |

A mission is complete when its single mission-closing PR merges into the default
branch with its acceptance criteria satisfied, adequate evidence accepted by an
independent reviewer, and no unresolved Pantheon-semantic blocking finding.
GitHub closes the Issue from the closing relationship at that point.

A supporting PR merge does not complete or close the mission.

Issue closure records the engineering decision that the mission was satisfied;
it is not infallible proof. If later repository evidence disproves the outcome,
reopen the mission or create the appropriate new mission according to what
actually changed.

### Native state stays native

Pantheon does not mirror Draft/Ready, native review state, unresolved threads,
check state, mergeability, linked Issue state, or Issue closure with labels such
as `agent-working`, `ready-to-merge`, `validation-passed`, or
`mission-complete`.

If future automation needs a state GitHub does not represent, add it only when a
real consumer and ownership rule exist.

### How a pull request lands

A pull request lands on `main` as a merge commit. GitHub's squash and rebase
options are not used.

A pull request is the candidate change and its evidence, and the merge commit is
where that unit enters `main`. Its second parent states which commits formed one
reviewed change, so that boundary survives in the repository itself rather than
only in GitHub. Squashing discards the boundary's interior; rebasing discards
the boundary and rewrites the commits as well, so what lands is not the history
that was reviewed and verified.

The tradeoff is accepted rather than overlooked. History is not linear, and
reading `main` shows every intermediate commit rather than one entry per change;
`git log --first-parent` recovers the per-change view when that is what is
wanted.

Every merge to `main` so far has used this method, so the contract and the
repository describe the same thing.

Restricting GitHub's merge button to this method is mechanical enforcement, and
follows the same rule as the rest of this section: the semantic contract comes
first.

### Deferred enforcement

The following are deliberately deferred:

- required reviews;
- required checks beyond current repository validation;
- required conversation resolution;
- CODEOWNERS;
- signed-commit/linear-history rules;
- merge queue;
- auto-merge;
- reviewer bots/automatic review assignment.

The semantic contract comes first. Add mechanical enforcement when real
repository behavior demonstrates that a rule is needed and there is a reliable
signal to enforce.

## Mechanical validation

`scripts/check-docs-links.sh` verifies that `.github/pull_request_template.md`
exists and keeps its stable headings.

It does not grade PR evidence, reviewer judgment, or handoff quality. Those are
semantic judgments, and pretending a shell check can make them reliably would
create false confidence.

### Action references are immutable

Repository validation is the evidence a candidate change is sound, so what runs
it has to be exactly what was reviewed. A tag or a branch is a moving pointer
owned by whoever controls the action's repository: the same reference can mean
different code on two runs, and the reference does not say so. A full-length
commit SHA is currently the only immutable way to name an action.

Every `uses:` reference in a workflow, and in any composite action this
repository later defines, therefore states a full-length commit SHA, and records
the release that SHA corresponds to as a trailing comment:

```yaml
- uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
```

The comment is not decoration. A bare SHA cannot be reviewed or updated without
first decoding it, and a pin nobody can read is a pin nobody revisits. A
reference beginning with `./` is a local action from this repository at this
commit, and needs neither.

`scripts/check-action-pins.sh` enforces the shape, so a mutable reference fails
verification rather than reaching review. It cannot tell whether a recorded
release name is true — that requires the network, and `scripts/verify.sh`
deliberately needs nothing beyond the pinned toolchain and ordinary build
prerequisites. Resolve a pin against the action's own repository when you write
it; a SHA copied from documentation or another project is not evidence that it
is the release it claims to be.

Updating a pin is an ordinary change, made deliberately and reviewed like any
other. Pantheon has no automatic dependency-update bot, and adding one is a
separate decision — it reintroduces the question of who reviews an automated
bump.
