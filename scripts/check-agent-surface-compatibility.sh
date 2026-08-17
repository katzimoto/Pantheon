#!/bin/sh
# Upstream compatibility observation for the external coding-agent surfaces
# Pantheon intentionally supports (Claude Code, Codex CLI, OpenCode, and the
# Agent Skills format). Reports each tested capability as pass, known
# limitation, or unsupported-to-test, names the tested version, and fails only
# when a documented upstream-contract assumption Pantheon relies on is
# violated. The compatibility matrix and the tested boundary are recorded in
# docs/development/agent-skills-and-hooks.md.
#
# Deliberately separate from ./scripts/verify.sh. The surfaces monitored here
# change independently of Pantheon, so this is never an unconditional
# correctness gate and is not wired into verify.sh; the workflow in
# .github/workflows/agent-surface-compatibility.yml runs it on a schedule and
# on demand, and a failure only ever signals that a human should review the
# affected assumption. It does not duplicate the deterministic repository-local
# checks (skill bodies, symlink integrity, hook self-tests, documentation
# structure) that ./scripts/verify.sh already owns.
#
# What it checks and against which documented contracts (verified 2026-08-17):
#
#   Claude Code  https://code.claude.com/docs/en/hooks
#     verified events: Stop, PostToolUse (matcher "Edit|Write")
#     verified inputs: hook_event_name, tool_name, tool_input,
#       last_assistant_message; env CLAUDE_PROJECT_DIR; block via exit 2.
#   Codex CLI    https://developers.openai.com/codex/hooks
#     verified event: Stop
#     Stop output {"decision":"block","reason":"..."} with a non-empty reason;
#     activation preconditions [features] codex_hooks = true and a trusted
#     project are user-controlled and recorded, not exercised here.
#   OpenCode     https://opencode.ai/docs/plugins
#     verified hooks: event (event.type "session.idle"), tool.execute.before
#       (documented output.args.filePath; output.args.patchText for apply_patch
#       is a practical field recorded in the repository docs).
#     tool.execute.after is documented as {title, output, metadata} with no
#       file-path field, so it is deliberately not used.
#   Agent Skills https://agentskills.io/specification
#     upstream reference tooling: skills-ref, pinned by immutable version
#     (PyPI skills-ref==0.1.1). A reference/demonstration implementation, so
#     its verdict is a conformance smoke signal, not a production guarantee.
#
# Usage:
#   scripts/check-agent-surface-compatibility.sh [--pin name=version]...
#   scripts/check-agent-surface-compatibility.sh --self-test
#
# Environment:
#   AGENT_COMPAT_TOOLS_DIR   directory where the workflow installs pinned tools;
#                            a marker file "<name>.install-failed" records an
#                            upstream availability failure.
#
# Exit status: 0 when every checked contract holds; 1 when any contract
# assumption is violated. Known limitations and unsupported-to-test results are
# reported but are not failures.

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)

# ---- the verified contract record (machine-readable compatibility matrix) ----

CLAUDE_VERIFIED_EVENTS='Stop PostToolUse'
CODEX_VERIFIED_EVENTS='Stop'
OPENCODE_VERIFIED_EVENT_TYPES='session.idle'

CLAUDE_DOC='https://code.claude.com/docs/en/hooks'
CODEX_DOC='https://developers.openai.com/codex/hooks'
OPENCODE_DOC='https://opencode.ai/docs/plugins'
SKILLS_SPEC='https://agentskills.io/specification'
ANCHOR='docs/development/agent-skills-and-hooks.md'

status=0

report() {
	# $1 = label, $2 = surface, $3 = detail
	printf '[%s] %s: %s\n' "$1" "$2" "$3"
}

pass() { report PASS "$@"; }
limitation() { report 'KNOWN LIMITATION' "$@"; }
unsupported() { report 'UNSUPPORTED-TO-TEST' "$@"; }
fail() {
	report FAIL "$@"
	status=1
}

# capture CMD... — run CMD, leaving stdout+stderr in $__out and its exit code in
# $__rc, without `set -e` aborting the script.
capture() {
	set +e
	__out=$("$@" 2>&1)
	__rc=$?
	set -e
}

