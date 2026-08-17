// OpenCode adapter for Pantheon's shared lifecycle hook scripts
// (scripts/hooks/). No decision logic lives here — every branch below only
// parses OpenCode's own event/tool-call shapes into the same synthetic JSON
// payload scripts/hooks/*.sh already accepts from Claude Code, then shells
// out. See docs/development/agent-skills-and-hooks.md for why this is the
// thinnest of the three vendor adapters and what it cannot do.
//
// Directory: OpenCode discovers project-local plugins at
// .opencode/plugins/ (plural) — https://opencode.ai/docs/plugins/.
//
// Event/argument shapes used below are taken from that page as written:
//   - the generic `event` hook receiving `{ event }` with `event.type`
//     (the page's own session.idle example uses this pattern, not a
//     dedicated top-level "session.idle" hook key);
//   - `tool.execute.before`: `(input, output)` where `input.tool` names the
//     tool and `output.args.filePath` is documented for file-reading/
//     editing tools (shown in the page's own .env-blocking example).
// `tool.execute.after`'s documented output shape is `{ title, output,
// metadata }` with no file-path field, so it is not used here — using it
// would mean guessing an undocumented field, which this adapter does not
// do. `tool.execute.before` fires ahead of the write landing on disk, so
// narrow validation here is an early advisory nudge, not a true post-edit
// check the way Claude Code's PostToolUse is; that timing difference is a
// real, accepted limitation of this adapter, not an oversight.
//
// GPT-series models make OpenCode substitute `apply_patch` for `edit`/
// `write`; that tool carries its target path(s) inside `output.args.patchText`
// as `*** Add File: <path>` / `*** Update File: <path>` / `*** Delete File:
// <path>` marker lines rather than `output.args.filePath`. Both shapes are
// handled below so narrow validation does not silently go blind under a
// GPT-backed session.

export const PantheonHooks = async (ctx) => {
	const { directory, $ } = ctx;
	const hooksDir = `${directory}/scripts/hooks`;

	async function dispatchNarrowValidate(filePath) {
		const payload = JSON.stringify({ tool_input: { file_path: filePath } });
		try {
			await $`printf '%s' ${payload} | ${hooksDir}/narrow-validate.sh`.quiet();
		} catch (err) {
			// PostToolUse-equivalent: never blocks. Surface findings the same
			// way Claude Code shows narrow-validate.sh's stderr to the agent.
			console.error(String(err));
		}
	}

	function patchTextPaths(patchText) {
		if (typeof patchText !== "string") return [];
		const paths = [];
		const marker = /^\*\*\* (?:Add|Update|Delete) File: (.+)$/gm;
		let match;
		while ((match = marker.exec(patchText)) !== null) {
			paths.push(match[1].trim());
		}
		return paths;
	}

	return {
		event: async ({ event }) => {
			if (event.type === "session.idle") {
				// Dispatch-only: all decision logic (fingerprint comparison,
				// dirty-tree handling) lives in check-stale-verification.sh.
				// No `last_assistant_message` equivalent is available from this
				// event per OpenCode's documented payload, so a handoff cannot
				// be detected here; this can only warn, never block — see
				// docs/development/agent-skills-and-hooks.md's portability
				// boundary table for why that is an accepted, documented gap
				// rather than a guess.
				try {
					await $`printf '{}' | ${hooksDir}/check-stale-verification.sh`.quiet();
				} catch {
					console.error(
						"pantheon-change-verification: working tree may be stale/" +
							"unverified. Run ./scripts/verify.sh before treating this " +
							"as complete (this OpenCode adapter can only warn, not " +
							"block — see docs/development/agent-skills-and-hooks.md).",
					);
				}
			}
		},

		"tool.execute.before": async (input, output) => {
			const args = output && output.args;
			if (!args) return;

			if (input.tool === "apply_patch" && typeof args.patchText === "string") {
				for (const path of patchTextPaths(args.patchText)) {
					await dispatchNarrowValidate(path);
				}
				return;
			}

			if (typeof args.filePath === "string") {
				await dispatchNarrowValidate(args.filePath);
			}
		},
	};
};
