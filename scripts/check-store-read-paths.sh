#!/bin/sh
# Validate that every public method on `pantheon-store`'s `Store` has a caller
# outside test code.
#
# Twice in Pantheon's history an independent review found the same defect: a
# durable fact was stored, a read path for it existed, and nothing ever called
# it — so the fence the schema implied did not exist. #24's materialization
# compared Goal revision *numbers* while `goal_revision_json` sat unread and
# the recorded `proposal_digest` was never consulted. Both were invisible to
# the compiler, because a `pub fn` with a caller in another crate is not dead
# code, and invisible to the suite, because tests were the only callers.
#
# This makes that shape mechanical. It does not decide whether a fence is
# needed — a reviewer does that. It refuses to let a read path exist without
# someone having said, in writing, why nothing calls it.
#
# The allowlist is the point. An entry is not a dismissal but a statement of
# what the method is for and what is expected to consume it, so an unconsumed
# read path is tracked debt rather than an oversight. A growing list means read
# paths are being written ahead of the code that needs them, which
# `docs/development/implementation.md` already argues against for dependencies
# and which applies just as well here.
#
# Scope is deliberately narrow: `impl Store` in `pantheon-store`, because that
# is where the defect appeared and where a durable fact becomes reachable.
# Widening it to every crate would drown the signal in constructors and
# accessors.
#
# Two conventions this repository already follows make the scan reliable, and
# the check fails loudly rather than silently if either stops holding:
#
#   - an `impl Store` block opens with `impl Store {` at column zero and closes
#     with `}` at column zero;
#   - a test-only block is preceded by `#[cfg(test)]` at column zero.
#
# Two limitations are deliberate, and stated here rather than discovered later:
#
#   - A caller is matched by method name, not by receiver type. A same-named
#     method on another type — `Writer::revision_of` beside `Store::revision_of`
#     — is counted as a caller of the `Store` one. Resolving the receiver would
#     need type inference this script has no business doing, so instead the
#     stale-entry error prints the matching lines: the direction that would
#     otherwise go silent is a maintainer removing an allowlist entry the tool
#     called stale, and they now see which line made it say so.
#   - Integration tests under `crates/*/tests/` are not callers, for the same
#     reason unit tests are not: a read path exercised only by a test that was
#     written for it is exactly the shape this check exists to find. So a
#     method reached only from `crates/pantheon-store/tests/` is still
#     uncalled, and still needs an allowlist entry with its reason.
#
# Uses POSIX shell and standard POSIX utilities (find, awk, grep, sort, comm).
#
# Usage: scripts/check-store-read-paths.sh   (run from anywhere in the repository)

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# Public methods with no production caller, each with the reason it exists.
#
#   revision_of               Reads a row's revision through the read-only
#                             connection. Its own documentation steers callers
#                             to `Writer::revision_of` whenever the decision is
#                             made inside the transaction that acts on it,
#                             which is every current caller. It stays for the
#                             observe-then-attempt case no code has yet.
#   planning_operation        The durable PlanningOperation record.
#                             Materialization re-reads it inside its own write
#                             transaction rather than through this accessor.
#                             Consumed by the mission that exposes planning
#                             history.
#   planning_record_proposal  The immutable recorded proposal.
#                             `materialize_plan` compares its digest inside the
#                             write transaction, which is the only place that
#                             comparison can be authoritative; this accessor
#                             exists for the same planning-history surface.
allowlist() {
	cat <<'EOF'
planning_operation
planning_record_proposal
revision_of
ensure_test_run
nonreleased_sandbox_inventory
EOF
}

tmp=${TMPDIR:-/tmp}/pantheon-store-read-paths.$$
trap 'rm -f "$tmp".*' EXIT HUP INT TERM