# tool_version NAME — first line of `NAME --version`, or empty on failure.
tool_version() {
	set +e
	__ver=$("$1" --version 2>/dev/null | head -n 1)
	__rc=$?
	set -e
	if [ -n "$__ver" ]; then
		__ver=$(printf '%s' "$__ver" | tr -d '\r' | grep -oE '[0-9]+(\.[0-9]+)+' | head -n 1)
		if [ -n "$__ver" ]; then
			printf '%s' "$__ver"
		fi
	fi
}

# hook_event_keys FILE — prints the keys of the object whose own key is exactly
# "hooks" (i.e. the hook event names), one per line. POSIX awk only; no JSON
# tool. Tracks brace depth so an inner `"hooks": [...]` group is not mistaken
# for a hook event, and inner keys such as `command` or `args` are excluded.
hook_event_keys() {
	awk '
		function update_depth(line,   i, n, ch) {
			n = length(line)
			for (i = 1; i <= n; i++) {
				ch = substr(line, i, 1)
				if (ch == "{") depth++
				else if (ch == "}") depth--
			}
		}
		{
			key_depth = depth
			if (match($0, /"[^"]+"/)) {
				key = substr($0, RSTART + 1, RLENGTH - 2)
				rest = substr($0, RSTART + RLENGTH)
				if (key == "hooks" && rest ~ /^[[:space:]]*:[[:space:]]*\{/)
					hooks_depth = key_depth + 1
				if (hooks_depth > 0 && key_depth == hooks_depth &&
				    rest ~ /^[[:space:]]*:[[:space:]]*\[/)
					print key
			}
			update_depth($0)
		}
	' "$1"
}

# check_verified_events SURFACE FILE VERIFIED_EVENTS DOC_URL — every hook event
# wired in FILE must be in Pantheon's verified set, and every verified event
# must be wired.
check_verified_events() {
	surface=$1
	file=$2
	verified=$3
	doc=$4
	ok=1
	[ -f "$file" ] || {
		fail "$surface" "adapter file $file is missing; the wired surface cannot be verified (anchor: $ANCHOR, upstream: $doc)"
		return 1
	}
	events=$(hook_event_keys "$file" | tr '\n' ' ' | sed 's/ $//')
	[ -n "$events" ] || {
		fail "$surface" "no hook events found in $file; expected at least: $verified (anchor: $ANCHOR, upstream: $doc)"
		return 1
	}
	for ev in $events; do
		case " $verified " in
		*" $ev "*) ;;
		*)
			fail "$surface" "hook event '$ev' is not in Pantheon's verified set ($verified). Either it is undocumented upstream or its contract has not been recorded; record it in $ANCHOR and extend the verified set, or remove the wiring (anchor: $file, upstream: $doc)"
			ok=0
			;;
		esac
	done
	for ev in $verified; do
		case " $events " in
		*" $ev "*) ;;
		*)
			fail "$surface" "verified hook event '$ev' is not wired in $file (anchor: $ANCHOR, upstream: $doc)"
			ok=0
			;;
		esac
	done
	[ "$ok" -eq 1 ] &&
		pass "$surface" "wired hook events ($events) match Pantheon's verified set ($verified)"
	return "$ok"
}

# check_cli NAME PIN — attribute the tested version of a vendor CLI. Absence is
# unsupported-to-test, never a fabricated pass.
check_cli() {
	name=$1
	pin=${2:-}
	ver=$(tool_version "$name")
	if [ -n "$ver" ]; then
		pass "$name" "installed and version-recorded (tested ${name}@${ver}${pin:+; declared pin $pin})"
		if [ -n "$pin" ] && [ "$pin" != "$ver" ]; then
			limitation "$name" "installed version $ver differs from the declared pin $pin; the workflow installs the pinned version, local runs report whichever is installed"
		fi
		return 0
	fi
	if [ -n "${AGENT_COMPAT_TOOLS_DIR:-}" ] && [ -f "$AGENT_COMPAT_TOOLS_DIR/$name.install-failed" ]; then
		unsupported "$name" "$(cat "$AGENT_COMPAT_TOOLS_DIR/$name.install-failed"); not exercised this cycle"
		return 0
	fi
	if command -v "$name" >/dev/null 2>&1; then
		limitation "$name" "installed but the version could not be read"
		return 0
	fi
	unsupported "$name" "not installed; the workflow installs a pinned ${name} version and exercises the honestly testable subset (local runs report unsupported-to-test rather than fabricating behavior)"
}

