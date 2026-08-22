#!/bin/sh
# Hermetic regression evidence for the mutant-record harness (#82).
#
# Every property the mission names is proven here against synthetic fixtures
# under a scratch directory — never against working-tree state, never with a
# real build:
#
#   formatting resilience  an anchor authored against one rustfmt shape still
#                          resolves and applies after re-indentation or line
#                          wrapping, through the same engine modes and the same
#                          script paths the real suite uses;
#   fast stale failure     an anchor that no longer matches is rejected by the
#                          structural checker with a record-and-file diagnostic,
#                          without building anything;
#   validation ordering    a stale LAST record aborts the suite before any
#                          cargo invocation, proven hermetically by a sentinel
#                          stub on PATH rather than by timing;
#   application parity     a record accepted by preflight applies at the same
#                          occurrence through execution and changes real bytes;
#   diagnostics            unknown keys, duplicate names, missing targets,
#                           occurrence overruns and no-change mutations fail
#                          closed with precise messages;
#   counting semantics     comments and string-like text count like code —
#                          deterministically and visibly, so authors disambiguate
#                          by occurrence rather than by hope;
#   compatibility          every record in the repository's real manifest passes
#                          the fast check.
#
# Usage: scripts/check-mutants-selftest.sh   (seconds; no network, no cargo)

set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
engine=$repo/scripts/mutants.awk
harness=$repo/scripts/check-mutants.sh

tmp=${TMPDIR:-/tmp}/pantheon-mutants-selftest.$$
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
mkdir -p "$tmp"

fail() {
	printf 'SELFTEST FAIL: %s\n' "$*" >&2
	exit 1
}

ok() {
	printf 'ok: %s\n' "$1"
}

# Installs the engine and harness into a fresh fixture root, so the real
# scripts run exactly as they do in production, only elsewhere.
make_root() {
	root=$tmp/$1
	mkdir -p "$root/scripts" "$root/src"
	cp "$engine" "$root/scripts/mutants.awk"
	cp "$harness" "$root/scripts/check-mutants.sh"
}

check_record() {
	# check_record <root> <name> <find> <replace> <occurrence> <file>
	MUTANT_MODE=check MUTANT_NAME="$2" MUTANT_FIND="$3" MUTANT_REPLACE="$4" \
		MUTANT_WANT="$5" awk -f "$root/scripts/mutants.awk" "$6"
}

# ---- parse: values survive the manifest format byte for byte ---------------

make_root parse-torture
cat >"$root/mutants.txt" <<'EOF'
name: weird-values
file: src/code.rs
find: if a  == "b\tc" { # mid-line hash, double space, backslash
replace: if true { /* "quoted" \n stays literal */ }
scope: -p fixture --lib
test: fixture::tests::weird

name: second-record
file: src/code.rs
find: x + 1
replace: x + 2
scope: -p fixture --lib
test: fixture::tests::second
EOF
printf 'fn f() { if a  == "b\\tc" { } }\nfn g() { x + 1 }\n' >"$root/src/code.rs"

parsed=$(MUTANT_MODE=parse awk -f "$engine" "$root/mutants.txt")
[ "$(printf '%s\n' "$parsed" | wc -l | tr -d ' ')" = "2" ] ||
	fail "parse did not emit two records"
field_of() {
	# field_of <record-number> <field-number>
	printf '%s\n' "$parsed" | sed -n "$1p" | awk -F'\t' -v f="$2" 'NR == 1 { print $f }'
}
[ "$(field_of 1 1)" = "weird-values" ] || fail "name did not round-trip"
[ "$(field_of 1 3)" = 'if a  == "b\tc" { # mid-line hash, double space, backslash' ] ||
	fail "find value did not round-trip verbatim: $(field_of 1 3)"
[ "$(field_of 1 4)" = 'if true { /* "quoted" \n stays literal */ }' ] ||
	fail "replace value did not round-trip verbatim: $(field_of 1 4)"
[ "$(field_of 1 7)" = "3" ] && [ "$(field_of 1 8)" = "1" ] ||
	fail "defaults runs=3 occurrence=1 not applied"
[ "$(field_of 2 1)" = "second-record" ] &&
	[ "$(field_of 2 6)" = "fixture::tests::second" ] ||
	fail "second record mis-parsed"
ok "manifest values round-trip (spaces, backslashes, quotes, hashes); defaults applied"

# ---- formatting resilience --------------------------------------------------

authored='fn f(x: i64) -> i64 {
    if x > 0 && x < 100 {
        x * 2
    } else {
        0
    }
}'

reindented='fn f(x: i64) -> i64 {
        if x > 0 && x < 100 {
                x * 2
        } else {
                0
        }
}'

wrapped='fn f(x: i64) -> i64 {
    if x > 0 &&
        x < 100
    {
        x * 2
    } else {
        0
    }
}'

make_root formats
for variant in authored reindented wrapped; do
	eval "body=\$$variant"
	printf '%s\n' "$body" >"$root/src/lib.rs"
	cat >"$root/mutants.txt" <<'EOF'
