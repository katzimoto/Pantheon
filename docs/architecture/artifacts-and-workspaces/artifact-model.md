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

Source-path authorization is not permission to dereference arbitrary filesystem indirection. When Artifact bytes originate from an Agent-writable Workspace, Pantheon resolves/captures them through the trusted-root, root-confined, no-follow object boundary in `docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md`. Symlinks are captured as symlink data where the Artifact kind permits them; privileged sealing never follows an Agent-created symlink to obtain target contents. Unsupported special files fail closed rather than being opened/read as ordinary payload.

Ordinary local CAS seal ordering:

```text
confined source-object read/capture
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

### Canonical identity (settled by #33)

The digest is taken over one canonical JSON document with exactly three keys:
`task` and `run` (the immutable ids) and `outputs`, an array of
`{"artifact": "sha256:<hex>", "slot": <name>}` entries sorted by raw slot-name
bytes. Nothing else participates. Two Run-related distinctions must not be
blurred:

- The **Run the Candidate belongs to** (`run`) is part of identity. Two Runs
  submitting byte-identical output mappings for the same Task produce two
  distinct Candidates — one per Run, as `at most one Candidate per Run`
  requires.
- The **Run that produced a referenced Artifact** is production provenance and
  sits entirely outside identity. Artifacts are content-addressed and shared;
  which lineage sealed one is carried relationally by its ProductionRecords —
  content reuse is not ownership, and it never folds into the digest.

Worker claims (prose, verdicts, self-assessments, evidence) are likewise
outside identity: changing them cannot change a Candidate's digest.
Duplicate output slots are refused before a Candidate can exist; slot names
are bounded at 128 bytes. A v1 Candidate carries no `summary` field — the
contract requires only the Task, Run and normalized mapping, so none exists to
bound or canonicalize.

## Evidence is separate

Evidence binds a Candidate/Artifact/GoalCompletionCandidate to a criterion + exact EvaluatorVersion + verdict/provenance. Evidence is not embedded into Artifact identity, and evaluating the same Artifact differently does not produce a different Artifact.

## Code changeset Artifact

A `code.changeset` **must be complete from Pantheon's CAS plus its immutable manifest**. Pantheon must not depend on the repository's mutable/prunable Git object database as the only payload store.

For integration correctness, completeness includes the exact changed-path **preimage** and **result** state needed to reconcile the changeset against a later target. Git object retention may accelerate richer integration, but it is not required to recover the changed-path semantics of a retained Artifact.

### Canonical identity

The changeset manifest binds at least:

```text
repository identity
resolved immutable base commit
canonical ordered changed-path entries
result-tree Git OID as optional/verification metadata where applicable
```

Each changed-path entry is canonical before/after data, conceptually:

```yaml
- path: <lossless canonical path representation>
  operation: add | modify | delete

  before:
    state: present | absent
    mode: <canonical repository file mode/type>        # present only
    blob: sha256:<Pantheon CAS blob digest>            # present payload where applicable

  after:
    state: present | absent
    mode: <canonical repository file mode/type>        # present only
    blob: sha256:<Pantheon CAS blob digest>            # present payload where applicable
```

The operation and states must agree:

```text
add      -> before absent,  after present
modify   -> before present, after present
             including content, mode or supported type change
delete   -> before present, after absent
```

`before` is authoritative only when derived by Pantheon from the resolved immutable base through controller-owned/trusted repository state or another equally authoritative immutable source. An Agent-supplied path, patch, local commit or claimed old blob can never define the preimage.

`after` is authoritative only when captured from the exact settled Workspace state through the root-confined/no-follow capture boundary.

For v1 Git-style code trees, supported content modes/types are conceptually:

```text
regular file
executable file
symbolic link
declared gitlink/submodule only where repository policy explicitly supports it
```

For a regular/executable file, a present-state `blob` is the digest of that file's bytes. For `before`, those bytes come from the immutable trusted base. For `after`, those bytes come from the exact confined Workspace source object.

For a symbolic link, `mode` identifies symlink semantics (for Git, conventionally mode `120000`) and `blob` is the digest of the **link-target bytes themselves**. Pantheon never dereferences either the base-side or result-side link while constructing the Artifact. An absolute or escaping-looking target remains inert manifest/content data unless a later authorized materializer deliberately interprets it under its own safe policy.

FIFOs, Unix sockets, block/character devices and undeclared filesystem/mount escapes are not valid v1 `code.changeset` payload entries. Capture rejects them rather than interacting with them as files.

Entries are sorted by canonical path bytes. Rename is not a distinct identity requirement in v1; it may be represented as delete+add because semantic candidate identity is resulting changed-path state, not a diff heuristic.

For repositories whose paths are not representable losslessly as normal UTF-8 strings, the manifest uses an explicitly lossless byte encoding rather than lossy path normalization. The v1 encoding, settled by #32 so implementations do not re-derive it: a path spells itself literally when its bytes are valid UTF-8 and contain no `%`; otherwise it spells as `%` followed by lowercase hex pairs for *every* byte. Because literal form can never contain `%`, the two forms cannot be confused, decoding is total and injective, ordinary paths stay readable, and non-UTF-8 paths stay exact. Entries are ordered by raw path bytes, not by their encoded spellings.

### Changed-path preimage completeness

Every present `before` or `after` state whose semantics depend on payload bytes must reference immutable Pantheon CAS content. A retained changeset therefore retains the changed-path preimage bytes needed for later reconciliation as normal Artifact members.

Pantheon does **not** need to duplicate the entire base repository in CAS. Only the preimage state for paths represented by the changeset is required by this contract.

`baseCommit` remains useful provenance/verification metadata and may support richer repository-level integration when its Git objects remain available, but the preimage blobs in the changeset are what make changed-path integration independent of Task-local Git object retention.

### No Git-generated patch bytes in identity

A human-readable patch/diff may be generated as a derived Artifact/member for review, but **Git-version-dependent patch text is not part of the authoritative `code.changeset` identity**.

This avoids identity changing because different Git versions/configuration render equivalent changes differently.

If Pantheon ever standardizes a canonical patch representation, its exact format/version must be an explicit immutable schema, not "whatever `git diff` produced".

### Git object IDs are verification/provenance, not sole storage

`baseCommit`, observed/result tree OIDs and worker commit OIDs are useful immutable Git identifiers and may be recorded for integration/reconciliation. But retaining those OIDs does not make Git's ODB the Artifact store.

Pantheon CAS stores both required changed-path preimage bytes and resulting regular/executable file bytes or symlink target bytes needed to reconcile/apply the changeset even if Git GC later prunes Task-local objects.

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
  ↓ controller pins trusted capture root
  ↓ controller captures WorkspaceRevision through root-confined/no-follow source reads
  ↓ compare canonical final state to immutable trusted base
  ↓ capture each changed path's authoritative before state from the immutable base
  ↓ capture each changed path's authoritative after state from settled Workspace state
  ↓ copy required before/after regular/executable bytes and symlink-target bytes into Pantheon CAS
  ↓ reject unsupported special filesystem objects
  ↓ build canonical code.changeset manifest
  ↓ verify before/after completeness/digests
  ↓ optional Git object pins
  ↓ commit Artifact/Candidate refs
```

