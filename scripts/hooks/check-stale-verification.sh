#!/bin/sh
# Claude Code Stop hook entrypoint. Reads the Stop event JSON payload from
# stdin and blocks the stop (exit 2) only when all of the following hold:
#
#   - the working tree has uncommitted changes (nothing to falsely claim
#     otherwise: a clean tree matches whatever was last committed/reviewed);
#   - those changes do not match the fingerprint of the last successful
#     ./scripts/verify.sh run;
#   - the assistant's final message for this turn is not a
#     docs/development/change-lifecycle.md handoff (`## Handoff`).
#
# A handoff is explicitly not a completion claim (Issue #21 acceptance
# criteria), so it is never blocked by this hook regardless of verification
# state.
#
# This is a guardrail, not a security boundary: any missing precondition
# (no git, no repo root, unreadable stdin) fails open (exit 0) rather than
# blocking a session outside Pantheon's control.
#
# Wired via .claude/settings.json's Stop hook. See
# docs/development/agent-skills-and-hooks.md for the full contract and the
# portability boundary with other agent surfaces.

self_dir=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC1091
. "$self_dir/lib.sh"

payload=$(cat 2>/dev/null) || exit 0

root=$(pantheon_repo_root) || root=""
[ -n "$root" ] || exit 0
command -v git >/dev/null 2>&1 || exit 0
git -C "$root" rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 0

# Clean tree: nothing uncommitted to falsely claim as verified.
if [ -z "$(git -C "$root" status --porcelain 2>/dev/null)" ]; then
	exit 0
fi

# A handoff is not a completion claim; never block it. JSON does not escape
# '#' or ordinary word characters, so a literal substring match on the raw
# payload is sufficient and avoids depending on a JSON parser.
case "$payload" in
*'## Handoff'*) exit 0 ;;
esac

state_dir=$(pantheon_hook_state_dir "$root")
recorded_file="$state_dir/verified-tree"
current_fingerprint=$("$self_dir/tree-fingerprint.sh" "$root" 2>/dev/null) || exit 0

if [ -f "$recorded_file" ]; then
	recorded_fingerprint=$(cat "$recorded_file" 2>/dev/null)
	if [ "$recorded_fingerprint" = "$current_fingerprint" ]; then
		exit 0
	fi
fi

cat >&2 <<'EOF'
pantheon-change-verification: the working tree has uncommitted changes that
do not match the last successful `./scripts/verify.sh` run (or verify.sh has
not been run yet on this tree). Before finishing:

  - run `./scripts/verify.sh` again and let it record the new state, or
  - if this work is genuinely unfinished, write a `## Handoff` instead of
    stopping as if it were complete (docs/development/change-lifecycle.md).

See the `pantheon-change-verification` skill for the full procedure.
EOF
exit 2
