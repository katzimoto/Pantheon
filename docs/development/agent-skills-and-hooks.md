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
└── pantheon-independent-review/SKILL.md
```

### The four MVP skills

| Skill | Operationalizes | Use when |
|---|---|---|
| `pantheon-mission-planning` | `AGENTS.md` "Start here", `docs/development/missions.md` | Starting work on a mission Issue, before the first edit |
| `pantheon-change-verification` | `AGENTS.md` "Validation", `./scripts/verify.sh` | Before claiming a change done; after any edit; when the Stop hook reports stale verification |
| `pantheon-pr-evidence` | `docs/development/change-lifecycle.md` pull request contract | Opening or updating a pull request |
| `pantheon-independent-review` | `docs/development/change-lifecycle.md` independent review contract | Reviewing a PR as a principal distinct from its author, from fresh context |

Each skill's own frontmatter `description` and body state its trigger and
non-trigger conditions precisely; this table is a map, not a substitute for
reading them. `pantheon-independent-review` in particular refuses to apply to
self-review — read it before assuming any review skill is interchangeable
with the others.

No skill restates architecture, Mission, Version, PR-lifecycle, label, or
release semantics. Each points at the document that owns its subject. This
keeps the skill catalog small on purpose (Issue #21's constraint): a new
skill is justified only by a repeated, Pantheon-specific procedural gap, the
same bar `docs/reviews/2026-08-rust-agent-skill-research.md` (Issue #22)
applied to Rust-specific candidates. Generic Rust, Git, formatting, or
"write idiomatic code" content is out of scope for this catalog regardless
of format support.

## Portability: one body, several agent surfaces

The Agent Skills format is an open, cross-platform standard, not a
Claude-specific mechanism, so the goal is exactly one editable copy of each
skill body with vendor directories pointing at it — never a second body that
can quietly drift out of sync.

| Surface | Where it looks | How it reaches the canonical body |
|---|---|---|
| OpenCode | `.agents/skills/<name>/SKILL.md` (native project search path) | Direct — no adapter needed |
| Claude Code | `.claude/skills/<name>/SKILL.md` | `.claude/skills/<name>` is a symlink to `../../.agents/skills/<name>` |
| Codex CLI | `.codex/skills/<name>/SKILL.md` | `.codex/skills/<name>` is a symlink to `../../.agents/skills/<name>` |

`scripts/check-skill-symlinks.sh` (run by `./scripts/verify.sh`) mechanically
enforces this: every `.claude/skills/*` and `.codex/skills/*` entry must be a
symlink that resolves into `.agents/skills/`, and every canonical skill's
frontmatter `name` must match its directory. A real, independently-editable
`SKILL.md` under a vendor directory fails verification rather than silently
becoming a second copy.

## Lifecycle hooks

Hook *logic* lives once, in `scripts/hooks/`, as ordinary POSIX shell with no
dependency beyond what `./scripts/verify.sh` already requires (Git and
standard utilities — no `jq`, no other language runtime). Vendor
configuration only wires an event to a script; it contains no independent
decision logic to keep in sync.

| Script | Purpose |
|---|---|
| `scripts/hooks/lib.sh` | Shared helpers (portable sha256, repo-root/state-dir resolution). Sourced, not run directly. |
| `scripts/hooks/tree-fingerprint.sh` | Deterministic fingerprint of the current working tree (HEAD + tracked diff + untracked file content). |
| `scripts/hooks/record-verified.sh` | Called by `./scripts/verify.sh` itself on success; records the fingerprint under `.git/pantheon/verified-tree`. Not a second verification command — verify.sh remembering its own result. |
| `scripts/hooks/check-stale-verification.sh` | Stop-event entrypoint: blocks a completion claim on a changed, unverified tree; never blocks a `## Handoff`. |
| `scripts/hooks/narrow-validate.sh` | Post-edit entrypoint: runs only the one existing validator relevant to the changed file's path. |

### Why `.git/` and not a repository file

The recorded fingerprint is local, transient, and disposable by design (the
constraint Issue #21 states explicitly): it never becomes committed
repository authority, is meaningless to anyone but the local checkout that
produced it, and its absence only ever means "treat this tree as
unverified" — never the reverse. Storing it under `.git/pantheon/` rather
than a tracked path is what makes that guarantee structural rather than a
convention someone could accidentally commit past.

### The stale-verification guardrail

`scripts/hooks/check-stale-verification.sh` is wired as Claude Code's `Stop`
hook (`.claude/settings.json`). On every turn that is about to end:

1. A clean tree (no uncommitted changes) never blocks — there is nothing to
   falsely claim as verified beyond what was already committed.
2. A dirty tree whose fingerprint matches the last successful
   `./scripts/verify.sh` run never blocks.
3. A dirty tree containing the literal text `## Handoff` in the turn's final
   message never blocks — `docs/development/change-lifecycle.md` treats a
   handoff as explicitly not a completion claim, and this hook honors that
   distinction rather than forcing every stop through verification.
4. Otherwise, it blocks (`exit 2`) with a message naming the two ways
   forward: re-run `./scripts/verify.sh`, or write a proper handoff.

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

`scripts/check-hooks.sh` (run by `./scripts/verify.sh`) exercises both
required scenarios end to end, entirely against a disposable scratch Git
repository:

- a verified tree that is then changed produces a blocked completion attempt
  until it is re-verified;
- the same changed, unverified tree with a `## Handoff` message is never
  blocked;
- a sensitive-file edit dispatches its one relevant validator; an unrelated
  edit dispatches nothing.

## Portability boundary: where a vendor cannot enforce identical behavior

| Concern | Claude Code | Codex CLI | OpenCode |
|---|---|---|---|
| Skill consumption | Native (`.claude/skills/`, symlinked) | Native (`.codex/skills/`, symlinked) | Native (`.agents/skills/`, no adapter) |
| Stale-verification guardrail | Full: `Stop` hook, documented JSON schema, can block (`exit 2`) | Not wired. Codex's hook mechanism is presently behind a feature flag with no stable published payload schema at the time this was written; scripting against it now would mean guessing a contract that could change under it. Revisit once it stabilizes. | Best-effort only (`.opencode/plugin/pantheon-hooks.js`, `session.idle`): can warn, cannot reliably block, because blocking correctly requires detecting a `## Handoff` in the same turn, and OpenCode's public plugin docs do not confirm that `session.idle` exposes the turn's message text the way Claude Code's `Stop` payload's `last_assistant_message` field does. |
| Narrow post-edit feedback | Full: `PostToolUse` hook | Not wired, same reason as above | Best-effort only (`tool.execute.after`): reads several plausible file-path field names defensively since OpenCode's plugin docs name the event but do not publish its payload schema; no-ops if none match |

Claude Code is the fully-specified reference implementation because its hook
event set and JSON payload schema are precisely documented (event names,
`transcript_path`/`last_assistant_message`/`tool_input` fields, exit-code and
`hookSpecificOutput` semantics). Where another surface's own documentation
does not yet publish an equivalent contract precisely enough to script
against reliably, this repository does not fabricate one — it says so here,
implements the honest subset that surface's confirmed capabilities support,
and defers full parity until that surface's contract is stable enough to
implement without guessing. All three surfaces get the same skill bodies
regardless: the portability gap is entirely in hook enforcement strength, not
in what procedural guidance is available.

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
knowledge, per the same evaluation bar `docs/reviews/2026-08-rust-agent-skill-research.md`
applied. Add it under `.agents/skills/<name>/SKILL.md`, symlink it from
`.claude/skills/` and `.codex/skills/`, and it is automatically covered by
`scripts/check-skill-symlinks.sh`.

A new hook needs a deterministic, fast, inspectable, fail-safe check that
narrow validators or `./scripts/verify.sh` do not already cover at the right
altitude. Add the logic once in `scripts/hooks/`, wire it from whichever
vendor configuration needs it, and extend `scripts/check-hooks.sh` with the
scenario that proves it works — an unexercised hook is a claim, not
evidence.
