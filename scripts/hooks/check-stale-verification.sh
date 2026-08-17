#!/bin/sh
# Claude Code Stop hook entrypoint. Reads the Stop event JSON payload from
# stdin and blocks the stop (exit 2) only when both of the following hold:
#
#   - a prior successful ./scripts/verify.sh run was recorded for this
#     checkout, and the current tree no longer matches its fingerprint —
#     whether that drift is still uncommitted or was folded into a later
#     commit; committing an unverified change is still drift, not a fresh
#     clean baseline (see pantheon_tree_matches_last_verified in lib.sh);
#   - the assistant's final message for this turn is not a
#     docs/development/change-lifecycle.md handoff (`## Handoff`).
#
# A handoff is explicitly not a completion claim (Issue #21 acceptance
# criteria), so it is never blocked by this hook regardless of verification
# state. A checkout where verify.sh has never once succeeded is out of scope
# for this specific check (see lib.sh); it does not block on every turn.
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

# A handoff is not a completion claim; never block it. JSON does not escape
# '#' or ordinary word characters, so a literal substring match on the raw
# payload is sufficient and avoids depending on a JSON parser.
case "$payload" in
*'## Handoff'*) exit 0 ;;
esac

pantheon_tree_matches_last_verified "$root" "$self_dir" && exit 0

cat >&2 <<'EOF'
pantheon-change-verification: the working tree no longer matches the last
successful `./scripts/verify.sh` run recorded for this checkout (this
includes a tree changed and then committed without re-verifying — a commit
does not by itself make an unverified change clean). Before finishing:

  - run `./scripts/verify.sh` again and let it record the new state, or
  - if this work is genuinely unfinished, write a `## Handoff` instead of
    stopping as if it were complete (docs/development/change-lifecycle.md).

See the `pantheon-change-verification` skill for the full procedure.
EOF
exit 2