After this point Acceptance never needs to trust a mutable Workspace, and later integration does not need Task-local base objects merely to recover the changed-path preimages.

## Integration

Integration Controller consumes accepted immutable `code.changeset` content. It materializes/reconstructs the result against the intended repository and may perform a controlled changed-path three-way/application strategy with explicit expected target-ref CAS.

For each changed path, the canonical Artifact provides:

```text
base/preimage = before
proposed      = after
current       = authorized current target state
```

Useful deterministic cases include:

```text
current == before
→ cleanly apply after

current == after
→ already applied / no-op for that path

current differs from both
→ perform the permitted three-way/conflict analysis
```

For text/content that the configured Integration policy can merge safely, the three semantic inputs are the current target bytes, Artifact `before` bytes, and Artifact `after` bytes. For binary, mode/type, structural or otherwise unsupported/ambiguous merges, Pantheon fails with an Integration conflict rather than guessing.

This changed-path self-containment does not promise bit-for-bit reproduction of every Git history-aware merge algorithm. If the trusted repository still has `baseCommit`/tree objects, Integration Controller may use them for richer verification or repository-level merge behavior under its controller-owned Git boundary; absence of those optional objects cannot erase the Artifact's own before/after changed-path semantics.

An integration conflict does not invalidate the accepted Artifact; it means current external target state differs from the integration precondition or cannot be reconciled safely under current integration policy.

Materialization treats symlink-target bytes as symlink content according to the declared changeset mode; it does not substitute the contents of the path to which that symlink happens to resolve on the materializing host.

## Large evaluator/output content

Logs, test reports and other large evaluation output become normal Artifacts referenced from Evidence; Events/Evidence rows contain metadata/digests rather than unbounded content.

## Secrets excluded

Active credential material is never an Artifact kind. `artifact.seal` cannot read SecretProvider material or use Artifact retention as a secret-storage lifecycle. Filesystem indirection from an authorized Workspace path is never a mechanism for reaching SecretProvider/control-plane/host credential material.

## Garbage collection

GC follows reachability/retention metadata, not mere age. Before deleting a Blob, Pantheon verifies no retained Artifact manifest references it. For retained `code.changeset` Artifacts this includes required `before` preimage blobs as well as `after` result blobs. Before releasing optional Git object pins, Pantheon verifies no Integration/Workspace obligation still depends on them and the canonical CAS-complete Artifact remains available.

## Core invariants

1. Artifact identity is immutable canonical content; storage/provenance/retention are separate.
2. Workspace/path/session/provider state is never Artifact identity.
3. Pantheon computes/verifies all authoritative hashes.
4. Candidate is immutable/content-addressed and at most one exists per Run.
5. Evidence is separate and bound to exact immutable subject/evaluator version.
6. `code.changeset` is complete from canonical manifest + Pantheon CAS payload; it never relies solely on prunable Git ODB state.
7. Every changed path carries canonical before/after state: before is derived from the trusted immutable base, after from the settled confined Workspace, and every required present-state payload is retained in CAS.
8. A retained changeset therefore preserves the preimage/result material required to reconcile its changed paths against a later target even when Task-local Git base objects are gone.
9. Git-generated patch text is not authoritative changeset identity in v1.
10. Git OIDs/optional controller refs are useful verification/retention metadata, not a substitute for CAS-complete payload.
11. Workspace-derived Artifact capture uses trusted-root, root-confined, no-follow source-object resolution; symlink payload is link-target bytes, never dereferenced target content.
12. Unsupported special filesystem objects do not become v1 `code.changeset` payloads.
13. Artifact refs are not bearer capabilities.
14. Active secrets are never Artifacts.