find crates/pantheon-store/src -name '*.rs' -exec awk '
	$0 == "impl Store {" {
		if (previous != "#[cfg(test)]") { inside = 1 }
		previous = $0
		next
	}
	inside && $0 == "}" { inside = 0; previous = $0; next }
	inside && /^[ \t]*pub (const |async )?fn [a-z_]+/ {
		name = $0
		sub(/^[ \t]*pub (const |async )?fn /, "", name)
		sub(/[^a-z_].*$/, "", name)
		# Test-only inherent methods follow the repository naming convention.
		if (name !~ /_for_test$/) { print name }
	}
	{ previous = $0 }
' {} + | sort -u >"$tmp.declared"

if [ ! -s "$tmp.declared" ]; then
	printf 'ERROR: no public Store methods were found.\nThe `impl Store {` / `}` column-zero convention this scan relies on has changed; fix scripts/check-store-read-paths.sh rather than assuming there is nothing to check.\n' >&2
	exit 1
fi

# Production sources: every Rust file that is not a test module or an
# integration test. The three shapes a test module takes in this workspace are
# `tests.rs`, a `*_tests.rs` sibling, and `test_support.rs`; all are declared
# under `#[cfg(test)]` and none is production code.
find crates -name '*.rs' \
	! -name 'tests.rs' ! -name '*_tests.rs' ! -name 'test_support.rs' \
	! -path '*/tests/*' >"$tmp.sources"

: >"$tmp.uncalled"
while IFS= read -r fn; do
	[ -n "$fn" ] || continue
	# A method is invoked either through a value (`.name(`) or as an
	# associated function (`Store::name(`). Both count; the declaration reads
	# `fn name(` and matches neither.
	if ! grep -hE "(\.|Store::)$fn\(" $(cat "$tmp.sources") >/dev/null 2>&1; then
		printf '%s\n' "$fn" >>"$tmp.uncalled"
	fi
done <"$tmp.declared"

sort -u "$tmp.uncalled" >"$tmp.uncalled.sorted"
allowlist | sort -u >"$tmp.allowed"

status=0

# 1. An uncalled method that is not accounted for.
comm -23 "$tmp.uncalled.sorted" "$tmp.allowed" >"$tmp.unlisted"
while IFS= read -r fn; do
	[ -n "$fn" ] || continue
	printf 'ERROR: pantheon-store Store::%s has no caller outside test code.\nA durable read path nothing calls is a fence that does not exist. Call it from the code that needs the fact, remove it, or add it to the allowlist in scripts/check-store-read-paths.sh with the reason it still exists.\n' \
		"$fn" >&2
	status=1
done <"$tmp.unlisted"

# 2. An allowlist entry that is now called, or has stopped existing. Either way
#    the reason recorded for it is no longer true.
#
#    The caller lines are printed rather than described, because the scan
#    matches on method name alone and a same-named method on another type looks
#    identical to it (see the limitation in the header). Reading the actual
#    lines is what tells a maintainer whether the entry is really stale, or
#    whether the receiver is a `Writer` and the entry must stay.
comm -13 "$tmp.uncalled.sorted" "$tmp.allowed" >"$tmp.stale"
while IFS= read -r fn; do
	[ -n "$fn" ] || continue
	printf 'ERROR: the allowlist in scripts/check-store-read-paths.sh names %s, which now has a caller or no longer exists.\nRemove the entry, and its recorded reason, so the list keeps describing only real debt.\n' \
		"$fn" >&2
	if grep -nHE "(\.|Store::)$fn\(" $(cat "$tmp.sources") >"$tmp.callers" 2>/dev/null &&
		[ -s "$tmp.callers" ]; then
		printf 'Matching lines — confirm the receiver is a `Store` before removing the entry:\n' >&2
		head -5 "$tmp.callers" >&2
	else
		printf 'No matching lines: the method no longer exists.\n' >&2
	fi
	status=1
done <"$tmp.stale"

[ "$status" -eq 0 ] || exit 1

printf 'store read path check: OK (%s public Store methods, %s allowlisted without a caller)\n' \
	"$(wc -l <"$tmp.declared" | tr -d ' ')" \
	"$(wc -l <"$tmp.allowed" | tr -d ' ')"
