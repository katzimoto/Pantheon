# Artifact Model

## Status

Canonical Pantheon Artifact/Candidate specification.

## Purpose

Pantheon needs immutable verifiable result objects independent of mutable Workspaces, provider sessions and storage paths.

> **Artifacts are immutable content-addressed logical results. Mutable workspaces, files-in-progress, streams, paths, sessions and storage locations are not Artifacts.**

## Blob and Artifact

```text
Blob
  immutable raw bytes
  identity = digest + size

Artifact
  typed immutable canonical manifest over zero or more Blobs
  identity = digest(canonical manifest)
```

V1 uses SHA-256 and RFC-8785/JCS-style canonical JSON discipline for hash-bearing manifests.

External form:

```text
artifact://sha256/<manifest-digest>
```

The Artifact manifest contains immutable semantic content only. Mutable retention/location/availability metadata is separate.

## Artifact kind versus media type

`artifactKind` describes Pantheon semantics such as:

```text
code.changeset
research.report
test.report
log.bundle
architecture.document
```

`mediaType` describes representation of individual Blob members. The two are not interchangeable.

## Separate graphs

Pantheon keeps four concerns separate:

```text
CONTENT
  Blob/Artifact immutable identity

PROVENANCE
  ProductionRecord: who/what produced the Artifact

STORAGE
  replicas/availability/verification

CONTROL
  retention pins/authorization/lifecycle
```

Producing the exact same Artifact twice yields one content identity and multiple ProductionRecords.

## Artifact sealing

Only Pantheon computes authoritative digests. An Agent may request `artifact.seal` against authorized Task Workspace/input state, but it cannot supply an authoritative digest or arbitrary host path.

Ordinary local CAS seal ordering:

```text
write temporary bytes
fsync/finalize
atomic rename into digest location
verify digest/size
then commit Blob/Artifact/Production/Candidate DB refs
```

An orphan CAS object after a DB crash is GC-able. A committed DB Artifact referencing missing payload is a recovery/storage fault.

## Storage

One local v1 CAS is sufficient, conceptually:

```text
~/.pantheon/store/objects/sha256/<digest>
```

Storage path is not Artifact identity.

Replica state may be:

```text
AVAILABLE
PARTIAL
MISSING
CORRUPT
```

Pantheon verifies bytes against the manifest rather than trusting filesystem presence.

## Artifact authorization

Artifact refs are identifiers, not bearer capabilities. Reading/materializing an Artifact requires `artifact.read` authorization/current Task/Run policy. Raw CAS paths are not exposed to untrusted Sandboxes.

## Retention

Retention/pinning is separate from content identity. Goal deliverables, active Candidates/Evidence, current Workspaces/integration obligations and explicit operator pins can establish retention roots.

Removing a retention pin never changes Artifact ID.

## CandidateResult

A CandidateResult is an immutable content-addressed proposed answer to one Task from one Run.

Conceptually:

```yaml
candidate:
  task: task_123
  run: run_17
  outputs:
    changeset: artifact://sha256/A
    diagnosis: artifact://sha256/B
  summary: ...
```

Candidate ID:

```text
candidate://sha256/<canonical-candidate-digest>
```

V1 permits at most one Candidate per Run. Candidate identity never changes during Acceptance.

## Evidence is separate

Evidence binds a Candidate/Artifact/GoalCompletionCandidate to a criterion + exact EvaluatorVersion + verdict/provenance. Evidence is not embedded into Artifact identity, and evaluating the same Artifact differently does not produce a different Artifact.

## Code changeset Artifact

A `code.changeset` **must be complete from Pantheon's CAS plus its immutable manifest**. Pantheon must not depend on the repository's mutable/prunable Git object database as the only payload store.

### Canonical identity

The changeset manifest binds at least:

```text
repository identity
resolved immutable base commit
canonical ordered changed-path entries
result-tree Git OID as optional/verification metadata where applicable
```

Each changed-path entry is canonical data, conceptually:

```yaml
- path: <lossless canonical path representation>
  operation: add | modify | delete
  mode: <canonical repository file mode>
  blob: sha256:<Pantheon CAS blob digest>   # add/modify only
```

