# Secret Store and Credential Brokering

## Status

Canonical Pantheon secret and credential security subsystem specification.

## Purpose

Pantheon needs to let authorized control-plane operations authenticate to external systems without turning model context, worker environments, SQLite history, Artifacts, Events, workspaces, or backend attachments into secret stores.

The central rule is:

> **`secret.use` authorizes a Pantheon-owned broker to use secret material on behalf of a principal. It does not authorize disclosure of that material to the principal or arbitrary principal-controlled code.**

Raw credential injection into an arbitrary Agent-controlled shell/process is therefore equivalent to `secret.read`, regardless of how the injection mechanism is named.

## 1. Core concepts

Pantheon distinguishes:

```text
SecretRef
  stable logical non-secret identifier

SecretDescriptor
  durable non-secret metadata describing how Pantheon obtains a secret

SecretMaterial
  sensitive bytes held by a SecretProvider

CredentialLease
  optional short-lived derived credential/session with its own lifecycle
```

Example logical identifier:

```text
secret://github/pantheon-write
```

The logical identifier is not the secret value.

## 2. Secret material is not stored in Pantheon SQLite

Pantheon SQLite may store:

- Secret ID;
- logical SecretRef;
- SecretProvider instance;
- provider-private item locator/reference;
- current non-secret version ID;
- classification/state;
- metadata and timestamps.

Pantheon SQLite does not store:

- passwords;
- PATs;
- OAuth refresh/access tokens;
- private keys;
- cloud credentials;
- database passwords;
- other long-lived secret bytes.

This is true even if application-level encryption could be added. Database backup, restore, debugging, migration and recovery must not automatically become secret-material handling operations.

## 3. SecretProvider abstraction

Secret storage is delegated to a pluggable `SecretProvider`.

```text
Secret Broker
     │
     ▼
SecretProvider
     │
     ▼
platform/service secure secret store
```

Pantheon core understands only provider-neutral facts such as:

- SecretRef;
- SecretDescriptor;
- SecretVersionId;
- availability/state;
- provider capabilities.

Concrete platform keychains, secret services, external vaults, cloud secret stores and their internal identifiers remain provider-private.

## 4. No insecure fallback in v1

Pantheon v1 requires a secure SecretProvider for persisted credential workflows.

It does not silently fall back to plaintext files or a home-grown encrypted `~/.pantheon/secrets.json` store.

If a secure provider is unavailable, operations requiring stored credentials are unavailable/fail closed. Unrelated Tasks and Runs remain unaffected.

## 5. Logical identity versus material version

A logical SecretRef may remain stable across credential rotation:

```text
secret://github/pantheon-write
  version 7
  version 8
  version 9
```

Each material version receives a random non-secret `SecretVersionId`.

Pantheon does not identify secret versions by storing a hash of the secret bytes. Durable unsalted hashes of low-entropy passwords/secrets can become useful offline guessing material and are unnecessary for Pantheon's reconciliation model.

## 6. CredentialBinding

Configuration resolves semantic operations/resources to logical credential authority.

Conceptually:

```yaml
binding:
  resource: repo://Pantheon/origin
  action: git.push
  credential:
    ref: secret://github/pantheon-write
  mechanism:
    kind: git-credential
```

The model normally requests the semantic action (`git.push`), not a SecretRef.

`CredentialBindingRegistry` is an immutable `credentialBindings` component of ConfigurationRevision. Its canonical component identity is `credentialBindingRegistryDigest`.

Each compiled exact binding additionally has a canonical `credentialBindingAuthorityDigest` over at least:

```text
normalized semantic action
normalized resource scope
logical SecretRef
broker mechanism class
credential-use constraints
```

The authority digest deliberately excludes:

```text
SecretVersionId
secret bytes
provider's current material value
```

because those facts describe rotatable material, not which credential authority the Run was allowed to use.

## 7. Frozen authority, rotatable material

A Run's immutable ExecutionBinding freezes the `credentialBindingRegistryDigest` from the ConfigurationRevision captured at T3.

That frozen registry is the Run's maximum logical credential-mapping universe. A later current configuration can remove/tighten/change an exact binding and thereby deny an operation, but it cannot remap the existing Run onto a different credential authority.

