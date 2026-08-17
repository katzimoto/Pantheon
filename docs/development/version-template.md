# Pantheon Version — `<version>`

## Status

`PLANNED | ACTIVE | RELEASE-CANDIDATE | RELEASED | ABANDONED`

## Release outcome

Describe the single meaningful capability this version adds to Pantheon.

This should answer:

> **What can a user do after this version that they could not reliably do before it?**

Keep this outcome user/system observable. Do not describe the internal implementation plan here.

## Product proof

The smallest end-to-end scenario that proves the version is real.

```text
<starting state>
  ↓
<user action>
  ↓
<Pantheon behavior>
  ↓
<observable successful result>
```

A version is not complete merely because its component missions are merged. This scenario must work against the final integrated version.

## Primary user workflow

Describe the canonical workflow this version is designed to support.

Example shape:

```text
1. User ...
2. Pantheon ...
3. Pantheon ...
4. User observes ...
```

This is a product-level workflow, not a decomposition into implementation tasks.

## Included capabilities

Capabilities that **must work** for this version to satisfy its outcome.

- `<capability>`
- `<capability>`
- `<capability>`

A capability belongs here only when removing it would make the Product Proof incomplete, unsafe, or materially misleading.

## Explicitly deferred

Capabilities deliberately **not required** for this version.

- `<deferred capability>`
- `<deferred capability>`
- `<deferred capability>`

Deferred functionality must not be silently pulled into a mission merely because the architecture already anticipates it.

If implementation discovers that a deferred capability is actually required for the Product Proof or a canonical safety invariant, the version boundary must be reconsidered explicitly.

## Supported envelope

State the deliberately supported operating envelope of this version.

### Platform

- `<supported OS / architecture>`
- `<local / remote>`
- `<single-user / multi-user>`

### Execution

- `<supported backend classes>`
- `<supported sandbox/isolation classes>`
- `<concurrency limits or other deliberate ceilings>`

### Workloads

- `<supported task/workload classes>`

### Interfaces

- `<CLI/API/etc.>`

Anything outside this envelope is unsupported unless another section explicitly says otherwise.

## Safety and correctness properties

List the architectural properties that the Product Proof must preserve.

These are not optional polish.

- `<durability/recovery property>`
- `<authorization/isolation property>`
- `<idempotency/duplicate-prevention property>`
- `<acceptance/verifiability property>`
- `<data-integrity property>`

Reference canonical architecture documents rather than restating their complete contracts.

## Architecture entry points

Canonical contracts most important for understanding this version:

- `<docs/architecture/...>`
- `<docs/architecture/...>`
- `<docs/architecture/...>`

This is navigation only. The architecture documents remain authoritative for subsystem semantics.

## Mission portfolio

Engineering Missions that collectively deliver this version.

Each item is a GitHub Engineering Mission Issue.

```text
#<issue> — <mission title>
#<issue> — <mission title>
#<issue> — <mission title>
```

The Milestone/Issue relationship is authoritative for version membership.

Do not mirror individual mission status, PR status, blockers, or completion percentages here; GitHub already owns those facts.

### Decomposition rule

A mission should represent one independently reviewable outcome.

The version may gain, split, reorder, or remove missions while ACTIVE without changing the version contract, provided the Release Outcome, Product Proof, Supported Envelope, and Release Gate remain unchanged.

If those change materially, update the version definition deliberately.

## Integration checkpoints

Identify the few points where individually correct missions must be exercised together before development proceeds too far.

### `<checkpoint name>`

Must demonstrate:

```text
<subsystems/capabilities>
  ↓
<integrated observable behavior>
```

Required before:

`<later mission/capability or release stage>`

Do not create a checkpoint for every mission. Use them where interface or lifecycle integration risk is substantial.

## Release gate

The version may enter `RELEASE-CANDIDATE` only when all of the following are true:

- [ ] Every required Engineering Mission is complete.
- [ ] Canonical repository verification passes from the final integrated tree.
- [ ] The Product Proof succeeds end to end.
- [ ] Supported-envelope behavior has been exercised on every platform/configuration claimed by this version.
- [ ] Required restart/recovery/failure-path tests pass.
- [ ] No unresolved correctness, security, data-integrity, or architectural blocker remains.
- [ ] Version-specific documentation accurately describes what is actually supported.
- [ ] An independent integrated review finds no blocking issue.

Additional version-specific gates:

- [ ] `<gate>`
- [ ] `<gate>`

## Release evidence

The release decision must point to durable evidence rather than relying on session history or narrative confidence.

Record or link:

- final verification result;
- end-to-end Product Proof result;
- relevant recovery/fault-injection results;
- supported-platform CI results;
- integrated review;
- any known limitations accepted for this version.

Evidence should describe what was proven without copying generic CI state already visible in GitHub.

## Known limitations

Limitations intentionally accepted while still truthfully claiming this version's Supported Envelope.

- `<limitation>`
- `<limitation>`

A known limitation must not contradict the Release Outcome, Product Proof, Safety and Correctness Properties, or Supported Envelope.

## Release identity

### Version

`<semver>`

### Release type

`MVP | ALPHA | BETA | STABLE | ...`

### Compatibility

Describe only compatibility guarantees intentionally made by this version.

```text
Operator API:
Configuration:
Database:
Artifacts:
Agent protocol:
```

Do not imply compatibility guarantees merely because something happens to work.

## Completion

A version becomes `RELEASED` when:

1. its Release Gate is satisfied;
2. the Product Proof has passed against the final release candidate;
3. the release artifact/tag is created from that exact accepted revision.

`ACTIVE`, all missions merged, or CI green individually are not release completion.

After release, new desired behavior belongs to a later version rather than silently expanding the released version's scope.
