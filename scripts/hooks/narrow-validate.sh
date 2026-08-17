#!/bin/sh
# Claude Code PostToolUse hook entrypoint (matcher: Edit|Write). Reads the
# PostToolUse JSON payload from stdin, and for the one changed file, runs
# only the existing sensitive-file validator(s) that file's path is actually
# relevant to — never the full ./scripts/verify.sh suite. This is early
# feedback, not the gate; a pass here is not evidence of anything beyond the
# one narrow check that ran.
#
# Dispatch table (path -> existing verify.sh check, unchanged, not
# reimplemented here):
#   Cargo.toml / Cargo.lock / crates/*/Cargo.toml -> scripts/check-crate-deps.sh
#   .github/workflows/**                     -> scripts/check-action-pins.sh
#   **/*.md, AGENTS.md, CLAUDE.md,
#     .github/pull_request_template.md,
#     .github/ISSUE_TEMPLATE/**              -> scripts/check-docs-links.sh
#
# A path matching none of these triggers nothing: most edits (ordinary Rust
# source, for example) are covered by the full suite at completion time and
# do not need early narrow feedback here.
#
# Set PANTHEON_HOOK_DRY_RUN=1 to print the dispatch decision (which
# check(s), for which path) without actually running the check. Used by
# scripts/check-hooks.sh to test dispatch selectivity without requiring the
# pinned Rust toolchain. Never set this for real hook use.
#
# This hook never blocks (PostToolUse cannot block; the tool already ran).
# It fails open on any unreadable input.

self_dir=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC1091
. "$self_dir/lib.sh"

payload=$(cat 2>/dev/null) || exit 0

root=$(pantheon_repo_root) || root=""
[ -n "$root" ] || exit 0

# Extract tool_input.file_path without a JSON parser: it is a flat string
# field with no embedded quotes/newlines to worry about (a path), unlike
# assistant-message fields elsewhere.
file_path=$(printf '%s' "$payload" |
	sed -n 's/.*"file_path" *: *"\([^"]*\)".*/\1/p' | head -n1)
[ -n "$file_path" ] || exit 0

# Normalize to repo-root-relative for matching, in case the tool reported an
# absolute path under $root.
case "$file_path" in
"$root"/*) rel_path=${file_path#"$root"/} ;;
*) rel_path=$file_path ;;
esac

run_check() {
	name=$1
	script=$2
	if [ "${PANTHEON_HOOK_DRY_RUN:-}" = "1" ]; then
		printf 'DRY-RUN: would run %s for %s\n' "$name" "$rel_path"
		return 0
	fi
	if ! "$root/$script" >/tmp/pantheon-narrow-validate.$$ 2>&1; then
		printf 'pantheon narrow validator (%s) failed for %s:\n' "$name" "$rel_path" >&2
		cat /tmp/pantheon-narrow-validate.$$ >&2
		rm -f /tmp/pantheon-narrow-validate.$$
		return 1
	fi
	rm -f /tmp/pantheon-narrow-validate.$$
	return 0
}

status=0
case "$rel_path" in
Cargo.toml | Cargo.lock | crates/*/Cargo.toml)
	run_check "check-crate-deps" "scripts/check-crate-deps.sh" || status=1
	;;
esac
case "$rel_path" in
.github/workflows/*)
	run_check "check-action-pins" "scripts/check-action-pins.sh" || status=1
	;;
esac
case "$rel_path" in
*.md | .github/pull_request_template.md | .github/ISSUE_TEMPLATE/*)
	run_check "check-docs-links" "scripts/check-docs-links.sh" || status=1
	;;
esac

exit "$status"