check_agent_skills() {
	surface=agent-skills
	tool=
	for t in skills-ref agentskills; do
		if command -v "$t" >/dev/null 2>&1; then tool=$t; break; fi
	done
	if [ -z "$tool" ]; then
		if [ -n "${AGENT_COMPAT_TOOLS_DIR:-}" ] && [ -f "$AGENT_COMPAT_TOOLS_DIR/skills-ref.install-failed" ]; then
			unsupported "$surface" "$(cat "$AGENT_COMPAT_TOOLS_DIR/skills-ref.install-failed"); conformance not run this cycle"
		else
			unsupported "$surface" "skills-ref is not installed; the workflow installs skills-ref@${PIN_skills_ref:-0.1.1} (PyPI, immutable version) as the spec's reference tooling (spec: $SKILLS_SPEC). Local runs: pip install skills-ref==${PIN_skills_ref:-0.1.1}"
		fi
		return 0
	fi
	ver=$(tool_version "$tool")
	ok=1
	for d in "$root"/.agents/skills/*/; do
		[ -f "$d/SKILL.md" ] || continue
		capture "$tool" validate "${d%/}"
		if [ "$__rc" -ne 0 ]; then
			printf '%s\n' "$__out"
			fail "$surface" "$tool validate rejected ${d%/} (tool $tool@$ver)"
			ok=0
		fi
	done
	[ "$ok" -eq 1 ] &&
		pass "$surface" "every canonical .agents/skills/<name>/SKILL.md passes $tool validate (tested $tool@$ver)"
	limitation "$surface" "skills-ref is the specification's reference implementation ('demonstration purposes only'); its verdict is a conformance smoke signal against $SKILLS_SPEC, not a production guarantee"
}

check_claude_code() {
	surface=claude-code
	settings=$root/.claude/settings.json
	ok=1
	check_verified_events "$surface" "$settings" "$CLAUDE_VERIFIED_EVENTS" "$CLAUDE_DOC" || ok=0
	if [ -f "$settings" ]; then
		grep -qF 'Edit|Write' "$settings" ||
			{ fail "$surface" "PostToolUse matcher is not the documented 'Edit|Write' (anchor: $settings, upstream: $CLAUDE_DOC)"; ok=0; }
		grep -qF '${CLAUDE_PROJECT_DIR}' "$settings" ||
			{ fail "$surface" "adapter does not use the documented CLAUDE_PROJECT_DIR project-relative path (anchor: $settings, upstream: $CLAUDE_DOC)"; ok=0; }
		grep -qF 'check-stale-verification.sh' "$settings" ||
			{ fail "$surface" "Stop hook does not reference scripts/hooks/check-stale-verification.sh (anchor: $settings)"; ok=0; }
		grep -qF 'narrow-validate.sh' "$settings" ||
			{ fail "$surface" "PostToolUse hook does not reference scripts/hooks/narrow-validate.sh (anchor: $settings)"; ok=0; }
	fi
	[ -f "$root/scripts/hooks/check-stale-verification.sh" ] ||
		{ fail "$surface" "scripts/hooks/check-stale-verification.sh is missing (anchor: $ANCHOR)"; ok=0; }
	[ -f "$root/scripts/hooks/narrow-validate.sh" ] ||
		{ fail "$surface" "scripts/hooks/narrow-validate.sh is missing (anchor: $ANCHOR)"; ok=0; }
	[ "$ok" -eq 1 ] &&
		pass "$surface" "adapter shape matches the documented Claude Code hook contract (Stop, PostToolUse 'Edit|Write', \${CLAUDE_PROJECT_DIR})"
	limitation "$surface" "session-level skill discovery and hook firing are not exercised: they require an authenticated interactive Claude Code session; documented shapes are verified statically and no payload fields are fabricated"
	check_cli claude "${PIN_claude:-}"
}

check_codex_cli() {
	surface=codex-cli
	hooks=$root/.codex/hooks.json
	ok=1
	check_verified_events "$surface" "$hooks" "$CODEX_VERIFIED_EVENTS" "$CODEX_DOC" || ok=0
	if [ -f "$hooks" ]; then
		grep -qF 'check-stale-verification-codex.sh' "$hooks" ||
			{ fail "$surface" "Stop hook does not reference scripts/hooks/check-stale-verification-codex.sh (anchor: $hooks)"; ok=0; }
	fi
	[ -f "$root/scripts/hooks/check-stale-verification-codex.sh" ] ||
		{ fail "$surface" "scripts/hooks/check-stale-verification-codex.sh is missing (anchor: $ANCHOR)"; ok=0; }
	[ "$ok" -eq 1 ] &&
		pass "$surface" "adapter shape matches the documented Codex Stop-hook contract ({'decision':'block','reason':'...'}, non-empty reason)"
	limitation "$surface" "Stop-hook firing requires the user-controlled [features] codex_hooks = true flag and a trusted project; those activation preconditions are outside this repository, so firing is not exercised here and the two gates are recorded, not simulated"
	check_cli codex "${PIN_codex:-}"
}

check_opencode_file() {
	surface=opencode
	plugin=$1
	ok=1
	[ -f "$plugin" ] || {
		fail "$surface" "plugin file $plugin is missing (anchor: $ANCHOR, upstream: $OPENCODE_DOC)"
		return 1
	}
	grep -qF '"tool.execute.before"' "$plugin" ||
		{ fail "$surface" "the returned hooks do not include tool.execute.before (anchor: $plugin, upstream: $OPENCODE_DOC)"; ok=0; }
	grep -qF '"tool.execute.after"' "$plugin" &&
		{ fail "$surface" "the returned hooks use tool.execute.after; its documented output shape ({title, output, metadata}) has no file-path field, so using it would be an undocumented-field guess (anchor: $plugin)"; ok=0; }
	grep -qF 'output.args.filePath' "$plugin" ||
		{ fail "$surface" "tool.execute.before does not read the documented output.args.filePath field (anchor: $plugin, upstream: $OPENCODE_DOC)"; ok=0; }
	grep -qF 'output.args.patchText' "$plugin" ||
		{ fail "$surface" "apply_patch handling does not read output.args.patchText; GPT-series sessions substitute apply_patch for edit/write and narrow validation would go blind without it (anchor: $ANCHOR)"; ok=0; }
	grep -qF 'narrow-validate.sh' "$plugin" ||
		{ fail "$surface" "plugin does not dispatch scripts/hooks/narrow-validate.sh (anchor: $plugin)"; ok=0; }
	grep -qF 'check-stale-verification.sh' "$plugin" ||
		{ fail "$surface" "plugin does not dispatch scripts/hooks/check-stale-verification.sh (anchor: $plugin)"; ok=0; }
	for t in $(grep -oE 'event\.type === "[^"]+"' "$plugin" | sed 's/.*"\([^"]*\)".*/\1/'); do
		case " $OPENCODE_VERIFIED_EVENT_TYPES " in
		*" $t "*) ;;
		*)
			fail "$surface" "event hook keys off '$t', which is not in the verified documented event types ($OPENCODE_VERIFIED_EVENT_TYPES); keying off an unverified event would be an invented contract (anchor: $plugin, upstream: $OPENCODE_DOC)"
			ok=0
			;;
		esac
	done
	[ "$ok" -eq 1 ] &&
		pass "$surface" "plugin uses only the documented event hook (event.type '$OPENCODE_VERIFIED_EVENT_TYPES') and tool.execute.before; no blocking parity with Claude/Codex is claimed"
}

