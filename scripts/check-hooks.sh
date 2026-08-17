#!/bin/sh
# Self-test for Pantheon's lifecycle hook scripts (Issue #21 acceptance
# criteria: representative trigger/non-trigger scenarios recorded/executable,
# and the two required positive/negative demonstrations). Runs entirely
# against a disposable scratch Git repository; never touches this
# repository's own state.
#
# Usage: scripts/check-hooks.sh   (run from anywhere in the repository)

set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
hooks="$root/scripts/hooks"

status=0
fail() {
	printf 'FAIL: %s\n' "$1" >&2
	status=1
}

# Runs "$@", capturing its exit code without `set -e` aborting the script.
run_rc() {
	set +e
	"$@" >/tmp/pantheon-check-hooks.$$ 2>&1
	rc=$?
	set -e
	cat /tmp/pantheon-check-hooks.$$
	rm -f /tmp/pantheon-check-hooks.$$
	return "$rc"
}

stop_hook() {
	# $1 = last_assistant_message text (already JSON-escaped by the caller)
	printf '{"hook_event_name":"Stop","last_assistant_message":"%s"}' "$1" |
		CLAUDE_PROJECT_DIR="$tmp" "$hooks/check-stale-verification.sh"
}

tmp=$(mktemp -d)
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT

git init -q "$tmp"
git -C "$tmp" config user.email test@example.com
git -C "$tmp" config user.name test
echo "hello" >"$tmp/a.txt"
git -C "$tmp" add a.txt
git -C "$tmp" commit -qm init

# --- Scenario 1: verified tree A -> agent changes state to tree B ->
# completion attempt detects stale verification -> reverification required.

"$hooks/record-verified.sh" "$tmp" >/dev/null

rc=0
run_rc stop_hook "Done." >/dev/null || rc=$?
[ "$rc" -eq 0 ] ||
	fail "clean, just-verified tree: Stop hook blocked but should not have (exit $rc)"

echo "world" >>"$tmp/a.txt"

rc=0
run_rc stop_hook "Done, ready to merge." >/dev/null || rc=$?
[ "$rc" -eq 2 ] ||
	fail "changed tree, no re-verify, no handoff: Stop hook should block with exit 2, got $rc"

# --- Scenario 2: interrupted/blocked work still terminates through the
# handoff contract, without the hook falsely reporting completion.

rc=0
run_rc stop_hook '## Handoff\n\n### Remaining\nFinish X.' >/dev/null || rc=$?
[ "$rc" -eq 0 ] ||
	fail "dirty tree with a ## Handoff message: Stop hook should not block, got exit $rc"

# Re-verifying the changed tree clears the block.
"$hooks/record-verified.sh" "$tmp" >/dev/null
rc=0
run_rc stop_hook "Done." >/dev/null || rc=$?
[ "$rc" -eq 0 ] ||
	fail "tree re-verified after change: Stop hook should not block, got exit $rc"

# --- Scenario 3: committing an unverified change must not read as a clean,
# verified tree (regression test for the commit-bypass finding: a dirty tree
# that gets committed without re-verifying is still stale, not a fresh
# clean baseline).

echo "more" >>"$tmp/a.txt"
git -C "$tmp" add a.txt
git -C "$tmp" commit -qm "unverified change, committed without re-verifying"

rc=0
run_rc stop_hook "Done, ready to merge." >/dev/null || rc=$?
[ "$rc" -eq 2 ] ||
	fail "committed-but-unverified change on an otherwise clean tree: Stop hook should still block with exit 2, got $rc"

rc=0
run_rc stop_hook '## Handoff\n\n### Remaining\nFinish X.' >/dev/null || rc=$?
[ "$rc" -eq 0 ] ||
	fail "committed-but-unverified change with a ## Handoff message: Stop hook should not block, got exit $rc"

"$hooks/record-verified.sh" "$tmp" >/dev/null
rc=0
run_rc stop_hook "Done." >/dev/null || rc=$?
[ "$rc" -eq 0 ] ||
	fail "tree re-verified after the committed change: Stop hook should not block, got exit $rc"

# --- Scenario 4: a checkout where ./scripts/verify.sh has never once
# succeeded is out of scope for this drift check and must not block every
# turn (see lib.sh: pantheon_tree_matches_last_verified).

never_verified=$(mktemp -d)
git init -q "$never_verified"
git -C "$never_verified" config user.email test@example.com
git -C "$never_verified" config user.name test
echo x >"$never_verified/f.txt"
git -C "$never_verified" add f.txt
git -C "$never_verified" commit -qm init
rc=0
printf '{"hook_event_name":"Stop","last_assistant_message":"Done."}' |
	CLAUDE_PROJECT_DIR="$never_verified" run_rc "$hooks/check-stale-verification.sh" >/dev/null || rc=$?
rm -rf "$never_verified"
[ "$rc" -eq 0 ] ||
	fail "never-verified checkout should not block on every turn, got exit $rc"

# --- Scenario 5: a sensitive-file edit dispatches its relevant narrow
# validator; an unrelated edit dispatches nothing (no full-suite work on
# every edit). PANTHEON_HOOK_DRY_RUN avoids requiring the pinned Rust
# toolchain here; the dispatched checks themselves run for real elsewhere in
# ./scripts/verify.sh.

dispatch_for() {
	printf '{"tool_name":"Edit","tool_input":{"file_path":"%s"}}' "$1" |
		PANTHEON_HOOK_DRY_RUN=1 CLAUDE_PROJECT_DIR="$root" "$hooks/narrow-validate.sh"
}

out=$(dispatch_for "crates/pantheon-store/Cargo.toml")
case "$out" in
*check-crate-deps*) ;;
*) fail "Cargo.toml edit should dispatch check-crate-deps, got: $out" ;;
esac

out=$(dispatch_for "docs/architecture/overview.md")
case "$out" in
*check-docs-links*) ;;
*) fail "docs/*.md edit should dispatch check-docs-links, got: $out" ;;
esac

out=$(dispatch_for ".github/workflows/rust.yml")
case "$out" in
*check-action-pins*) ;;
*) fail ".github/workflows/*.yml edit should dispatch check-action-pins, got: $out" ;;
esac

out=$(dispatch_for "crates/pantheon-core/src/lib.rs")
[ -z "$out" ] ||
	fail "unrelated Rust source edit should dispatch nothing, got: $out"

if [ "$status" -eq 0 ]; then
	echo "hooks self-test: OK (stale-verification block/unblock, handoff bypass, narrow-validator dispatch)"
fi
exit "$status"
