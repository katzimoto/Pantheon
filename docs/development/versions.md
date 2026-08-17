# Version Planning

## Status

Canonical for how Pantheon defines a release-level version boundary. It does not replace Engineering Missions, pull requests, architecture contracts, or GitHub's native issue and milestone state.

## Purpose

A Pantheon version defines a release-level outcome: what must be demonstrably true before a release can be claimed. It groups Engineering Missions without becoming another executable work-item type.

```text
GitHub Milestone = Pantheon Version record
  -> Engineering Mission Issues
    -> pull request change/evidence
```

## Canonical record

> **A GitHub Milestone is the canonical Pantheon Version record.**

The Milestone owns:

- version identity through its title;
- the release contract through its description;
- Engineering Mission membership through native Milestone assignment;
- live open/closed mission progress through GitHub;
- an optional target date when one is genuinely useful;
- open/closed Milestone state.

Do not mirror any of those facts in a repository version file, project board field, label set, or second planning document.

The Milestone description owns the release outcome, end-to-end product proof, supported envelope, deliberate deferrals, integrated release gate, release evidence and known limitations. Create that description from `docs/development/version-template.md`.

Engineering Missions remain the units of executable engineering work. Their semantics are defined by `docs/development/missions.md`, and their implementation/review/closure lifecycle is defined by `docs/development/change-lifecycle.md`.

## Membership and decomposition

Milestone membership is authoritative for which Engineering Missions belong to a version. Do not enumerate a second mission portfolio in the Milestone description and do not copy mission status, blockers, pull request state, or completion percentages into it; GitHub already owns those facts.

A version may be refined while its Milestone is open by splitting, adding, reordering, or removing missions without changing its product contract. If the release outcome, product proof, supported envelope, deliberate deferrals, or release gate changes materially, edit the Milestone description explicitly rather than allowing mission drift to redefine the release accidentally.

A mission should represent one independently reviewable outcome. A version is allowed to contain many missions and a mission belongs to the version by being assigned to that Milestone.

## Changelog projection

The changelog and GitHub Release notes are user-facing projections of a Version; they are not another version authority. Pantheon generates the canonical release-note candidate from **completed Engineering Missions assigned to the Version Milestone**, not from every commit or pull request between tags.

Every Mission assigned to a Version carries exactly one repository changelog-classification label:

```text
changelog:added
changelog:changed
changelog:deprecated
changelog:removed
changelog:fixed
changelog:security
changelog:none
```

`changelog:none` is explicit: it means the Mission is intentionally absent from user-facing release notes. No classification is different; it means release metadata is incomplete. Supporting pull requests never create extra changelog entries because the Mission is the outcome being released.

Do not maintain a second live `Unreleased` inventory in `CHANGELOG.md`. While a Version is in development, its Milestone and assigned Missions are the live release state. Near release, automation generates a preview from that native state; a future release-preparation change may materialize the accepted version entry into `CHANGELOG.md` and reuse it for the GitHub Release body.

`.github/release.yml` configures GitHub's native generated release notes only as a supplemental merged-pull-request view. GitHub categorizes that view from pull-request metadata, while Pantheon's changelog classification lives on Engineering Mission Issues; therefore Pantheon does not copy `changelog:*` labels onto PRs merely to influence native notes. Native generated notes, commit ranges and pull-request labels never redefine Version membership or the authoritative Pantheon changelog candidate.

## Automation

GitHub Actions reinforce this contract without becoming state owners:

- `.github/workflows/version-policy.yml` validates Version Milestone shape and Mission-closing PR metadata;
- `.github/workflows/changelog-preview.yml` renders a read-only Milestone/Mission changelog preview into the Actions job summary;
- `.github/workflows/version-readiness.yml` manually evaluates a selected Version against Mission closure, changelog classification, mission-closing PRs, repository verification and release-tag preconditions;
- `.github/workflows/version-labels.yml` is the explicit manual reconciliation path for the approved repository taxonomy: `changelog:*`, `area:*`, contributor-discovery labels, and removal of explicitly deprecated GitHub defaults.

The automation may report or fail when GitHub-native state violates this contract, but it does not mirror Milestone progress into repository files and it does not publish a release. Release publication, artifact provenance and immutable binary distribution are introduced only when Pantheon has actual release artifacts to publish.

## Template

Use `docs/development/version-template.md` as the authoring template for a GitHub Milestone description.

The template is intentionally release-oriented rather than implementation-oriented. Define the product boundary before decomposing it into Engineering Missions. Once missions exist, assign them to the Milestone instead of listing them manually in its description.

## Completion

Closing every Engineering Mission is necessary but not sufficient to release the version.

The Milestone may close as released only when its integrated release gate and product proof pass against the final release candidate and the release/tag is created from that accepted revision. If a version is abandoned instead, close the Milestone only after its description records that disposition clearly enough that a later agent will not mistake it for a shipped release.
