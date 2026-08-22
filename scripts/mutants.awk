#!/usr/bin/env awk -f
# The mutant-record engine shared by ./scripts/check-mutants.sh paths.
#
# One program owns everything semantic about applying a record, so the fast
# structural preflight and the expensive suite cannot drift apart: parsing,
# matching, occurrence selection and replacement are the same code in both.
#
# Modes (MUTANT_MODE):
#
#   parse   Read the manifest named by FILENAME argument; emit one
#           tab-separated record per mutant:
#             name file find replace scope test runs occurrence
#           Defaults runs=3, occurrence=1. Unknown keys and duplicate names
#           fail closed. Comments (#) and blank paragraphs are skipped.
#
#   check   Locate the anchor of one record in its target file and prove the
#           mutation would change bytes — without writing anything. Exit 0
#           and print "<count>\t<startline>\t<chosenline>"; exit 1 with a
#           precise diagnostic naming the file and failure otherwise.
#
#   apply   Emit the mutated file to stdout using identical resolution;
#           exit 1 with a diagnostic if the anchor is unresolvable or the
#           result equals the original.
#
# Matching semantics (the property #82 exists for):
#
#   Anchor and target are compared in whitespace-normalized form: every run
#   of spaces, tabs and line breaks becomes one space. A rustfmt pass that
#   re-indents or wraps lines therefore cannot invalidate an anchor authored
#   against the same token sequence, and an anchor may span the lines it was
#   written across. Normalization is a transport detail, not a license: the
#   match still consumes exactly one contiguous token-sequence region, its
#   position is mapped back through an offset table onto the original bytes,
#   and only that region is replaced. Occurrence N selects the Nth
#   non-overlapping normalized match, deterministically, with the total
#   count reported so ambiguity stays visible.
#
# Values reach the engine through the environment (MUTANT_FIND,
# MUTANT_REPLACE, MUTANT_WANT, MUTANT_NAME) rather than awk -v assignments,
# because -v expands escape sequences: an anchor carrying \" would silently
# stop matching and a broken application would masquerade as a surviving
# mutant.

function fail(msg) {
	printf "mutants.awk: %s\n", msg > "/dev/stderr"
	exit 1
}

# Collapse every run of blanks (spaces and tabs) in a single line to one
# space; the caller supplies line junctions.
function collapse(text,    out, i, n, c, pending) {
	out = ""
	pending = 0
	n = length(text)
	for (i = 1; i <= n; i++) {
		c = substr(text, i, 1)
		if (c == " " || c == "\t") {
			pending = 1
		} else {
			if (pending && out != "") out = out " "
			out = out c
			pending = 0
		}
	}
	return out
}

# Normalize an anchor: trimmed at the ends, blank runs collapsed. Line
# junctions were already folded to single spaces by the caller-side loader.
function normalize_anchor(raw) {
	text = collapse(raw)
	sub(/^ +/, "", text)
	sub(/ +$/, "", text)
	return text
}

# Load the target file into:
#   src[l]      the original l-th line, byte-faithful
#   norm        the whole file normalized: every maximal run of blanks or
#               line breaks becomes exactly one space
#   oline[p]    original line of normalized character p
#   ocol[p]     original column of normalized character p
# plus srclast (line count).
#
# Line endings are folded into the surrounding whitespace run like any
# other blank, so a blank line adds no second separator and wrapping adds
# exactly one. The engine canonicalizes the file tail to end with exactly
# one newline on both the original and mutated sides, which keeps the
# change-proof comparison byte-exact; every current target is
# newline-terminated text.
function load(file,    line, i, n, c, pending, ws_l, ws_c) {
	srclast = 0
	norm = ""
	npos = 0
	pending = 0
	while ((getline line < file) > 0) {
		srclast++
		src[srclast] = line
		# Walk the ORIGINAL line so every recorded column is exact; the
		# offset map is what maps a normalized match back onto bytes.
		n = length(line)
		for (i = 1; i <= n; i++) {
			c = substr(line, i, 1)
			if (c == " " || c == "\t") {
				if (!pending) {
					ws_l = srclast
					ws_c = i
				}
				pending = 1
			} else {
				if (pending && npos > 0) {
					npos++
					norm = norm " "
					oline[npos] = ws_l
					ocol[npos] = ws_c
				}
				npos++
				norm = norm c
				oline[npos] = srclast
				ocol[npos] = i
				pending = 0
			}
		}
		# The line break itself is whitespace: it joins the run.
		pending = 1
	}
	close(file)
	if (srclast == 0) fail(FILENAME ": empty or unreadable target")
}

