# Budget, Usage, and Rate Limits

## Status

Canonical Pantheon accounting and consumption-control specification.

## Purpose

Pantheon keeps compatibility capacity, reservable resources, cumulative budget authority, factual usage, accounting charges and replenishing rate limits separate.

```text
Context capacity     -> compatibility
ResourceReservation  -> reversible capacity
BudgetAccount        -> cumulative allowance
BudgetHold           -> reserved future spending authority
UsageRecord          -> factual observed consumption
ChargeRecord         -> accounting consequence of usage
RateLimitSnapshot    -> temporary replenishing availability
```

No subsystem may collapse these into one generic quota object.

## BudgetAccount / BudgetPeriod

A BudgetAccount defines an allowance domain; BudgetPeriods provide concrete limits/windows where applicable. Multiple overlapping accounts may apply to one operation and all must permit the Hold/charge.

Authoritative amounts use integral base units, never floating-point accounting.

Available Pantheon-authoritative budget is conceptually:

```text
limit - consumed - currently held
```

subject to enforcement mode and account semantics.

## BudgetHold

A BudgetHold reserves bounded future spending authority/headroom. It is not factual consumption and not a ResourceReservation.

Runs receive an initial bounded tranche at scheduling commit. Extensions are separate atomic transactions. Control operations such as Evaluation may use `holder: control-operation` where billable usage exists.

Failed Attempts may consume their real usage; failure does not refund factual spend.

## UsageRecord

UsageRecord is append-only factual observation, for example tokens, seconds, bytes, requests or provider-specific normalized meters.

Usage quality is explicit:

```text
EXACT
PROVIDER_REPORTED
ESTIMATED
UNKNOWN
```

Usage is never clamped merely because a budget limit was exceeded. If actual usage exceeded authority, Pantheon records truthful overdraw and applies policy afterward.

## Usage provenance and idempotency

An adapter-authored key is not globally authoritative by itself. Pantheon namespaces every external usage observation with control-plane provenance.

Canonical idempotency identity is equivalent to:

```text
backend_id
+
attempt_id (or explicit control-operation id)
+
adapter_operation_key
+
meter
```

For Attempt usage, Pantheon accepts the record only when:

- the referenced Attempt exists;
- the immutable Run Binding names the reporting backend as the backend responsible for that Attempt lineage;
- the meter/units are valid for the reported usage contract;
- the namespaced source identity has not already been ingested with conflicting content.

The backend cannot claim usage for another backend's Attempt by choosing a colliding `adapter_operation_key`.

Duplicate delivery of the same namespaced observation is idempotent. Same identity with materially different content is a structured integrity conflict, not a second charge.

## Controller lease epoch is not usage truth

A delayed valid UsageRecord may arrive after Pantheon controller ownership/lease rotation. Usage is a factual observation, not a stale controller command.

Therefore Pantheon **does not discard otherwise valid usage merely because it carries an old control epoch**.

ControlLease fencing applies to authority-bearing state transitions/callbacks. Usage ingestion instead validates immutable Attempt/backend provenance plus idempotency identity and observation quality.

If a reporting protocol includes control epoch/incarnation, retain it as provenance/anomaly evidence; do not treat epoch mismatch alone as proof that factual usage is false.

## ChargeRecord

Usage and accounting charge are separate facts. Tariff/reconciliation logic maps UsageRecords into ChargeRecords while retaining the tariff/accounting revision used.

Examples:

```text
Usage: 42,000 input tokens
Charge: 42,000 token allowance units
```

or a later monetary mapping if enabled. Tokens are not assumed to be a universal monetary cost unit.

V1 may simplify/omit monetary tariff conversion while retaining the Usage/Charge separation.

## Atomic usage ingestion

A usage ingestion transaction conceptually performs:

```text
BEGIN IMMEDIATE

validate provenance/idempotency
insert UsageRecord if new
compute/insert ChargeRecord(s) where applicable
insert BudgetConsumption ledger entries
convert/release matching Hold authority as defined
update mutable period aggregates
append Event

COMMIT
```

Recovery/invariant checks can recompute aggregate `consumed/held` from immutable ledger facts and compare them with mutable counters.

## Unknown final usage

If external execution may have consumed usage but final usage is unknown:

- keep the Usage quality/state truthful;
- retain/fence unresolved Hold/headroom conservatively according to policy;
- do not invent a UsageRecord;
- do not fabricate a ChargeRecord merely to close the ledger.

An operator force-resolution may explicitly release/write off/fence unresolved spending authority as an administrative accounting decision, but it must be represented separately from factual consumption.

Useful concepts include an explicit unresolved/forfeited liability or administrative settlement record. It must never masquerade as provider-reported actual usage.

This corrects the dangerous shortcut of "assume the whole remaining hold was consumed" when the truth is unknown.

## Overdraw

If late/provider-reported actual usage exceeds the prior Hold or budget limit, Pantheon records the entire factual Usage/Charge and marks the account/period overdrawn. Enforcement may stop future work, request approval or otherwise react, but history is never rewritten to fit the limit.

## External allowance mirrors

Provider/subscription allowances may be mirrored as freshness-qualified observations. Upstream remains authority for those allowances.

A mirror records at least source, observed state/value, freshness and observation time. Stale mirrors cannot be treated as guaranteed capacity.

## RateLimitSnapshot

Rate limits are temporary replenishing availability, not cumulative budgets. A rate-limited backend/offer may become temporarily unavailable with a retry-after/refresh observation without consuming retry semantics merely because Pantheon waits.

Where an existing backend execution remains continuous while waiting for provider rate availability, Pantheon reconciles the same Attempt rather than creating a new Attempt.

## Enforcement modes

Accounts may use modes such as:

```text
HARD
GUARDED
OBSERVATIONAL
```

Meaning is account-specific but must not alter factual usage recording. A HARD budget can deny new Hold authority; it cannot erase actual overdraw.

## Relationship to Resource Ledger

```text
RESOURCE LEDGER
  reversible concurrency/capacity

BUDGET LEDGER
  cumulative allowance + Holds

USAGE LEDGER
  factual observations + Charges

RATE-LIMIT STATE
  temporary replenishing upstream availability
```

All may influence scheduling feasibility but remain distinct authorities.

## Recovery

UNKNOWN execution retains/fences budget headroom conservatively. Recovery never equates ControlLease expiry with safe budget release.

Operator force-resolution is audited and can settle unresolved administrative authority, but late legitimate usage can still be ingested afterward if immutable provenance validates it. Such late usage may create overdraw; it is not rejected simply because the execution was administratively tombstoned.

## Core invariants

1. Context capacity, ResourceReservations, BudgetHolds, factual Usage, Charges and rate limits are distinct.
2. Usage is append-only factual truth and never clamped to a budget limit.
3. Usage source identity is Pantheon-namespaced by backend + execution/control-operation + adapter key + meter.
4. A backend may report usage only for the execution lineage it owns in the immutable Binding.
5. Duplicate source delivery is idempotent; conflicting duplicate content is an integrity error.
6. Controller lease/epoch rotation does not by itself invalidate delayed factual usage.
7. UNKNOWN final usage does not authorize fabricated consumption.
8. Failed work still consumes real usage.
9. Late usage after administrative force-resolution remains recordable and may create truthful overdraw.
10. External allowance/rate observations include freshness and never become local fabricated authority.