check_opencode() {
	check_opencode_file "$root/.opencode/plugins/pantheon-hooks.js"
	limitation "opencode" "plugin events require a live session and are not exercised here; the adapter can warn but cannot reliably block (documented in the portability boundary), so no blocking parity is claimed or tested"
	check_cli opencode "${PIN_opencode:-}"
}

check_workflow_file() {
	surface=workflow-placement
	wf=$1
	ok=1
	[ -f "$wf" ] || {
		fail "$surface" "compatibility workflow $wf is missing (anchor: $ANCHOR)"
		return 1
	}
	grep -qE '^[[:space:]]*on:' "$wf" ||
		{ fail "$surface" "workflow has no 'on:' trigger (anchor: $wf)"; ok=0; }
	grep -qE '^[[:space:]]*(workflow_dispatch|schedule)' "$wf" ||
		{ fail "$surface" "workflow is not driven by schedule and/or workflow_dispatch; a non-blocking compatibility signal must be scheduled or manually dispatched, never a required PR check (anchor: $wf)"; ok=0; }
	if grep -qE '^[[:space:]]*pull_request' "$wf"; then
		fail "$surface" "workflow triggers on pull_request, which would make it a blocker for ordinary unrelated PRs; keep it scheduled/manual so a compatibility failure never gates a PR (anchor: $wf)"
		ok=0
	fi
	[ "$ok" -eq 1 ] &&
		pass "$surface" "workflow is schedule + workflow_dispatch driven and does not trigger on pull_request; compatibility failures stay non-blocking for ordinary PRs"
}