For a credential-bearing action, Pantheon resolves the exact normalized action/resource against both:

```text
Run frozen CredentialBindingRegistry
current active CredentialBindingRegistry
```

V1 compatibility is deliberately conservative:

```text
frozen exact binding exists
AND current exact binding exists
AND frozen credentialBindingAuthorityDigest
    == current credentialBindingAuthorityDigest
→ credential mapping may proceed to secret.use/current SecretDescriptor checks

otherwise
→ DENY / credential-binding-stale
```

Whole-registry digest equality is **not** required. Changing an unrelated credential mapping must not invalidate an existing Run.

A later change from one logical SecretRef to a different, broader SecretRef therefore does not silently broaden an existing Run.

However, rotation of the material behind the same logical SecretRef may apply to existing authorized Runs:

```text
same SecretRef + new SecretVersionId
= rotation

different SecretRef / mechanism / credential-use constraint
= authority/configuration change
```

Effective credential authority for a live operation is constrained by the Run's frozen exact binding, the current compatible exact binding, current semantic authorization, `secret.use` authorization and current SecretDescriptor/provider state.

## 8. Action-first credential resolution

Preferred flow:

```text
Agent requests semantic action
        ↓
authenticate Attempt
        ↓
authorize action/resource under current policy + frozen Run authorization ceiling
        ↓
resolve exact binding from Run frozen CredentialBindingRegistry
        ↓
resolve exact binding from current CredentialBindingRegistry
        ↓
require exact credentialBindingAuthorityDigest equality
        ↓
atomic Grant/ticket redemption + exact broker-operation creation
        ↓
authorize secret.use as required
        ↓
require usable/reconciled SecretDescriptor
        ↓
retrieve current permitted SecretVersion/material
        ↓
perform exact broker operation
```

The exact broker-operation authority, including non-secret binding provenance, is committed before secret material retrieval or external effect. A retry/reconciliation follows that same broker operation; it does not re-resolve into another credential mapping merely because configuration later changed.

`secret.use` is primarily an internal sub-authorization rather than a generic Agent tool.

Agents do not need to enumerate or choose credentials for normal operations.

## 9. `secret.use` means brokered use

The preferred architecture is:

```text
Agent
  │ semantic action
  ▼
Pantheon Broker
  │
  ├─ obtains secret material
  ├─ performs the exact authorized operation
  └─ releases/zeroizes transient material
       │
       ▼
external system
```

The worker does not receive a reusable credential.

## 10. `secret.read` in v1

`secret.read` remains a canonical action for future compatibility but is hard-denied for Agent Control principals in v1 and is non-approvable.

Agent Control exposes no generic:

```text
secret.list
secret.get
secret.search
```

surface.

Operator secret management is a separate authority domain and does not imply Agent `secret.read`.

## 11. No arbitrary worker environment injection

The following is not `secret.use`:

```text
GITHUB_TOKEN=...
agent-owned shell/process
```

Because arbitrary Agent-controlled code can print, copy or exfiltrate the credential, this is equivalent to disclosure and therefore requires `secret.read`, which v1 denies.

Secret values are also never placed in command-line arguments.

## 12. Broker-owned compatibility processes

Some third-party tools accept credentials only via environment variable, stdin, file descriptor or temporary file.

Pantheon may support these mechanisms only inside a tightly broker-owned process boundary where:

- executable identity is fixed/validated;
- argv is validated and not arbitrary shell text;
- environment is controller-owned;
- Agent cannot replace/wrap the executable;
- filesystem/workspace exposure is bounded;
- outputs are scrubbed/handled as sensitive where necessary;
- sandbox restrictions apply.

Preference order is:

```text
native credential protocol / signing agent
  ↓
file descriptor / pipe
  ↓
protected ephemeral file
  ↓
environment variable only if unavoidable
```

The defining property is that the receiving process is broker-controlled, not arbitrary model-controlled code.

## 13. Git credential brokering

Git operations are a primary v1 use case.

Instead of exposing a PAT in the worker environment, a Git Integration Broker can invoke controlled Git with a Pantheon credential helper/askpass-style adapter that obtains credentials from the Secret Broker.

The producing Agent requests `git.push`; Pantheon remains responsible for authorization, exact ref/resource scope, credential resolution and execution.

