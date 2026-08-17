#!/bin/sh
# Validate Pantheon's internal crate dependency graph against an allowlist.
#
# Pantheon's architecture defines which subsystem may depend on which. Written
# only in prose, that survives exactly as long as everyone remembers it. This
# script makes the important direction of each edge mechanical: a change that
# lets the CLI reach the database, or the semantic core reach persistence, fails
# validation instead of being noticed in review, or not.
#
# It checks three things:
#
#   1. Every internal dependency edge is on the allowlist below.
#   2. Every workspace member appears in the allowlist, so a newly added crate
#      must state its boundary rather than silently inheriting none.
#   3. The allowlist names no crate that has stopped existing, so it cannot rot
#      into a list of rules about nothing.
#
# The graph comes from `cargo tree`, which reads Cargo's own resolved
# dependencies. Pantheon deliberately does not grep the manifests: a dependency
# can be declared under `[dev-dependencies]`, `[build-dependencies]` or a
# `[target.'cfg(...)'.dependencies]` table, renamed with `package = `, inherited
# from the workspace, or hidden behind an optional feature. A TOML grep or a
# default-feature-only graph can therefore report a clean boundary while a
# forbidden edge remains declarable.
#
# Four flags are load-bearing, and each exists to expose a class of dependency
# edge that must not bypass the architecture boundary:
#
#   --all-features       Optional dependencies are still architectural edges.
#                        This checker asks what a crate *can* depend on, not
#                        which feature combination is a meaningful product
#                        build. Normal build/test verification deliberately does
#                        not use --all-features.
#   --target all         Without it, cargo tree reports only the host platform,
#                        so an edge behind `cfg(windows)` is invisible on Linux
#                        and CI passes while the boundary is broken.
#   -e normal,build,dev  Dev and build dependencies are real code paths. A
#                        test-only edge from the CLI to the store would let a
#                        test reach around Operator Control.
#   --no-dedupe          Keeps every direct edge on its own line rather than
#                        collapsing a repeated package.
#
# Third-party dependencies are ignored here. This script owns the internal
# architecture graph; which outside crates Pantheon takes on is a judgement,
# and `docs/development/implementation.md` states that policy.
#
# Uses POSIX shell and standard POSIX utilities (awk, sed, sort, comm, cut).
# The pinned Rust toolchain is the only other requirement.
#
# Usage: scripts/check-crate-deps.sh    (run from anywhere in the repository)

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# The allowed internal dependency graph. One line per workspace crate:
#
#     <crate> [<crate it may depend on> ...]
#
# A crate listed alone may not depend on any other Pantheon crate. These are
# ceilings, not requirements: an allowed edge is declared in a manifest only
# when real code needs it, which is why most of these crates currently declare
# no dependencies at all.
#
# `docs/development/implementation.md` explains why each boundary exists. Change
# a line here only together with that document.
allowlist() {
	cat <<'EOF'
pantheon-core
pantheon-store pantheon-core
pantheon-engine pantheon-core pantheon-store
pantheon-operator-protocol
pantheon-operator-api pantheon-core pantheon-engine pantheon-operator-protocol
pantheond pantheon-core pantheon-store pantheon-engine pantheon-operator-api pantheon-operator-protocol
pantheon-cli pantheon-operator-protocol
EOF
}

tmp=${TMPDIR:-/tmp}/pantheon-crate-deps.$$
trap 'rm -f "$tmp".*' EXIT HUP INT TERM
: >"$tmp.errors"

cargo tree --workspace --locked --all-features --depth 1 --prefix depth --no-dedupe \
	-e normal,build,dev --target all >"$tmp.tree"

# `--prefix depth` prints the depth immediately before the package name, so
# `0pantheon-cli` is a workspace root and the `1...` lines beneath it are its
# direct dependencies. Emit one record per workspace member and one per edge.
awk '
	{
		if (match($1, /^[0-9]+/) == 0) next
		depth = substr($1, 1, RLENGTH) + 0
		name = substr($1, RLENGTH + 1)
		if (name == "") next
		if (depth == 0) { root = name; print "MEMBER " name }
		else if (depth == 1 && root != "") { print "EDGE " root " " name }
	}
' "$tmp.tree" >"$tmp.records"

sed -n 's/^MEMBER //p' "$tmp.records" | sort -u >"$tmp.members"
allowlist | cut -d' ' -f1 | sort -u >"$tmp.listed"

# 2. A workspace member missing from the allowlist.
comm -13 "$tmp.listed" "$tmp.members" >"$tmp.unlisted"
while IFS= read -r crate; do
	[ -n "$crate" ] || continue
	printf 'ERROR: workspace crate %s is not in the allowlist in scripts/check-crate-deps.sh.\nAdd a line stating which Pantheon crates it may depend on, and record the boundary in docs/development/implementation.md.\n' \
		"$crate" >>"$tmp.errors"
done <"$tmp.unlisted"

# 3. An allowlist line for a crate that no longer exists.
comm -23 "$tmp.listed" "$tmp.members" >"$tmp.stale"
while IFS= read -r crate; do
	[ -n "$crate" ] || continue
	printf 'ERROR: the allowlist in scripts/check-crate-deps.sh names %s, which is not a workspace crate.\nRemove the line, or correct the crate name.\n' \
		"$crate" >>"$tmp.errors"
done <"$tmp.stale"

# Report the allowed set the way the allowlist states it, so the fix is visible
# in the error rather than requiring the script to be opened.
allowed_for() {
	allowlist | while IFS= read -r line; do
		case "$line" in
		"$1" | "$1 "*)
			rest=${line#"$1"}
			printf '%s\n' "${rest# }"
			break
			;;
		esac
	done
}

# 1. Every internal edge is allowed. Only edges whose target is itself a
#    workspace member are internal; everything else is a third-party crate.
sed -n 's/^EDGE //p' "$tmp.records" | sort -u >"$tmp.edges"
: >"$tmp.internal-edges"
while read -r from to; do
	[ -n "${to:-}" ] || continue
	grep -qxF "$to" "$tmp.members" || continue
	if [ "$from" = "$to" ]; then continue; fi
	printf '%s %s\n' "$from" "$to" >>"$tmp.internal-edges"
	allowed=$(allowed_for "$from")
	case " $allowed " in
	*" $to "*) ;;
	*)
		printf 'ERROR: %s may not depend on %s.\nAllowed internal dependencies: %s.\n' \
			"$from" "$to" "${allowed:-none}" >>"$tmp.errors"
		;;
	esac
done <"$tmp.edges"

if [ -s "$tmp.errors" ]; then
	cat "$tmp.errors" >&2
	exit 1
fi

printf 'crate dependency check: OK (%s crates, %s internal edges)\n' \
	"$(wc -l <"$tmp.members" | tr -d ' ')" \
	"$(wc -l <"$tmp.internal-edges" | tr -d ' ')"
