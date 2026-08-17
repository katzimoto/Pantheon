#!/usr/bin/env python3
"""Repository-local GitHub label policy for Pantheon.

Labels are deliberately narrow: release projection, broad subsystem discovery,
and contributor discovery. Native GitHub state owns lifecycle, version,
dependencies, hierarchy, ownership, and review status.
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

AREA_COLOR = "c5def5"
AREA_LABELS = {
    "area:goals-planning": (AREA_COLOR, "Goals, planning, and TaskGraph materialization."),
    "area:tasks": (AREA_COLOR, "Task model, dependencies, and lifecycle."),
    "area:scheduling": (AREA_COLOR, "Scheduling, admission, claims, and resource ownership."),
    "area:execution": (AREA_COLOR, "Execution Fabric, Runs, Attempts, and backend control."),
    "area:agents-context": (AREA_COLOR, "Logical Agents, context, memory, and skills."),
    "area:evaluation-acceptance": (AREA_COLOR, "Evaluation, evidence, acceptance, and completion."),
    "area:artifacts-workspaces": (AREA_COLOR, "Artifacts, CAS, workspaces, and Git integration."),
    "area:security": (AREA_COLOR, "Authorization, sandboxing, secrets, and credentials."),
    "area:persistence-recovery": (AREA_COLOR, "SQLite persistence, recovery, backup, and restore."),
    "area:operations": (AREA_COLOR, "Operator API/CLI, configuration, observability, and daemon operations."),
    "area:repository": (AREA_COLOR, "Repository tooling, CI, documentation process, and development infrastructure."),
}

CONTRIBUTOR_LABELS = {
    "good first issue": ("7057ff", "Good for newcomers"),
    "help wanted": ("008672", "Extra attention is needed"),
}

DEPRECATED_DEFAULTS = (
    "accessibility",
    "bug",
    "documentation",
    "duplicate",
    "enhancement",
    "invalid",
    "question",
    "wontfix",
)

APPROVED_MANAGED_LABELS = {**AREA_LABELS, **CONTRIBUTOR_LABELS}
CLOSING_RE = re.compile(
    r"(?im)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#(?P<number>\d+)\b"
)


class PolicyError(RuntimeError):
    pass


@dataclass(frozen=True)
class Repo:
    owner: str
    name: str

    @classmethod
    def from_env(cls) -> "Repo":
        value = os.environ.get("GITHUB_REPOSITORY", "")
        if "/" not in value:
            raise PolicyError("GITHUB_REPOSITORY must be owner/name")
        owner, name = value.split("/", 1)
        return cls(owner, name)


class GitHub:
    def __init__(self, repo: Repo):
        self.repo = repo
        self.api = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")
        self.token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
        if not self.token:
            raise PolicyError("GH_TOKEN or GITHUB_TOKEN is required")

    def request(self, method: str, path: str, data: Any | None = None) -> tuple[Any, dict[str, str]]:
        url = path if path.startswith("http") else f"{self.api}{path}"
        body = None if data is None else json.dumps(data).encode()
        request = urllib.request.Request(
            url,
            data=body,
            method=method,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self.token}",
                "X-GitHub-Api-Version": "2022-11-28",
                "User-Agent": "pantheon-repository-labels",
                "Content-Type": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(request) as response:
                payload = response.read()
                parsed = json.loads(payload) if payload else None
                return parsed, dict(response.headers.items())
        except urllib.error.HTTPError as exc:
            payload = exc.read().decode(errors="replace")
            raise PolicyError(f"GitHub API {method} {url} failed: {exc.code} {payload}") from exc

    def get(self, path: str) -> Any:
        return self.request("GET", path)[0]

    def post(self, path: str, data: Any) -> Any:
        return self.request("POST", path, data)[0]

    def patch(self, path: str, data: Any) -> Any:
        return self.request("PATCH", path, data)[0]

    def delete(self, path: str) -> None:
        self.request("DELETE", path)

    def paginate(self, path: str) -> list[Any]:
        values: list[Any] = []
        current: str | None = path
        while current:
            payload, headers = self.request("GET", current)
            if not isinstance(payload, list):
                raise PolicyError(f"expected list from {current}")
            values.extend(payload)
            current = parse_next(headers.get("Link"))
        return values


def repo_path(repo: Repo, suffix: str) -> str:
    return f"/repos/{repo.owner}/{repo.name}{suffix}"


def parse_next(link: str | None) -> str | None:
    if not link:
        return None
    for part in link.split(","):
        if 'rel="next"' in part:
            match = re.search(r"<([^>]+)>", part)
            return match.group(1) if match else None
    return None


def label_names(issue: dict[str, Any]) -> list[str]:
    return [label.get("name", "") for label in issue.get("labels") or []]


def validate_areas(issue: dict[str, Any]) -> list[str]:
    names = label_names(issue)
    areas = sorted(name for name in names if name.startswith("area:"))
    errors: list[str] = []
    unknown = [name for name in areas if name not in AREA_LABELS]
    if unknown:
        errors.append(f"Mission #{issue.get('number')} has unknown area label(s): {', '.join(unknown)}.")
    if len(areas) == 0:
        errors.append(f"Mission #{issue.get('number')} must have one primary `area:*` label.")
    elif len(areas) > 2:
        errors.append(
            f"Mission #{issue.get('number')} may have one primary and at most one secondary `area:*` label "
            f"(found {len(areas)})."
        )
    return errors


def closing_issue_numbers(body: str) -> list[int]:
    return sorted({int(match.group("number")) for match in CLOSING_RE.finditer(body or "")})


def plan_reconciliation(existing: dict[str, dict[str, Any]]) -> list[tuple[str, str]]:
    actions: list[tuple[str, str]] = []
    for name, (color, description) in APPROVED_MANAGED_LABELS.items():
        current = existing.get(name)
        if current is None:
            actions.append(("create", name))
        elif current.get("color", "").lower() != color or (current.get("description") or "") != description:
            actions.append(("update", name))
    for name in DEPRECATED_DEFAULTS:
        if name in existing:
            actions.append(("delete", name))
    return actions


def write_summary(markdown: str) -> None:
    path = os.environ.get("GITHUB_STEP_SUMMARY")
    if path:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(markdown)
            if not markdown.endswith("\n"):
                handle.write("\n")
    print(markdown)


def reconcile(_: argparse.Namespace) -> int:
    repo = Repo.from_env()
    gh = GitHub(repo)
    labels = gh.paginate(repo_path(repo, "/labels?per_page=100"))
    existing = {label["name"]: label for label in labels}
    actions = plan_reconciliation(existing)
    lines = ["# Repository label reconciliation", ""]
    for action, name in actions:
        if action == "create":
            color, description = APPROVED_MANAGED_LABELS[name]
            gh.post(repo_path(repo, "/labels"), {"name": name, "color": color, "description": description})
        elif action == "update":
            color, description = APPROVED_MANAGED_LABELS[name]
            encoded = urllib.parse.quote(name, safe="")
            gh.patch(repo_path(repo, f"/labels/{encoded}"), {"color": color, "description": description})
        else:
            encoded = urllib.parse.quote(name, safe="")
            gh.delete(repo_path(repo, f"/labels/{encoded}"))
        lines.append(f"- {action}d `{name}`")
    if not actions:
        lines.append("- taxonomy already reconciled")
    lines.append("")
    write_summary("\n".join(lines))
    return 0


def validate_event(args: argparse.Namespace) -> int:
    repo = Repo.from_env()
    gh = GitHub(repo)
    with open(args.event_path, encoding="utf-8") as handle:
        event = json.load(handle)
    errors: list[str] = []
    issue = event.get("issue")
    if issue and issue.get("milestone"):
        errors.extend(validate_areas(issue))
    pr = event.get("pull_request")
    if pr:
        refs = closing_issue_numbers(pr.get("body") or "")
        if len(refs) == 1:
            mission = gh.get(repo_path(repo, f"/issues/{refs[0]}"))
            if mission.get("milestone"):
                errors.extend(validate_areas(mission))
    markdown = "# Mission area policy\n\n"
    if errors:
        markdown += "\n".join(f"- {error}" for error in errors) + "\n"
    else:
        markdown += "✅ Mission area-label policy checks passed.\n"
    write_summary(markdown)
    return 1 if errors else 0


def validate_milestone(args: argparse.Namespace) -> int:
    repo = Repo.from_env()
    gh = GitHub(repo)
    query = urllib.parse.urlencode({"milestone": args.milestone, "state": "all", "per_page": 100})
    issues = gh.paginate(repo_path(repo, f"/issues?{query}"))
    errors: list[str] = []
    for issue in issues:
        if not issue.get("pull_request"):
            errors.extend(validate_areas(issue))
    markdown = f"# Mission area readiness — Milestone #{args.milestone}\n\n"
    if errors:
        markdown += "\n".join(f"- {error}" for error in errors) + "\n"
    else:
        markdown += "✅ Every Engineering Mission has a valid area classification.\n"
    write_summary(markdown)
    return 1 if errors else 0


def self_test(_: argparse.Namespace) -> int:
    base = {"number": 1, "labels": [{"name": "area:execution"}]}
    assert validate_areas(base) == []
    assert validate_areas({"number": 1, "labels": [{"name": "area:execution"}, {"name": "area:security"}]}) == []
    assert validate_areas({"number": 1, "labels": []})
    assert validate_areas({"number": 1, "labels": [{"name": "area:made-up"}]})
    assert validate_areas({"number": 1, "labels": [
        {"name": "area:execution"}, {"name": "area:security"}, {"name": "area:tasks"}
    ]})

    existing = {
        "bug": {"name": "bug", "color": "d73a4a", "description": "Something isn't working"},
        "good first issue": {"name": "good first issue", "color": "000000", "description": "wrong"},
        "custom:keep": {"name": "custom:keep", "color": "ffffff", "description": "preserve"},
    }
    plan = plan_reconciliation(existing)
    assert ("delete", "bug") in plan
    assert ("update", "good first issue") in plan
    assert ("create", "area:execution") in plan
    assert not any(name == "custom:keep" for _, name in plan)
    print("repository label policy self-test: OK")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("reconcile").set_defaults(func=reconcile)
    event = commands.add_parser("validate-event")
    event.add_argument("--event-path", default=os.environ.get("GITHUB_EVENT_PATH", ""))
    event.set_defaults(func=validate_event)
    milestone = commands.add_parser("validate-milestone")
    milestone.add_argument("--milestone", required=True, type=int)
    milestone.set_defaults(func=validate_milestone)
    commands.add_parser("self-test").set_defaults(func=self_test)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "validate-event" and not args.event_path:
        raise PolicyError("--event-path or GITHUB_EVENT_PATH is required")
    return args.func(args)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except PolicyError as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