## 14. Backend operational credentials

ExecutorBackend credentials are separate from Task/Agent credentials.

A backend configuration may contain SecretRefs which its adapter resolves through Secret Broker for provider API calls.

The model never receives those credentials.

Secret Broker therefore serves control-plane/broker/backend principals as well as Agent-triggered operations, with principal-specific authorization.

Run-frozen CredentialBindingRegistry semantics apply to Run/Agent-triggered credential authority. Control-plane backend credentials follow their own immutable backend/configuration provenance rather than pretending to be Run CredentialBindings.

## 15. Secret material path

Desired material flow:

```text
SecretProvider
    ↓
Secret Broker protected memory
    ↓
specific broker/credential adapter
    ↓
external authentication
    ↓
release/zeroize transient material
```

Secret bytes never enter:

- ContextPlan;
- AgentControl requests;
- Events;
- Artifacts;
- Evidence;
- Run snapshots;
- backend attachment state;
- Pantheon SQLite;
- ordinary diagnostic logs/traces.

Redaction is defense in depth; avoiding propagation is the primary protection.

## 16. SecretDescriptor

Conceptual durable metadata:

```yaml
secret:
  id: secret_123
  ref: secret://github/pantheon-write

  provider:
    instance: secrets://platform-default
    opaqueItem: provider-private

  kind: credential
  currentVersion: secretver_8

  status:
    state: ACTIVE
```

No secret value is represented.

## 17. Secret states

V1 normalized states:

```text
ACTIVE
LOCKED
MISSING
DRIFTED
UNKNOWN
DISABLED
```

- `ACTIVE`: provider confirms the expected item/version.
- `LOCKED`: operator/user interaction is needed before retrieval.
- `MISSING`: provider conclusively reports no expected secret.
- `DRIFTED`: provider item exists but its observed version identity does not match Pantheon durable metadata/history.
- `UNKNOWN`: provider truth cannot currently be established.
- `DISABLED`: operator/policy prohibits use.

A locked provider surfaces operator interaction; an Agent never answers secure-store unlock/user-presence prompts directly.

## 18. Provider reconciliation metadata

A SecretProvider used for Pantheon-managed persisted secrets must provide:

- stable logical item identity;
- an observable non-secret Pantheon version marker.

Pantheon records random metadata such as:

```text
installationId
secretId
secretVersionId
```

with the external secret item when the provider supports metadata/attributes.

These values enable crash/restore reconciliation without hashing or exposing secret material.

## 19. Secret mutation protocol

SQLite and SecretProvider are separate transactional systems.

Mutations therefore use durable intent before external mutation:

```text
BEGIN
create SecretMutationIntent
record target SecretVersionId
COMMIT

→ provider mutation
→ inspect provider state

BEGIN
update SecretDescriptor observed/current version
complete mutation intent
append Event
COMMIT
```

No SQLite transaction remains open across a Keychain/secret-service/network call.

## 20. Secret command idempotency

Generic Pantheon `commandId` idempotency applies, but secret bytes are never part of a persisted request body/hash.

A secret mutation uses:

```text
commandId
+ non-secret metadata hash
+ SecretMutationIntent
```

The command ID is single-use. Repeating the same command ID reconciles/returns the existing mutation rather than creating another mutation.

Pantheon does not persist or compare retried secret bytes.

## 21. Operator secret UX

Operator Control may expose operations such as:

```text
pantheon secret list
pantheon secret status
pantheon secret set
pantheon secret rotate
pantheon secret delete
```

Secret input is sensitive interactive input/stdin or an equivalent secure local mechanism.

Pantheon should not encourage:

```text
--value <secret>
```

command-line arguments.

V1 does not need a generic `secret get --raw`/display-value command.

Sensitive request bodies are excluded from Event bodies, Command request storage, traces and ordinary request logging.

## 22. CredentialLease

Some future SecretProviders may derive short-lived credentials.

Conceptually:

```yaml
credentialLease:
  id: credlease_123
  secretRef: secret://database/reporting-role
  holder:
    operation: brokerop_77
  expiresAt: ...
  provider:
    leaseRef: opaque
  state: ACTIVE
```

