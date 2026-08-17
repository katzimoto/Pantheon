#!/usr/bin/env python3
"""GitHub-native Version/Mission release automation for Pantheon.

This script intentionally treats GitHub Milestones and Engineering Mission Issues
as authority. It never derives an authoritative version from commits or PR ranges.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any

VERSION_TITLE_RE = re.compile(
    r"^(?P<version>v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?)"
    r"(?:\s+—\s+.+)?$"
)
CLOSING_RE = re.compile(
    r"(?im)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#(?P<number>\d+)\b"
)

REQUIRED_MILESTONE_HEADINGS = (
    "Release outcome",
    "Product proof",
    "Primary user workflow",
    "Included capabilities",
    "Explicitly deferred",
    "Supported envelope",
    "Safety and correctness properties",
    "Architecture entry points",
    "Integration checkpoints",
    "Release gate",
    "Release evidence",
    "Known limitations",
    "Compatibility",
    "Completion",
)

REQUIRED_MISSION_HEADINGS = ("Outcome", "Acceptance criteria", "Evidence")

CHANGELOG = {
    "changelog:added": ("Added", "0e8a16", "User-visible capability added."),
    "changelog:changed": ("Changed", "1d76db", "Existing user-visible behavior changed."),
    "changelog:deprecated": ("Deprecated", "fbca04", "User-visible behavior marked for future removal."),
    "changelog:removed": ("Removed", "b60205", "User-visible behavior removed."),
    "changelog:fixed": ("Fixed", "5319e7", "User-visible defect fixed."),
    "changelog:security": ("Security", "d93f0b", "User-visible security hardening or security fix."),
    "changelog:none": ("Internal", "ededed", "Intentionally omitted from user-facing release notes."),
}
VISIBLE_ORDER = (
    "changelog:added",
    "changelog:changed",
    "changelog:deprecated",
    "changelog:removed",
    "changelog:fixed",
    "changelog:security",
)


class ValidationError(RuntimeError):
    pass


@dataclass
class Repo:
    owner: str
    name: str

    @classmethod
    def from_env(cls) -> "Repo":
        value = os.environ.get("GITHUB_REPOSITORY", "")
        if "/" not in value:
            raise ValidationError("GITHUB_REPOSITORY must be owner/name")
        owner, name = value.split("/", 1)
        return cls(owner, name)


class GitHub:
    def __init__(self, repo: Repo):
        self.repo = repo
        self.api = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
        self.token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
        if not self.token:
            raise ValidationError("GH_TOKEN or GITHUB_TOKEN is required")

    def _request(self, method: str, path: str, data: Any | None = None) -> tuple[Any, dict[str, str]]:
        url = path if path.startswith("http") else f"{self.api}{path}"
        body = None if data is None else json.dumps(data).encode()
        req = urllib.request.Request(
            url,
            data=body,
            method=method,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self.token}",
                "X-GitHub-Api-Version": "2022-11-28",
                "User-Agent": "pantheon-version-automation",
                "Content-Type": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(req) as response:
                payload = response.read()
                parsed = json.loads(payload) if payload else None
                return parsed, dict(response.headers.items())
        except urllib.error.HTTPError as exc:
            payload = exc.read().decode(errors="replace")
            raise ValidationError(f"GitHub API {method} {url} failed: {exc.code} {payload}") from exc

    def get(self, path: str) -> Any:
        return self._request("GET", path)[0]

    def post(self, path: str, data: Any) -> Any:
        return self._request("POST", path, data)[0]

    def patch(self, path: str, data: Any) -> Any:
        return self._request("PATCH", path, data)[0]

    def paginate(self, path: str) -> list[Any]:
        items: list[Any] = []
        next_path: str | None = path
        while next_path:
            payload, headers = self._request("GET", next_path)
            if not isinstance(payload, list):
                raise ValidationError(f"expected list from {next_path}")
            items.extend(payload)
            next_path = parse_next(headers.get("Link"))
        return items

    def graphql(self, query: str, variables: dict[str, Any]) -> Any:
        payload = self.post("/graphql", {"query": query, "variables": variables})
        if payload.get("errors"):
            raise ValidationError(f"GitHub GraphQL failed: {payload['errors']}")
        return payload["data"]


def parse_next(link: str | None) -> str | None:
    if not link:
        return None
    for part in link.split(","):
        if 'rel="next"' in part:
            match = re.search(r"<([^>]+)>", part)
            return match.group(1) if match else None
    return None


def heading_count(body: str, heading: str, level: int = 2) -> int:
    prefix = "#" * level
    return len(re.findall(rf"(?m)^{re.escape(prefix)}\s+{re.escape(heading)}\s*$", body or ""))


def validate_milestone(milestone: dict[str, Any], *, release_ready: bool = False) -> list[str]:
    errors: list[str] = []
    title = milestone.get("title") or ""
    description = milestone.get("description") or ""
    if not VERSION_TITLE_RE.fullmatch(title):
        errors.append("Milestone title must be `v<semver>` or `v<semver> — <name>`.")
    for heading in REQUIRED_MILESTONE_HEADINGS:
        count = heading_count(description, heading)
        if count != 1:
            errors.append(f"Milestone description must contain exactly one `## {heading}` heading (found {count}).")
    if release_ready:
        if re.search(r"\bTBD\b", description, re.IGNORECASE):
            errors.append("Milestone description still contains `TBD`.")
        if re.search(r"<[^>\n]+>", description):
            errors.append("Milestone description still contains template placeholders.")
    return errors


def validate_mission(issue: dict[str, Any]) -> list[str]:
    body = issue.get("body") or ""
    errors: list[str] = []
    for heading in REQUIRED_MISSION_HEADINGS:
        count = heading_count(body, heading, level=3)
        if count != 1:
            errors.append(f"Mission #{issue.get('number')} must contain exactly one `### {heading}` heading (found {count}).")
    return errors


def label_names(issue: dict[str, Any]) -> set[str]:
    labels = issue.get("labels") or []
    result: set[str] = set()
    for label in labels:
        result.add(label if isinstance(label, str) else label.get("name", ""))
    return result


def classification(issue: dict[str, Any]) -> tuple[str | None, list[str]]:
    matches = sorted(label_names(issue) & CHANGELOG.keys())
    if len(matches) == 1:
        return matches[0], []
    if not matches:
        return None, [f"Mission #{issue.get('number')} needs exactly one `changelog:*` classification label."]
    return None, [f"Mission #{issue.get('number')} has multiple changelog classifications: {', '.join(matches)}."]


def version_from_title(title: str) -> str | None:
    match = VERSION_TITLE_RE.fullmatch(title or "")
    return match.group("version") if match else None


def closing_issue_numbers(body: str) -> list[int]:
    return sorted({int(match.group("number")) for match in CLOSING_RE.finditer(body or "")})


def render_changelog(milestone: dict[str, Any], issues: list[dict[str, Any]]) -> tuple[str, list[str]]:
    grouped = {key: [] for key in VISIBLE_ORDER}
    warnings: list[str] = []
    closed = 0
    for issue in sorted(issues, key=lambda item: item["number"]):
        if issue.get("pull_request"):
            continue
        if issue.get("state") == "closed":
            closed += 1
        kind, errors = classification(issue)
        if errors:
            warnings.extend(errors)
            continue
        if issue.get("state") != "closed":
            continue
        if kind in grouped:
            grouped[kind].append(issue)

    total = len([issue for issue in issues if not issue.get("pull_request")])
    lines = [
        f"# {milestone.get('title', 'Version')} changelog preview",
        "",
        f"Progress: **{closed}/{total} Engineering Missions closed**",
        "",
    ]
    any_entries = False
    for key in VISIBLE_ORDER:
        entries = grouped[key]
        if not entries:
            continue
        any_entries = True
        lines.extend([f"## {CHANGELOG[key][0]}", ""])
        for issue in entries:
            lines.append(f"- {issue['title']} (#{issue['number']})")
        lines.append("")
    if not any_entries:
        lines.extend(["_No completed user-facing Mission entries yet._", ""])
    internal = [
        issue for issue in issues
        if not issue.get("pull_request")
        and issue.get("state") == "closed"
        and "changelog:none" in label_names(issue)
    ]
    if internal:
        lines.extend(["## Internal / excluded", ""])
        for issue in internal:
            lines.append(f"- {issue['title']} (#{issue['number']})")
        lines.append("")
    if warnings:
        lines.extend(["## Warnings", ""])
        for warning in warnings:
            lines.append(f"- ⚠️ {warning}")
        lines.append("")
    return "\n".join(lines).rstrip() + "\n", warnings


def write_summary(markdown: str) -> None:
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(markdown)
            if not markdown.endswith("\n"):
                handle.write("\n")
    print(markdown)


def repo_path(repo: Repo, suffix: str) -> str:
    return f"/repos/{repo.owner}/{repo.name}{suffix}"


def get_milestone(gh: GitHub, number: int) -> dict[str, Any]:
    return gh.get(repo_path(gh.repo, f"/milestones/{number}"))


def milestone_issues(gh: GitHub, number: int) -> list[dict[str, Any]]:
    query = urllib.parse.urlencode({"milestone": number, "state": "all", "per_page": 100})
    return gh.paginate(repo_path(gh.repo, f"/issues?{query}"))


def all_milestones(gh: GitHub) -> list[dict[str, Any]]:
    query = urllib.parse.urlencode({"state": "all", "per_page": 100})
    return gh.paginate(repo_path(gh.repo, f"/milestones?{query}"))


def format_errors(errors: list[str]) -> str:
    return "\n".join(f"- {error}" for error in errors)


def policy(args: argparse.Namespace) -> int:
    repo = Repo.from_env()
    gh = GitHub(repo)
    with open(args.event_path, encoding="utf-8") as handle:
        event = json.load(handle)

    errors: list[str] = []
    notices: list[str] = []

    if event.get("milestone"):
        errors.extend(validate_milestone(event["milestone"]))

    issue = event.get("issue")
    if issue and issue.get("milestone"):
        errors.extend(validate_milestone(issue["milestone"]))
        errors.extend(validate_mission(issue))
        _, class_errors = classification(issue)
        errors.extend(class_errors)

    pr = event.get("pull_request")
    if pr:
        refs = closing_issue_numbers(pr.get("body") or "")
        if len(refs) > 1:
            errors.append(f"PR closes more than one Engineering Mission: {', '.join(map(str, refs))}.")
        elif len(refs) == 1:
            if pr.get("base", {}).get("ref") != event.get("repository", {}).get("default_branch"):
                errors.append("A Mission-closing PR must target the default branch.")
            mission = gh.get(repo_path(repo, f"/issues/{refs[0]}"))
            errors.extend(validate_mission(mission))
            version_milestones = [m for m in all_milestones(gh) if VERSION_TITLE_RE.fullmatch(m.get("title") or "")]
            if mission.get("milestone"):
                errors.extend(validate_milestone(mission["milestone"]))
                _, class_errors = classification(mission)
                errors.extend(class_errors)
            elif version_milestones:
                errors.append(f"Mission #{refs[0]} is not assigned to a Version Milestone.")
            else:
                notices.append("No Version Milestone exists yet; Mission→Version membership enforcement is in bootstrap mode.")

    lines = ["# Version policy", ""]
    if errors:
        lines += ["## Errors", "", format_errors(errors), ""]
    if notices:
        lines += ["## Notices", ""] + [f"- {notice}" for notice in notices] + [""]
    if not errors:
        lines += ["✅ Version/Mission policy checks passed.", ""]
    write_summary("\n".join(lines))
    return 1 if errors else 0


def resolve_milestone_number(args: argparse.Namespace, event: dict[str, Any] | None) -> int | None:
    if getattr(args, "milestone", None):
        return int(args.milestone)
    if not event:
        return None
    if event.get("milestone"):
        return int(event["milestone"]["number"])
    issue = event.get("issue")
    if issue and issue.get("milestone"):
        return int(issue["milestone"]["number"])
    return None


def preview(args: argparse.Namespace) -> int:
    repo = Repo.from_env()
    gh = GitHub(repo)
    event = None
    if args.event_path and os.path.exists(args.event_path):
        with open(args.event_path, encoding="utf-8") as handle:
            event = json.load(handle)
    number = resolve_milestone_number(args, event)
    if number is None:
        write_summary("# Changelog preview\n\n_No Version Milestone is associated with this event; nothing to preview._\n")
        return 0
    milestone = get_milestone(gh, number)
    errors = validate_milestone(milestone)
    markdown, _ = render_changelog(milestone, milestone_issues(gh, number))
    if errors:
        markdown += "\n## Version contract errors\n\n" + format_errors(errors) + "\n"
    write_summary(markdown)
    return 1 if errors else 0


CLOSING_PRS_QUERY = """
query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    issue(number: $number) {
      closedByPullRequestsReferences(first: 20) {
        nodes {
          number
          merged
          baseRefName
          url
        }
      }
    }
  }
}
"""


def closing_prs(gh: GitHub, issue_number: int) -> list[dict[str, Any]]:
    data = gh.graphql(
        CLOSING_PRS_QUERY,
        {"owner": gh.repo.owner, "name": gh.repo.name, "number": issue_number},
    )
    issue = data["repository"]["issue"]
    if not issue:
        return []
    return issue["closedByPullRequestsReferences"]["nodes"]


def readiness(args: argparse.Namespace) -> int:
    repo = Repo.from_env()
    gh = GitHub(repo)
    milestone = get_milestone(gh, int(args.milestone))
    errors = validate_milestone(milestone, release_ready=True)
    issues = [issue for issue in milestone_issues(gh, int(args.milestone)) if not issue.get("pull_request")]
    default_branch = args.default_branch

    if not issues:
        errors.append("Version Milestone has no Engineering Missions.")

    for issue in issues:
        errors.extend(validate_mission(issue))
        _, class_errors = classification(issue)
        errors.extend(class_errors)
        if issue.get("state") != "closed":
            errors.append(f"Mission #{issue['number']} is still open.")
            continue
        prs = [
            pr for pr in closing_prs(gh, issue["number"])
            if pr.get("merged") and pr.get("baseRefName") == default_branch
        ]
        if len(prs) != 1:
            errors.append(
                f"Mission #{issue['number']} must have exactly one merged mission-closing PR to `{default_branch}` "
                f"(found {len(prs)})."
            )

    version = version_from_title(milestone.get("title") or "")
    if version:
        encoded = urllib.parse.quote(f"tags/{version}", safe="")
        try:
            gh.get(repo_path(repo, f"/git/ref/{encoded}"))
        except ValidationError as exc:
            if " 404 " not in str(exc):
                raise
        else:
            errors.append(f"Release tag `{version}` already exists.")

    changelog, warnings = render_changelog(milestone, issues)
    lines = [
        f"# Release readiness — {milestone.get('title')}",
        "",
        f"Default branch: `{default_branch}`",
        f"Engineering Missions: **{len(issues)}**",
        "",
    ]
    if errors:
        lines.extend(["## Blocking findings", "", format_errors(errors), ""])
    else:
        lines.extend(["✅ Milestone, Missions, closing PRs, changelog classifications, and tag preconditions are ready.", ""])
    if warnings:
        lines.extend(["## Changelog warnings", "", format_errors(warnings), ""])
    lines.extend(["## Proposed changelog", "", changelog])
    write_summary("\n".join(lines))
    return 1 if errors else 0


def bootstrap_labels(_: argparse.Namespace) -> int:
    repo = Repo.from_env()
    gh = GitHub(repo)
    query = urllib.parse.urlencode({"per_page": 100})
    existing = {
        label["name"]: label
        for label in gh.paginate(repo_path(repo, f"/labels?{query}"))
    }
    lines = ["# Changelog label bootstrap", ""]
    for name, (_, color, description) in CHANGELOG.items():
        if name in existing:
            gh.patch(
                repo_path(repo, f"/labels/{urllib.parse.quote(name, safe='')}"),
                {"color": color, "description": description},
            )
            lines.append(f"- updated `{name}`")
        else:
            gh.post(
                repo_path(repo, "/labels"),
                {"name": name, "color": color, "description": description},
            )
            lines.append(f"- created `{name}`")
    lines.append("")
    write_summary("\n".join(lines))
    return 0


def self_test(_: argparse.Namespace) -> int:
    description = "\n".join(f"## {heading}\nvalue" for heading in REQUIRED_MILESTONE_HEADINGS)
    milestone = {"title": "v0.1.0 — MVP", "description": description}
    assert validate_milestone(milestone) == []
    assert version_from_title(milestone["title"]) == "v0.1.0"
    assert validate_milestone({"title": "MVP", "description": description})
    assert closing_issue_numbers("Closes #12\nPart of #10\nFixes #12") == [12]

    mission_body = "### Outcome\nx\n### Acceptance criteria\n- [ ] y\n### Evidence\nz\n"
    issue = {
        "number": 12,
        "title": "Add useful thing",
        "body": mission_body,
        "state": "closed",
        "labels": [{"name": "changelog:added"}],
    }
    assert validate_mission(issue) == []
    assert classification(issue)[0] == "changelog:added"
    duplicate = {**issue, "labels": [{"name": "changelog:added"}, {"name": "changelog:fixed"}]}
    assert classification(duplicate)[1]

    preview_text, warnings = render_changelog(milestone, [issue])
    assert "## Added" in preview_text and "(#12)" in preview_text
    assert not warnings

    placeholder = {**milestone, "description": description + "\nTBD"}
    assert validate_milestone(placeholder, release_ready=True)
    print("version automation self-test: OK")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)

    policy_parser = sub.add_parser("policy")
    policy_parser.add_argument("--event-path", default=os.environ.get("GITHUB_EVENT_PATH", ""))
    policy_parser.set_defaults(func=policy)

    preview_parser = sub.add_parser("preview")
    preview_parser.add_argument("--event-path", default=os.environ.get("GITHUB_EVENT_PATH", ""))
    preview_parser.add_argument("--milestone")
    preview_parser.set_defaults(func=preview)

    readiness_parser = sub.add_parser("readiness")
    readiness_parser.add_argument("--milestone", required=True, type=int)
    readiness_parser.add_argument("--default-branch", default="main")
    readiness_parser.set_defaults(func=readiness)

    labels_parser = sub.add_parser("bootstrap-labels")
    labels_parser.set_defaults(func=bootstrap_labels)

    test_parser = sub.add_parser("self-test")
    test_parser.set_defaults(func=self_test)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "policy" and not args.event_path:
        raise ValidationError("--event-path or GITHUB_EVENT_PATH is required")
    return args.func(args)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
