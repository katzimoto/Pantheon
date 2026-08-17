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

`CredentialBindingRegistry` is versioned ConfigurationRevision state.

## 7. Frozen authority, rotatable material

A Run freezes the logical credential binding/authority ceiling that existed when its Binding was formed.

A later change from one logical SecretRef to a different, broader SecretRef does not silently broaden an existing Run.

However, rotation of the material behind the same logical SecretRef may apply to existing authorized Runs:

```text
same SecretRef + new SecretVersionId
= rotation

different SecretRef
= authority/configuration change
```

Effective credential authority for a live operation is constrained by the Run's frozen binding/ceiling and current policy/configuration.

## 8. Action-first credential resolution

Preferred flow:

```text
Agent requests semantic action
        ↓
authenticate Attempt
        ↓
authorize action/resource
        ↓
resolve frozen/current CredentialBinding
        ↓
authorize secret.use as required
        ↓
perform exact broker operation
```

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
credential:
  secretId: secret_123
  version: secretver_9
brokerOperation: brokerop_77
result: succeeded
```

They never contain:

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
current policy
  ↓
atomic scoped Grant-use redemption/CAS if needed
  ↓
exact broker operation authority established
  ↓
resolve CredentialBinding
  ↓
secret.use authorization
  ↓
retrieve secret material
  ↓
execute
```

Secret retrieval never occurs optimistically before authorization.

A stale bearer capability may not bypass current policy/state. Any internal capability ticket is short-lived/exact and revalidated at redemption.

## 26. Availability is operation-scoped

A locked/missing SecretProvider does not make Pantheon globally unready.

Only work requiring the corresponding credential is blocked/temporarily unavailable.

Normalized failures/conditions may include:

```text
secret.unavailable
secret.locked
secret.missing
secret.drifted
```

## 27. Child Task inheritance

Child Tasks inherit authority ceilings, not secret material and not an implicit credential inventory.

A child can use a credential only if its effective Task/Goal/Agent/current policy plus frozen credential binding permits the semantic operation requiring it.

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

## 30. Rotation

When the material behind the same logical SecretRef rotates:

```text
new broker operations
→ current SecretVersionId
```

Existing Runs may continue using the logical credential if still permitted by their frozen authority/current policy.

Audit records which material version each operation used.

Rotation does not require a new Run solely because bytes changed.

## 31. Secrets are not Artifacts

Active secret material may never be sealed/stored as a normal Artifact.

Artifact retention/content-addressing/provenance semantics are inappropriate for active credential lifecycle.

Agent `artifact.seal` cannot use SecretRef as a secret-material source.

## 32. Persistence metadata

Likely metadata tables include:

```text
secret_descriptors
secret_mutation_intents
credential_leases
credential_use_records
```

None contain secret bytes.

Exact DDL remains implementation-level.

## 33. Management authority

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
6. Runs freeze logical CredentialBinding authority, not secret material versions.
7. Rotation may affect existing Runs; changing logical credential authority may not silently broaden them.
8. Agents request semantic operations; credential resolution is normally internal.
9. `secret.use` means brokered use, not credential disclosure.
10. `secret.read` is hard-denied for Agent principals in v1.
11. Worker environments and command-line arguments are not generic secret injection channels.
12. Compatibility credential injection is restricted to broker-owned processes.
13. Backend credentials remain control-plane credentials and are never exposed to models.
14. Secret bytes never enter ContextPlan, Events, Artifacts, Evidence, Run snapshots, backend attachments, SQLite or ordinary logs.
15. Secret mutation uses durable intent, external mutation, reconciliation and durable completion.
16. Secret mutation command idempotency never persists secret bytes.
17. CredentialLease revocation is a durable recovery obligation.
18. Secret retrieval happens only after current authorization/grant redemption.
19. Secret availability only blocks operations that require that credential.
20. Child Tasks inherit authority ceilings, not credential material.
21. Disaster restore reconciles external secret identity/version metadata and fails closed on drift.
22. Secret material is never treated as an Artifact.
23. Only Operator Control manages secret descriptors/material/bindings.
