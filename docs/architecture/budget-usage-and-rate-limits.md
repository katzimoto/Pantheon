# Budget, Usage, and Rate Limits

## Status

Draft design — Pantheon accounting and consumption-control specification.

## Purpose

This document defines how Pantheon represents finite consumption budgets, factual execution usage, external allowance mirrors, and replenishing rate limits without collapsing them into the Resource Ledger.

The central rule is:

> **Context capacity, reservable resources, cumulative budgets, factual usage, and rate limits are different control-plane concepts and must remain separate.**

See also:

- `docs/architecture/execution-fabric.md`
- `docs/architecture/execution-offer-routing-and-admission-handshake.md`
- `docs/architecture/scheduler-resource-ledger-and-admission.md`
- `docs/architecture/scheduler-reservations-ownership-and-leases.md`
- `docs/architecture/run-and-attempt.md`

## Concept separation

```text
CONTEXT CAPACITY
  Compatibility: can this execution support the required context size?

RESOURCE LEDGER
  Reversible capacity: memory, worktrees, concurrency, sandbox capacity.

BUDGET LEDGER
  Finite cumulative allowance: tokens, currency, credits, premium units.

USAGE LEDGER
  Immutable factual metering: what was actually consumed.

RATE-LIMIT SIGNALS
  Replenishing temporary availability: requests/minute, tokens/minute, retry-after.
```

None of these is a substitute for another.

## Context capacity is compatibility, not consumption

A requirement such as:

```yaml
context:
  minTokens: 64000
```

means only that an execution candidate must provide at least that much usable context capacity. Context size is not reserved, depleted, or settled as a budget.

Actual input/output/cache token consumption is Usage.

## Usage Ledger

The Usage Ledger is append-only factual accounting.

A normalized UsageRecord is attributable to the control-plane entity that caused the consumption and contains one or more extensible measurements.

Conceptual shape:

```yaml
usage:
  id: usage_123

  attribution:
    goal: goal_12
    task: task_31
    run: run_44
    attempt: attempt_2
    agent: agent://coder
    backend: executor://a

  source:
    operationId: native-or-adapter-stable-id
    revision: 1

  measurements:
    - meter: genai.input_tokens
      quantity: 52000
      quality: provider-reported

    - meter: genai.output_tokens
      quantity: 8700
      quality: provider-reported

    - meter: genai.cache_read_input_tokens
      quantity: 34000
      quality: provider-reported

  occurredAt: ...
```

Field names are conceptual; the semantic boundary is normative.

### Extensible meters

Tokens are important but not universal. Standard meters may include:

```text
genai.input_tokens
genai.output_tokens
genai.cache_read_input_tokens
genai.cache_write_input_tokens
genai.reasoning_output_tokens
request.count
tool.web_search_requests
time.wall_ms
```

Future adapter-specific meters may use namespaced identifiers.

Missing usage is `UNKNOWN`, never zero.

## Canonical token normalization

Pantheon canonical input-token totals represent total input processed, including cached input.

Cache read/write counters are retained as breakdowns when available.

Conceptually:

```text
genai.input_tokens = total input presented/processed

cache_read_input_tokens
cache_write_input_tokens
= subsets/breakdowns where reported
```

Backend adapters normalize provider-native token semantics into this canonical form.

Likewise, canonical output tokens represent total generated output; reasoning-output tokens may be exposed as a subset when available.

Pantheon core never implements provider-specific tokenizer logic.

## Usage quality

Useful normalized qualities are:

```text
EXACT
PROVIDER_REPORTED
ESTIMATED
UNKNOWN
```

`UNKNOWN` must not silently become `0`.

Budget policies may require a minimum acceptable metering quality.

## Usage is not Charge

A UsageMeasurement records what happened.

A Charge records what finite allowance that usage consumed.

Examples:

```text
100k tokens
→ may cost different currency amounts on different tariffs
→ may consume credits on one backend
→ may have zero incremental monetary charge on another
```

Therefore token counts are not a universal Pantheon cost currency.

Conceptual ChargeRecord:

```yaml
charge:
  id: charge_123
  usage: usage_123

  unit: unit://currency/USD/micro
  amount: 18400

  tariff:
    revision: tariff_82
    hash: sha256:...

  source: adapter-calculated
```

Alternative units may include backend-specific credit/premium-unit namespaces.

Historical ChargeRecords retain the tariff revision/hash used at the time. Pantheon must never reconstruct historical cost by multiplying old usage by current pricing.

## BudgetAccount

A BudgetAccount represents a finite cumulative allowance for one unit and one accounting period.

Conceptual shape:

```yaml
budget:
  id: budget://goal/goal-123/tokens

  unit: unit://genai/token

  scope:
    goal: goal-123

  authority:
    kind: pantheon

  enforcement: hard

  period:
    type: lifetime

  limit: 2000000

  accounting:
    consumed: 620000
    held: 180000
```

For Pantheon-authoritative accounts:

```text
available = limit - consumed - held
```

If actual usage exceeds the configured limit, Pantheon records the real consumed quantity and marks the account overdrawn. Usage is never clamped to a configured ceiling.