Only metadata is durable. Generated credential material is not stored in Pantheon SQLite.

V1 may implement static secret providers only; the `CredentialLease` lifecycle is defined now because it creates recovery obligations.

## 23. CredentialLease lifecycle

A dynamically acquired lease may be:

```text
ACTIVE
REVOKING
REVOKED
EXPIRED
UNKNOWN
```

Revocation/cleanup is an external side-effect obligation. If revocation state is unknown, Global Recovery preserves the unresolved obligation and does not falsify successful revocation.

Deletion of a static stored secret only means Pantheon can no longer retrieve it; it does not claim remote issuer revocation.

## 24. Audit

Secret-related audit records contain only metadata such as:

```yaml
operation: git.push
credentialBinding:
  frozenRegistry: sha256:CBR1
  currentRegistry: sha256:CBR2
  authority: sha256:CBA
credential:
  secretId: secret_123
  version: secretver_9
brokerOperation: brokerop_77
result: succeeded
```

The binding registry/authority digests explain **why this logical credential authority was permitted**. `SecretVersionId` explains **which rotatable material version was factually used**. The latter is not part of the authority digest.

Audit records never contain:

- secret material;
- authorization headers;
- secret hashes;
- private keys;
- passwords/tokens.

## 25. Capability/grant redemption before secret retrieval

Credential-bearing operations follow this order:

```text
request
  ↓
current Attempt/principal identity
  ↓
current Task/Run state
  ↓
current semantic policy
  ↓
load Run frozen credentialBindingRegistryDigest
  ↓
resolve exact normalized action/resource in frozen + current registries
  ↓
require exact credentialBindingAuthorityDigest equality
  ↓
atomic scoped Grant-use redemption/CAS if needed
+ exact broker-operation creation with binding provenance
  ↓
secret.use authorization
  ↓
require usable/reconciled SecretDescriptor
  ↓
retrieve current permitted secret material
  ↓
execute exact broker effect
```

Secret retrieval never occurs optimistically before authorization or before the broker operation has durably captured the exact credential authority being redeemed.

A stale bearer capability may not bypass current policy/state or current credential-binding compatibility. Any internal capability ticket is short-lived/exact and revalidated at redemption.

A retry of an already-created broker operation uses/reconciles that operation's original binding authority and external idempotency identity. It does not re-run current credential resolution to transform the old operation into a different logical credential authority.

## 26. Availability is operation-scoped

A locked/missing SecretProvider does not make Pantheon globally unready.

Only work requiring the corresponding credential is blocked/temporarily unavailable.

Normalized failures/conditions may include:

```text
secret.unavailable
secret.locked
secret.missing
secret.drifted
credential-binding-stale
```

## 27. Child Task inheritance

Child Tasks inherit authority ceilings, not secret material and not an implicit credential inventory.

A child can use a credential only if its effective Task/Goal/Agent/current policy plus its own Run's frozen/current-compatible credential binding permits the semantic operation requiring it.

Spawn requests contain no raw secret propagation mechanism.

## 28. Sandbox dependency

Credential brokering reduces exposure but does not replace containment.

Worker environments must be prevented from bypassing the broker by accessing:

- Pantheon's secure-store authority/session;
- operator broker/control sockets;
- Pantheon process memory/files;
- peer worker channels;
- host credential agents/control sockets unless explicitly brokered.

These physical guarantees belong to the Sandbox Broker architecture.

## 29. Disaster restore

Pantheon SQLite backups intentionally exclude secret material.

After restore, durable SQLite metadata and the current external SecretProvider may represent different histories.

Secret Reconciler compares expected `SecretVersionId`/identity metadata with provider-observed metadata.

Mismatch becomes `DRIFTED` and credential use fails closed until operator reconciliation.

Pantheon never silently assumes that whichever current secure-store value exists belongs to the restored database history.

Frozen CredentialBindingRegistry/authority provenance is non-secret configuration/audit history and may survive restore. It still cannot make an old-generation broker operation executable: RestoreGeneration fencing and normal restore reconciliation apply first.

## 30. Global Recovery integration

SecretProvider state is an external-effect domain and participates in the same recovery barrier/fencing model as executors, Sandboxes, Integration and other independently transactional systems.

