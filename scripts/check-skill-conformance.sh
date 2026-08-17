#!/bin/sh
# Validate canonical Agent Skills against the stable, machine-checkable
# constraints of the Agent Skills specification
# (https://agentskills.io/specification). This is the deterministic half of
# the skill evaluation layer; the probabilistic behavioral half lives in
# `scripts/run-skill-evals.py` and is deliberately not part of
# `./scripts/verify.sh`.
#
# Enforced on every `.agents/skills/<name>/SKILL.md`:
#
#   - SKILL.md opens with the `---` frontmatter delimiter on line 1 and has a
#     matching closing `---` followed by a non-empty Markdown body.
#   - `name` is present exactly once, 1-64 characters, lowercase letters and
#     digits separated by single hyphens (no leading/trailing/double hyphen),
#     and matches the skill's directory name.
#   - `description` is present exactly once, non-empty, and at most 1024
#     characters.
#   - `compatibility`, when present, is at most 500 characters.
#   - `metadata`, when present, is a flat map from string keys to string
#     values (no nested lists or maps).
#   - No two canonical skills share a `name` (duplicate identity).
#
# `scripts/check-skill-symlinks.sh` separately enforces the one-canonical-body
# and vendor-symlink rules; this script does not duplicate them, and it does
# not inspect evaluation fixtures (`evals/evals.json`), whose shape is owned by
# the behavioral harness. Both scripts run in `./scripts/verify.sh`.
#
# `--self-test` runs the same validator against a disposable scratch tree
# holding one conforming skill and one deliberately broken skill per failure
# class, so the negative cases are proven to be rejected rather than merely
# claimed. It needs nothing beyond this repository and POSIX utilities.
#
# Uses POSIX shell and standard POSIX utilities only (awk, sed, sort, uniq,
# wc). No interpreter, package manager or external tooling.
#
# Usage:
#   scripts/check-skill-conformance.sh             # validate .agents/skills
#   scripts/check-skill-conformance.sh --self-test # validate the validator

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# Parse one SKILL.md and print either a single `E: <reason>` line on failure,
# or the extracted fields on success:
#
#   NAME:<value>
#   DESC:<value>
#   COMPAT:<value>        (only when the optional field is present)
#
# Field syntax is validated in shell below; this awk pass owns delimiter,
# presence, uniqueness and metadata-shape checks.
parse_one() {
	awk '
		NR==1 && $0 != "---" { print "E: missing opening frontmatter delimiter (---) on line 1"; exit }
		NR==1 { in_fm=1; next }
		in_fm {
			if ($0 == "---") { in_fm=0; closed=1; next }
			if ($0 ~ /^[[:space:]]*$/) next
			if ($0 ~ /^[A-Za-z0-9_-]+:/) {
				in_meta = 0
				if ($0 ~ /^name:[[:space:]]*/) {
					if (name_seen) { print "E: duplicate name field"; exit }
					name_seen = 1
					v = $0; sub(/^name:[[:space:]]*/, "", v); name = v
				} else if ($0 ~ /^description:[[:space:]]*/) {
					if (desc_seen) { print "E: duplicate description field"; exit }
					desc_seen = 1
					v = $0; sub(/^description:[[:space:]]*/, "", v); desc = v
				} else if ($0 ~ /^compatibility:[[:space:]]*/) {
					if (compat_seen) { print "E: duplicate compatibility field"; exit }
					compat_seen = 1
					v = $0; sub(/^compatibility:[[:space:]]*/, "", v); compat = v
				} else if ($0 ~ /^metadata:[[:space:]]*$/) {
					in_meta = 1
				}
				next
			}
			if (in_meta) {
				if ($0 ~ /^[[:space:]]+[A-Za-z0-9_-]+:[[:space:]]*[^[:space:]]/) next
				print "E: metadata must map string keys to scalar string values"
				exit
			}
			print "E: unexpected indented line outside a metadata block"
			exit
		}
		!in_fm {
			if (closed && $0 !~ /^[[:space:]]*$/) body = 1
		}
		END {
			if (!closed) { print "E: missing closing frontmatter delimiter (---)"; exit }
			if (!name_seen) { print "E: missing required name field"; exit }
			if (!desc_seen) { print "E: missing required description field"; exit }
			if (!body) { print "E: empty Markdown body after frontmatter"; exit }
			print "NAME:" name
			print "DESC:" desc
			if (compat_seen) print "COMPAT:" compat
		}
	' "$1"
}

