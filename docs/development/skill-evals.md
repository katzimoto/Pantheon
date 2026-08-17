# Skill Evaluation

## Status

**Canonical for how Pantheon's Agent Skills are validated and behaviorally
evaluated.** It has no authority over what Pantheon *is*
(`docs/architecture/`), where code belongs
(`docs/development/implementation.md`), or how an agent operates generally
(`AGENTS.md`). Behavioral evaluation results are experimental evidence, never
repository authority; only the deterministic conformance check is part of the
correctness gate.

## Two layers, one boundary

Skill evaluation splits deliberately into a deterministic gate and a
probabilistic evidence path. The split is the point: no live model, network,
vendor account, or subjective judgement may change whether an unrelated
Pantheon change passes `./scripts/verify.sh`.

| Layer | Tool | Runs in `./scripts/verify.sh`? | Nature |
|---|---|---|---|
| Format/spec conformance | `scripts/check-skill-conformance.sh` | Yes | Deterministic, POSIX shell |
| Behavioral trigger/workflow value | `scripts/run-skill-evals.py` | **No** | Probabilistic, manual/explicit, experimental |

### Conformance: the deterministic gate

`scripts/check-skill-conformance.sh` validates every
`.agents/skills/<name>/SKILL.md` against the stable, machine-checkable
constraints of the [Agent Skills specification](https://agentskills.io/specification):

- frontmatter opens with `---` on line 1, closes with a matching `---`, and is
  followed by a non-empty Markdown body;
- `name` is present exactly once, 1-64 characters, lowercase letters/digits
  separated by single hyphens, and matches the skill's directory name;
- `description` is present exactly once, non-empty, at most 1024 characters;
- `compatibility`, when present, is at most 500 characters;
- `metadata`, when present, is a flat map from string keys to string values;
- no two canonical skills share a `name` (duplicate identity).

It complements, and does not replace, `scripts/check-skill-symlinks.sh`, which
still enforces the one-canonical-body and vendor-symlink rules. Its
`--self-test` mode runs the same validator against a disposable scratch tree —
one conforming skill plus one deliberately broken skill per failure class — so
the negative cases are proven to be rejected, not merely claimed.

Pantheon deliberately does **not** run the upstream `skills-ref` reference
library in the gate. It is published as demonstration tooling, and letting an
unpinned, evolving external tool sit inside the canonical verification path
would let it redefine pass/fail for unrelated changes. The conformance check is
repository-local POSIX shell, so it needs nothing but the repository itself.

### Behavioral evaluation: the separate, experimental path

Triggering and workflow value cannot be established by a frontmatter check:
the question is whether the skill activates on realistic Pantheon prompts,
stays dormant on near-misses, and improves invariant/acceptance-criterion
coverage without inventing requirements. Those are probabilistic questions, so
their answers are recorded as experimental evidence rather than gate results.

`scripts/run-skill-evals.py` is the thin harness around that path. It never
calls a model and needs no network; it scaffolds run cards from fixtures,
accepts the observations a runner/grader authors, and aggregates them into an
inspectable benchmark.

```
scripts/run-skill-evals.py list
scripts/run-skill-evals.py scaffold <workspace> [--skill NAME] [--eval ID]
scripts/run-skill-evals.py benchmark <workspace>
```

## Evaluation fixtures

A canonical skill may carry trigger fixtures at
`.agents/skills/<name>/evals/evals.json`, in the upstream Agent Skills
evaluation shape plus one Pantheon field, `trigger`:

```json
{
  "skill_name": "example-skill",
  "evals": [
    {
      "id": "stable-id",
      "trigger": "positive",
      "prompt": "a realistic Pantheon prompt",
      "expected_output": "what correct behavior looks like",
      "files": [],
      "assertions": ["verifiable statements about the expected behavior"]
    }
  ]
}
```

`trigger` is `"positive"` when the skill should activate, `"negative"` when it
should stay dormant (a near-miss or documented non-trigger). Fixtures may
reference real missions and contracts — they assert *evaluation targets*, they
do not copy acceptance criteria or architecture semantics, which remain owned
by the mission Issues and `docs/architecture/` respectively.

The initial fixtures cover a deliberately small representative subset rather
than the whole catalog: `persistence-and-recovery-transaction-review` (drawn
from #16–#18 and #42), `dependency-change-procedure` (from #41), and
`pantheon-mission-planning` (used by every mission). Those are the skills whose
trigger boundaries map most directly to real mission scenarios; the remaining
skills grow fixtures the same way when their boundaries need regression
evidence.

## Running a with/without comparison

1. Scaffold run cards: `scripts/run-skill-evals.py scaffold <workspace>`.
   Each card names the eval, the condition (`with_skill` / `without_skill`),
   the prompt, the assertions, and (for `with_skill`) the skill path.
2. Run each card in a **fresh agent context**: load the skill first for
   `with_skill`, do not load it for `without_skill`, then give the prompt.
3. Record one observation per card under `<workspace>/observations/`. An
   observation preserves, per `docs/development/agent-skills-and-hooks.md`'s
   evidence requirement: scenario identity (`eval_id`, prompt), skill
   version/source, agent/model surface, observed result, grader/rationale,
   per-assertion pass/evidence, a `coverage` map (`covered`, `omissions`,
   `invented_requirements`), and `context_cost` (tokens, duration, tool calls).
4. Aggregate: `scripts/run-skill-evals.py benchmark <workspace>`, which writes
   `<workspace>/benchmark.json` with the per-condition summaries and the
   with-vs-without delta.

The observation and benchmark shapes are documented in full at the top of
`scripts/run-skill-evals.py`.

## Adding and updating evaluations

**Add** a fixture when a skill's trigger/non-trigger boundary is worth
regression evidence: add `evals/evals.json` under the skill, run `list` to
confirm it parses, then run the comparison. Fixtures are cheap; keep the
initial set to one positive and one near-miss negative drawn from a real
mission rather than a generic prompt.

**Update** a fixture when the skill's trigger or non-trigger conditions change,
when a mission changes the workflow the skill operationalizes (a new invariant
family, a new contract anchor), or when an assertion proves to always pass or
always fail in both conditions and so measures nothing.

## What each kind of check is for

| Check | Owns | Deterministic? | In the gate? |
|---|---|---|---|
| `scripts/check-skill-conformance.sh` | SKILL.md frontmatter/spec shape | Yes | Yes |
| `scripts/check-skill-symlinks.sh` | one canonical body + vendor symlinks | Yes | Yes |
| `scripts/check-hooks.sh` | lifecycle-hook *scripts* behave (scratch-repo self-test) | Yes | Yes |
| ordinary Rust tests (`cargo test`) | crate behavior | Yes | Yes |
| `scripts/run-skill-evals.py` | skill *triggering and workflow value* | No (agent runs are probabilistic) | **No** |

Conformance, symlinks, hook self-tests, and Rust tests are mechanical and belong
to the gate. Behavioral skill evaluation is a judgement about an agent's
behavior and must never become a mechanical pass/fail substitute for
`pantheon-independent-review` (`docs/development/change-lifecycle.md`).

## Recorded demonstration

`.agents/skills/persistence-and-recovery-transaction-review/evals/results/`
holds one worked with-skill/without-skill comparison for the
`store-revision-cas-review` fixture (drawn from #17). It is experimental
evidence — a single author-run demonstration, not a claim about every agent or
model — and shows the shape a recorded comparison takes:
`run-cards.json`, `observations/`, and `benchmark.json`.
