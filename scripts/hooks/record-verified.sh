#!/bin/sh
# Record the current working-tree fingerprint as verified. Called by
# ./scripts/verify.sh itself immediately after every check has passed — this
# is not a second verification command, it is verify.sh remembering its own
# result.
#
# Storage is local, transient, and uncommitted (.git/pantheon/), per the
# constraint in Issue #21: this state must never become repository
# authority. Deleting it, or deleting .git entirely, only means the next
# completion attempt is treated as unverified — never the reverse.
#
# Usage: scripts/hooks/record-verified.sh [repo-root]

set -eu

self_dir=$(cd "$(dirname "$0")" && pwd)
. "$self_dir/lib.sh"

root=${1:-$(pantheon_repo_root)}
if [ -z "$root" ]; then
	echo "pantheon hooks: could not resolve repository root" >&2
	exit 1
fi

state_dir=$(pantheon_hook_state_dir "$root")
mkdir -p "$state_dir"

fingerprint=$("$self_dir/tree-fingerprint.sh" "$root")
printf '%s\n' "$fingerprint" >"$state_dir/verified-tree"
date -u +%Y-%m-%dT%H:%M:%SZ >"$state_dir/verified-at" 2>/dev/null || true
