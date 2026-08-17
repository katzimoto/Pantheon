---
name: dependency-change-procedure
description: Add or materially change a Rust dependency, or add a new crate, in line with Pantheon's dependency policy and the supply-chain diligence its mechanical checks do not cover. Use when adding a new crates.io dependency, adding an unavoidable Git dependency, changing an existing dependency's features/source/version, or adding a new crate under crates/. Do not use for ordinary edits that only use an already-declared dependency, or for a toolchain-pin change.
metadata:
  pantheon-authority: procedural-guidance-only
---

# Pantheon dependency-change procedure

This operationalizes the dependency and new-crate policy in
`docs/development/implementation.md` ("When a dependency is justified",
"When a new crate is justified") and the mechanical checks in
`scripts/check-crate-deps.sh`. Read those for the actual rules; this skill
is the diligence checklist that gets a dependency change through them
correctly on the first attempt, covering the supply-chain review those
mechanical checks do not perform.

## When this applies

- Adding a new crates.io dependency to any workspace crate (for example,
  the SQLite driver `pantheon-store` needs for #16).
- Adding an unavoidable Git dependency.
- Materially changing an existing dependency's features, source, or
  version — not just consuming what is already declared.
- Adding a new crate under `crates/`, whose boundary and dependency
  declaration must be established.

Non-triggers:

- An ordinary code change that only uses a dependency already declared in
  a manifest. Nothing here applies; write the code.
- Changing the pinned toolchain in `rust-toolchain.toml`. That is a
  different, narrower procedure than a dependency change and is not
  covered here.

## Procedure

1. **Confirm the dependency is justified now, not speculatively.**
   `docs/development/implementation.md` is explicit: a dependency enters
   with the first code that needs it. Do not add it ahead of the code that
   uses it, and do not add it because the architecture is expected to need
   it eventually.

2. **Reject what the policy already prohibits** before going further: a
   wildcard `*` version, or a Git dependency on a mutable branch. An
   unavoidable Git dependency requires explicit mission justification and
   an exact immutable revision — confirm the mission actually grants that
   before proceeding, rather than deciding it yourself.

3. **Perform targeted due diligence on the exact candidate and version**
   before adding it. This is procedural review done by hand — Pantheon
   runs no `cargo-audit`, `cargo-deny`, `cargo-vet`, or equivalent
   supply-chain gate in CI today, and this step does not claim otherwise
   or require adding one:
   - Known RustSec/security advisories against the exact version being
     pinned.
   - Upstream maintenance and provenance: is it actively maintained, does
     the source repository match the published crate, is the
     maintainer/publishing history consistent with a trustworthy source.
   - License compatibility with Pantheon and its existing dependencies.
   - Compatibility with Pantheon's pinned stable toolchain and edition
     (`rust-toolchain.toml`; see `docs/development/implementation.md`
     "Toolchain") — the crate's minimum supported Rust version must not
     exceed it.
   - Which features are required versus enabled by default; disable
     defaults that pull in surface area the code does not use.
   - What the dependency transitively pulls in — a small direct addition
     can be a large transitive one.

4. **Keep the change narrow.** No speculative dependency, no wildcard
   version, no mutable-branch Git dependency, minimal necessary features.
   When updating `Cargo.lock`, run `cargo update -p <crate>` for the one
   crate involved — not a broad `cargo update` — so the lockfile diff stays
   minimal and reviewable rather than an unrelated dependency refresh.

5. **For a new crate**, consult
   `docs/development/implementation.md` ("When a new crate is justified")
   for the canonical boundary rules, then, only when a real boundary
   applies, add together in the same change: the crate's `//!` boundary
   statement, its name in the explicit `members` list in the workspace root
   `Cargo.toml`, and its allowlist entry in `scripts/check-crate-deps.sh`
   naming exactly which Pantheon crates it may depend on. This skill does
   not decide that a new crate is architecturally warranted — that
   determination belongs to `docs/development/implementation.md`'s
   criteria and the mission that requested the crate.

6. **Finish through `./scripts/verify.sh`**, per
   `pantheon-change-verification` — the same canonical interface as any
   other change, with no substitute flags or second verification command.
   It runs `--locked`, so a lockfile that does not match the manifests
   fails there rather than silently drifting.

## What this skill must not do

- It does not decide whether a new crate boundary is architecturally
  warranted — `docs/development/implementation.md` and the requesting
  mission's acceptance criteria decide that.
- It does not replace `scripts/check-crate-deps.sh` or
  `./scripts/verify.sh`, and it does not introduce a second verification
  command.
- It does not add `cargo-audit`, `cargo-deny`, `cargo-vet`,
  Dependabot/Renovate, or any other supply-chain tooling; the diligence
  above is manual review, not a claim that such a gate runs in CI.
- It does not perform or authorize an automatic dependency upgrade.

## Non-triggers

- Ordinary code that only consumes a dependency already declared in a
  manifest.
- A toolchain-pin-only change to `rust-toolchain.toml`.
