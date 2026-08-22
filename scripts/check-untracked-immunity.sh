#!/bin/sh
# Prove that canonical verification depends only on the candidate repository
# tree (#94).
#
# A checker that discovers files by walking the workspace can be sent sideways
# by ambient untracked state — editor droppings, generated output, a vendored
# dependency tree — and then a perfectly valid candidate fails locally because
# of files no pull request could ever contain. This stage is the regression
# fence for that property, in the direction a unit test cannot reach: it builds
# an isolated copy of exactly the candidate tree, fills the copy with hostile
# untracked decoys, and runs the affected checks *from the copy*, using the
# candidate's own checker scripts. If any check consumes anything untracked,
# this stage fails for the intended reason; if discovery ever drifts back to
# workspace walking, this stage catches it before review has to.
#
# The decoys deliberately include shapes with distinct contamination paths:
#
#   .decoy/node_modules/broken.md            hidden directory AND vendored-
#                                            package spelling mandated by #94;
#   notes-drafts/draft-notes.md              plain non-hidden ambient Markdown
#                                            with broken references (the shape
#                                            that reproduced the failure);
#   docs/architecture/zz-drafts/…            an untracked file where the
#                                            architecture map inventories
#                                            contracts, which would otherwise
#                                            be reported as unlisted.
#
# Scope: only the documentation and dependency checks run here, never
# ./scripts/verify.sh itself — this stage is part of verify.sh, and recursion
# would make the gate depend on itself.
#
# Cleanup is fail-safe: the scratch path is constructed from TMPDIR and the
# process id, matched against that exact pattern before deletion, and removed
# by a trap installed for EXIT, HUP, INT and TERM.
#
# Uses POSIX shell and ordinary existing prerequisites already required by
# scripts/verify.sh (git, the pinned toolchain via cargo). No new package,
# runtime or task runner.
#
# Usage: scripts/check-untracked-immunity.sh    (run from anywhere)

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

scratch=${TMPDIR:-/tmp}/pantheon-untracked-immunity.$$

cleanup() {
	# Delete only the path this process constructed: non-empty, not a bare
	# slash, and matching our own namespace pattern.
	case $scratch in
	*/pantheon-untracked-immunity.*) rm -rf "$scratch" ;;
	esac
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$scratch/tree"

# The candidate tree: every tracked file, byte-faithful from the working
# tree, and nothing else. Untracked workspace state is precisely what this
# check exists to prove irrelevant, so it is excluded here structurally —
# by construction, not by an exclusion list that could rot.
git ls-files >"$scratch/candidate.list"
[ -s "$scratch/candidate.list" ] || {
	echo "untracked immunity: git ls-files listed nothing; refusing to validate an empty candidate" >&2
	exit 1
}
: >"$scratch/materialized.list"
: >"$scratch/materialized.nul"
while IFS= read -r f; do
	# A path can be tracked while its working-tree file is absent (staged
	# deletion); there are simply no bytes to copy for it, and the scratch's
	# index must not name it either.
	case $f in
	*/*) mkdir -p "$scratch/tree/${f%/*}" ;;
	esac
	if [ -L "$f" ]; then
		# Symlinks are part of the candidate's shape (the agent skills are
		# symlinks): recreate them rather than copying what they point at.
		ln -s "$(readlink "$f")" "$scratch/tree/$f"
	elif [ -f "$f" ]; then
		cp "$f" "$scratch/tree/$f"
	else
		continue
	fi
	printf '%s\n' "$f" >>"$scratch/materialized.list"
	printf '%s\0' "$f" >>"$scratch/materialized.nul"
done <"$scratch/candidate.list"

# Hostile untracked state, planted only inside the isolated copy. Every file
# here carries references that would fail validation if any checker consumed
# untracked content.
mkdir -p \
	"$scratch/tree/.decoy/node_modules" \
	"$scratch/tree/notes-drafts" \
	"$scratch/tree/docs/architecture/zz-drafts"
printf '# broken decoy\nsee [nothing](missing-target.md)\nref `docs/nope.md`\n' \
	>"$scratch/tree/.decoy/node_modules/broken.md"
printf '# draft notes\nlink: `scripts/no-such-check.sh`\nand [gone](also-gone.md)\n' \
	>"$scratch/tree/notes-drafts/draft-notes.md"
printf '# draft contract\n' >"$scratch/tree/docs/architecture/zz-drafts/draft-contract.md"

# Make the copy workspace-shaped: the candidate's checkers define their scope
# through git, so the scratch becomes a git repository whose tracked set is
# exactly the materialized candidate files — the decoys stay untracked,
# faithfully playing the role ambient state plays in a developer workspace.
# --force because a tracked file may sit under a path its own .gitignore
# ignores; the index here is constructed from an explicit allowlist, not from
# git's opinion about what matters.
(
	cd "$scratch/tree"
	git init --quiet
	git add --force --pathspec-from-file="$scratch/materialized.nul" --pathspec-file-nul
)

status=0
if ! (cd "$scratch/tree" && ./scripts/check-docs-links.sh); then
	printf 'untracked immunity: the documentation check consumed untracked state inside the isolated candidate\n' >&2
	status=1
fi

if [ "$status" -eq 0 ]; then
	if ! (cd "$scratch/tree" && ./scripts/check-crate-deps.sh); then
		printf 'untracked immunity: the crate dependency check failed inside the isolated candidate\n' >&2
		status=1
	fi
fi

if [ "$status" -eq 0 ]; then
	echo "untracked immunity: OK (documentation and dependency checks unaffected by hostile untracked decoys)"
fi
exit "$status"
