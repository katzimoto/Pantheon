#!/bin/sh
# Fast inner-loop check for a single workspace crate.
#
# Performs package-scoped formatting verification, compilation, and tests
# using the pinned workspace toolchain and lockfile. This is early feedback
# only; ./scripts/verify.sh remains the canonical completion gate and must
# still be run before any work is marked complete.
#
# Usage: scripts/dev-check.sh <crate> [test-filter]
#
# Works from anywhere inside or outside the repository.

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
	printf 'usage: scripts/dev-check.sh <crate> [test-filter]\n' >&2
	exit 1
fi

crate=$1
filter=${2:-}

# Reject option-like crate names to prevent argument injection.
case "$crate" in
-*)
	printf 'error: crate name must not begin with - (got: %s)\n' "$crate" >&2
	exit 1
	;;
esac

# Validate crate against workspace members.
if ! cargo metadata --format-version 1 --no-deps 2>/dev/null |
	grep -q "\"name\":\"$crate\""; then
	printf 'error: %s is not a workspace member\n' "$crate" >&2
	exit 1
fi

# Formatting check. cargo fmt supports -p for package-scoped formatting.
printf '==> checking format for %s ...\n' "$crate"
if ! cargo fmt -p "$crate" -- --check; then
	printf 'error: formatting check failed for %s\n' "$crate" >&2
	exit 1
fi

# Compilation check covering all relevant targets.
printf '==> checking compilation for %s ...\n' "$crate"
if ! cargo check -p "$crate" --locked; then
	printf 'error: compilation check failed for %s\n' "$crate" >&2
	exit 1
fi

# Tests.
if [ -n "$filter" ]; then
	printf '==> running tests for %s (filter: %s) ...\n' "$crate" "$filter"
	# Verify the filter matches at least one test before running, so a
	# misspelled filter cannot be mistaken for success.
	count=$(cargo test -p "$crate" --locked -- --list "$filter" 2>&1 |
		grep -cE ': test$|: benchmark$' || true)
	if [ "$count" -eq 0 ]; then
		printf 'error: filter %s matched no tests in %s\n' "$filter" "$crate" >&2
		exit 1
	fi
	if ! cargo test -p "$crate" --locked -- "$filter"; then
		printf 'error: tests failed for %s (filter: %s)\n' "$crate" "$filter" >&2
		exit 1
	fi
else
	printf '==> running tests for %s ...\n' "$crate"
	if ! cargo test -p "$crate" --locked; then
		printf 'error: tests failed for %s\n' "$crate" >&2
		exit 1
	fi
fi

printf '\ndev-check: OK (%s%s)\n' "$crate" \
	"${filter:+ / filter: $filter}"
printf 'REMINDER: ./scripts/verify.sh is the only canonical completion gate.\n'
printf '          Run it before marking any work complete.\n'