# Find the want-th non-overlapping normalized match.
# On success returns 1 and sets m_start/m_end (normalized coordinates),
# m_count (total matches), m_sl/m_sc and m_el/m_ec (original coordinates of
# the first and last matched characters).
function locate(anchor, want,    from, idx, count) {
	m_count = 0
	from = 1
	count = 0
	while (from <= length(norm)) {
		idx = index(substr(norm, from), anchor)
		if (idx == 0) break
		idx += from - 1
		count++
		if (count == want) {
			m_start = idx
			m_end = idx + length(anchor) - 1
			m_sl = oline[m_start]
			m_sc = ocol[m_start]
			m_el = oline[m_end]
			m_ec = ocol[m_end]
		}
		from = idx + length(anchor)
	}
	m_count = count
	return count >= want
}

# Assemble the mutated text around the located span, replacing exactly the
# original characters the normalized match consumed (which may include
# wrapped line breaks and their indentation) with the replacement verbatim.
function render(replacement,    out, l, tail) {
	out = ""
	for (l = 1; l < m_sl; l++) out = out src[l] "\n"
	out = out substr(src[m_sl], 1, m_sc - 1)
	out = out replacement
	tail = substr(src[m_el], m_ec + 1)
	for (l = m_el + 1; l <= srclast; l++) tail = tail "\n" src[l]
	return out tail "\n"
}

function original_text(    out, l) {
	out = ""
	for (l = 1; l <= srclast; l++) {
		out = out src[l]
		if (l < srclast) out = out "\n"
	}
	return out "\n"
}

# ---- manifest parser -------------------------------------------------------

function parse_manifest(manifest,    line, key, value) {
	manifest_path = manifest
	while ((getline line < manifest) > 0) {
		if (line ~ /^[[:space:]]*#/) continue
		if (line ~ /^[[:space:]]*$/) {
			flush_record()
			continue
		}
		key = line
		sub(/:.*$/, "", key)
		value = line
		sub(/^[^:]*:[[:space:]]?/, "", value)
		if (key == "name") r_name = value
		else if (key == "file") r_file = value
		else if (key == "find") r_find = value
		else if (key == "replace") r_replace = value
		else if (key == "scope") r_scope = value
		else if (key == "test") r_test = value
		else if (key == "runs") r_runs = value
		else if (key == "occurrence") r_occurrence = value
		else fail(sprintf("%s: unknown key %s", manifest, key))
	}
	close(manifest)
	flush_record()
}

function flush_record() {
	if (r_name == "") return
	if (r_name in rec_seen) fail(manifest_path ": duplicate mutant name " r_name)
	rec_seen[r_name] = 1
	if (r_runs == "") r_runs = 3
	if (r_occurrence == "") r_occurrence = 1
	printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", \
		r_name, r_file, r_find, r_replace, r_scope, r_test, r_runs, r_occurrence
	r_name = ""; r_file = ""; r_find = ""; r_replace = ""
	r_scope = ""; r_test = ""; r_runs = ""; r_occurrence = ""
}

# ---- dispatch --------------------------------------------------------------

BEGIN {
	mode = ENVIRON["MUTANT_MODE"]
	if (mode == "parse") {
		parse_manifest(ARGV[1])
		exit 0
	}
	if (mode != "check" && mode != "apply") fail("MUTANT_MODE must be parse, check or apply")
	if (ARGC < 2 || ARGV[1] == "") fail("no target file given")

	find = ENVIRON["MUTANT_FIND"]
	replace = ENVIRON["MUTANT_REPLACE"]
	want = ENVIRON["MUTANT_WANT"] + 0
	label = ENVIRON["MUTANT_NAME"]
	if (want < 1) want = 1

	target = ARGV[1]
	load(target)
	anchor = normalize_anchor(find)
	if (anchor == "")
		fail(sprintf("%s: %s has an empty anchor", target, label))

	if (!locate(anchor, want)) {
		if (m_count == 0)
			fail(sprintf("%s: %s: the anchor no longer matches; the source moved or was reformatted past recognition", target, label))
		fail(sprintf("%s: %s: %d match(es), fewer than the requested occurrence %d", target, label, m_count, want))
	}

	mutated = render(replace)
	if (mutated == original_text())
		fail(sprintf("%s: %s: applying the mutation changes nothing (anchor and replacement coincide)", target, label))

	if (mode == "apply") {
		printf "%s", mutated
		exit 0
	}
	printf "%d\t%d\t%d\n", m_count, m_sl, m_el
	exit 0
}
