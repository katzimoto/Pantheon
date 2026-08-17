# Shared POSIX shell helpers for Pantheon's lifecycle hook scripts.
#
# Sourced by other scripts in this directory. Not executable on its own, and
# deliberately dependency-free beyond ordinary POSIX utilities and Git,
# matching the rest of scripts/: no jq, no other language runtime.
#
# Usage: . "$(dirname "$0")/lib.sh"

# Print a sha256 hex digest of stdin. macOS ships `shasum -a 256` and no
# `sha256sum`; most Linux distributions have `sha256sum` and no `shasum`.
# Prefer whichever is present rather than assuming one platform.
sha256_of_stdin() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum | cut -d' ' -f1
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 | cut -d' ' -f1
	else
		echo "pantheon hooks: no sha256sum or shasum available" >&2
		return 1
	fi
}

# Resolve the Pantheon repository root a hook should operate against.
#
# ${CLAUDE_PROJECT_DIR} is authoritative when a vendor sets it (Claude Code
# always does for hook invocations). Otherwise fall back to discovering the
# root from the current directory, so the same scripts are directly testable
# from any checkout without a vendor environment.
pantheon_repo_root() {
	if [ -n "${CLAUDE_PROJECT_DIR:-}" ]; then
		printf '%s\n' "$CLAUDE_PROJECT_DIR"
		return 0
	fi
	git rev-parse --show-toplevel 2>/dev/null
}

# Local, transient, uncommitted state directory for hook bookkeeping.
# Deliberately inside Git's own per-worktree state (see
# docs/development/agent-skills-and-hooks.md) so it can never become
# committed repository authority.
#
# Not simply "$root/.git/pantheon": in a linked worktree, .git is a *file*
# pointing at the real per-worktree git-dir under the main checkout's
# .git/worktrees/<name>/, not a directory. `git rev-parse --git-path` is the
# supported way to resolve a path inside the git-dir that Git itself treats
# as private to the current worktree, and `--path-format=absolute` avoids a
# path relative to the wrong cwd.
pantheon_hook_state_dir() {
	root=$1
	git -C "$root" rev-parse --path-format=absolute --git-path pantheon 2>/dev/null
}

# Returns 0 (true) when the current working tree matches the fingerprint of
# the last successful ./scripts/verify.sh run recorded for $1, OR when no
# such record exists at all for this checkout. Returns 1 only when a prior
# record exists and the current tree no longer matches it: drift since the
# last verified state, whether that drift is still uncommitted or was
# folded into a later commit — both invalidate the prior verification, so
# neither is treated as "clean" on its own. A checkout where verify.sh has
# never once succeeded is out of scope for this specific drift check;
# AGENTS.md's ordinary "run verify.sh before claiming done" instruction
# covers that case independently of this guardrail.
#
# $1 = root, $2 = the scripts/hooks directory (callers already know it).
pantheon_tree_matches_last_verified() {
	root=$1
	hooks_dir=$2
	state_dir=$(pantheon_hook_state_dir "$root") || return 0
	recorded_file="$state_dir/verified-tree"
	[ -f "$recorded_file" ] || return 0
	current_fingerprint=$("$hooks_dir/tree-fingerprint.sh" "$root" 2>/dev/null) || return 0
	recorded_fingerprint=$(cat "$recorded_file" 2>/dev/null)
	[ "$recorded_fingerprint" = "$current_fingerprint" ]
}

# Print $1 as a double-quoted JSON string. Escapes only backslash and
# double-quote: good enough for this repository's own static, single-line,
# printable-ASCII guidance text (the only thing any current caller passes),
# not a general-purpose JSON encoder.
pantheon_json_string() {
	value=$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')
	printf '"%s"' "$value"
}
