#!/bin/sh
# Validate that every GitHub Actions reference in this repository is immutable.
#
# A tag or branch is a moving pointer owned by whoever controls the action's
# repository. `actions/checkout@v5` today and `actions/checkout@v5` tomorrow can
# be different code, and nothing in the reference says so. A full-length commit
# SHA is currently the only way to reference an action immutably, so that is
# what Pantheon requires.
#
# Pinning by hand fixes each reference once; nothing stops the next one from
# arriving mutable. That is exactly how `.github/workflows/docs.yml` sat on a
# mutable tag two majors behind the rest of the repository without anyone
# noticing. This script makes the rule mechanical: introducing a mutable
# reference fails verification instead of depending on a reviewer's attention.
#
# It checks three things:
#
#   1. Every reference resolves to an immutable revision. For an action or a
#      reusable workflow that means a full-length 40-character commit SHA; for a
#      `docker://` reference it means a `sha256:` digest. A reference with no
#      revision at all resolves to the default branch, which is mutable too.
#
#   2. Every pinned reference records the release it corresponds to, as a
#      trailing comment. A bare SHA is unreviewable and unupdatable without
#      going and decoding it, so the human-readable name travels with it.
#
#   3. At least one workflow file is scanned, so a rename or a move that makes
#      this check inspect nothing fails rather than reporting success over an
#      empty set.
#
# A reference beginning with `./` is a local action living in this repository.
# It resolves within the commit being run, so it is already immutable and needs
# no revision or release comment.
#
# What this cannot check is whether a recorded release name is the truth: SHA
# and comment agree only if whoever wrote them resolved the pin against the
# action's own repository. Verifying that here would mean reaching the network,
# and `scripts/verify.sh` deliberately needs nothing but the pinned toolchain
# and ordinary build prerequisites. Resolving a pin is a step in authoring a
# change; this script keeps the shape enforceable.
#
# GitHub resolves `uses:` from workflows and from composite action definitions,
# so those are the files scanned. Uses POSIX shell and standard POSIX utilities
# only (find, awk, grep, sort). No interpreter, package manager or external
# tooling.
#
# Usage: scripts/check-action-pins.sh    (run from anywhere in the repository)

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

status=0
report() {
	printf '%s:%s: %s\n  uses: %s\n' "$1" "$2" "$3" "$4" >&2
	status=1
}

if [ -d .github/workflows ]; then
	workflows=$(find .github/workflows -type f \( -name '*.yml' -o -name '*.yaml' \) | sort)
else
	workflows=""
fi

if [ -d .github/actions ]; then
	composites=$(find .github/actions -type f \( -name 'action.yml' -o -name 'action.yaml' \) | sort)
else
	composites=""
fi

# 3. Something was actually inspected.
if [ -z "$workflows" ]; then
	printf 'ERROR: no workflow files found under .github/workflows.\nIf workflows moved, update scripts/check-action-pins.sh so the pin rule still applies to them.\n' >&2
	exit 1
fi

files=$(printf '%s\n%s\n' "$workflows" "$composites" | grep -v '^$' | sort)

# Extract one record per `uses:` key: file, line number, the reference, and any
# trailing comment. Matching the key at the start of a line — optionally after a
# YAML sequence dash — keeps this to real keys rather than the word appearing
# inside some other value. YAML requires whitespace before a `#` that starts a
# comment, which is what separates the reference from the release name.
records=$(
	for file in $files; do
		awk -v file="$file" '
			/^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*/ {
				rest = $0
				sub(/^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*/, "", rest)
				comment = ""
				if (match(rest, /[[:space:]]+#/)) {
					comment = substr(rest, RSTART + RLENGTH)
					rest = substr(rest, 1, RSTART - 1)
				}
				sub(/[[:space:]]+$/, "", rest)
				gsub(/^["\047]+|["\047]+$/, "", rest)
				sub(/^[[:space:]]+/, "", comment)
				sub(/[[:space:]]+$/, "", comment)
				if (rest == "") next
				print file "|" FNR "|" rest "|" comment
			}
		' "$file"
	done
)

count=0
while IFS='|' read -r file lineno ref comment; do
	[ -n "${file:-}" ] || continue
	count=$((count + 1))

	case "$ref" in
	./*)
		# A local action: part of this repository, at this commit.
		continue
		;;
	docker://*)
		if ! printf '%s' "$ref" | grep -Eq '@sha256:[0-9a-f]{64}$'; then
			report "$file" "$lineno" \
				'container reference is mutable; pin it to a sha256: digest.' "$ref"
		fi
		continue
		;;
	esac

	# 1. Immutable revision.
	if ! printf '%s' "$ref" | grep -Eq '^[^@[:space:]]+@[0-9a-f]{40}$'; then
		report "$file" "$lineno" \
			'action reference is not pinned to a full-length commit SHA. A tag or branch can be moved by whoever controls the action; resolve the release against that repository and pin the commit it points at.' \
			"$ref"
		continue
	fi

	# 2. Recorded release. Requiring a release-shaped first word keeps this from
	#    being satisfied by a comment that explains nothing.
	if ! printf '%s' "$comment" | grep -Eq '^v?[0-9][0-9A-Za-z.+-]*([[:space:]]|$)'; then
		report "$file" "$lineno" \
			'pinned action records no release. Add the release the SHA corresponds to as a trailing comment, e.g. `# v7.0.1`, so the pin can be reviewed and updated deliberately.' \
			"$ref"
	fi
done <<EOF
$records
EOF

if [ "$status" -ne 0 ]; then
	exit 1
fi

printf 'action pin check: OK (%s references in %s files)\n' \
	"$count" "$(printf '%s\n' "$files" | wc -l | tr -d ' ')"
