# Artifact Model

## Status

Draft design — Pantheon artifact subsystem specification.

## Purpose

Pantheon needs immutable, verifiable result objects that can be referenced by Tasks, Runs, Acceptance, recovery, learning, and future external APIs without depending on mutable workspace paths or backend-specific storage.

The central rule is:

> **Artifacts are immutable, content-addressed logical results. Mutable workspaces, files-in-progress, streams, paths, sessions, and storage locations are not Artifacts.**

This specification separates content identity, provenance, evidence, storage availability, retention, and candidate-result identity.

See also:

- `docs/architecture/task-object.md`
- `docs/architecture/task-acceptance-and-completion.md`
- `docs/architecture/run-and-attempt.md`
- `docs/architecture/recovery-policy.md`

## 1. Blob and Artifact are different objects

A `Blob` is immutable bytes identified by content digest.

Conceptually:

```yaml
blob:
  digest: sha256:abc...
  size: 18372
```

An `Artifact` is a typed immutable logical result represented by a canonical Artifact Manifest that references one or more Blobs.

```text
Artifact
   │
   ▼
Artifact Manifest
   │
   ├── Blob A
   ├── Blob B
   └── Blob C
```

Artifact therefore does not mean "file". It may represent a report, changeset, test result, finding set, binary, image, research bundle, or another typed result.

## 2. Artifact identity is content-addressed

V1 uses an algorithm-qualified digest of the canonical Artifact Manifest:

```text
artifact://sha256/<manifest-digest>
```

Conceptually:

```yaml
artifactManifest:
  schemaVersion: 1
  artifactKind: research.report

  contents:
    - name: report.md
      mediaType: text/markdown
      digest: sha256:def...
      size: 48219
```

The manifest is serialized canonically using RFC 8785 JSON Canonicalization Scheme semantics and hashed with SHA-256.

Digest syntax remains algorithm-qualified so a later algorithm migration does not require redefining Artifact references.

## 3. Payload bytes are preserved exactly

Pantheon canonicalizes only its own Artifact Manifest.

Payload Blobs are hashed over exact bytes.

Pantheon must not silently parse, normalize, reorder, pretty-print, line-ending-convert, or otherwise rewrite Agent/user payloads before computing their content digest.

If a producer submits a JSON file, its Blob identity is the digest of the exact submitted bytes, not of a reserialized JSON value.

## 4. Artifact kind and media type are distinct

`artifactKind` expresses semantic meaning to Pantheon.

Examples:

```text
code.changeset
code.snapshot
research.report
security.findings
test.results
diagnosis
design.document
report
artifact.file
```

`mediaType` describes payload encoding.

Examples:

```text
text/markdown
application/json
application/octet-stream
image/png
```

A `research.report` may be encoded as Markdown, JSON, PDF, or a multi-content bundle without changing what the Artifact means semantically.

## 5. Mutable metadata is excluded from Artifact identity

The hashed Artifact Manifest must not contain mutable or contextual data such as:

- creation time;
- Task/Run/Attempt identity;
- Agent/backend identity;
- storage path or URL;
- replica state;
- retention policy;
- labels that may change;
- access timestamps;
- human-facing display title when not semantically part of the content.

Artifact identity answers:

> What immutable logical result is this?

It does not answer who produced it, where it lives, how long it is retained, or whether a replica is currently available.

## 6. Provenance is a separate immutable ProductionRecord

Different executions may independently produce identical Artifact content.

Those executions share the same Artifact identity but retain separate provenance.

Conceptually:

```yaml
production:
  id: production_123

  subjects:
    - artifact://sha256/91ab...

  producer:
    task: task_44
    run: run_17
    attempt: attempt_2

  strategy:
    agent: agent://researcher
    binding: binding_88

  inputs:
    - artifact://sha256/12ef...
    - artifact://sha256/72aa...

  createdAt: ...
```

ProductionRecord is append-only/immutable evidence of production context. It is not part of the Artifact digest.

This distinction permits both content deduplication and complete audit history.

## 7. Evidence remains separate from Artifact and provenance

Pantheon distinguishes:

```text
Artifact
  What immutable result exists?

ProductionRecord
  Who/what produced it, under what strategy, from which inputs?

Evidence
  What did an evaluator determine about it?
```

An evaluator may itself produce report Artifacts, but authoritative acceptance verdicts remain Evidence records governed by the Acceptance subsystem.

Evidence binds to exact immutable subjects/digests and becomes stale if the evaluated candidate changes.