Entries are sorted by canonical path bytes. Deletion has no payload Blob. Rename is not a distinct identity requirement in v1; it may be represented as delete+add because semantic candidate identity is resulting content, not a diff heuristic.

For repositories whose paths are not representable losslessly as normal UTF-8 strings, the manifest uses an explicitly lossless byte encoding rather than lossy path normalization.

### No Git-generated patch bytes in identity

A human-readable patch/diff may be generated as a derived Artifact/member for review, but **Git-version-dependent patch text is not part of the authoritative `code.changeset` identity**.

This avoids identity changing because different Git versions/configuration render equivalent changes differently.

If Pantheon ever standardizes a canonical patch representation, its exact format/version must be an explicit immutable schema, not "whatever `git diff` produced".

### Git object IDs are verification/provenance, not sole storage

`baseCommit`, observed/result tree OIDs and worker commit OIDs are useful immutable Git identifiers and may be recorded for integration/reconciliation. But retaining those OIDs does not make Git's ODB the Artifact store.

Pantheon CAS stores the changed file payload Blobs required to reconstruct/apply the changeset even if Git GC later prunes Task-local objects.

### Optional Git object pinning

For efficient local integration/review, Workspace/Integration Controller may additionally pin relevant Git objects under controller-owned refs such as:

```text
refs/pantheon/artifacts/<artifact-id>/...
```

before a DB state relies on their continued Git availability.

These refs are **storage/optimization retention**, not Artifact identity. Artifact correctness must still be recoverable from the canonical manifest + CAS payload unless the Artifact kind explicitly defines another complete immutable payload representation.

This dual rule prevents both failure modes:

```text
Git gc prunes only copy -> Artifact broken
```

and:

```text
Git implementation renders different patch text -> Artifact ID changes
```

## WorkspaceRevision versus code.changeset

WorkspaceRevision is an immutable controller checkpoint of Task Workspace state, typically recording base/result Git tree metadata. It is not itself the final portable Artifact payload.

Sealing a code candidate:

```text
Task Workspace mutable state
  ↓ controller captures WorkspaceRevision
  ↓ compare canonical final state to immutable base
  ↓ copy changed-file bytes into Pantheon CAS
  ↓ build canonical code.changeset manifest
  ↓ verify completeness/digests
  ↓ optional Git object pins
  ↓ commit Artifact/Candidate refs
```

After this point Acceptance never needs to trust a mutable Workspace.

## Integration

Integration Controller consumes accepted immutable `code.changeset` content. It may materialize/reconstruct the result against the intended repository and perform a controlled three-way/application strategy with explicit expected target-ref CAS.

An integration conflict does not invalidate the accepted Artifact; it means current external target state differs from the integration precondition.

## Large evaluator/output content

Logs, test reports and other large evaluation output become normal Artifacts referenced from Evidence; Events/Evidence rows contain metadata/digests rather than unbounded content.

## Secrets excluded

Active credential material is never an Artifact kind. `artifact.seal` cannot read SecretProvider material or use Artifact retention as a secret-storage lifecycle.

## Garbage collection

GC follows reachability/retention metadata, not mere age. Before deleting a Blob, Pantheon verifies no retained Artifact manifest references it. Before releasing optional Git object pins, Pantheon verifies no Integration/Workspace obligation still depends on them and the canonical CAS-complete Artifact remains available.

## Core invariants

1. Artifact identity is immutable canonical content; storage/provenance/retention are separate.
2. Workspace/path/session/provider state is never Artifact identity.
3. Pantheon computes/verifies all authoritative hashes.
4. Candidate is immutable/content-addressed and at most one exists per Run.
5. Evidence is separate and bound to exact immutable subject/evaluator version.
6. `code.changeset` is complete from canonical manifest + Pantheon CAS payload; it never relies solely on prunable Git ODB state.
7. Git-generated patch text is not authoritative changeset identity in v1.
8. Git OIDs/optional controller refs are useful verification/retention metadata, not a substitute for CAS-complete payload.
9. Artifact refs are not bearer capabilities.
10. Active secrets are never Artifacts.
