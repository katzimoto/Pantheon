#!/bin/sh
# Prove that Pantheon's tests are load-bearing, by breaking the code on purpose.
#
# `./scripts/verify.sh` establishes that the tree is sound. It cannot establish
# that the tests would notice if it stopped being sound — a test that asserts a
# constant against itself, or checks a readable prefix instead of the identity
# underneath it, passes either way. Every entry in `tests/mutants.txt` exists
# because a real surviving mutant exposed exactly that.
#
# This is deliberately NOT part of `./scripts/verify.sh`, and that exception is
# recorded in AGENTS.md rather than assumed here. verify.sh is the gate: one
# command, seconds, run after every change. This is evidence generation: it
# rebuilds a scratch copy of the workspace once per mutant and takes minutes.
# Folding it into the gate would make the gate something agents avoid running,
# which is a worse outcome than a second command with a stated reason.
#
# The suite runs in two phases.
#
# Preflight — always first, over the COMPLETE manifest, even when individual
# mutants were selected by name: every record's shape, target file, anchor and
# occurrence are validated structurally, and each mutation is proven to change
# its target, without compiling or running anything. A stale record therefore
# fails in seconds with a diagnostic naming the record and the file, instead of
# surfacing partway through an hour of scratch rebuilds (#82).
#
# Execution — for each selected record: apply the mutation to the scratch tree
# through the same engine that validated it (`scripts/mutants.awk`), run the
# named test, and require that test to FAIL. A mutant the test survives is
# reported: either the mutant no longer removes the property, or the test never
# checked it.
#
# Anchors match in whitespace-normalized form: runs of blanks and line breaks
# collapse to one space on both sides before comparison, so reformatting a
# source with rustfmt — re-indentation, line wrapping — does not invalidate a
# record whose anchor still names the same token sequence. Normalization never
# widens what a mutation replaces: the matched region maps back onto exact
# original coordinates, only that contiguous region is replaced, occurrence N
# selects the Nth non-overlapping match deterministically, and ambiguity stays
# visible through reported match counts.
#
# A shared CARGO_TARGET_DIR across mutants keeps execution to an incremental
# rebuild of one crate per record rather than a full workspace build each time.
#
# Uses POSIX shell, standard POSIX utilities, rsync, and the pinned toolchain.
#
# Usage:
#   scripts/check-mutants.sh              preflight everything, run every mutant
#   scripts/check-mutants.sh <name> ...   preflight everything, run only these
#   scripts/check-mutants.sh --check      structural validation only; no build
#
# Environment overrides, for hermetic self-tests of the harness itself:
#   MUTANTS_ROOT      repository root (default: derived from this script)
#   MUTANTS_MANIFEST  manifest path relative to the root

set -eu

root=${MUTANTS_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}
cd "$root"

manifest=${MUTANTS_MANIFEST:-tests/mutants.txt}
[ -f "$manifest" ] || {
	printf 'ERROR: %s does not exist.\n' "$manifest" >&2
	exit 1
}
engine=scripts/mutants.awk
[ -f "$engine" ] || {
	printf 'ERROR: %s does not exist.\n' "$engine" >&2
	exit 1
}

scratch=${TMPDIR:-/tmp}/pantheon-mutants.$$
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
mkdir -p "$scratch"
CARGO_TARGET_DIR="$scratch/target"
export CARGO_TARGET_DIR

check_only=0
selected=
for arg in "$@"; do
	case $arg in
	--check) check_only=1 ;;
	*) selected="$selected $arg" ;;
	esac
done

# Parse once, through the engine every later step shares. Unknown keys,
# duplicate names, missing defaults: all fail closed here.
records=$(MUTANT_MODE=parse awk -f "$engine" "$manifest")
[ -n "$records" ] || {
	printf 'ERROR: %s contains no mutant records.\n' "$manifest" >&2
	exit 1
}

total=$(printf '%s\n' "$records" | awk 'END { print NR }')

# ---- phase one: structural preflight of the whole manifest -----------------

preflight_failed=$scratch/preflight.failed
: >"$preflight_failed"

printf '%s\n' "$records" | while IFS="$(printf '\t')" read -r name file find replace scope test runs occurrence; do
	if [ -z "$name" ] || [ -z "$file" ] || [ -z "$find" ] || [ -z "$scope" ] || [ -z "$test" ]; then
		printf 'ERROR [%s]: the record is missing a required field; name, file, find, scope and test must all be present.\n' \
			"${name:-<unnamed>}" >&2
		echo "${name:-<unnamed>}" >>"$preflight_failed"
		continue
	fi
	if [ ! -f "$file" ]; then
		printf 'ERROR [%s]: %s does not exist.\n' "$name" "$file" >&2
		echo "$name" >>"$preflight_failed"
		continue
	fi
	# The engine resolves the anchor against the real file and proves the
	# mutation would change bytes. Its diagnostics name both on failure;
	# nothing is written anywhere.
	if ! MUTANT_MODE=check MUTANT_NAME="$name" MUTANT_FIND="$find" \
		MUTANT_REPLACE="$replace" MUTANT_WANT="$occurrence" \
		awk -f "$engine" "$file" >/dev/null; then
		echo "$name" >>"$preflight_failed"
	fi
