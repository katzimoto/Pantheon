# Pantheon Version Milestone Template

## Status

Authoring template for the description of a Pantheon Version GitHub Milestone. Version semantics are defined by `docs/development/versions.md`.

Create the Milestone with a version-bearing title such as `v0.1.0 — MVP`, then copy the sections below into the Milestone description and replace the placeholders. The Milestone itself owns version identity, Engineering Mission membership, live progress and open/closed state; do not duplicate those facts in the description.

---

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

A version is not complete merely because its component missions are closed. This scenario must work against the final integrated version.

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

If implementation discovers that a deferred capability is actually required for the Product Proof or a canonical safety invariant, reconsider the version boundary explicitly.

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

`<later capability or release stage>`

Do not create a checkpoint for every mission. Use them where interface or lifecycle integration risk is substantial.

## Release gate

The version may be released only when all of the following are true:

- [ ] Every Engineering Mission assigned to this Milestone that is required for release is complete.
- [ ] Every assigned Engineering Mission has exactly one `changelog:*` classification, including explicit `changelog:none` for intentionally internal work.
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
- generated changelog/release-note candidate;
- any known limitations accepted for this version.

Evidence should describe what was proven without copying generic CI or mission state already visible in GitHub.

## Known limitations

Limitations intentionally accepted while still truthfully claiming this version's Supported Envelope.

- `<limitation>`
- `<limitation>`

A known limitation must not contradict the Release Outcome, Product Proof, Safety and Correctness Properties, or Supported Envelope.

## Compatibility

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

Release this version only when:

1. its Release Gate is satisfied;
2. the Product Proof has passed against the final release candidate;
3. the release artifact/tag is created from that exact accepted revision.

Closing all assigned missions or seeing green CI individually is not release completion. After release, new desired behavior belongs to a later Milestone rather than silently expanding this version's scope.