check_workflow_placement() {
	check_workflow_file "$root/.github/workflows/agent-surface-compatibility.yml"
}

usage() {
	echo "usage: scripts/check-agent-surface-compatibility.sh [--pin name=version]..."
	echo "       scripts/check-agent-surface-compatibility.sh --self-test"
}

main() {
	mode=matrix
	while [ "$#" -gt 0 ]; do
		case "$1" in
		--pin)
			shift
			[ "$#" -ge 1 ] || { echo "--pin needs name=version (one of claude, codex, opencode, skills-ref)" >&2; exit 2; }
			case "$1" in
			claude=*) PIN_claude=${1#claude=} ;;
			codex=*) PIN_codex=${1#codex=} ;;
			opencode=*) PIN_opencode=${1#opencode=} ;;
			skills-ref=*) PIN_skills_ref=${1#skills-ref=} ;;
			*) echo "unknown pin: $1 (expected one of claude=, codex=, opencode=, skills-ref=)" >&2; exit 2 ;;
			esac
			shift
			;;
		--self-test) mode=self-test; shift ;;
		-h | --help) usage; exit 0 ;;
		*) usage >&2; exit 2 ;;
		esac
	done

	if [ "$mode" = self-test ]; then
		run_self_test
		exit "$status"
	fi

	echo "Agent surface compatibility — checked against documented upstream contracts"
	echo "Repository: $root"
	echo "Matrix and assumption anchors: $ANCHOR"
	echo "Contract references (verified 2026-08-17):"
	echo "  Claude Code   $CLAUDE_DOC"
	echo "  Codex CLI     $CODEX_DOC"
	echo "  OpenCode      $OPENCODE_DOC"
	echo "  Agent Skills  $SKILLS_SPEC"
	echo

	check_agent_skills
	check_claude_code
	check_codex_cli
	check_opencode
	check_workflow_placement

	echo
	if [ "$status" -eq 0 ]; then
		echo "compatibility: no contract violation found (known limitations and unsupported-to-test results are recorded above, not failures)"
	else
		echo "compatibility: FAIL — at least one verified contract assumption is violated; see the [FAIL] diagnostics above" >&2
	fi
	exit "$status"
}

