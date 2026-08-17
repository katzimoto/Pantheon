#!/bin/sh
# Print a fingerprint of the current working-tree state: HEAD, tracked
# changes against HEAD, and untracked file content. Two runs print the same
# fingerprint exactly when nothing a `git commit` or `git add` would see has
# changed in between.
#
# This is not a security digest; it is a local guardrail input (see
# docs/development/agent-skills-and-hooks.md). It only has to be fast,
# deterministic, and sensitive to the changes an author could actually make.
#
# Usage: scripts/hooks/tree-fingerprint.sh [repo-root]

set -eu

self_dir=$(cd "$(dirname "$0")" && pwd)
. "$self_dir/lib.sh"

root=${1:-$(pantheon_repo_root)}
if [ -z "$root" ]; then
	echo "pantheon hooks: could not resolve repository root" >&2
	exit 1
fi

head=$(git -C "$root" rev-parse HEAD 2>/dev/null || echo "no-head")
tracked_diff_hash=$(git -C "$root" diff --binary HEAD 2>/dev/null | sha256_of_stdin)

# Untracked files: hash each file's content, then hash the sorted list of
# "hash  path" lines so a rename or a content change both change the result.
# Runs with cwd = $root so hash-file.sh's relative paths resolve correctly,
# and uses `xargs -0` rather than `read -d ''`, which is not POSIX.
untracked_hash=$(
	cd "$root" && git ls-files --others --exclude-standard -z 2>/dev/null |
		xargs -0 "$self_dir/hash-file.sh" 2>/dev/null |
		sort | sha256_of_stdin
)

printf '%s:%s:%s\n' "$head" "$tracked_diff_hash" "$untracked_hash" | sha256_of_stdin
