---
name: pantheon-independent-review
description: Perform independent review of a Pantheon pull request against its mission, contracts, and evidence. Use only as a reviewer principal distinct from whoever authored the change, from fresh context not inherited from the authoring session. Do not use this for self-review of your own change.
metadata:
  pantheon-authority: procedural-guidance-only
---

# Pantheon independent review

This operationalizes the independent-review contract in
`docs/development/change-lifecycle.md` ("Independent review"). Read that
document for the actual rules; this skill is the review checklist and the
gate that keeps this from becoming self-review with extra steps.

## Before you start: the fresh-principal check

Stop and confirm before doing anything else:

- **Are you the same principal/session that authored this change** (wrote
  the diff, drafted the PR body, or ran `pantheon-pr-evidence` for it in
  this same context)? If yes, **this skill does not apply to you.** Use
  self-review guidance in `docs/development/change-lifecycle.md` instead,
  and do not record a review verdict — a self-review is not an independent
  review no matter how thoroughly it is performed.
- Are you a fresh context that has not inherited the authoring session's
  reasoning (a different human, a different agent, or the same agent
  product started in a new context with no carried-over authoring memory)?
  If yes, proceed.

`docs/development/change-lifecycle.md` distinguishes the **logical review
principal** from the **GitHub credential** used to record the review. Two
different principals may share one GitHub account; that does not make a
shared-credential review self-review, and a distinct GitHub account does not
by itself make a same-principal review independent. The principal
distinction is what matters, not which account clicks the button.

## Procedure

1. **Re-derive the mission from the Issue**, not from the PR body's summary
   of it. Read the actual acceptance criteria.

2. **Read only what the change touches**: the relevant canonical
   contract(s), implementation, and tests. Widen scope only when a concrete
   reason — a dependency, an invariant, a cross-reference — requires it.

3. **Review in priority order**, stopping to surface a high-priority
   blocker rather than burying it under lower-value commentary:
   1. Mission mismatch or incomplete mission satisfaction.
   2. Correctness, data loss, security, or safety failure.
   3. Canonical contract/invariant violation.
   4. Evidence that does not support the claim.
   5. Recovery, concurrency, persistence, or compatibility risk.
   6. Maintainability problems that materially increase future risk.
   7. Non-blocking improvements.

4. **Check evidence against the actual diff and, where practical, actual
   execution** — do not accept an evidence claim on the PR body's word
   alone. A cited check only counts when its result is actually proof of
   the specific criterion.

5. **Apply signal discipline.** Prefer a few actionable findings over
   exhaustive noise. Do not spend review attention on style/lint issues
   already caught mechanically, personal wording preferences, speculative
   failures with no plausible path, duplicate comments for one root
   problem, unrelated nearby cleanup, or comments that only demonstrate
   review occurred. Every finding states what is wrong and how it is known.

6. **Record the verdict.**
   - With a distinct GitHub actor, use GitHub's native review state:
     Request changes (blocking), Comment (non-blocking or a question),
     Approve (accepted).
   - With a shared GitHub actor, post a top-level PR comment starting with
     the stable marker `## Independent review`, stating `Verdict: approve
     | request changes | comment` and the findings/rationale.

## What this skill must not do

- It does not substitute for `./scripts/verify.sh`/CI as the correctness
  gate — it is a semantic judgment layered on top of a passing verification
  run, not a replacement for one.
- It does not grade PR evidence mechanically or pretend a checklist can
  replace judgment; `docs/development/change-lifecycle.md` is explicit that
  this is a human/agent judgment call.
- It does not resolve an author/reviewer disagreement that repository
  evidence cannot settle — that requires a human decision.
- It never records an "independent" verdict from the authoring principal or
  session, regardless of GitHub account used.

## Non-triggers

- You authored, or materially co-authored, the change under review in this
  same context.
- The change has no pull request yet (use `pantheon-mission-planning` or
  `pantheon-pr-evidence` first).
