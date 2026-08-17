#!/bin/sh
# Validate documentation structure and agent-contract wiring. Six independent
# checks, numbered as they appear below:
#
#   1. Inline-code references resolve. Pantheon docs reference each other with
#      repository-root-relative paths written as inline code, e.g.
#      `docs/architecture/tasks/task-lifecycle.md`. Each must point at something
#      that exists.
#
#   2. Ordinary Markdown links to local files resolve.
#
#   3. The architecture map is a complete inventory. Every canonical contract
#      under a docs/architecture domain directory must be listed in
#      docs/architecture/README.md exactly once, and every contract the map
#      lists must exist. A contract missing from the map is invisible to agents
#      navigating by it, which checks 1 and 2 cannot detect.
#
#   4. The canonical import is present. AGENTS.md is the agent operating
#      contract, and CLAUDE.md must import it so Claude Code loads that contract
#      rather than a separate copy. This verifies only that the import line is
#      still there; it does not inspect the rest of CLAUDE.md.
#
#   5. The mission form and its chooser configuration are intact. A GitHub
#      Issue is Pantheon's mission record (docs/development/missions.md), and
#      the Engineering Mission form is the default way a human authors one.
#      This verifies that the form still exists and that blank issues stay
#      disabled, so the form is still what the template chooser offers.
#
#   6. The pull request template exists and keeps its stable headings. A pull
#      request is the candidate change and its evidence
#      (docs/development/change-lifecycle.md), and the headings under which that
#      evidence is written are the part other tools read. This checks the
#      template only. Nothing inspects the body of an actual pull request.
#
# Uses POSIX shell and standard POSIX utilities only (find, grep, sed, sort,
# comm, uniq, wc, tr). No interpreter, package manager or external tooling.
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

# Repository documentation only. `target/` is pruned along with `.git` because
# it is generated: rustdoc emits Markdown into target/doc, and a dependency's
# vendored Markdown lands there too. Scanning it would make this check report
# broken references in files nobody wrote, and would make the result depend on
# whether the tree had been built — `scripts/verify.sh` runs this check and then
# `cargo doc`, so the next run would be inspecting the previous run's output.
files=$(find . -path ./.git -prune -o -path ./target -prune -o -name '*.md' -print |
	sed 's|^\./||' | sort)

# 1. Inline-code references of the form `docs/.../file.md`, `schemas/file.json`,
#    `scripts/check.sh` or `.github/.../file.yml`. Covering scripts/ means a
#    validation command cited in documentation or in AGENTS.md fails the check
#    once it stops existing; covering .github/ does the same for a repository
#    configuration file that documentation points at.
for f in $files; do
	grep -noE '`(docs|schemas|scripts|\.github)/[A-Za-z0-9._/-]+`' "$f" 2>/dev/null | while IFS=: read -r line match; do
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
map=docs/architecture/README.md
if [ -f "$map" ]; then
	# Contracts listed in the map's domain tables, one per row.
	sed -n 's#^| `\(docs/architecture/[a-z0-9-]*/[a-z0-9-]*\.md\)` |.*#\1#p' "$map" |
		sort >/tmp/docs-map-listed.$$

	# Contracts on disk. -mindepth 2 skips overview.md and the architecture map
	# itself; -name excludes any per-domain README. Those are navigation or
	# system-model documents, not domain contracts, and are linked in prose.
	find docs/architecture -mindepth 2 -name '*.md' ! -name 'README.md' |
		sed 's|^\./||' | sort >/tmp/docs-map-ondisk.$$

	# Contract paths are lowercase kebab-case (see docs/README.md). Report a
	# violation as a naming error and hold it out of the inventory comparison
	# below: the map parser cannot match a non-conforming row, so comparing it
	# would claim the contract is unlisted even when a row exists.
	naming='^docs/architecture/[a-z0-9-]*/[a-z0-9-]*\.md$'
	grep -Ev "$naming" /tmp/docs-map-ondisk.$$ | while read -r odd; do
		printf '%s: contract path is not lowercase kebab-case: %s\n' \
			"$map" "$odd" >&2
		echo fail >>/tmp/docs-map-status.$$
	done
	grep -E "$naming" /tmp/docs-map-ondisk.$$ >/tmp/docs-map-conform.$$ || true
	mv /tmp/docs-map-conform.$$ /tmp/docs-map-ondisk.$$

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