The generic inventory in `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md` is intentionally non-exhaustive. For the Secret subsystem, Global Recovery must additionally discover at least:

```text
SecretMutationIntent not durably completed
CredentialLease in ACTIVE | REVOKING | UNKNOWN
SecretDescriptor whose provider identity/version must be freshly reconciled
  because an unresolved intent/lease exists
  or because restore may have rewound SQLite behind the SecretProvider
```

A SecretRef/provider problem is blast-radius scoped. Recovery may satisfy the global barrier by durably fencing the affected SecretRef/provider scope while unrelated work continues. Any credential-bearing operation that requires a fenced SecretRef fails closed until its provider truth is established or an operator resolves it.

### SecretMutationIntent recovery

A mutation intent is the stable Pantheon reconciliation identity for the pending logical secret mutation. It records only non-secret facts required to interpret provider state, including the target `SecretVersionId`, provider/item identity, mutation kind and expected prior non-secret identity/state where applicable.

Recovery always **inspects the provider before deciding whether another provider mutation is permitted**.

Fresh provider observation is interpreted conservatively:

```text
provider shows the exact target SecretVersionId / expected target marker
→ CONFIRMED
→ update SecretDescriptor to the observed target
→ complete the existing SecretMutationIntent

provider conclusively shows the expected prior state and proves target mutation did not apply
→ NOT_APPLIED
→ record/reconcile that outcome
→ do not automatically replay lost secret material

provider shows a different item/version/history than either expected prior or target
→ DRIFTED
→ preserve the intent as reconciliation evidence
→ fence credential use for that SecretRef

provider truth cannot be established
→ UNKNOWN
→ preserve the unresolved intent
→ fence credential use for that SecretRef
```

`set`/`rotate` secret bytes are deliberately absent from SQLite. Therefore a daemon crash cannot turn a pending intent into authority to regenerate or reissue those bytes. If the provider conclusively proves the mutation was not applied and fresh material is still desired, the operator performs a new explicit secret mutation with new command authority/material after current state is observed. Pantheon never fabricates bytes, derives them from the old intent, or treats a repeated transport request as proof that the sensitive payload is identical.

An operation whose external effect is safely repeatable without secret material, such as an exact provider delete/revoke against a stable item/lease identity, may be retried only when the SecretProvider adapter contract explicitly guarantees idempotent repetition for that exact identity and current policy still authorizes it. Otherwise the state remains reconciled/fenced for operator action.

### CredentialLease recovery

A nonterminal CredentialLease is recovered by the same durable Pantheon lease identity plus its stable provider `leaseRef`; a new lease identity is never created merely because acknowledgement/revocation state is ambiguous.

```text
ACTIVE
→ inspect the same leaseRef
→ preserve ACTIVE only when current provider evidence supports it

REVOKING
→ inspect the same leaseRef first
→ repeat revoke only if exact-lease revoke is adapter-declared idempotent/safe

REVOKED | EXPIRED
→ retain factual terminal metadata

UNKNOWN
→ do not claim REVOKED
→ keep the lease/relevant SecretRef fenced until current provider evidence or explicit operator resolution is sufficient
```

Provider expiry/revocation observation is factual state; a timeout or daemon restart is not proof of revocation.

### Disaster-restore rule

Restore can rewind both the descriptor and the presence/absence of mutation-intent rows while the external secure store remains at a newer history. Therefore a restored `SecretDescriptor.currentVersion`, a restored PENDING intent, or absence of a later intent is snapshot evidence only. None proves what happened after the backup.

Before post-restore credential use, Secret Reconciler obtains fresh provider identity/version evidence for the SecretRef. Matching current provider metadata may re-establish `ACTIVE`; mismatching current metadata becomes `DRIFTED`; inability to establish provider truth becomes `UNKNOWN`. A restored pending/absent mutation record is never authority to replay a provider mutation.

RecoveryFindings/audit contain only non-secret item/version/intent/lease identities and observations. Secret material remains excluded from recovery state, Events and diagnostics.

## 31. Rotation

When the material behind the same logical SecretRef rotates:

```text
same logical SecretRef
same exact credentialBindingAuthorityDigest
new SecretVersionId
→ existing otherwise-authorized Run may use current version
```