# validate_one <file> <dirname> <name-output-file>
# Checks one skill. Prints nothing on success; on failure prints an ERROR line
# to stderr. Appends the validated name to name-output-file for duplicate
# detection. Returns 0 on success, 1 on failure.
validate_one() {
	file=$1
	dirname=$2
	names=$3

	out=$(parse_one "$file")

	case "$out" in
	E:*) printf 'ERROR: %s: %s\n' "$file" "${out#E: }" >&2; return 1 ;;
	esac

	[ -n "$out" ] || { printf 'ERROR: %s: could not parse frontmatter\n' "$file" >&2; return 1; }

	fm_name=$(printf '%s\n' "$out" | sed -n 's/^NAME://p')
	fm_desc=$(printf '%s\n' "$out" | sed -n 's/^DESC://p')
	fm_compat=$(printf '%s\n' "$out" | sed -n 's/^COMPAT://p')

	# Record the claimed identity before judging it: duplicate detection must
	# see names that also fail a per-skill check (a name mismatch or bad format
	# is exactly how a duplicate identity shows up).
	[ -n "$fm_name" ] && printf '%s\n' "$fm_name" >>"$names"

	bad=0
	if ! printf '%s' "$fm_name" | grep -Eq '^[a-z0-9]+(-[a-z0-9]+)*$'; then
		printf 'ERROR: %s: name "%s" is not lowercase letters/digits with single hyphens\n' \
			"$file" "$fm_name" >&2
		bad=1
	elif [ "${#fm_name}" -gt 64 ]; then
		printf 'ERROR: %s: name is longer than 64 characters\n' "$file" >&2
		bad=1
	elif [ "$fm_name" != "$dirname" ]; then
		printf 'ERROR: %s: frontmatter name "%s" does not match directory name "%s"\n' \
			"$file" "$fm_name" "$dirname" >&2
		bad=1
	fi

	if [ -z "$fm_desc" ]; then
		printf 'ERROR: %s: description is empty\n' "$file" >&2
		bad=1
	elif [ "${#fm_desc}" -gt 1024 ]; then
		printf 'ERROR: %s: description is longer than 1024 characters\n' "$file" >&2
		bad=1
	fi

	if [ -n "$fm_compat" ] && [ "${#fm_compat}" -gt 500 ]; then
		printf 'ERROR: %s: compatibility is longer than 500 characters\n' "$file" >&2
		bad=1
	fi

	[ "$bad" -eq 0 ] || return 1
	return 0
}