## Multiple applicable budgets

One operation may be subject to several cumulative budgets at once, for example:

```text
Goal token ceiling
Project currency ceiling
External premium-credit allowance
```

All applicable hard Pantheon-authoritative budgets must permit the operation.

The accounting model must not assume only one Goal-level budget exists.

## BudgetHold

A BudgetHold prevents concurrent work from independently observing the same remaining allowance and collectively overspending it.

Example:

```text
limit = 100k
consumed = 0

Run A hold = 80k
available = 20k

Run B request = 80k
→ denied
```

BudgetHold is not a ResourceReservation:

```text
ResourceReservation
  all unused capacity returns after safe release.

BudgetHold
  actual spend becomes permanently consumed;
  only unused headroom returns.
```

## Hold ownership

Worker execution normally uses a Run-scoped BudgetHold shared by that Run's Attempts.

```text
Run hold = 150k
Attempt 1 consumes 40k
Attempt 2 consumes 60k
Run consumed = 100k
unused hold = 50k
```

Failed Attempts do not refund actual usage.

Control-plane intelligence may also consume budget outside worker Runs, for example planning, semantic Agent selection, acceptance review, reflection, or skill evaluation.

Therefore BudgetHold uses a generic holder reference, conceptually:

```yaml
holder:
  kind: run
  ref: run_123
```

or:

```yaml
holder:
  kind: control-operation
  ref: planning_456
```

This prevents orchestration overhead from becoming invisible spend.

## Initial tranches and hold extension

A Run should not reserve an entire Goal budget.

Instead, the initial Run commitment creates an initial budget tranche sized by policy/estimate.

When remaining held headroom approaches a threshold, the Run Controller may request an atomic hold extension.

Conceptual flow:

```text
Run hold 100k
consumed 92k
      ↓
request +50k
      ↓
Budget Controller
      ↓
recheck applicable budgets / policy / current period
      ↓
extend | deny | require approval
```

The worker/model cannot grant itself more budget.

A denied extension is a policy/accounting fact; Failure/Retry/Escalation decides the higher-level response.

## Enforcement classes

Execution systems differ in how precisely Pantheon can bound future spend.

Pantheon therefore distinguishes:

```text
HARD
  Pantheon/backend can prevent further charge before violating the configured bound.

GUARDED
  Pantheon can meter/enforce at safe operation boundaries, but one bounded operation may overshoot.

OBSERVATIONAL
  Pantheon can monitor or estimate usage but cannot guarantee the ceiling.
```

If a user/operator requires a hard budget, Agent + ExecutionOffer candidates that cannot provide compatible metering/enforcement are invalid unless the requirement is explicitly relaxed.

Pantheon must not claim a hard token/cost guarantee for an opaque execution path it cannot measure or stop safely.

## Metering capabilities in ExecutionOffer

ExecutionOffer may advertise factual normalized metering capabilities, for example:

```yaml
metering:
  genai.input_tokens:
    precision: provider-reported
    enforcement: guarded

  genai.output_tokens:
    precision: provider-reported
    enforcement: guarded

  unit://currency/USD/micro:
    precision: exact
```

These are mechanism facts, not quality or preference scores.

Routing validates required budget/metering compatibility before ranking feasible candidates.

## External allowance mirrors

Some finite allowances are authoritative outside Pantheon, such as subscription credits or externally managed spend pools.

Pantheon may mirror them:

```yaml
budget:
  authority:
    kind: external
    source: executor://a

  observed:
    remaining: ...
    observedAt: ...
    sourceRevision: ...
```

For external accounts, Pantheon's snapshot is not the final billing authority.

Other consumers may spend the same allowance outside Pantheon, so freshness is part of the accounting state.

Pantheon may conservatively derive effective local headroom from external observed remaining minus its own local holds, but the upstream system may still reject usage.

If the external allowance cannot be measured numerically, Pantheon records factual coarse state such as:

```text
available
constrained
exhausted
unknown
```

It must not invent a numerical balance.

## Budget periods and reset

Budgets may be lifetime or periodic.

A reset does not erase historical consumption.

Conceptually:

```text
BudgetAccount
  ├─ Period 17: consumed ...
  └─ Period 18: consumed 0 ...
```

External reset time/revision comes from the external authority when available rather than being guessed by Pantheon.

## Idempotent usage ingestion

Usage events may be replayed after reconnect or adapter restart.

Every normalized chargeable operation therefore needs stable source identity or equivalent monotonic checkpointing.

```text
same source operation reported twice
→ one accounting debit
```

Adapters that receive cumulative counters must normalize them into monotonic deltas/checkpoints so reconnect/replay does not double-charge.

## Usage attribution and aggregation

Normal worker usage is attributable at Attempt granularity and aggregates upward:

```text
Attempt
  ↓
Run
  ↓
Task
  ↓
Goal
```

The ledger may additionally retain Agent, backend, project, and control-operation dimensions for analysis.

This allows Pantheon to compare retry cost, Agent efficiency, backend efficiency, rejected-candidate cost, and orchestration overhead without mutating immutable Task/Agent specs.

## Atomic usage/charge accounting

