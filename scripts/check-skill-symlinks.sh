#!/bin/sh
# Verify Pantheon's one-canonical-skill-body contract (Issue #21): every
# skill lives once, under .agents/skills/<name>/SKILL.md, and every
# vendor-specific skill directory that needs its own path is a symlink back
# to that canonical file — never a second real, independently editable copy.
#
# Usage: scripts/check-skill-symlinks.sh   (run from anywhere in the repository)

set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

status=0
canonical_dir=".agents/skills"

if [ ! -d "$canonical_dir" ]; then
	echo "missing canonical skill directory: $canonical_dir" >&2
	exit 1
fi

# Every canonical skill must have valid frontmatter with a name matching its
# directory, per the Agent Skills specification (agentskills.io/specification).
for skill_dir in "$canonical_dir"/*/; do
	[ -d "$skill_dir" ] || continue
	name=$(basename "$skill_dir")
	skill_file="$skill_dir/SKILL.md"
	if [ ! -f "$skill_file" ]; then
		echo "$skill_dir: missing SKILL.md" >&2
		status=1
		continue
	fi
	# awk rather than sed range addressing: portable across BSD and GNU sed
	# variants, which differ on regex-range/negation extensions.
	frontmatter_name=$(awk '/^---$/{n++; next} n==1 && /^name: /{sub(/^name: */,""); print; exit}' "$skill_file")
	if [ "$frontmatter_name" != "$name" ]; then
		printf '%s: frontmatter name "%s" does not match directory name "%s"\n' \
			"$skill_file" "$frontmatter_name" "$name" >&2
		status=1
	fi
done

# Vendor directories that must be symlinks resolving into the canonical
# directory, never a real independently-editable SKILL.md. OpenCode and
# Codex CLI both discover .agents/skills/ natively and need no entry here;
# Claude Code only looks in its own .claude/skills/, so that is the one
# vendor directory this mechanism has to bridge.
for vendor_dir in ".claude/skills"; do
	[ -d "$vendor_dir" ] || continue
	for entry in "$vendor_dir"/*; do
		# -e follows symlinks and is false for a broken one, so check -L
		# first: a broken symlink is exactly one of the problems this script
		# exists to catch, not something to skip past.
		[ -e "$entry" ] || [ -L "$entry" ] || continue
		name=$(basename "$entry")
		if [ ! -L "$entry" ]; then
			printf '%s: not a symlink (real skill bodies belong only in %s)\n' \
				"$entry" "$canonical_dir" >&2
			status=1
			continue
		fi
		link_target=$(readlink "$entry")
		target=$(cd "$(dirname "$entry")" && cd "$link_target" 2>/dev/null && pwd) || target=""
		expected=$(cd "$canonical_dir/$name" 2>/dev/null && pwd) || expected=""
		if [ -z "$target" ] || [ "$target" != "$expected" ]; then
			printf '%s: symlink does not resolve to %s/%s\n' \
				"$entry" "$canonical_dir" "$name" >&2
			status=1
		fi
	done
done

if [ "$status" -eq 0 ]; then
	count=$(find "$canonical_dir" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
	echo "skill symlink check: OK ($count canonical skills)"
fi
exit "$status"
