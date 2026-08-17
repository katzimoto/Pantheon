# Agent Context Evaluation (Issue #49)

## Status

Evaluation evidence, not canonical authority. This records the baseline and
candidate measurements and the fresh-context runs behind Issue #49's
conclusion. It deliberately reuses Issue #48's evaluation conventions
(`docs/development/skill-evals.md`, observation and benchmark shapes) rather
than introducing an incompatible format. The scenarios are evaluation
targets; canonical documents in `docs/`, `AGENTS.md` and `./scripts/verify.sh`
remain the source of truth.

## What was measured

The hot/common agent context is what a fresh coding agent loads without
selective retrieval: `AGENTS.md` (auto-loaded on every agent surface) and the
navigation layer the `AGENTS.md` start-here sequence sends every fresh agent
to — `docs/README.md` and `docs/architecture/README.md`. The architecture map
was the largest and fastest-growing member.

| Document | Baseline | Candidate A | Change |
|---|---|---|---|
| `AGENTS.md` | 173 lines / 7,816 B | 174 lines / 7,868 B | +1 line (navigation-accuracy pointer to `recipes.md`) |
| `docs/architecture/README.md` | 304 lines / 16,675 B | 217 lines / 12,871 B | −87 lines / −3,804 B |
| `docs/architecture/recipes.md` | (did not exist) | 99 lines / 4,620 B | conditional-retrieval document |
| Always-loaded navigation layer (AGENTS.md + architecture README) | 477 lines / 24,491 B | 391 lines / 20,739 B | −86 lines (−18.0%) / −3,752 B (−15.3%) |

Baseline retrieved at commit `b42af4f`; candidate A is that tree plus the
uncommitted candidate edits recorded below. Token estimates at ~4 characters
per token put the always-loaded layer near 6,100 tokens at baseline and
5,185 tokens in candidate A (≈ −15%).

## The retrieval sequence a fresh agent follows

From `AGENTS.md` "Start here" (unchanged by the candidate):

1. Establish the task from the Issue: requested outcome, acceptance criteria.
2. Read `docs/README.md` (what is canonical, source-of-truth precedence).
3. Read `docs/architecture/overview.md` only if the system model is needed.
4. Use `docs/architecture/README.md` to find the domain; follow the matching
   change-specific reading recipe (in `docs/architecture/recipes.md` under
   candidate A) instead of reading the tree.
5. Read only the contracts it names, then schemas, then implementation/tests.
6. For a code change, read `docs/development/implementation.md` to place it.
7. Decide the smallest correct change.

## Layouts compared

- **Baseline**: `docs/architecture/README.md` as one combined map + recipe
  document ("Reading recipes" section inline, 96 lines).
- **Candidate A**: `docs/architecture/README.md` kept as the mechanically
  checked canonical inventory/map with a clear link to a new secondary
  document `docs/architecture/recipes.md` carrying the conditional
  change-specific reading recipes. `AGENTS.md`'s start-here step 4 names the
  recipe location so the recipe path stays deterministic.
- **AGENTS.md further trimming**: considered and rejected. Every remaining
  `AGENTS.md` section is in the universally-visible class Issue #49 protects
  (authority/precedence, skill triggers, start-here, scope, `verify.sh`,
  completion/handoff); the only removable section ("Changing architecture")
  would either drop a unique operating rule or merely relocate bytes inside
  the always-loaded navigation path, producing no hot-context reduction.
  Per the mission's own instruction, the smaller layout is not forced.

## Fresh-context comparison

Two representative Engineering Mission scenarios were run in fresh agent
contexts against each layout, following the run-card/observation/benchmark
shape from `docs/development/skill-evals.md`:

- `persistence-cas-mission` — a persistence/recovery mission (revisioned
  authoritative write + CAS predicate in `pantheon-store`).
- `skill-evals-infrastructure` — a repository-infrastructure mission (skill
  conformance/behavioral-evaluation harness).

Cards, observations and the aggregated benchmark live alongside this file:
`run-cards.json`, `observations/`, `benchmark.json`. Each observation was
graded against the fixture's five assertions: canonical document retrieval,
correct crate/domain or scripts/docs placement, mandatory skill use, canonical
`./scripts/verify.sh` verification, and scope discipline.

**Result: no regression.** Both layouts scored 10/10 assertions across the two
scenarios. Candidate A agents still retrieved the persistence-and-recovery
contracts, placed code in `crates/pantheon-store`, invoked the mandatory
skills, planned `./scripts/verify.sh`, and — critically — followed the recipes
link from the split map into `docs/architecture/recipes.md`, preserving the
conditional-retrieval consults that the baseline carried inline. The
repository-infrastructure scenario showed no navigation degradation either
(the change correctly routes to `docs/development/` rather than the
architecture map, unchanged by the split).

## Conclusion

Candidate A (recipes split) is a measurable hot-context reduction (≈ −15%
bytes in the always-loaded navigation layer, −28.6% lines in the architecture
README) with no regression on the representative evaluation set, so it is the
layout Issue #49 lands. `AGENTS.md` is retained at its current size because
the evaluation and the mission's own constraints support keeping it.

## Reproduction

1. Check out the baseline tree, run the two scenario prompts in fresh agent
   contexts (see `run-cards.json`), and grade against the assertions in
   `scenarios.json`.
2. Apply candidate A (split `docs/architecture/recipes.md` out of
   `docs/architecture/README.md`; keep the map linked), repeat the runs.
3. Compare via `benchmark.json`; the delta reports candidate − baseline.

Agent surface for the recorded runs: OpenCode `general` subagents
(`deepseek-v4-flash-free`), single-session author demonstration, planning-only
(no file edits). `context_cost.tokens` and `tool_calls` are estimates derived
from the files each run reported reading; duration was not instrumented for
subagent runs.