# run_self_test — positive controls against the real repository files, then
# controlled negative fixtures (built in a disposable scratch directory) that
# change one adapter/spec assumption each and must be caught. No vendor tools
# are needed.
run_self_test() {
	status=0
	selftest_status=0
	tmp=$(mktemp -d)
	trap 'rm -rf "$tmp"' EXIT

	pos_ok() {
		before=$status
		"$@" >/dev/null 2>&1 || true
		if [ "$status" -ne "$before" ]; then
			printf 'self-test: FAIL positive control: %s (a real repository file failed its own check)\n' "$*" >&2
			selftest_status=1
		fi
		status=$before
	}

	neg_case() {
		desc=$1
		shift
		before=$status
		"$@" >/dev/null 2>&1 || true
		if [ "$status" -gt "$before" ]; then
			printf 'self-test: OK   %s (violation caught)\n' "$desc"
		else
			printf 'self-test: FAIL %s (violation was NOT caught)\n' "$desc" >&2
			selftest_status=1
		fi
		status=$before
	}

	pos_ok check_claude_code
	pos_ok check_codex_cli
	pos_ok check_opencode
	pos_ok check_workflow_placement

	mkdir -p "$tmp/claude" "$tmp/codex" "$tmp/opencode" "$tmp/wf"

	# Negative 1: Claude wiring an invented (undocumented/unverified) hook event.
	cat > "$tmp/claude/settings.json" <<'EOF'
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          { "type": "command", "command": "${CLAUDE_PROJECT_DIR}/scripts/hooks/check-stale-verification.sh" }
        ]
      }
    ],
    "MadeUpEvent": [
      { "hooks": [ { "type": "command", "command": "echo hi" } ] }
    ]
  }
}
EOF
	neg_case "claude-code invented hook event is rejected" \
		check_verified_events claude-code "$tmp/claude/settings.json" "$CLAUDE_VERIFIED_EVENTS" "$CLAUDE_DOC"

	# Negative 2: Claude losing the relied-on PostToolUse wiring.
	cat > "$tmp/claude/settings.json" <<'EOF'
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          { "type": "command", "command": "${CLAUDE_PROJECT_DIR}/scripts/hooks/check-stale-verification.sh" }
        ]
      }
    ]
  }
}
EOF
	neg_case "claude-code missing PostToolUse wiring is rejected" \
		check_verified_events claude-code "$tmp/claude/settings.json" "$CLAUDE_VERIFIED_EVENTS" "$CLAUDE_DOC"

	# Negative 3: Codex wiring an invented hook event.
	cat > "$tmp/codex/hooks.json" <<'EOF'
{
  "hooks": {
    "Stop": [
      { "command": "sh -c 'exec scripts/hooks/check-stale-verification-codex.sh'" }
    ],
    "FabricatedEvent": [
      { "command": "echo hi" }
    ]
  }
}
EOF
	neg_case "codex-cli invented hook event is rejected" \
		check_verified_events codex-cli "$tmp/codex/hooks.json" "$CODEX_VERIFIED_EVENTS" "$CODEX_DOC"

	# Negative 4: OpenCode keying off an unverified event type.
	cat > "$tmp/opencode/plugin.js" <<'EOF'
export const P = async () => {
	return {
		event: async ({ event }) => {
			if (event.type === "session.fabricated") {
				console.error("unverified event");
			}
		},
		"tool.execute.before": async (input, output) => {
			if (input.tool === "apply_patch" && typeof output.args.patchText === "string") {}
			if (typeof output.args.filePath === "string") {}
		},
	};
};
EOF
	neg_case "opencode unverified event type is rejected" \
		check_opencode_file "$tmp/opencode/plugin.js"

	# Negative 5: OpenCode guessing an undocumented tool.execute.after field.
	cat > "$tmp/opencode/plugin-after.js" <<'EOF'
export const P = async () => {
	return {
		event: async ({ event }) => {
			if (event.type === "session.idle") {}
		},
		"tool.execute.after": async (input, output) => {
			if (typeof output.filePath === "string") {}
		},
		"tool.execute.before": async (input, output) => {
			if (typeof output.args.filePath === "string") {}
		},
	};
};
EOF
	neg_case "opencode tool.execute.after file-path guess is rejected" \
		check_opencode_file "$tmp/opencode/plugin-after.js"

	# Negative 6: workflow gaining a pull_request trigger would gate ordinary PRs.
	cat > "$tmp/wf/compat.yml" <<'EOF'
name: compat
on:
  workflow_dispatch:
  push:
    branches: [main]
  pull_request:
  schedule:
    - cron: '0 3 * * 1'
EOF
	neg_case "workflow with a pull_request trigger is rejected" \
		check_workflow_file "$tmp/wf/compat.yml"

	if [ "$selftest_status" -eq 0 ]; then
		echo "compatibility self-test: OK (positive controls pass; six negative fixtures caught)"
		status=0
	else
		echo "compatibility self-test: FAIL — see diagnostics above" >&2
		status=1
	fi
}

main "$@"