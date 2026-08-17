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

if [ "$status" -eq 0 ]; then
	echo "docs link check: OK ($(echo "$files" | wc -l | tr -d ' ') markdown files)"
fi
exit "$status"