done

if [ -s "$preflight_failed" ]; then
	printf 'ERROR: %s of %s records failed structural validation; nothing was built or tested.\n' \
		"$(wc -l <"$preflight_failed" | tr -d ' ')" "$total" >&2
	printf 'Repair the records above, then rerun. `--check` reruns just this phase.\n' >&2
	exit 1
fi

if [ "$check_only" -eq 1 ]; then
	printf 'mutation preflight: OK (%s records structurally valid; nothing built or tested)\n' "$total"
	exit 0
fi

# ---- phase two: execution ---------------------------------------------------

mkdir -p "$scratch/tree"

# One copy, reused. Each mutant restores the single file it touched rather
# than re-copying the workspace.
rsync -a --exclude target --exclude .git ./ "$scratch/tree/"

survivors=0
checked=0

printf '%s\n' "$records" | while IFS="$(printf '\t')" read -r name file find replace scope test runs occurrence; do
	[ -n "$name" ] || continue
	if [ -n "$selected" ]; then
		case " $selected " in
		*" $name "*) ;;
		*) continue ;;
		esac
	fi

	# Apply through the same engine that preflighted the record, so
	# validation and execution cannot drift apart: identical parsing,
	# matching, occurrence selection and replacement.
	cp "$file" "$scratch/tree/$file"
	if ! MUTANT_MODE=apply MUTANT_NAME="$name" MUTANT_FIND="$find" \
		MUTANT_REPLACE="$replace" MUTANT_WANT="$occurrence" \
		awk -f "$engine" "$file" >"$scratch/tree/$file.mutant"; then
		printf 'ERROR [%s]: application failed after a passing preflight; aborting before any test ran.\n' "$name" >&2
		exit 1
	fi
	mv "$scratch/tree/$file.mutant" "$scratch/tree/$file"

	# A mutant that changed nothing is not a surviving mutant, it is a broken
	# one — and reporting it as a survivor would send someone hunting a test
	# weakness that does not exist. Fail closed. Unreachable while preflight
	# and application share one engine, kept as the guard that proves it.
	if cmp -s "$file" "$scratch/tree/$file"; then
		printf 'ERROR [%s]: applying the mutant changed nothing.\n' "$name" >&2
		exit 1
	fi

	killed=1
	run=1
	while [ "$run" -le "$runs" ]; do
		if (cd "$scratch/tree" && cargo test --locked $scope "$test" >"$scratch/out" 2>&1); then
			passed=1
		else
			passed=0
		fi
		# Distinguish a test that failed from a build that failed: a mutant
		# that does not compile proves nothing about the test.
		if [ "$passed" -eq 0 ] &&
			grep -qE '^error(\[|:)' "$scratch/out" &&
			! grep -q '^test result: FAILED' "$scratch/out"; then
			printf 'ERROR [%s]: the mutant does not compile, so it proves nothing.\n' "$name" >&2
			sed -n '/^error/,+6p' "$scratch/out" | head -12 >&2
			exit 1
		fi
		# Both outcomes are meaningless unless the named test actually ran, so
		# this is checked before the pass/fail decision rather than inside the
		# failure branch. `cargo test` exits 0 for a filter that matches
		# nothing: a renamed or mistyped `test:` key would otherwise take the
		# pass branch and be reported as a surviving mutant, sending someone
		# hunting a test weakness that does not exist.
		if ! grep -q "^test .*$test" "$scratch/out"; then
			printf 'ERROR [%s]: no test matching `%s` ran under `%s`.\n' "$name" "$test" "$scope" >&2
			exit 1
		fi
		if [ "$passed" -eq 1 ]; then
			killed=0
			break
		fi
		run=$((run + 1))
	done

	# Restore the pristine file for the next mutant.
	cp "$file" "$scratch/tree/$file"

	checked=$((checked + 1))
	if [ "$killed" -eq 1 ]; then
		printf 'killed    %s\n' "$name"
	else
		printf 'SURVIVED  %s -- `%s` passes with the mutant applied\n' "$name" "$test" >&2
		survivors=$((survivors + 1))
	fi
	printf '%s %s\n' "$killed" "$name" >>"$scratch/results"
done

# The loop above runs in a subshell because of the pipe, so the tallies are
# read back from the file it wrote rather than from variables.
[ -f "$scratch/results" ] || {
	printf 'ERROR: no mutants ran.%s\n' "${selected:+ No manifest entry matched:$selected}" >&2
	exit 1
}
total=$(wc -l <"$scratch/results" | tr -d ' ')
survived=$(awk '$1 == 0' "$scratch/results" | wc -l | tr -d ' ')

if [ "$survived" -ne 0 ]; then
	printf '\nmutation check: %s of %s mutants SURVIVED.\nA surviving mutant means the named test does not actually check the property it claims to. Strengthen the test — do not weaken the mutant.\n' \
		"$survived" "$total" >&2
	exit 1
fi

printf '\nmutation check: OK (%s mutants, all killed)\n' "$total"