## 8. Mutable workspace state is never an Artifact

`workspace://...` identifies mutable working state.

An Agent may edit a workspace repeatedly across Attempts or Runs. Therefore workspace identity cannot serve as acceptance/output identity.

Correct boundary:

```text
mutable workspace
       ↓
      seal
       ↓
immutable Artifact
```

The same rule applies to temporary files, PTY streams, live logs, working Git trees, and other mutable external state.

## 9. Artifact sealing is a control-plane operation

Pantheon exposes a canonical operation conceptually equivalent to:

```text
artifact.seal
```

Sealing flow:

```text
producer identifies output payload(s)
        ↓
Pantheon imports/reads payload
        ↓
Pantheon computes/verifies size + digest
        ↓
write/verify Blob(s) in CAS
        ↓
build canonical Artifact Manifest
        ↓
compute Artifact Manifest digest
        ↓
persist Artifact metadata
        ↓
return ArtifactRef
```

Pantheon computes/verifies digests itself. A producer-provided digest may be a hint/claim but is never trusted as authoritative without verification.

## 10. Sealing is naturally idempotent

Identical payload descriptors and identical canonical Artifact Manifest content produce the same Artifact ID.

Repeated sealing therefore does not create semantically duplicate Artifacts.

Separate ProductionRecords may still state that different Runs produced the same Artifact.

## 11. Blob content is globally deduplicated

The local Content Addressable Store stores one Blob per digest.

Artifacts may share Blobs.

```text
Artifact A
  ├── report-a
  └── diagram

Artifact B
  ├── report-b
  └── diagram

CAS
  ├── report-a
  ├── report-b
  └── diagram
```

This reduces storage duplication without coupling Artifact identity to storage layout.

## 12. Multi-content Artifacts are first-class

V1 supports a simple ordered/named content list.

Example:

```yaml
artifactKind: security.findings
contents:
  - name: findings.json
    mediaType: application/json
    digest: sha256:...
    size: ...

  - name: exploit.py
    mediaType: text/x-python
    digest: sha256:...
    size: ...

  - name: screenshot.png
    mediaType: image/png
    digest: sha256:...
    size: ...
```

Content names are part of the logical Manifest and therefore affect Artifact identity.

V1 does not attempt to define a universal filesystem Merkle-tree representation. Workspace/Git integration may define specialized `code.snapshot` or `code.changeset` representations using Git-native immutable objects.

## 13. Git representation is an Artifact payload strategy, not the Artifact model

Git already provides immutable content-addressed blobs, trees, and commits.

Pantheon should leverage those primitives for code-related Artifact kinds, but generic Artifact identity must not simply be defined as a Git commit hash.

Conceptually:

```text
Artifact
  kind = code.changeset
      ↓
representation
      ↓
Git-specific immutable descriptor
```

The Workspace/Git subsystem defines exactly which Git descriptor(s) are sealed for each code Artifact kind.

## 14. Storage replicas are mutable state outside Artifact identity

The same Artifact may have zero, one, or multiple payload replicas.

Conceptually:

```yaml
replica:
  artifact: artifact://sha256/...
  location: store://local/...
  state: available
  verifiedAt: ...
```

Storage migration, replication, eviction, or restoration does not change Artifact identity.

Useful replica observations include:

```text
AVAILABLE
PARTIAL
MISSING
CORRUPT
```

These are storage observations, not Artifact lifecycle phases.

## 15. Corruption never mutates Artifact identity

If a replica expected to contain `sha256:ABC` verifies as another digest, the replica is `CORRUPT`.

The Artifact does not become the newly observed content.

Acceptance, provenance, and Task output references continue to point to the original immutable digest and fail closed until a valid replica is available.

## 16. ArtifactRef is an identifier, not authority

Knowledge of an Artifact digest does not grant access to the payload.

Artifact read/export operations remain subject to Pantheon authorization, for example:

```text
action: artifact.read
resource: artifact://sha256/...
```

Artifact refs may safely appear in logs, events, provenance, Task inputs, or acceptance records without becoming bearer tokens.

## 17. Retention and pinning are separate

Retention is mutable policy and must not affect Artifact identity.

Potential retention roots include:

- active Task/Run references;
- Goal deliverables;
- Acceptance Evidence;
- user/operator pins;
- audit-retention policy;
- Genome/evaluation references.

V1 should prefer mark-and-sweep style garbage collection from retained roots over making correctness depend exclusively on reference counting.

