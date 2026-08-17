# Backup and Restore

## Status

Canonical Pantheon backup-scope, payload-completeness, and restore-input specification.

## Purpose

Pantheon stores authoritative relational state in SQLite while immutable Artifact payload lives in a separate content-addressed store (CAS). A consistent SQLite snapshot is therefore necessary for disaster recovery, but it is not by itself proof that every immutable payload referenced by that snapshot is still available.

The central rule is:

> **A consistent SQLite snapshot is a valid control-plane snapshot. It is a payload-complete Pantheon backup only when the immutable CAS closure required by that exact database snapshot has also been captured, verified, and bound to the same immutable backup manifest.**

This document defines that distinction and composes the existing contracts in:

- `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`;
- `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`;
- `docs/architecture/artifacts-and-workspaces/artifact-model.md`.

It does not create a new Backup Controller, storage provider abstraction, or remote-backup service in v1.

## 1. Two backup classes

Pantheon distinguishes two explicit backup products.

### ControlPlaneSnapshot

A `ControlPlaneSnapshot` contains:

```text
one SQLite-consistent pantheon.db snapshot
+ non-sensitive backup metadata/checksums
```

It preserves authoritative relational history at one database point in time.

It does **not** guarantee that every Artifact/Blob payload referenced by that snapshot remains available in the local CAS or another trusted replica.

SQLite Online Backup API, `VACUUM INTO`, or another SQLite-supported consistent snapshot mechanism can produce this database snapshot. A raw copy of only `pantheon.db` while live WAL/journal state exists is not a valid ControlPlaneSnapshot.

Where other Pantheon documents refer generically to a consistent SQLite backup/snapshot, that mechanism yields at least a `ControlPlaneSnapshot`; it becomes a `DurableStateBackup` only after the additional payload-completeness contract below is satisfied.

### DurableStateBackup

A `DurableStateBackup` contains:

```text
one ControlPlaneSnapshot S
+
all immutable CAS objects required by the retention roots represented in S
+
one immutable backup manifest binding S to that exact required object set
```

It is Pantheon's v1 payload-complete disaster-backup unit for durable state.

The name deliberately does **not** imply capture of arbitrary external or mutable state. It does not promise to contain:

- active SecretProvider material;
- live ExecutorBackend processes/jobs;
- Sandbox runtime state;
- remote Git refs or external repository state;
- mutable unsealed Workspace working state;
- process-local queues/caches;
- other external systems that Global Recovery must reconcile after restore.

## 2. Snapshot retention closure

The required CAS payload set for a `DurableStateBackup` is derived from the **retention graph in the immutable database snapshot S**, not from the live database after S is created.

Conceptually:

```text
SQLite snapshot S
        ↓
retention roots represented in S
        ↓
retained Artifact manifests
        ↓
referenced Blob/CAS object closure
        ↓
requiredBackupObjects(S)
```

Canonical retention roots are defined by the Artifact subsystem and include the roots applicable at S, such as:

- retained Goal deliverables;
- active/current Candidates and Evidence that establish retention;
- current Workspace/integration obligations that pin immutable Artifacts;
- explicit operator retention pins;
- other canonical retention roots defined by the Artifact model.

A historical Artifact identity that is not retained at S does not become retained merely because a backup is being created.

The backup contract therefore preserves the durable payload that the database snapshot itself says must remain available; it does not turn all historical Artifact identities into permanent storage roots.

## 3. Backup capture and CAS GC exclusion

SQLite and CAS are separate durability domains. Pantheon does not attempt a distributed transaction across them.

V1 instead uses a short-lived backup/GC exclusion around snapshot capture:

```text
acquire backup CAS-deletion/GC exclusion
        ↓
create consistent SQLite snapshot S
        ↓
read retention graph from S
        ↓
derive requiredBackupObjects(S)
        ↓
copy/export every required immutable CAS object
        ↓
verify digest + size for every copied object
        ↓
build and verify immutable backup manifest
        ↓
durably publish completed DurableStateBackup
        ↓
release backup CAS-deletion/GC exclusion
```

The exclusion begins **before** S is created so an object retained by S cannot be deleted between snapshot creation and backup copying.

Normal CAS writes may continue during backup creation. The exclusion blocks only deletion/GC that could remove source bytes needed by the in-progress snapshot-derived object set.

Because v1 uses a single authoritative local daemon and local CAS, this exclusion need not become a new durable lease/table solely for crash recovery. If backup creation is interrupted, the staged output remains incomplete/unpublished and conveys no `DurableStateBackup` guarantee. Temporary copied objects/manifests may later be discarded or deduplicated safely.

