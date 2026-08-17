#!/bin/sh
# Codex CLI Stop hook entrypoint. Reads the Stop event JSON payload from
# stdin (documented turn-scoped fields: session_id, transcript_path, cwd,
# hook_event_name, model, turn_id) and, on the same drift condition
# check-stale-verification.sh uses for Claude Code, prints
# {"decision":"block","reason":"..."} so Codex continues the turn with that
# reason as a follow-up prompt instead of letting it end silently.
#
# Decision logic (fingerprint comparison, handoff bypass) is not duplicated
# here: it is identical to check-stale-verification.sh's, factored into
# pantheon_tree_matches_last_verified() in lib.sh. Only the vendor-specific
# parts differ, because they have to: Codex's Stop payload carries no
# message-text field the way Claude Code's `last_assistant_message` does, so
# handoff detection here reads the turn's own transcript file instead —
# still a plain substring search, not a parse of an assumed transcript
# schema, so it needs no more confidence in that schema's shape than
# check-stale-verification.sh needs in Claude Code's JSON shape.
#
# This is a guardrail, not a security boundary: any missing precondition
# (no git, no repo root, unreadable stdin/transcript) fails open (prints no
# block decision) rather than blocking a session outside Pantheon's control.
#
# Wired via .codex/hooks.json's Stop hook, active only once the user has
# both trusted this project and set `[features] codex_hooks = true` in
# their own ~/.codex/config.toml — neither of which a repository file can
# set on a user's behalf. See docs/development/agent-skills-and-hooks.md for
# the full contract and the portability boundary with other agent surfaces.

self_dir=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC1091
. "$self_dir/lib.sh"

allow() {
	printf '{}\n'
	exit 0
}

block() {
	reason=$1
	printf '{"decision":"block","reason":%s}\n' "$(pantheon_json_string "$reason")"
	exit 0
}

payload=$(cat 2>/dev/null) || allow

cwd=$(printf '%s' "$payload" | sed -n 's/.*"cwd" *: *"\([^"]*\)".*/\1/p' | head -n1)
transcript_path=$(printf '%s' "$payload" |
	sed -n 's/.*"transcript_path" *: *"\([^"]*\)".*/\1/p' | head -n1)

if [ -n "$cwd" ]; then
	root=$cwd
else
	root=$(pantheon_repo_root) || root=""
fi
[ -n "$root" ] || allow
command -v git >/dev/null 2>&1 || allow
git -C "$root" rev-parse --is-inside-work-tree >/dev/null 2>&1 || allow

# A handoff is not a completion claim; never block it. Plain substring
# search over the transcript file's raw bytes, same reasoning as the raw
# stdin substring search in check-stale-verification.sh: this needs no
# assumption about the transcript's exact JSON/JSONL schema, only that the
# turn's own text appears somewhere in the file Codex names.
if [ -n "$transcript_path" ] && [ -r "$transcript_path" ]; then
	if grep -qF '## Handoff' "$transcript_path" 2>/dev/null; then
		allow
	fi
fi

if pantheon_tree_matches_last_verified "$root" "$self_dir"; then
	allow
fi

block "pantheon-change-verification: the working tree no longer matches the last successful ./scripts/verify.sh run recorded for this checkout (this includes a tree changed and then committed without re-verifying). Run ./scripts/verify.sh again, or write a proper ## Handoff instead of stopping as if the work were complete."
