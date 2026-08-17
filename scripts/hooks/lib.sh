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
# Deliberately under .git/ so it can never become committed repository
# authority (see docs/development/agent-skills-and-hooks.md).
pantheon_hook_state_dir() {
	root=$1
	printf '%s/.git/pantheon\n' "$root"
}