External tooling may drive backup creation only through a mechanism that obtains the same daemon/GC coordination. A filesystem copy that races CAS GC does not become a valid `DurableStateBackup` by convention or filename.

## 4. Backup manifest

A completed `DurableStateBackup` has one immutable canonical manifest binding the database snapshot to the required immutable payload set.

Conceptually:

```yaml
backup:
  formatVersion: 1
  backupId: backup_...
  kind: durable-state

  installationId: ...
  createdAt: ...

  database:
    digest: sha256:DB
    schemaVersion: ...
    applicationVersion: ...

  cas:
    retentionRootSetDigest: sha256:ROOTS
    objectInventoryDigest: sha256:OBJECTS
    objectCount: ...

  manifestDigest: sha256:...
```

The concrete object inventory may be embedded for small backups or stored as another immutable indexed manifest. Its representation is implementation-level, but the canonical backup identity must bind at least:

```text
exact database snapshot digest
exact retention-root closure identity
exact required CAS object identities + sizes
backup format/version metadata
```

No secret material is included.

The manifest is published as complete only after every required payload object verifies against its expected digest and size.

## 5. Missing or corrupt payload during backup

If a retained object required by snapshot S is `MISSING` or `CORRUPT`, Pantheon may first attempt normal Artifact-replica repair from another trusted replica.

If the exact bytes cannot be recovered and verified:

```text
ControlPlaneSnapshot
→ may still be valid

DurableStateBackup
→ must not be published as complete
```

Pantheon never omits a required object from the manifest while claiming payload completeness.

An operator/tool may choose to retain the DB-only snapshot as a `ControlPlaneSnapshot`, with the weaker restore guarantee explicit in its metadata/UI.

## 6. Secrets are excluded from both backup classes

Pantheon backups never become credential escrow.

Neither backup class contains:

- passwords;
- PATs;
- OAuth access/refresh tokens;
- private keys;
- cloud credentials;
- other SecretProvider material.

SQLite contributes only the non-secret SecretDescriptor/version/intent/lease metadata already permitted by the Secret subsystem.

After restore, Secret Reconciler must compare restored historical metadata with fresh current SecretProvider observation before credential use. A payload-complete CAS backup does not weaken the SecretProvider recovery/fencing contract.

## 7. Mutable Workspace and external-state scope

V1 `DurableStateBackup` guarantees durable SQLite state plus the retained immutable CAS closure. It does not guarantee arbitrary mutable Workspace work-in-progress.

A mutable Workspace that never produced/sealed the immutable Artifact/Candidate state represented in the backup may be unavailable after disaster. Global Recovery treats that as possible work loss and follows the Workspace/recovery policy rather than inventing an Artifact from narration or stale metadata.

Likewise, restoring SQLite and CAS does not rewind external execution, Sandboxes, provider state, remote Git refs, or other independent systems. All normal restore fencing/reconciliation remains mandatory.

## 8. Supported restore input

Both backup classes may be restored only through the supported disaster-restore entry procedure defined by Global Recovery:

```text
exclusive installation maintenance authority
restore.pending latch
fresh T0 RestoreGeneration
JournalEpoch handling
external-domain reconciliation
recovery barrier
```

Raw out-of-band replacement of `pantheon.db` is not made safe by backup completeness.

The selected backup's installation identity, format, manifest/checksums and database integrity are validated before authority-bearing work is enabled.

## 9. DurableStateBackup restore ordering

For a `DurableStateBackup`, immutable CAS payload should be verified/staged **before** installing the database snapshot:

```text
acquire exclusive installation maintenance lock
        ↓
validate backup manifest + installation identity
        ↓
verify database snapshot digest
        ↓
verify required CAS object inventory
        ↓
install/deduplicate verified immutable objects into local CAS
        ↓
verify retained payload completeness locally
        ↓
create + fsync restore.pending
        ↓
install selected SQLite snapshot
        ↓
open in forced restore mode
        ↓
T0 fresh RestoreGeneration
        ↓
normal Global Recovery reconciliation/barrier
```

This ordering makes a crash before database replacement conservative:

```text
extra verified immutable CAS objects
→ harmless/orphan/GC-able under normal retention rules
```

Pantheon should not knowingly install a `DurableStateBackup` database while its manifest-declared retained payload is unavailable or corrupt.