Metadata retention and payload retention are distinct.

Payload bytes may be intentionally garbage-collected while durable metadata/provenance still records that an Artifact with a given digest existed.

Policies requiring long-term reproducibility pin the required payloads.

## 18. CandidateResult is canonical and content-addressed

A Run submits exactly one immutable candidate that binds Task output names to Artifact references.

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

Pantheon canonicalizes the CandidateResult and derives a digest/reference such as:

```text
candidate://sha256/C
```

Changing any output binding or semantically hashed candidate field changes the candidate digest and invalidates prior candidate-level acceptance evidence.

Candidate identity is separate from Artifact identity because one candidate may contain multiple output Artifacts.

## 19. Candidate submission is a sealing boundary

Workers should not submit mutable paths as final outputs.

Preferred flow:

```text
artifact.seal(path/content)
        ↓
ArtifactRef

artifact.seal(...)
        ↓
ArtifactRef

Task result submission
        ↓
CandidateResult(outputs = ArtifactRefs)
        ↓
Candidate digest frozen
        ↓
Task → Evaluating
```

This gives Acceptance exact immutable subjects.

## 20. Intermediate Artifacts are allowed

A Run may seal intermediate results without declaring them Task outputs.

Examples:

- investigation notes;
- downloaded datasets;
- generated traces;
- benchmark results;
- intermediate patches;
- temporary analysis bundles.

Relationships such as `intermediate`, `candidate-output`, `evaluation-input`, or `deliverable` belong to Production/usage metadata, not intrinsic Artifact identity.

## 21. Production lineage forms a derivation DAG

ProductionRecords may reference input Artifacts, producing a lineage graph:

```text
Artifact A
   ↓
Run 1
   ↓
Artifact B
   ↓
Run 2
   ↓
Artifact C
```

This lineage remains separate from the immutable Artifact Manifests.

Future interoperability may export selected provenance into external attestation formats without making those formats Pantheon's internal source of truth.

## 22. Local-first v1 storage

V1 may use one opaque local CAS plus SQLite metadata.

Conceptually:

```text
~/.pantheon/store/
  objects/
    sha256/
      ab/
        abcdef...
```

SQLite stores/control-indexes:

- Artifact records/Manifest metadata;
- ProductionRecords;
- CandidateResults;
- Evidence references;
- replica state;
- retention/pins;
- lineage relationships.

The CAS itself stores opaque immutable bytes and should not be the sole source of relational/control-plane truth.

## v1 non-goals

Defer:

- distributed object storage;
- remote CAS protocol;
- automatic signing/transparency log;
- universal source-tree Merkle representation;
- cross-cluster Artifact federation;
- sophisticated retention tiers;
- external attestation interoperability as a core dependency.

## Key decisions

1. Artifact is immutable and content-addressed; mutable workspace/file/stream state is never an Artifact.
2. Raw bytes are stored as content-addressed Blobs; Artifact is a typed Manifest over one or more Blobs.
3. V1 Artifact ID is SHA-256 over an RFC-8785-canonicalized Artifact Manifest.
4. Payload Blob hashes are over exact bytes; Pantheon does not silently canonicalize producer payloads.
5. `artifactKind` expresses semantic meaning; `mediaType` expresses encoding.
6. Mutable metadata, storage locations, producer/timestamps, retention, and backend details are excluded from Artifact identity.
7. Production/provenance is a separate immutable record, so identical content may have one Artifact ID with multiple production histories.
8. Evidence remains separate from Artifact and ProductionRecord.
9. Pantheon computes/verifies size and digest during sealing.
10. Sealing is naturally idempotent because identical canonical Manifest content yields the same Artifact ID.
11. CAS deduplicates shared Blob content globally.
12. V1 supports simple multi-content Artifacts; source-tree representation is specialized by Workspace/Git integration.
13. Storage replicas and availability are mutable observations outside Artifact identity.
14. Artifact refs are identifiers, never authorization capabilities.
15. Retention/pinning is separate from Artifact identity; payload GC does not rewrite historical metadata.
16. CandidateResult is canonical/content-addressed and binds Task output names to exact Artifact IDs.
17. Acceptance normally binds to the Candidate digest while criterion evidence may additionally bind constituent Artifacts.
18. Runs may produce intermediate Artifacts; Task-output status is a relationship, not an intrinsic Artifact property.
19. Production lineage forms a separate derivation DAG.
20. V1 uses one local opaque CAS plus SQLite control/provenance/replica/retention state.
