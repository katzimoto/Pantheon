#!/usr/bin/env python3
"""Pantheon behavioral skill-evaluation harness.

This is the *probabilistic* half of the skill evaluation layer, kept strictly
separate from the deterministic half: `scripts/check-skill-conformance.sh`
validates SKILL.md frontmatter against the Agent Skills specification and runs
inside `./scripts/verify.sh`; this harness never runs there.

It reads trigger fixtures (`.agents/skills/<name>/evals/evals.json`),
scaffolds a workspace with one run card per (eval, condition), and aggregates
the observations a runner/grader later authors into an inspectable benchmark.
It never calls a model, requires no network or vendor account, and its own
steps are deterministic; the agent runs and the grading judgement happen
separately and are recorded as experimental evidence, not repository authority.

Fixture shape (upstream `evals/evals.json` convention, plus a Pantheon
`trigger` field):

    {
      "skill_name": "<directory name>",
      "evals": [
        {
          "id": "<stable string>",
          "trigger": "positive" | "negative",
          "prompt": "<realistic prompt>",
          "expected_output": "<what correct behavior looks like>",
          "files": [],
          "assertions": ["<verifiable statements>"]
        }
      ]
    }

Observation shape (authored per eval/condition by the runner/grader):

    {
      "eval_id": "<id>",
      "skill": "<skill_name>",
      "condition": "with_skill" | "without_skill" | "previous_skill",
      "agent_surface": "<model/vendor where known>",
      "skill_source": "<git rev or path, or null>",
      "observed_result": "<what the agent actually produced>",
      "grader": "<how it was judged>",
      "assertion_results": [
        {"text": "<assertion>", "passed": true, "evidence": "<quote/reference>"}
      ],
      "coverage": {
        "covered": ["<required acceptance-criteria/invariants the run covered>"],
        "omissions": ["<required items the run missed>"],
        "invented_requirements": ["<requirements the run invented>"]
      },
      "context_cost": {"tokens": 0, "duration_ms": 0, "tool_calls": 0}
    }

The `coverage` fields are what criterion 7 asks a with-skill/without-skill
comparison to make inspectable: required coverage, severe omissions, invented
requirements, and context/tool cost.

Usage:
    scripts/run-skill-evals.py list
    scripts/run-skill-evals.py scaffold WORKSPACE
    scripts/run-skill-evals.py benchmark WORKSPACE
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SKILLS_DIR = ROOT / ".agents" / "skills"

CONDITIONS = ("with_skill", "without_skill")


def _fail(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def _load_fixture(skill_dir: Path) -> dict:
    path = skill_dir / "evals" / "evals.json"
    try:
        data = json.loads(path.read_text())
    except FileNotFoundError:
        _fail(f"missing fixture: {path}")
    except json.JSONDecodeError as exc:
        _fail(f"malformed JSON in {path}: {exc}")
    if not isinstance(data, dict):
        _fail(f"{path}: top level must be an object")
    name = data.get("skill_name")
    if name != skill_dir.name:
        _fail(f"{path}: skill_name {name!r} does not match directory {skill_dir.name!r}")
    evals = data.get("evals")
    if not isinstance(evals, list) or not evals:
        _fail(f"{path}: 'evals' must be a non-empty list")
    for i, ev in enumerate(evals):
        for field in ("id", "trigger", "prompt", "expected_output", "assertions"):
            if field not in ev:
                _fail(f"{path}: eval #{i} missing field {field!r}")
        if ev["trigger"] not in ("positive", "negative"):
            _fail(f"{path}: eval {ev['id']!r} trigger must be 'positive' or 'negative'")
        if not isinstance(ev["assertions"], list) or not ev["assertions"]:
            _fail(f"{path}: eval {ev['id']!r} assertions must be a non-empty list")
    return data


def _fixtures() -> list[tuple[Path, dict]]:
    out = []
    for skill_dir in sorted(p for p in SKILLS_DIR.iterdir() if p.is_dir()):
        if (skill_dir / "evals" / "evals.json").exists():
            out.append((skill_dir, _load_fixture(skill_dir)))
    return out


def cmd_list(_args) -> int:
    found = False
    for skill_dir, data in _fixtures():
        found = True
        print(f"{skill_dir.name}")
        for ev in data["evals"]:
            print(f"  - {ev['id']}  [{ev['trigger']}]")
    if not found:
        print("no skill evaluation fixtures found")
    return 0


def _cards(fixtures) -> list[dict]:
    cards = []
    for skill_dir, data in fixtures:
        rel_skill = skill_dir.relative_to(ROOT).as_posix()
        for ev in data["evals"]:
            for condition in CONDITIONS:
                cards.append(
                    {
                        "eval_id": ev["id"],
                        "skill": data["skill_name"],
                        "trigger": ev["trigger"],
                        "prompt": ev["prompt"],
                        "expected_output": ev["expected_output"],
                        "assertions": list(ev["assertions"]),
                        "condition": condition,
                        "skill_path": f"{rel_skill}/SKILL.md"
                        if condition == "with_skill"
                        else None,
                    }
                )
    return cards


def cmd_scaffold(args) -> int:
    workspace = Path(args.workspace)
    fixtures = _fixtures()
    if args.skill:
        fixtures = [f for f in fixtures if f[1]["skill_name"] == args.skill]
        if not fixtures:
            _fail(f"no fixtures for skill {args.skill!r}")
    if args.eval:
        narrowed = []
        for skill_dir, data in fixtures:
            kept = [e for e in data["evals"] if e["id"] == args.eval]
            if kept:
                narrowed.append((skill_dir, {**data, "evals": kept}))
        fixtures = narrowed
        if not fixtures:
            _fail(f"no fixture with eval id {args.eval!r}")
    if not fixtures:
        _fail("no skill evaluation fixtures found")
    workspace.mkdir(parents=True, exist_ok=True)
    cards = _cards(fixtures)
    (workspace / "run-cards.json").write_text(
        json.dumps(cards, indent=2) + "\n"
    )
    print(f"scaffolded {len(cards)} run cards into {workspace}/run-cards.json")
    print()
    print("Run protocol (one fresh context per card):")
    print("  1. Start a fresh agent context (no leftover state from prior runs).")
    print("  2. For with_skill cards, load the skill at skill_path first; for")
    print("     without_skill cards, do not load it.")
    print("  3. Give the agent the prompt and, where present, the input files.")
    print("  4. Capture the response and record one observation JSON per card")
    print(f"     under {workspace}/observations/ using the observation shape")
    print("     documented at the top of this script.")
    print(f"  5. Run: scripts/run-skill-evals.py benchmark {workspace}")
    return 0


def _read_observations(workspace: Path) -> list[dict]:
    obs_dir = workspace / "observations"
    if not obs_dir.is_dir():
        _fail(f"no observations directory: {obs_dir}")
    out = []
    for path in sorted(obs_dir.glob("*.json")):
        try:
            data = json.loads(path.read_text())
        except json.JSONDecodeError as exc:
            _fail(f"malformed JSON in {path}: {exc}")
        if not isinstance(data, dict):
            _fail(f"{path}: observation must be an object")
        for field in ("eval_id", "skill", "condition", "observed_result", "grader"):
            if field not in data:
                _fail(f"{path}: observation missing field {field!r}")
        if data["condition"] not in ("with_skill", "without_skill", "previous_skill"):
            _fail(f"{path}: unknown condition {data['condition']!r}")
        out.append(data)
    return out


def _summary(obs: dict) -> dict:
    results = obs.get("assertion_results") or []
    passed = sum(1 for r in results if r.get("passed"))
    total = len(results)
    coverage = obs.get("coverage") or {}
    cost = obs.get("context_cost") or {}
    return {
        "agent_surface": obs.get("agent_surface"),
        "skill_source": obs.get("skill_source"),
        "observed_result": obs["observed_result"],
        "grader": obs["grader"],
        "assertion_results": results,
        "pass_rate": (passed / total) if total else None,
        "passed": passed,
        "total": total,
        "coverage": coverage,
        "context_cost": cost,
    }


def _delta(a: dict | None, b: dict | None) -> dict:
    def num(d, key, default=0):
        if not d or d.get(key) is None:
            return default
        return d[key]

    pa = num(a, "pass_rate")
    pb = num(b, "pass_rate")
    return {
        "pass_rate": (None if (a is None or b is None or a.get("pass_rate") is None
                              or b.get("pass_rate") is None)
                      else pa - pb),
        "tokens": num(a, "context_cost", {}).get("tokens", 0)
        - num(b, "context_cost", {}).get("tokens", 0),
        "duration_ms": num(a, "context_cost", {}).get("duration_ms", 0)
        - num(b, "context_cost", {}).get("duration_ms", 0),
        "tool_calls": num(a, "context_cost", {}).get("tool_calls", 0)
        - num(b, "context_cost", {}).get("tool_calls", 0),
    }


def cmd_benchmark(args) -> int:
    workspace = Path(args.workspace)
    cards_path = workspace / "run-cards.json"
    if not cards_path.exists():
        _fail(f"missing run-cards.json; run `scaffold {workspace}` first")
    cards = json.loads(cards_path.read_text())
    observations = _read_observations(workspace)

    known = {(c["eval_id"], c["condition"]) for c in cards}
    for obs in observations:
        key = (obs["eval_id"], obs["condition"])
        if key not in known:
            _fail(f"observation {obs['eval_id']}/{obs['condition']} has no matching run card")

    by_eval: dict[str, dict] = {}
    for card in cards:
        e = by_eval.setdefault(
            card["eval_id"],
            {
                "eval_id": card["eval_id"],
                "skill": card["skill"],
                "trigger": card["trigger"],
                "prompt": card["prompt"],
                "expected_output": card["expected_output"],
                "assertions": card["assertions"],
                "conditions": {},
            },
        )
        e["conditions"].setdefault(card["condition"], None)

    for obs in observations:
        entry = by_eval[obs["eval_id"]]
        entry["conditions"][obs["condition"]] = _summary(obs)

    evals_out = []
    for eval_id, entry in by_eval.items():
        entry["delta"] = _delta(
            entry["conditions"].get("with_skill"),
            entry["conditions"].get("without_skill"),
        )
        evals_out.append(entry)

    benchmark = {
        "generated_by": "scripts/run-skill-evals.py benchmark",
        "evals": evals_out,
        "raw_observations": observations,
    }
    (workspace / "benchmark.json").write_text(
        json.dumps(benchmark, indent=2) + "\n"
    )
    print(f"wrote {workspace}/benchmark.json ({len(evals_out)} evals, "
          f"{len(observations)} observations)")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Pantheon skill-evaluation harness")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("list", help="list skills with evaluation fixtures")
    p = sub.add_parser("scaffold", help="scaffold run cards for fixtures")
    p.add_argument("workspace", help="workspace directory to create")
    p.add_argument("--skill", help="restrict to one skill name")
    p.add_argument("--eval", help="restrict to one eval id")
    p = sub.add_parser("benchmark", help="aggregate observations into a benchmark")
    p.add_argument("workspace", help="workspace directory produced by scaffold")
    args = parser.parse_args(argv)

    if args.command == "list":
        return cmd_list(args)
    if args.command == "scaffold":
        return cmd_scaffold(args)
    if args.command == "benchmark":
        return cmd_benchmark(args)
    _fail(f"unknown command: {args.command}")
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
