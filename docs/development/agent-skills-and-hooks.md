# Agent Skills and Lifecycle Hooks

## Status

**Canonical for where Pantheon-specific agent skills live, what a lifecycle
hook may and may not do, and how both relate to every other authority in this
repository.** It has no authority over what Pantheon *is*
(`docs/architecture/`), how an Engineering Mission is specified
(`docs/development/missions.md`), how a change is proven and landed
(`docs/development/change-lifecycle.md`), or how an agent operates generally
(`AGENTS.md`). This document establishes the mechanism; those documents
remain authoritative for their own subjects.

## Authority relationship

```text
AGENTS.md / docs/architecture/ / docs/development/*.md / ./scripts/verify.sh
  -> remain the sole authorities for what must be true

skills (.agents/skills/*/SKILL.md)
  -> procedural guidance that operationalizes those authorities
  -> point at them; never restate, replace, or compete with them

hooks (scripts/hooks/*.sh + thin vendor wiring)
  -> deterministic local guardrails and early feedback
  -> reinforce the authorities above; never substitute for them
```

A skill or a hook that drifts from this — that starts asserting what is true
rather than pointing at what already says so — is a defect in the skill or
hook, not a second source of truth to reconcile against. If a skill's
procedure and the document it operationalizes disagree, the document wins;
fix the skill.

Neither mechanism can:

- change what a mission's acceptance criteria are;
- change what `./scripts/verify.sh` checks or its result;
- record GitHub-native state (Draft/Ready, review, merge, Issue closure)
  another way;
- grant scope, authority, or exceptions no document already grants.

## Canonical skill location