The broker first rechecks current policy, exact frozen/current binding compatibility and current SecretDescriptor/provider state. Rotation is not a bypass around a removed/changed CredentialBinding.

Audit records which material version each operation used.

Rotation does not require a new Run solely because bytes/SecretVersionId changed.

## 32. Secrets are not Artifacts

Active secret material may never be sealed/stored as a normal Artifact.

Artifact retention/content-addressing/provenance semantics are inappropriate for active credential lifecycle.

Agent `artifact.seal` cannot use SecretRef as a secret-material source.

## 33. Persistence metadata

Likely metadata tables include:

```text
secret_descriptors
secret_mutation_intents
credential_leases
credential_use_records
```

CredentialBindingRegistry itself is immutable ConfigurationRevision component data rather than SecretProvider material. Credential-bearing `broker_operations` persist the non-secret frozen/current binding provenance and exact `credentialBindingAuthorityDigest` that authorized the operation; credential-use metadata may record the factual SecretVersionId actually used.

None contain secret bytes.

Exact DDL remains implementation-level.

## 34. Management authority

Only Operator Control may:

- create SecretDescriptors;
- set/rotate/delete secret material;
- enable/disable secrets;
- resolve drift;
- change CredentialBinding configuration.

Agent Control can only cause an already-authorized semantic broker operation to use a credential.

## Core invariants

1. SecretRef, SecretDescriptor, SecretMaterial and CredentialLease are separate concepts.
2. Pantheon SQLite never stores long-lived secret material.
3. Long-lived storage is delegated to a secure pluggable SecretProvider.
4. V1 has no insecure file fallback.
5. SecretRefs remain stable across material rotation; versions use random non-secret IDs.
6. ConfigurationRevision has an immutable CredentialBindingRegistry addressed by `credentialBindingRegistryDigest`; each exact binding has a `credentialBindingAuthorityDigest` over logical authority only.
7. ExecutionBinding freezes the Run's CredentialBindingRegistry digest, not secret material versions.
8. Credential-bearing Run operations require exact frozen/current binding-authority equality; current configuration may deny an old Run but may never remap it onto broader credential authority.
9. Whole-registry equality is not required; unrelated credential-binding changes do not invalidate an existing Run.
10. SecretVersionId/secret bytes are excluded from credential-binding authority; rotation behind the same logical SecretRef may affect existing otherwise-authorized Runs after current checks.
11. Agents request semantic operations; credential resolution is normally internal.
12. `secret.use` means brokered use, not credential disclosure.
13. `secret.read` is hard-denied for Agent principals in v1.
14. Worker environments and command-line arguments are not generic secret injection channels.
15. Compatibility credential injection is restricted to broker-owned processes.
16. Backend credentials remain control-plane credentials and are never exposed to models.
17. Secret bytes never enter ContextPlan, Events, Artifacts, Evidence, Run snapshots, backend attachments, SQLite or ordinary logs.
18. Credential-bearing broker-operation authority is durably committed with exact non-secret binding provenance before secret retrieval/external effect; retries cannot re-resolve into a different credential authority.
19. Secret mutation uses durable intent, external mutation, reconciliation and durable completion.
20. Secret mutation command idempotency never persists secret bytes.
21. Global Recovery inventories/reconciles incomplete SecretMutationIntents and nonterminal CredentialLeases and fences only their affected SecretRef/provider scope where possible.
22. A pending `set`/`rotate` intent never authorizes automatic replay of secret material after crash; provider observation establishes CONFIRMED, NOT_APPLIED, DRIFTED or UNKNOWN first.
23. CredentialLease revocation is a durable recovery obligation; UNKNOWN/timeout never becomes fabricated REVOKED state.
24. Disaster restore treats restored secret metadata/intent absence as snapshot evidence and requires fresh provider identity/version reconciliation before credential use.
25. Secret retrieval happens only after current authorization/grant redemption, exact frozen/current CredentialBinding compatibility and a usable reconciled SecretDescriptor.
26. Secret availability only blocks operations that require that credential.
27. Child Tasks inherit authority ceilings, not credential material.
28. Secret material is never treated as an Artifact.
29. Only Operator Control manages secret descriptors/material/bindings.
