#!/bin/sh
# Validate documentation references.
#
# Pantheon docs reference each other with repository-root-relative paths written
# as inline code, e.g. `docs/architecture/tasks/task-lifecycle.md`, plus ordinary
# Markdown links. This script checks that every such reference resolves to a file
# that exists. It has no dependencies beyond POSIX sh, grep and sed.
#
# Usage: scripts/check-docs-links.sh        (run from anywhere in the repository)

set -e
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

status=0
report() {
	printf '%s:%s: missing reference: %s\n' "$1" "$2" "$3" >&2
	status=1
}

files=$(find . -path ./.git -prune -o -name '*.md' -print | sed 's|^\./||' | sort)

# 1. Inline-code references of the form `docs/.../file.md` or `schemas/file.json`.
for f in $files; do
	grep -noE '`(docs|schemas)/[A-Za-z0-9._/-]+`' "$f" 2>/dev/null | while IFS=: read -r line match; do
		target=$(printf '%s' "$match" | tr -d '`')
		case "$target" in
		*/) [ -d "$target" ] || echo "MISS $f $line $target" ;;
		*) [ -e "$target" ] || echo "MISS $f $line $target" ;;
		esac
	done
done > /tmp/docs-link-check.$$ || true

# 2. Markdown links to local files, resolved relative to the linking file.
for f in $files; do
	dir=$(dirname "$f")
	grep -noE '\]\([^)#][^)]*\)' "$f" 2>/dev/null | while IFS=: read -r line match; do
		target=$(printf '%s' "$match" | sed -e 's|^](||' -e 's|)$||' -e 's|#.*$||')
		case "$target" in
		http://* | https://* | mailto:* | '') continue ;;
		/*) resolved=".$target" ;;
		*) resolved="$dir/$target" ;;
		esac
		[ -e "$resolved" ] || echo "MISS $f $line $target"
	done
done >> /tmp/docs-link-check.$$ || true

while read -r _ f line target; do
	report "$f" "$line" "$target"
done < /tmp/docs-link-check.$$
rm -f /tmp/docs-link-check.$$

# 3. Map inventory completeness, both directions.
#
# docs/architecture/README.md is the sole full inventory of canonical
# architecture contracts. A contract missing from it is effectively invisible to
# an agent navigating by the map, so absence is an error even though every path
# in the file still resolves. Check 1 above only proves mapped paths exist.
map=docs/architecture/README.md
if [ -f "$map" ]; then
	# Contracts listed in the map's domain tables, one per row.
	sed -n 's#^| `\(docs/architecture/[a-z0-9-]*/[a-z0-9-]*\.md\)` |.*#\1#p' "$map" |
		sort >/tmp/docs-map-listed.$$

	# Contracts on disk. overview.md and the navigation READMEs are not domain
	# contracts and are referenced in prose instead, so they are excluded.
	find docs/architecture -mindepth 2 -name '*.md' ! -name 'README.md' |
		sed 's|^\./||' | sort >/tmp/docs-map-ondisk.$$

	# Compare against a deduplicated listing so a duplicate row is reported only
	# as a duplicate, not also as a nonexistent contract.
	sort -u /tmp/docs-map-listed.$$ >/tmp/docs-map-uniq.$$

	comm -13 /tmp/docs-map-uniq.$$ /tmp/docs-map-ondisk.$$ | while read -r missing; do
		printf '%s: contract is not listed in the architecture map: %s\n' \
			"$map" "$missing" >&2
		echo fail >>/tmp/docs-map-status.$$
	done

	comm -23 /tmp/docs-map-uniq.$$ /tmp/docs-map-ondisk.$$ | while read -r stale; do
		printf '%s: map lists a contract that does not exist: %s\n' \
			"$map" "$stale" >&2
		echo fail >>/tmp/docs-map-status.$$
	done

	uniq -d /tmp/docs-map-listed.$$ | while read -r dupe; do
		printf '%s: contract is listed more than once in the map: %s\n' \
			"$map" "$dupe" >&2
		echo fail >>/tmp/docs-map-status.$$
	done

	listed=$(wc -l </tmp/docs-map-uniq.$$ | tr -d ' ')
	rm -f /tmp/docs-map-listed.$$ /tmp/docs-map-uniq.$$ /tmp/docs-map-ondisk.$$
	if [ -f /tmp/docs-map-status.$$ ]; then
		status=1
		rm -f /tmp/docs-map-status.$$
	fi
else
	echo "missing architecture map: $map" >&2
	status=1
fi

if [ "$status" -eq 0 ]; then
	echo "docs link check: OK ($(echo "$files" | wc -l | tr -d ' ') markdown files, $listed contracts mapped)"
fi
exit "$status"