# validate_dir <dir>
# Validates every <dir>/*/SKILL.md, then reports duplicate names. Returns 0
# when every skill conforms, 1 otherwise. Uses `rc` locally: POSIX sh has no
# `local`, and the caller's own `status` accumulator must not be clobbered.
validate_dir() {
	dir=$1
	names=$(mktemp)
	trap 'rm -f "$names"' EXIT HUP INT TERM

	[ -d "$dir" ] || { printf 'ERROR: missing canonical skill directory: %s\n' "$dir" >&2; return 1; }

	rc=0
	for skill_dir in "$dir"/*/; do
		[ -d "$skill_dir" ] || continue
		name=$(basename "$skill_dir")
		skill_file="$skill_dir/SKILL.md"
		if [ ! -f "$skill_file" ]; then
			printf 'ERROR: %s: missing SKILL.md\n' "$skill_dir" >&2
			rc=1
			continue
		fi
		validate_one "$skill_file" "$name" "$names" || rc=1
	done

	# Duplicate names: two directories whose frontmatter claims the same name.
	dups=$(sort "$names" | uniq -d)
	if [ -n "$dups" ]; then
		printf 'ERROR: duplicate skill name(s): %s\n' "$(printf '%s' "$dups" | tr '\n' ' ')" >&2
		rc=1
	fi

	rm -f "$names"
	trap - EXIT HUP INT TERM
	return "$rc"
}

self_test() {
	tmp=$(mktemp -d)
	trap 'rm -rf "$tmp"' EXIT

	write_case() {
		mkdir -p "$tmp/$1"
		cat >"$tmp/$1/SKILL.md"
	}

	# Conforming skill: accepted.
	write_case skills-valid/ok-skill <<'EOF'
---
name: ok-skill
description: A conforming skill with a name, description and scalar metadata.
metadata:
  pantheon-authority: procedural-guidance-only
---

# Ok skill

A non-empty body.
EOF

	# One broken skill per failure class. Each must be rejected.
	write_case skills-broken/no-desc <<'EOF'
---
name: no-desc
---

Body.
EOF

	write_case skills-broken/name-mismatch <<'EOF'
---
name: some-other-name
description: Name does not match the directory.
---

Body.
EOF

	write_case skills-broken/upper-name <<'EOF'
---
name: Upper-Case
description: Uppercase is not allowed in a skill name.
---

Body.
EOF

	write_case skills-broken/double-hyphen <<'EOF'
---
name: bad--name
description: Consecutive hyphens are not allowed.
---

Body.
EOF

	write_case skills-broken/empty-body <<'EOF'
---
name: empty-body
description: A body with no non-blank content.
---
EOF

	write_case skills-broken/no-close <<'EOF'
---
name: no-close
description: Missing the closing frontmatter delimiter.
EOF

	write_case skills-broken/meta-list <<'EOF'
---
name: meta-list
description: Metadata value must be a scalar string, not a list.
metadata:
  tags:
    - a
    - b
---

Body.
EOF

	# 1025-character description.
	long_desc=$(awk 'BEGIN { for (i=0;i<1025;i++) printf "a" }')
	write_case skills-broken/desc-too-long <<EOF
---
name: desc-too-long
description: $long_desc
---

Body.
EOF

	# 501-character compatibility value.
	long_compat=$(awk 'BEGIN { for (i=0;i<501;i++) printf "b" }')
	write_case skills-broken/compat-too-long <<EOF
---
name: compat-too-long
description: Compatibility over the length cap.
compatibility: $long_compat
---

Body.
EOF

	# Duplicate identity: two directories under one tree claim the same name.
	mkdir -p "$tmp/skills-collide/first" "$tmp/skills-collide/second"
	cat >"$tmp/skills-collide/first/SKILL.md" <<'EOF'
---
name: persistence-review
description: First directory claiming this name.
---

Body.
EOF
	cat >"$tmp/skills-collide/second/SKILL.md" <<'EOF'
---
name: persistence-review
description: Second directory claiming the same name.
---

Body.
EOF

	status=0

	# Valid set accepted.
	if ! validate_dir "$tmp/skills-valid" >/dev/null 2>&1; then
		printf 'FAIL: conforming skill was rejected\n' >&2
		status=1
	fi

	# Every broken class rejected. Counting the ERROR lines proves each class
	# was actually exercised rather than silently skipped.
	if broken_out=$(validate_dir "$tmp/skills-broken" 2>&1); then
		printf 'FAIL: broken skills were accepted\n' >&2
		status=1
	else
		expected=$(find "$tmp/skills-broken" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
		actual=$(printf '%s\n' "$broken_out" | grep -c '^ERROR:' || true)
		if [ "$actual" -ne "$expected" ]; then
			printf 'FAIL: expected %s broken-class errors, saw %s\n' "$expected" "$actual" >&2
			status=1
		fi
	fi

	# Duplicate identity rejected *as* a duplicate, not merely as two name
	# mismatches. Grep for the exact validator message so a path that merely
	# contains the word "duplicate" cannot satisfy it.
	if dups_out=$(validate_dir "$tmp/skills-collide" 2>&1); then
		printf 'FAIL: duplicate identities were accepted\n' >&2
		status=1
	elif ! printf '%s\n' "$dups_out" | grep -q 'duplicate skill name'; then
		printf 'FAIL: duplicate identities not reported as duplicate\n' >&2
		status=1
	fi

	rm -rf "$tmp"
	trap - EXIT

	if [ "$status" -eq 0 ]; then
		echo "skill conformance self-test: OK (valid accepted, malformed classes rejected)"
	fi
	return "$status"
}

if [ "${1:-}" = "--self-test" ]; then
	self_test
else
	# An optional explicit directory makes the validator testable against a
	# scratch tree; `./scripts/verify.sh` calls it with no argument.
	dir=${1:-.agents/skills}
	if validate_dir "$dir"; then
		count=$(find "$dir" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
		echo "skill conformance check: OK ($count canonical skills)"
	else
		exit 1
	fi
fi
