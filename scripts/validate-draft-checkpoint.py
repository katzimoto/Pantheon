#!/usr/bin/env python3
"""Validate a GitHub pull_request opened event for the Pantheon Draft checkpoint.

Reads the event payload from GITHUB_EVENT_PATH (default: /github/workflow/event.json),
validates structural opening facts, and exits with a concise diagnostic on failure.

When run with --self-test, exercises deterministic fixtures instead.
"""

import json
import os
import re
import sys

# Marker used in the durable review record for idempotency.
MARKER = "<!-- pantheon-draft-checkpoint -->"

# Closing keywords recognized by GitHub, case-insensitive.
# Supporting references are any other issue mention in the Mission section.
CLOSING_KEYWORDS = [
    "closes",
    "close",
    "closing",
    "fixes",
    "fix",
    "fixing",
    "resolves",
    "resolve",
    "resolving",
]


def strip_html_comments(text: str) -> str:
    """Remove HTML comments, including multi-line."""
    return re.sub(r"<!--[\s\S]*?-->", "", text)


def parse_sections(body: str) -> dict[str, str]:
    """Split a Markdown body into sections keyed by lower-case heading text.

    Headings must match ``## Heading`` at the start of a line.
    """
    sections: dict[str, str] = {}
    current: str | None = None
    lines: list[str] = []

    for line in body.splitlines():
        m = re.match(r"^##\s+(\S.*)$", line)
        if m:
            if current is not None:
                sections[current] = "\n".join(lines)
            current = m.group(1).strip().lower()
            lines = []
        elif current is not None:
            lines.append(line)

    if current is not None:
        sections[current] = "\n".join(lines)

    return sections


def validate(event: dict) -> list[str]:
    """Return a list of human-readable errors; empty list means pass."""
    errors: list[str] = []
    pr = event.get("pull_request", {})

    # 1. Draft state
    if not pr.get("draft"):
        errors.append("PR is not opened as Draft.")

    # 2. Target branch must be the repository default.
    base = pr.get("base", {})
    target = base.get("ref", "")
    default = base.get("repo", {}).get("default_branch", "")
    if target != default:
        errors.append(
            f"PR targets '{target}', not the default branch '{default}'."
        )

    # 3. Tree equivalence: the head tree must match the base tree.
    #    GitHub computes this as changed_files == 0 with zero additions/deletions.
    changed_files = pr.get("changed_files", 0)
    additions = pr.get("additions", 0)
    deletions = pr.get("deletions", 0)
    if changed_files != 0 or additions != 0 or deletions != 0:
        errors.append(
            "PR has tracked changes at opening (tree is not equivalent to base)."
        )

    # 4. Body structure: Mission, Change, Evidence must exist and contain real
    #    content after stripping HTML template comments.
    body = pr.get("body") or ""
    cleaned = strip_html_comments(body)
    sections = parse_sections(cleaned)

    required = ["mission", "change", "evidence"]
    for heading in required:
        if heading not in sections:
            errors.append(f"Missing required section: ## {heading.capitalize()}")
            continue

        content = sections[heading].strip()
        if not content:
            errors.append(
                f"## {heading.capitalize()} contains only whitespace or template comments."
            )

    # 5. Mission relationship: closing or supporting reference.
    if "mission" in sections:
        mission_text = sections["mission"].strip()
        has_closing = any(
            re.search(rf"\b{kw}\s+#\d+", mission_text, re.IGNORECASE)
            for kw in CLOSING_KEYWORDS
        )
        has_ref = re.search(r"#\d+", mission_text)
        if not has_closing and not has_ref:
            errors.append(
                "Mission section does not identify a mission relationship (closing or supporting)."
            )

    return errors


def selftest(fixtures_dir: str) -> int:
    """Run deterministic fixtures and report results."""
    import glob

    passed = 0
    failed = 0

    pattern = os.path.join(fixtures_dir, "*.json")
    fixture_paths = sorted(glob.glob(pattern))

    if not fixture_paths:
        print(f"ERROR: no fixtures found in {fixtures_dir}", file=sys.stderr)
        return 1

    for fixture_path in fixture_paths:
        with open(fixture_path, encoding="utf-8") as f:
            fixture = json.load(f)

        expected = fixture.pop("_expected", "fail")
        description = fixture.pop("_description", os.path.basename(fixture_path))

        errs = validate(fixture)
        actual = "pass" if not errs else "fail"

        if actual == expected:
            print(f"PASS: {description}")
            passed += 1
        else:
            print(f"FAIL: {description}")
            print(f"  expected: {expected}, got: {actual}")
            if errs:
                for e in errs:
                    print(f"  error: {e}")
            else:
                print("  (unexpected pass)")
            failed += 1

    print(f"\n{passed} passed, {failed} failed")
    return 0 if failed == 0 else 1


def main(argv: list[str] | None = None) -> int:
    if argv is None:
        argv = sys.argv[1:]

    if "--self-test" in argv:
        script_dir = os.path.dirname(os.path.abspath(__file__))
        fixtures_dir = os.path.join(script_dir, "fixtures", "draft-checkpoint")
        return selftest(fixtures_dir)

    event_path = os.environ.get("GITHUB_EVENT_PATH", "/github/workflow/event.json")
    try:
        with open(event_path, encoding="utf-8") as f:
            event = json.load(f)
    except FileNotFoundError:
        print(f"error: event payload not found at {event_path}", file=sys.stderr)
        return 1
    except json.JSONDecodeError as exc:
        print(f"error: invalid JSON in event payload: {exc}", file=sys.stderr)
        return 1

    errors = validate(event)
    if errors:
        for msg in errors:
            print(msg, file=sys.stderr)
        return 1

    print("Opening checkpoint passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