# 4. The canonical import is present. Claude Code reads CLAUDE.md and not
# AGENTS.md, so CLAUDE.md carries an @AGENTS.md import. Claude Code ignores
# imports written inside backticks or code fences, so require a bare import
# line.
if [ ! -f AGENTS.md ]; then
	echo "missing canonical agent contract: AGENTS.md" >&2
	status=1
elif [ ! -f CLAUDE.md ]; then
	echo "missing CLAUDE.md: it must import AGENTS.md" >&2
	status=1
elif ! grep -q '^@AGENTS\.md[[:space:]]*$' CLAUDE.md; then
	echo "CLAUDE.md: no bare '@AGENTS.md' import line; the canonical import must remain present" >&2
	status=1
fi

# 5. The mission form exists, and blank issues stay disabled so it remains what
# the template chooser offers. The form's own YAML is validated by GitHub against
# the repository, so re-checking it here would mean carrying a YAML toolchain to
# duplicate what the platform already does; the two facts GitHub cannot report
# are whether the canonical form is still present and what the chooser is
# configured to do.
#
# This deliberately does not assert that only one template exists. Whether
# Pantheon ever grows a route for something that is not a mission at all — a
# support or vulnerability path — is a decision for
# docs/development/missions.md, not something this script should forbid.
form=.github/ISSUE_TEMPLATE/engineering-mission.yml
chooser=.github/ISSUE_TEMPLATE/config.yml
[ -f "$chooser" ] || chooser=.github/ISSUE_TEMPLATE/config.yaml
if [ ! -f "$form" ]; then
	echo "missing Engineering Mission form: $form" >&2
	status=1
fi
if [ ! -f "$chooser" ]; then
	echo "missing template chooser configuration: .github/ISSUE_TEMPLATE/config.yml" >&2
	status=1
elif ! grep -qE '^blank_issues_enabled:[[:space:]]*false[[:space:]]*$' "$chooser"; then
	echo "$chooser: blank_issues_enabled is not false; a blank issue is a creation path again" >&2
	status=1
fi

# 6. The pull request template exists and still carries its stable headings.
# docs/development/change-lifecycle.md fixes three of them, in that order,
# because agents and future Pantheon adapters read them; the optional fourth
# section is deleted when a change has nothing to put in it, so it is not
# required here. The template is one authoring interface and not the contract,
# which is why this checks the file and stops there: grading the prose of a real
# pull request body is a judgement, and review is what makes it.
template=.github/pull_request_template.md
if [ ! -f "$template" ]; then
	echo "missing pull request template: $template" >&2
	status=1
else
	missing=
	for heading in Mission Change Evidence; do
		if ! grep -q "^## $heading[[:space:]]*$" "$template"; then
			printf '%s: missing stable heading: ## %s\n' "$template" "$heading" >&2
			missing=yes
			status=1
		fi
	done
	# Order is part of the contract, so check it — but only once all three are
	# present, since otherwise this would restate the error just reported.
	if [ -z "$missing" ]; then
		# `\(...\|...\)` alternation inside a sed BRE is a GNU extension, not
		# POSIX and not supported by BSD sed (macOS). `grep -E` alternation
		# is POSIX ERE and behaves identically on both; the sed pass after it
		# only strips a fixed prefix/suffix, no alternation needed.
		order=$(grep -E '^## (Mission|Change|Evidence)[[:space:]]*$' "$template" |
			sed -e 's/^## //' -e 's/[[:space:]]*$//' | tr '\n' ' ')
		if [ "$order" != "Mission Change Evidence " ]; then
			printf '%s: stable headings are out of order: %s\n' \
				"$template" "$order" >&2
			status=1
		fi
	fi
fi

if [ "$status" -eq 0 ]; then
	echo "docs link check: OK ($(echo "$files" | wc -l | tr -d ' ') markdown files, $listed contracts mapped)"
fi
exit "$status"
