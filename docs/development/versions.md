# Version Planning

## Status

Canonical for how Pantheon defines a release-level version boundary. It does not replace Engineering Missions, pull requests, architecture contracts, or GitHub's native issue state.

## Purpose

A Pantheon version defines a release-level outcome: what must be demonstrably true before a release can be claimed. It groups Engineering Missions without becoming another executable work-item type.

```text
Version / GitHub Milestone
  -> Engineering Mission Issues
    -> pull request change/evidence
```

The version definition owns the release outcome, end-to-end product proof, supported envelope, deliberate deferrals, integrated release gate, and release evidence.

Engineering Missions remain the units of executable engineering work. Their semantics are defined by `docs/development/missions.md`, and their implementation/review/closure lifecycle is defined by `docs/development/change-lifecycle.md`.

## Authority and state

Use a GitHub Milestone to group the Engineering Mission Issues that belong to a version. Milestone membership is authoritative for version membership; do not copy mission status, blockers, PR state, or completion percentages into the version definition.

A version may be refined while active by splitting, adding, reordering, or removing missions without changing its product contract. If the release outcome, product proof, supported envelope, or release gate changes materially, update the version definition explicitly rather than allowing mission drift to redefine the release accidentally.

## Template

Use `docs/development/version-template.md` to define a version.

The template is intentionally release-oriented rather than implementation-oriented. Fill the product boundary before decomposing it into Engineering Missions.

## Completion

A version is complete only when its integrated release gate and product proof pass against the final release candidate and the release/tag is created from that accepted revision. Completion of every individual mission is necessary but not sufficient.