Every Pantheon-specific skill body lives exactly once, at
`.agents/skills/<name>/SKILL.md`, using the open
[Agent Skills specification](https://agentskills.io/specification): YAML
frontmatter with at minimum `name` (matching the directory) and
`description`, then a Markdown body. `.agents/` is not a Pantheon invention —
OpenCode already treats it as a native project-local skill search path (see
Portability below), which is part of why it was chosen: the more agent
surfaces that need zero adapter code, the fewer places skill semantics can
drift.

```text
.agents/skills/
├── pantheon-mission-planning/SKILL.md
├── pantheon-change-verification/SKILL.md
├── pantheon-pr-evidence/SKILL.md
├── pantheon-independent-review/SKILL.md
├── dependency-change-procedure/SKILL.md
└── persistence-and-recovery-transaction-review/SKILL.md
```

### The skill catalog

| Skill | Operationalizes | Use when |
|---|---|---|
| `pantheon-mission-planning` | `AGENTS.md` "Start here", `docs/development/missions.md` | Starting work on a mission Issue, before the first edit |
| `pantheon-change-verification` | `AGENTS.md` "Validation", `./scripts/verify.sh` | Before claiming a change done; after any edit; when the Stop hook reports stale verification |
| `pantheon-pr-evidence` | `docs/development/change-lifecycle.md` pull request contract | Opening or updating a pull request |
| `pantheon-independent-review` | `docs/development/change-lifecycle.md` independent review contract | Reviewing a PR as a principal distinct from its author, from fresh context |
| `dependency-change-procedure` | `docs/development/implementation.md` dependency and new-crate policy | Adding or materially changing a Rust dependency, or adding a new crate |
| `persistence-and-recovery-transaction-review` | `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`, `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`, `docs/development/implementation.md` | Implementing or reviewing authoritative `pantheon-store` mutations, authoritative/revisioned schema, command/idempotency/Event Journal transactions, or recovery/reconciliation paths |

Each skill's own frontmatter `description` and body state its trigger and
non-trigger conditions precisely; this table is a map, not a substitute for
reading them. `pantheon-independent-review` in particular refuses to apply to
self-review — read it before assuming any review skill is interchangeable
with the others.

No skill restates architecture, Mission, Version, PR-lifecycle, label, or
release semantics. Each points at the document that owns its subject. This
keeps the skill catalog small on purpose (Issue #21's constraint): a new
skill is justified only by a repeated, Pantheon-specific procedural gap, the
same bar Issue #22's Rust-specific-skill research applied to its own
candidates. Generic Rust, Git, formatting, or "write idiomatic code" content
is out of scope for this catalog regardless of format support.

## Portability: one body, several agent surfaces

The Agent Skills format is an open, cross-platform standard, not a
Claude-specific mechanism, so the goal is exactly one editable copy of each
skill body with vendor directories pointing at it — never a second body that
can quietly drift out of sync.

| Surface | Where it looks | How it reaches the canonical body |
|---|---|---|
| OpenCode | `.agents/skills/<name>/SKILL.md` (native project search path) | Direct — no adapter needed |
| Codex CLI | `.agents/skills/<name>/SKILL.md` (native discovery) | Direct — no adapter needed |
| Claude Code | `.claude/skills/<name>/SKILL.md` | `.claude/skills/<name>` is a symlink to `../../.agents/skills/<name>` |

Claude Code is the one surface here that only looks in its own
`.claude/skills/`, so it is the one surface that needs a symlink at all.
Codex CLI's own skill catalog (`$skill-installer`, the wider ecosystem
around `agentskills.io`) already resolves and discovers skills through
`.agents/skills/` directly, the same as OpenCode — an earlier version of
this mechanism additionally symlinked `.codex/skills/<name>` out of caution
before that was confirmed; it was removed once multiple independent sources,
including OpenAI's own Codex review tooling, confirmed the native path makes
it unnecessary.

`scripts/check-skill-symlinks.sh` (run by `./scripts/verify.sh`) mechanically
enforces the one-canonical-body half: every `.claude/skills/*` entry must be a
symlink that resolves into `.agents/skills/`, and every canonical skill's
frontmatter `name` must match its directory. A real, independently-editable
`SKILL.md` under a vendor directory fails verification rather than silently
becoming a second copy.

`scripts/check-skill-conformance.sh` (also run by `./scripts/verify.sh`)
enforces the rest of the stable Agent Skills specification on every canonical
`SKILL.md`: frontmatter shape, name/description/compatibility constraints, and
metadata shape, plus duplicate-skill identity, with a `--self-test` that proves
the malformed cases are actually rejected. The behavioral half — whether a
skill triggers correctly and improves workflow value — is deliberately not in
the gate; `docs/development/skill-evals.md` owns that split, the
`evals/evals.json` fixture shape, and the separate `scripts/run-skill-evals.py`
harness.

## Lifecycle hooks

Hook *logic* lives once, in `scripts/hooks/`, as ordinary POSIX shell with no
dependency beyond what `./scripts/verify.sh` already requires (Git and
standard utilities — no `jq`, no other language runtime). Vendor
configuration only wires an event to a script; it contains no independent
decision logic to keep in sync.

| Script | Purpose |
|---|---|
| `scripts/hooks/lib.sh` | Shared helpers (portable sha256, repo-root/state-dir resolution, the fingerprint-comparison decision `pantheon_tree_matches_last_verified`, minimal JSON string escaping). Sourced, not run directly. |
| `scripts/hooks/tree-fingerprint.sh` | Deterministic fingerprint of the current working tree (HEAD + tracked diff + untracked file content). |
| `scripts/hooks/record-verified.sh` | Called by `./scripts/verify.sh` itself on success; records the fingerprint under Git's own per-worktree state directory (`git rev-parse --git-path pantheon`). Not a second verification command — verify.sh remembering its own result. |
| `scripts/hooks/check-stale-verification.sh` | Claude Code Stop-event entrypoint: blocks a completion claim on a changed, unverified tree; never blocks a `## Handoff`. |
| `scripts/hooks/check-stale-verification-codex.sh` | Codex CLI Stop-event entrypoint: the same decision (`pantheon_tree_matches_last_verified`), adapted to Codex's own stdin/stdout contract. |
| `scripts/hooks/narrow-validate.sh` | Post-edit entrypoint: runs only the one existing validator relevant to the changed file's path. |

### Why Git's own per-worktree state, and not a repository file

The recorded fingerprint is local, transient, and disposable by design (the
constraint Issue #21 states explicitly): it never becomes committed
repository authority, is meaningless to anyone but the local checkout that
produced it, and its absence only ever means "treat this tree as
unverified" — never the reverse. Recording it via `git rev-parse
--git-path pantheon` rather than a tracked path is what makes that guarantee
structural rather than a convention someone could accidentally commit past,
and it is also what makes the state correctly per-worktree: in a linked Git
worktree, `.git` is a *file* pointing at the real git-dir under the main
checkout's `.git/worktrees/<name>/`, not a directory, so a hardcoded
`$root/.git/pantheon` path breaks there. `git rev-parse --git-path` is the
supported way to resolve a path Git itself treats as private to the current
worktree, so each worktree gets its own independent verification record —
correct, since each worktree can have different tree state.

`./scripts/verify.sh`'s call to `record-verified.sh` is best-effort: every
other check already passed by the time it runs, so an environment quirk that
keeps this local bookkeeping step from writing (an unwritable `.git/`, an
unusual worktree layout) produces a warning, not a failed verification.

### The stale-verification guardrail

`scripts/hooks/check-stale-verification.sh` is wired as Claude Code's `Stop`
hook (`.claude/settings.json`); `scripts/hooks/check-stale-verification-codex.sh`
is wired the same way for Codex CLI (`.codex/hooks.json`). Both call the same
decision, `pantheon_tree_matches_last_verified` in `lib.sh`, on every turn
that is about to end:

1. If no successful `./scripts/verify.sh` run has ever been recorded for
   this checkout, the guardrail does not block — a checkout that has never
   been verified at all is out of scope for *this* specific drift check;
   `AGENTS.md`'s ordinary "verify before claiming done" instruction covers
   that case on its own, and blocking every single turn until the first
   `verify.sh` run would make the guardrail obstructive rather than useful.
2. Otherwise, the current tree's fingerprint is compared against the
   recorded one — **regardless of whether the tree is currently dirty or
   clean.** A clean tree is not automatically "nothing to check": committing
   an unverified change makes the working tree clean again without ever
   having been verified, and earlier versions of this guardrail wrongly
   treated a clean tree as always safe, which made that commit a silent
   bypass. Comparing fingerprints unconditionally closes it, because the
   fingerprint already encodes the current commit, not just uncommitted
   diff.
3. A turn whose final message/transcript contains the literal text
   `## Handoff` never blocks, regardless of the fingerprint comparison —
   `docs/development/change-lifecycle.md` treats a handoff as explicitly not
   a completion claim, and this hook honors that distinction rather than
   forcing every stop through verification. Detection is a plain substring
   search over the raw text available to each vendor (Claude Code's own
   `last_assistant_message` payload field; Codex's named transcript file),
   not a parse of an assumed message schema.
4. Otherwise, it blocks with a message naming the two ways forward: re-run
   `./scripts/verify.sh`, or write a proper handoff. Claude Code blocks via
   `exit 2`; Codex CLI via `{"decision":"block","reason":"..."}`, which
   Codex turns into a continuation prompt rather than a hard stop — a
   different mechanism with the same effect, "make the agent address this
   before the turn ends."

This is a guardrail, not a security boundary: every precondition (no Git, no
resolvable repository root, unreadable input) fails open rather than
blocking a session outside Pantheon's control, and it cannot detect a false
`## Handoff` written specifically to bypass it — the same trust model
`docs/development/change-lifecycle.md` already applies to a handoff's
content applies here.

### Narrow post-edit feedback

`scripts/hooks/narrow-validate.sh` is wired as Claude Code's `PostToolUse`
hook (matcher `Edit|Write`). It inspects only the one file that was just
changed and dispatches at most one existing check:

```text
Cargo.toml / Cargo.lock / crates/*/Cargo.toml -> scripts/check-crate-deps.sh
.github/workflows/**                          -> scripts/check-action-pins.sh
**/*.md, .github/pull_request_template.md,
  .github/ISSUE_TEMPLATE/**                   -> scripts/check-docs-links.sh
```

A path matching none of these dispatches nothing — most edits are covered by
the full suite at completion time and do not need early narrow feedback.
`PostToolUse` cannot block in Claude Code's hook model; a failure here is
visible feedback for the one file just touched, not a gate. Both checks
above already exist and are unchanged by this: the hook only decides when to
call them early, never reimplements them.

### Self-test

`scripts/check-hooks.sh` (run by `./scripts/verify.sh`) exercises the
required scenarios end to end, entirely against a disposable scratch Git
repository:

- a verified tree that is then changed produces a blocked completion attempt
  until it is re-verified;
- the same changed, unverified tree with a `## Handoff` message is never
  blocked;
- an unverified change that gets **committed** (not just left uncommitted)
  still blocks — the regression case for the commit-bypass this guardrail
  once had;
- a checkout where `./scripts/verify.sh` has never once succeeded does not
  block on every turn;
- a sensitive-file edit dispatches its one relevant validator; an unrelated
  edit dispatches nothing.

## Portability boundary: where a vendor cannot enforce identical behavior

| Concern | Claude Code | Codex CLI | OpenCode |
|---|---|---|---|
| Skill consumption | Native path, symlinked (`.claude/skills/`) | Native (`.agents/skills/`, no adapter) | Native (`.agents/skills/`, no adapter) |
| Stale-verification guardrail | Full: `Stop` hook, documented JSON schema, blocks via `exit 2` | Full: `Stop` hook (`.codex/hooks.json`), blocks via documented `{"decision":"block","reason":"..."}`. Handoff detection reads the turn's `transcript_path` file for the literal `## Handoff` text — a substring search, not a parse of an assumed transcript schema. Two activation gates outside this repository's control: the user's own `~/.codex/config.toml` needs `[features] codex_hooks = true`, and the project needs to be marked trusted in the user's Codex configuration. | Best-effort only (`.opencode/plugins/pantheon-hooks.js`, generic `event` hook, `event.type === "session.idle"`): can warn, cannot reliably block. Blocking correctly requires detecting a `## Handoff` in the same turn, and OpenCode's documented `session.idle` payload carries no message-text field the way Claude Code's `last_assistant_message` or Codex's `transcript_path` do, so this adapter cannot check for one at all — not "checks and might miss it," genuinely has nothing to check. |
| Narrow post-edit feedback | Full: `PostToolUse` hook (`Edit`\|`Write`) | **Not possible with Codex's current hook model.** Codex's `PostToolUse`/`PreToolUse` fire for the `shell`/Bash tool only; file-editing tools (`apply_patch`, `Edit`/`Write`/`Read`) do not fire either event at all. This is a confirmed architectural limitation of Codex's hook surface today, not an unresearched gap — there is no event to wire this to yet. | Best-effort (`tool.execute.before`, documented `output.args.filePath`, plus `output.args.patchText` marker-line parsing for the `apply_patch` tool GPT-series models substitute for `edit`/`write`). Fires *before* the edit lands on disk, so this is an early advisory nudge on the file about to change, not a true post-edit check the way Claude Code's `PostToolUse` is — a real, accepted timing difference, not an oversight. `tool.execute.after`'s documented payload (`{title, output, metadata}`) has no file-path field, so it is not used; using it would mean guessing an undocumented field. |

Claude Code is the fully-specified reference implementation because its hook
event set and JSON payload schema are precisely documented end to end (event
names, `transcript_path`/`last_assistant_message`/`tool_input` fields,
exit-code and `hookSpecificOutput` semantics). Codex CLI's `Stop` hook is
implemented with the same confidence once its documented fields were
confirmed; its `PostToolUse` gap is a real, confirmed limitation of the
underlying tool rather than a documentation gap on this repository's side.
OpenCode's adapter is deliberately the most limited of the three because its
own plugin documentation does not yet publish payload shapes for the events
this repository would need for full parity (`session.idle`'s fields,
`tool.execute.after`'s file path) — where that is true, this repository does
not fabricate a field to fill the gap; it implements the honest subset the
documented contract actually supports and says so here. All three surfaces
get the same skill bodies regardless: the portability gap is entirely in
hook enforcement strength, not in what procedural guidance is available.

## What hooks must never do

Per Issue #21's constraints, restated here because it is the operative rule
for anyone adding a hook later, not because this document is the authority
for it:

- replace or weaken `./scripts/verify.sh`, CI, GitHub-native review/check
  state, Mission semantics, or reviewer judgement — the canonical final
  correctness gate stays `./scripts/verify.sh`;
- automatically create or issue work, rewrite architecture to match
  implementation, merge pull requests, change GitHub workflow state, upgrade
  dependencies, or grade PR evidence/review quality semantically;
- run expensive/full validation on every edit — narrow post-edit checks are
  early feedback only; expensive validation belongs at completion;
- store durable state anywhere but a local, transient, uncommitted location.

## Adding a skill or a hook later

A new skill needs a repeated, Pantheon-specific procedural gap with clear
value — not "the format supports it" and not generic language/tooling
knowledge, per the same evaluation bar Issue #22's research applied. Add it
under `.agents/skills/<name>/SKILL.md`, symlink it from `.claude/skills/`,
and it is automatically covered by `scripts/check-skill-symlinks.sh` and
`scripts/check-skill-conformance.sh`. A skill whose trigger boundary is worth
regression evidence may also carry `evals/evals.json` per
`docs/development/skill-evals.md`.

A new hook needs a deterministic, fast, inspectable, fail-safe check that
narrow validators or `./scripts/verify.sh` do not already cover at the right
altitude. Add the logic once in `scripts/hooks/`, wire it from whichever
vendor configuration needs it, and extend `scripts/check-hooks.sh` with the
scenario that proves it works — an unexercised hook is a claim, not
evidence.