## 10. ControlPlaneSnapshot restore

A `ControlPlaneSnapshot` may still be useful and may be restored through the same supported restore-entry procedure, but its weaker guarantee is explicit:

> **Control-plane history is restored; immutable payload completeness is not guaranteed.**

After restore, Global Recovery validates referenced Artifact replicas normally.

For a retained Artifact/Blob whose payload is unavailable:

```text
replica → MISSING / CORRUPT
RecoveryFinding as appropriate
operations requiring bytes → blocked/fenced
```

Pantheon does not fabricate payload or reinterpret missing bytes as proof that the historical Artifact never existed.

## 11. Restore does not replay external effects

A `DurableStateBackup` is stronger than a DB-only snapshot only with respect to immutable Pantheon payload availability. It does not make restored external-effect state current.

After restoring either class:

- restored Grants/Tickets remain fenced by fresh RestoreGeneration;
- restored broker operations are reconciliation-only until current truth is established;
- old AgentControlSessions remain fenced;
- restored negative Attempt/Planner/Evaluation/Sandbox observations are historical snapshot evidence;
- SecretProvider truth is freshly reconciled;
- Integration/Git external state is re-inspected;
- Scheduler dispatch remains behind the recovery barrier and durable operator desired mode.

No backup class authorizes blind replay of historical external mutations.

## 12. Crash semantics of backup creation

Backup creation itself has no authority-bearing external side effect besides writing backup storage.

Conceptually:

```text
crash before SQLite snapshot completes
→ no valid snapshot

crash after ControlPlaneSnapshot completes
→ DB-only snapshot may be retained if its metadata/checksum is valid

crash while copying CAS payload
→ incomplete DurableStateBackup staging only
→ never advertise/publish as complete

crash after all payload verifies but before manifest publication
→ staging remains incomplete; reverify/rebuild publication

crash after immutable manifest publication
→ completed backup is independently verifiable
```

The completion marker/manifest publication must itself be durable/atomic for the chosen backup storage so consumers never infer completion from directory presence alone.

## 13. Verification and security

Before accepting a backup for restore, Pantheon verifies at least:

```text
supported backup format/version
expected installation identity or explicit supported migration/import semantics
database digest/checksum
SQLite schema/application compatibility and integrity checks
for DurableStateBackup:
  manifest digest
  object inventory identity
  every required object digest + size
```

Backup metadata/manifests are not bearer capabilities. Access to backup storage is an operator/deployment security concern and must not expose raw CAS/control-plane paths to untrusted Sandboxes.

Backup output can contain sensitive operational metadata even though it excludes secret material; deployment storage should therefore be access-controlled appropriately.

## 14. Operator surface is not fixed here

V1 architecture does not require a new `BackupController`, database table family, remote backup provider abstraction, or scheduled backup service merely to establish these semantics.

A later Operator API/CLI may expose backup creation/verification/restore workflows. Whatever surface is chosen must preserve this contract:

- distinguish DB-only from payload-complete backup;
- coordinate with CAS GC during capture;
- never claim completion before payload verification/manifest publication;
- enter restore through the supported restore latch/T0 path.

## Core invariants

1. A consistent SQLite snapshot is a `ControlPlaneSnapshot`, not automatically a payload-complete Pantheon backup.
2. `DurableStateBackup` binds one exact ControlPlaneSnapshot to the retained immutable CAS closure derived from that snapshot.
3. Backup payload closure is computed from snapshot retention roots, never from a later live-database view.
4. CAS deletion/GC is excluded from before snapshot creation until the required immutable objects are copied/verified and the completed backup is durably published; ordinary immutable CAS writes may continue.
5. An interrupted/unpublished CAS capture never conveys a `DurableStateBackup` guarantee.
6. Missing/corrupt required retained payload prevents publication of a complete DurableStateBackup unless another trusted replica repairs the exact bytes first.
7. Neither backup class contains SecretProvider material.
8. DurableStateBackup does not promise mutable unsealed Workspace state or external-system state.
9. DurableStateBackup restore stages/verifies required immutable CAS objects before installing the database snapshot.
10. ControlPlaneSnapshot restore is supported but explicitly may be payload-incomplete; missing payload becomes normal recovery/storage faults after restore.
11. Every restore still uses the out-of-database restore latch, fresh T0 RestoreGeneration and Global Recovery reconciliation; payload completeness never authorizes external-effect replay.
12. Backup completion is an immutable/verifiable manifest fact, not directory/file presence or process memory state.
