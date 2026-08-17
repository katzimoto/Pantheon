// Best-effort OpenCode adapter for Pantheon's shared lifecycle hook scripts
// (scripts/hooks/). This is intentionally the thinnest of the three vendor
// adapters — see docs/development/agent-skills-and-hooks.md for why, and
// for the parts of Issue #21's contract this adapter cannot fully honor.
//
// No hook logic lives here beyond dispatch: both events below shell out to
// the same scripts/hooks/*.sh that .claude/settings.json wires for Claude
// Code, so there is exactly one implementation of the actual checks.
//
// Known limitation (documented, not silently assumed away): OpenCode's
// public plugin documentation names the `tool.execute.after` and
// `session.idle` event hooks but does not publish a stable payload schema
// for either at the time this was written. This adapter reads several
// plausible field-name candidates for a changed file path defensively and
// no-ops when none match, and it never blocks on `session.idle` — it can
// only warn — because blocking would require reliably detecting a
// docs/development/change-lifecycle.md handoff in the same turn, which is
// not something this event's documented payload confirms exposing. The
// authoritative, fully-specified implementation of the stale-verification
// guardrail is the Claude Code Stop hook; treat this file as best-effort
// early feedback on OpenCode, not an equivalent guarantee.

export const PantheonHooks = async (ctx) => {
	const { directory, $ } = ctx;
	const hooksDir = `${directory}/scripts/hooks`;

	function candidateFilePath(input, output) {
		const candidates = [
			input && input.filePath,
			input && input.file_path,
			input && input.args && input.args.filePath,
			input && input.args && input.args.file_path,
			output && output.filePath,
			output && output.file_path,
		];
		return candidates.find((c) => typeof c === "string" && c.length > 0);
	}

	return {
		"tool.execute.after": async (input, output) => {
			const filePath = candidateFilePath(input, output);
			if (!filePath) return;
			const payload = JSON.stringify({
				tool_input: { file_path: filePath },
			});
			try {
				await $`sh -c ${`printf '%s' ${JSON.stringify(payload)} | "${hooksDir}/narrow-validate.sh"`}`.quiet();
			} catch (err) {
				// PostToolUse-equivalent: never blocks. Surface the narrow
				// validator's findings the same way Claude Code shows them.
				console.error(String(err));
			}
		},

		"session.idle": async () => {
			try {
				const result =
					await $`"${hooksDir}/tree-fingerprint.sh" ${directory}`.quiet();
				const current = String(result.stdout).trim();
				const recorded = await $`cat "${directory}/.git/pantheon/verified-tree"`
					.quiet()
					.nothrow();
				const status = await $`git -C ${directory} status --porcelain`
					.quiet()
					.nothrow();
				const dirty = String(status.stdout || "").trim().length > 0;
				if (dirty && String(recorded.stdout || "").trim() !== current) {
					console.error(
						"pantheon-change-verification: working tree has uncommitted, " +
							"unverified changes. Run ./scripts/verify.sh before treating " +
							"this as complete (this OpenCode adapter can only warn, not " +
							"block — see docs/development/agent-skills-and-hooks.md).",
					);
				}
			} catch {
				// Fail open: this is a best-effort warning, not a gate.
			}
		},
	};
};