One actual operation may produce several charges that apply to several budgets.

Pantheon records the UsageRecord once and transactionally applies all Pantheon-owned accounting debits/hold conversions that follow from it.

No partial state such as "Goal charged but Project budget missing" is acceptable after an actual usage record has been accepted.

Actual usage remains factual even if a budget becomes overdrawn; the overdraw blocks/changes future authority rather than rewriting history.

## Hold settlement

At safe Run/control-operation settlement:

```text
held allocation = 100k
actual consumed = 63k

63k → consumed permanently
37k → released
```

If external execution remains `UNKNOWN`, unused BudgetHold headroom remains fenced/held until reconciliation can establish safe settlement according to the accounting source.

## Rate limits are not budgets

Rate limits are replenishing throughput constraints such as requests/minute or tokens/minute.

They do not belong in BudgetAccount because consumed capacity returns over time/window reset.

A normalized RateLimitSnapshot may contain:

```yaml
rateLimit:
  key: limiter://backend/a/input-tokens
  unit: token

  limit: ...
  remaining: ...

  resetAt: ...
  retryAfter: ...
  observedAt: ...

  replenishment: continuous
```

Possible replenishment semantics include:

```text
continuous
fixed-window
opaque
```

The backend adapter owns the translation from native throttling rules.

A rate-limit hit is normally temporary execution availability, not budget exhaustion.

If an upstream system provides only a retry-after signal, Pantheon records that fact rather than fabricating remaining quota.

## Router interaction

Routing uses three independent factual inputs:

```text
Resource fit
Budget/metering compatibility
Rate-limit / allowance availability
```

Hard incompatibilities filter candidates before preference ranking.

Rate-limit exhaustion generally yields temporary unavailability with a recheck time when known.

Budget exhaustion means cumulative spending authority is unavailable until a policy/period/human change occurs.

Resource exhaustion means reservable capacity is unavailable.

Backends report facts. Pantheon route policy derives preference/scarcity behavior from those facts plus observed history.

## Run/Attempt interaction

BudgetHolds are normally Run-scoped; UsageRecords are normally Attempt-scoped.

```text
Run
  ├─ initial BudgetHold
  ├─ Attempt 1 usage
  ├─ Attempt 2 usage
  └─ possible hold extensions
```

Attempt usage aggregates into the Run's consumed amount.

A new Attempt under the same Run does not receive an unrelated fresh Goal budget; it consumes from the same Run allocation unless policy explicitly extends it.

## v1 scope

Include:

- extensible append-only UsageRecords;
- canonical normalized token meters;
- usage quality (`EXACT`, `PROVIDER_REPORTED`, `ESTIMATED`, `UNKNOWN`);
- separate ChargeRecords/tariff revisions where charge data is available;
- generic BudgetAccounts and periods;
- `limit`, `consumed`, `held`, and overdraw state;
- overlapping budgets;
- Run/control-operation BudgetHolds;
- initial budget tranches and incremental extension;
- `HARD`, `GUARDED`, `OBSERVATIONAL` enforcement classes;
- factual ExecutionOffer metering capabilities;
- external allowance mirrors with freshness;
- idempotent usage ingestion;
- Attempt → Run → Task → Goal aggregation;
- independent RateLimitSnapshots.

Defer:

- predictive ML budget sizing;
- universal cross-backend cost normalization;
- automated financial optimization across currencies/credits;
- complex distributed accounting authority;
- hidden provider-specific tokenizer logic in core;
- pretending opaque subscription allowance has exact numerical semantics when it does not.

## Key decisions

1. **Context capacity, ResourceReservations, budgets, Usage, and rate limits are separate concepts.**
2. **Usage Ledger is append-only factual metering; budgets govern future consumption authority.**
3. **Usage meters are extensible and tokens are not the only usage unit.**
4. **Canonical input-token totals include cached input; cache counters remain available as breakdowns.**
5. **Token counts are not a universal economic/quality currency across backends.**
6. **Usage and Charge are separate; pricing/credit conversion records a tariff revision.**
7. **Pantheon BudgetAccounts use `limit`, `consumed`, and `held`; actual usage can truthfully overdraw a budget.**
8. **Multiple applicable hard budgets may constrain one operation.**
9. **BudgetHold prevents concurrent overspend and normally belongs to Run, while control-plane intelligence may also hold budget.**
10. **Runs receive initial budget tranches and may request controller-approved extensions.**
11. **Actual usage is never clamped to the configured limit.**
12. **Metering enforcement is classified as HARD, GUARDED, or OBSERVATIONAL.**
13. **A requested hard budget requires a compatible execution/metering path.**
14. **External allowance accounts are mirrors with freshness; the external system remains authoritative.**
15. **Missing usage/quota data is UNKNOWN, not zero.**
16. **Budget resets create new periods instead of erasing history.**
17. **Usage ingestion is idempotent and replay-safe.**
18. **Attempt usage aggregates to Run, Task, and Goal; orchestration work is also attributable.**
19. **Unused BudgetHold headroom is released only after safe settlement.**
20. **Rate limits are replenishing availability signals, not BudgetAccounts.**
