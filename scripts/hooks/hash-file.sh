#!/bin/sh
# Print "<sha256>  <path>" for each argument. Helper for tree-fingerprint.sh,
# invoked via `xargs -0` so it works without a non-POSIX `read -d ''`.
#
# Usage: scripts/hooks/hash-file.sh path [path...]

set -eu

self_dir=$(cd "$(dirname "$0")" && pwd)
. "$self_dir/lib.sh"

for path in "$@"; do
	content_hash=$(sha256_of_stdin <"$path" 2>/dev/null || echo "unreadable")
	printf '%s  %s\n' "$content_hash" "$path"
done