name: gate-broken
file: src/lib.rs
find: if x > 0 && x < 100 {
replace: if false && x < 100 {
scope: -p fixture --lib
test: fixture::tests::gate
EOF
	MUTANTS_ROOT="$root" MUTANTS_MANIFEST="mutants.txt" \
		"$root/scripts/check-mutants.sh" --check >/dev/null 2>&1 ||
		fail "$variant: structural preflight rejected a whitespace-only reformat"
	applied=$(MUTANT_MODE=apply MUTANT_NAME=gate-broken \
		MUTANT_FIND='if x > 0 && x < 100 {' MUTANT_REPLACE='if false && x < 100 {' \
		MUTANT_WANT=1 awk -f "$engine" "$root/src/lib.rs") ||
		fail "$variant: apply failed where preflight passed (parity broken)"
	printf '%s' "$applied" | grep -qF 'if false && x < 100 {' ||
		fail "$variant: mutation text absent from applied output"
	printf '%s' "$applied" | grep -qF 'x * 2' ||
		fail "$variant: surrounding tokens damaged by application"
done
ok "anchors survive re-indentation and line wrapping (same engine path as the suite)"

# ---- stale anchor fails fast, naming record and file ------------------------

printf '%s\n' 'fn f(x: i64) -> i64 {
    if x >= 0 && x < 100 {
        x * 2
    } else {
        0
    }
}' >"$root/src/lib.rs"

stale_out=$(MUTANTS_ROOT="$root" MUTANTS_MANIFEST="mutants.txt" \
	"$root/scripts/check-mutants.sh" --check 2>&1 >/dev/null) &&
	fail "stale anchor accepted by preflight" || true
printf '%s' "$stale_out" | grep -qF 'gate-broken' ||
	fail "stale-anchor diagnostic does not name the record: $stale_out"
printf '%s' "$stale_out" | grep -qF 'src/lib.rs' ||
	fail "stale-anchor diagnostic does not name the file: $stale_out"
ok "stale anchor rejected in seconds, naming record and file"

# ---- validation ordering: nothing builds behind an incomplete manifest ------

make_root ordering
printf 'fn f() {}\n' >"$root/src/lib.rs"
cat >"$root/mutants.txt" <<EOF
name: early-ok
file: src/lib.rs
find: fn f() {}
replace: fn f() -> i64 {}
scope: -p fixture --lib
test: fixture::tests::early

name: late-stale
file: src/lib.rs
find: this anchor is deliberately gone
replace: x
scope: -p fixture --lib
test: fixture::tests::late
EOF

sentinel=$tmp/cargo.log
stub=$tmp/stub
mkdir -p "$stub"
cat >"$stub/cargo" <<EOF
#!/bin/sh
echo "\$@" >>"$sentinel"
exit 1
EOF
chmod +x "$stub/cargo"

suite_out=$(PATH="$stub:$PATH" MUTANTS_ROOT="$root" MUTANTS_MANIFEST="mutants.txt" \
	"$root/scripts/check-mutants.sh" 2>&1 >/dev/null) &&
	fail "suite completed despite a stale record" || true
[ ! -f "$sentinel" ] ||
	fail "cargo was invoked before complete structural validation"
printf '%s' "$suite_out" | grep -qF 'late-stale' ||
	fail "ordering failure does not name the late stale record: $suite_out"
printf '%s' "$suite_out" | grep -qF 'nothing was built or tested' ||
	fail "ordering failure does not state that nothing was built: $suite_out"
ok "stale last record aborts before any cargo invocation (hermetic sentinel)"

# ---- application parity and occurrence selection ----------------------------

make_root occurrence
printf 'fn a(x: u8) -> u8 {\n    x + 1\n}\n\nfn b(x: u8) -> u8 {\n    x + 1\n}\n' \
	>"$root/src/lib.rs"
counted=$(MUTANT_MODE=check MUTANT_NAME=occ MUTANT_FIND='x + 1' \
	MUTANT_REPLACE='x + 2' MUTANT_WANT=2 awk -f "$engine" "$root/src/lib.rs") ||
	fail "occurrence-2 record failed its own preflight"
[ "$(printf '%s' "$counted" | cut -f1)" = "2" ] ||
	fail "match count not reported as 2: $counted"
applied=$(MUTANT_MODE=apply MUTANT_NAME=occ MUTANT_FIND='x + 1' \
	MUTANT_REPLACE='x + 2' MUTANT_WANT=2 awk -f "$engine" "$root/src/lib.rs") ||
	fail "preflight-accepted record refused at application time"
printf '%s' "$applied" | awk '/^fn a/,/^}/' | grep -qF 'x + 1' ||
	fail "first occurrence was mutated although occurrence 2 was requested"
printf '%s' "$applied" | awk '/^fn b/,/^}/' | grep -qF 'x + 2' ||
	fail "second occurrence was not mutated"
ok "preflight-accepted record applies at the requested occurrence, changing bytes"

# ---- diagnostics ------------------------------------------------------------

diag_fail() {
	# diag_fail <expected-fragment> <command...>
	expected=$1
	shift
	out=$("$@" 2>&1) && fail "expected failure ('$expected') succeeded: $out" || true
	printf '%s' "$out" | grep -qF "$expected" ||
		fail "diagnostic missing '$expected': $out"
}

make_root diags
printf 'fn f() { x + 1 }\n' >"$root/src/lib.rs"

cat >"$root/dup.txt" <<'EOF'
name: dup
file: src/lib.rs
find: x + 1
replace: x + 2
scope: -p f --lib
test: t

name: dup
file: src/lib.rs
find: x + 1
replace: x + 3
scope: -p f --lib
test: t
EOF
diag_fail "duplicate mutant name dup" env MUTANTS_ROOT="$root" MUTANTS_MANIFEST="dup.txt" \
	"$root/scripts/check-mutants.sh" --check

cat >"$root/unknown.txt" <<'EOF'
name: odd-key
file: src/lib.rs
find: x + 1
replace: x + 2
scop: -p f --lib
test: t
EOF
diag_fail "unknown key scop" env MUTANTS_ROOT="$root" MUTANTS_MANIFEST="unknown.txt" \
	"$root/scripts/check-mutants.sh" --check

cat >"$root/missing.txt" <<'EOF'
name: vanished-file
file: src/gone.rs
find: x + 1
replace: x + 2
scope: -p f --lib
test: t
EOF
diag_fail "src/gone.rs does not exist" env MUTANTS_ROOT="$root" MUTANTS_MANIFEST="missing.txt" \
	"$root/scripts/check-mutants.sh" --check

diag_fail "fewer than the requested occurrence 5" env MUTANT_MODE=check \
	MUTANT_NAME=deep MUTANT_FIND='x + 1' MUTANT_REPLACE='x + 2' MUTANT_WANT=5 \
	awk -f "$engine" "$root/src/lib.rs"

diag_fail "applying the mutation changes nothing" env MUTANT_MODE=check \
	MUTANT_NAME=same MUTANT_FIND='x + 1' MUTANT_REPLACE='x + 1' MUTANT_WANT=1 \
	awk -f "$engine" "$root/src/lib.rs"

diag_fail "has an empty anchor" env MUTANT_MODE=check \
	MUTANT_NAME=empty MUTANT_FIND='   ' MUTANT_REPLACE='y' MUTANT_WANT=1 \
	awk -f "$engine" "$root/src/lib.rs"
ok "shape, existence, occurrence and change-proof diagnostics fail closed precisely"

# ---- counting semantics: comments are counted like code, visibly -----------

make_root counting
cat >"$root/src/lib.rs" <<'EOF'
// if x > 0 && x < 100 {
fn f(x: i64) -> i64 {
    if x > 0 && x < 100 {
        x * 2
    } else {
        0
    }
}
EOF
counted=$(MUTANT_MODE=check MUTANT_NAME=ambig MUTANT_FIND='if x > 0 && x < 100 {' \
	MUTANT_REPLACE='if false {' MUTANT_WANT=1 awk -f "$engine" "$root/src/lib.rs") ||
	fail "comment+code ambiguity rejected instead of counted"
[ "$(printf '%s' "$counted" | cut -f1)" = "2" ] ||
	fail "comment line not counted: $counted"
[ "$(printf '%s' "$counted" | cut -f2)" = "1" ] ||
	fail "occurrence 1 does not resolve to the comment line: $counted"
chosen=$(MUTANT_MODE=check MUTANT_NAME=ambig MUTANT_FIND='if x > 0 && x < 100 {' \
	MUTANT_REPLACE='if false {' MUTANT_WANT=2 awk -f "$engine" "$root/src/lib.rs")
[ "$(printf '%s' "$chosen" | cut -f2)" = "3" ] ||
	fail "occurrence 2 does not resolve to the code line: $chosen"
applied=$(MUTANT_MODE=apply MUTANT_NAME=ambig MUTANT_FIND='if x > 0 && x < 100 {' \
	MUTANT_REPLACE='if false {' MUTANT_WANT=2 awk -f "$engine" "$root/src/lib.rs")
printf '%s' "$applied" | sed -n '1p' | grep -qF '// if x > 0' ||
	fail "occurrence 2 disturbed the comment line"
printf '%s' "$applied" | sed -n '3p' | grep -qF 'if false {' ||
	fail "occurrence 2 missed the code line"
ok "comments count like code, visibly, and occurrence disambiguates"

# ---- compatibility: the repository's own records ---------------------------

cd "$repo"
real_names=$(grep -c '^name:' tests/mutants.txt || true)
parsed_real=$(MUTANT_MODE=parse awk -f "$engine" tests/mutants.txt | wc -l | tr -d ' ')
[ "$real_names" = "$parsed_real" ] ||
	fail "parser emitted $parsed_real records for $real_names manifest entries"
"$harness" --check >/dev/null ||
	fail "the repository's own manifest failed the fast structural check"
ok "all $parsed_real repository records pass the fast structural check"

printf '\nselftest: OK\n'
