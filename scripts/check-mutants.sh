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
# recorded in `AGENTS.md` rather than assumed here. verify.sh is the gate: one
# command, seconds, run after every change. This is evidence generation: it
# rebuilds a scratch copy of the workspace once per mutant and takes minutes.
# Folding it into the gate would make the gate something agents avoid running,
# which is a worse outcome than a second command with a stated reason.
#
# For each record it copies the tree to a scratch directory, applies one
# single-line edit, runs the named test, and requires that test to FAIL. A
# mutant the test survives is reported: either the mutant no longer removes the
# property, or the test never checked it.
#
# A shared CARGO_TARGET_DIR across mutants keeps this to an incremental rebuild
# of one crate per record rather than a full workspace build each time.
#
# Uses POSIX shell, standard POSIX utilities, rsync, and the pinned toolchain.
#
# Usage:
#   scripts/check-mutants.sh              run every mutant
#   scripts/check-mutants.sh <name> ...   run only the named mutants

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

manifest=tests/mutants.txt
[ -f "$manifest" ] || {
	printf 'ERROR: %s does not exist.\n' "$manifest" >&2
	exit 1
}

scratch=${TMPDIR:-/tmp}/pantheon-mutants.$$
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
mkdir -p "$scratch/tree"
CARGO_TARGET_DIR="$scratch/target"
export CARGO_TARGET_DIR

# One copy, reused. Each mutant restores the single file it touched rather than
# re-copying the workspace.
rsync -a --exclude target --exclude .git ./ "$scratch/tree/"

selected=$*
survivors=0
checked=0

# Emit each record as one tab-separated line so the shell loop below does not
# have to parse a paragraph format.
records=$(
	awk '
		function flush() {
			if (name != "") {
				if (runs == "") { runs = 3 }
				if (occurrence == "") { occurrence = 1 }
				printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", name, file, find, replace, scope, test, runs, occurrence
			}
			name = ""; file = ""; find = ""; replace = ""; scope = ""; test = ""; runs = ""; occurrence = ""
		}
		/^[[:space:]]*#/ { next }
		/^[[:space:]]*$/ { flush(); next }
		{
			key = $0; sub(/:.*$/, "", key)
			value = $0; sub(/^[^:]*:[[:space:]]?/, "", value)
			if (key == "name") { name = value }
			else if (key == "file") { file = value }
			else if (key == "find") { find = value }
			else if (key == "replace") { replace = value }
			else if (key == "scope") { scope = value }
			else if (key == "test") { test = value }
			else if (key == "runs") { runs = value }
			else if (key == "occurrence") { occurrence = value }
			else { printf "ERROR: unknown key %s in %s\n", key, FILENAME > "/dev/stderr"; exit 1 }
		}
		END { flush() }
	' "$manifest"
)

printf '%s\n' "$records" | while IFS="$(printf '\t')" read -r name file find replace scope test runs occurrence; do
	[ -n "$name" ] || continue
	if [ -n "$selected" ]; then
		case " $selected " in
		*" $name "*) ;;
		*) continue ;;
		esac
	fi

	[ -f "$file" ] || {
		printf 'ERROR [%s]: %s does not exist.\n' "$name" "$file" >&2
		exit 1
	}

	matches=$(grep -cF "$find" "$file" || true)
	if [ "$matches" -eq 0 ]; then
		printf 'ERROR [%s]: the anchor was not found in %s.\nThe code moved; update the mutant so it still removes the property it names.\n' \
			"$name" "$file" >&2
		exit 1
	fi
	if [ "$matches" -lt "$occurrence" ]; then
		printf 'ERROR [%s]: %s has %s matches, fewer than the requested occurrence %s.\n' \
			"$name" "$file" "$matches" "$occurrence" >&2
		exit 1
	fi
	# Apply to the copy, then run.
	#
	# `find` and `replace` travel through the environment rather than through
	# `awk -v`, because awk expands escape sequences in a `-v` assignment: an
	# anchor containing `\"` would silently stop matching, the mutant would not
	# be applied, and the run would look like a surviving mutant rather than a
	# broken one.
	cp "$file" "$scratch/tree/$file"
	MUTANT_FIND=$find MUTANT_REPLACE=$replace MUTANT_WANT=$occurrence awk '
		BEGIN { find = ENVIRON["MUTANT_FIND"]; replace = ENVIRON["MUTANT_REPLACE"]; want = ENVIRON["MUTANT_WANT"] + 0; seen = 0 }
		{
			if (!done && index($0, find) > 0) {
				seen++
				if (seen == want) {
					before = substr($0, 1, index($0, find) - 1)
					after = substr($0, index($0, find) + length(find))
					print before replace after
					done = 1
					next
				}
			}
			print
		}
	' "$file" >"$scratch/tree/$file.mutant"
	mv "$scratch/tree/$file.mutant" "$scratch/tree/$file"

	# A mutant that changed nothing is not a surviving mutant, it is a broken
	# one — and reporting it as a survivor would send someone hunting a test
	# weakness that does not exist. Fail closed.
	if cmp -s "$file" "$scratch/tree/$file"; then
		printf 'ERROR [%s]: applying the mutant changed nothing.\nThe anchor matched the raw grep but not the substitution, so the run would have reported a surviving mutant for a mutant that was never applied.\n' \
			"$name" >&2
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
	printf 'ERROR: no mutants ran.%s\n' "${selected:+ No manifest entry matched: $selected}" >&2
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
