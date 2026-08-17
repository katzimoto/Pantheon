# Permissions and Capabilities

## Status

Canonical Pantheon authorization specification.

## Purpose

Pantheon is the canonical authorization authority independent of execution backend/model/harness. The subsystem answers:

> May principal X perform action Y on resource Z under context C **now**?

Authorization is binary: `PERMIT` or `DENY`. Approval is not a third authorization outcome; approval creates a narrowly scoped Grant and the action is re-evaluated.

See also:

- `docs/architecture/execution/agent-control-channel.md`
- `docs/architecture/operations/configuration-and-policy-revisions.md`
- `docs/architecture/security/sandbox-broker-and-isolation.md`
- `docs/architecture/security/secret-store-and-credential-brokering.md`

## Foundational principles

1. **Pantheon owns authorization.** Models/backends may never broaden authority.
2. **Default deny.** No applicable permit means DENY.
3. **Hard forbid wins.** Lower scopes/Grants cannot bypass hard policy.
4. **Approval creates a scoped Grant, not broad trust.**
5. **Authorization and physical containment are separate.** Sandbox ambient authority must already be no broader than the semantic ceiling.
6. **Fail closed.** If Pantheon cannot enforce required authority, work does not start/continue.
7. **Every consequential privileged operation is auditable.**
8. **Authorization is checked at redemption/use time, not only when a request was first proposed.**
9. **Credential mapping is not authorization.** A permitted semantic action that requires credentials must separately satisfy the Run's frozen credential-binding ceiling, the current exact binding and `secret.use` before material retrieval.

## Request path

For Agent-triggered operations:

```text
Agent/model
  ↓
Agent Control Gateway
  ↓ authenticate Attempt
server derives Task/Run/Agent/current state
  ↓
Action Normalizer
  ↓
Policy Decision Point
  ↓
DENY or PERMIT
  ↓
transactional authority redemption
  ↓
for credential-bearing effects:
  exact frozen/current CredentialBinding compatibility
  + secret.use authorization
  ↓
Execution/Credential/Integration/etc. Broker
  ↓
external effect
```

Agent Control credentials authenticate the Attempt only. They grant no action authority.

Operator requests use the separate Operator Control principal/surface.

## Canonical actions and resources

Permissions describe semantic effects rather than concrete provider APIs. Initial action families include:

```text
filesystem.read
filesystem.write
filesystem.delete
shell.execute
process.spawn
network.connect
network.listen
git.read
git.commit
git.push
git.integrate
secret.use
secret.read
container.run
mcp.call
agent.delegate
browser.navigate
service.read
service.mutate
artifact.read
artifact.seal
task.spawn
task.graph.propose
task.submit_result
```

Resources use typed URI-like identifiers such as:

```text
workspace://
file://
repo://
net://
secret://
process://
container://
mcp://
agent://
service://
artifact://
```

Provider/backend-specific tool names are adapter-private translations.

## Configuration and current authority

Operator-controlled policy is compiled into immutable ConfigurationRevision components. Authorization decisions record at least:

```text
configRevision
authzPolicyDigest
frozen Run authorization ceiling digest (for Run principals)
Task/Goal restrictions
Grant refs actually consumed
```

For credential-bearing broker operations, authority provenance additionally records the Run's frozen `credentialBindingRegistryDigest` and the exact resolved `credentialBindingAuthorityDigest` that matched current configuration. These are separate from `authzPolicyDigest` and contain no secret material.

For a live Run, effective semantic authority is bounded by:

```text
built-in hard policy
∩ frozen Run authorization ceiling
∩ current active authorization policy
∩ current Task/Goal restrictions
+ applicable narrowly scoped Grant where the action is approvable
```

A policy relaxation never silently broadens an existing Run. A tightening may deny future operations immediately and may require Run termination if its Sandbox cannot physically enforce the new ceiling.

Credential-bearing actions have an additional conjunctive ceiling:

```text
semantic authority above
∩ exact binding from Run's frozen CredentialBindingRegistry
∩ exact compatible binding from current CredentialBindingRegistry
∩ current secret.use authorization
∩ current usable/reconciled SecretDescriptor
```

The exact frozen/current binding is compatible in v1 only when its canonical `credentialBindingAuthorityDigest` matches. Current configuration may remove/tighten/change a binding and deny the old Run, but it cannot remap the old Run onto a different credential authority.

## Cedar policy engine

Pantheon embeds Cedar (or an equivalent deterministic PDP matching this contract) for `principal/action/resource/context` evaluation. User/project configuration compiles into validated policy; invalid policy never activates.

Normal users need not author raw Cedar. Configuration composition/revision semantics are defined by `docs/architecture/operations/configuration-and-policy-revisions.md`.

## Approval and Grants

An approvable denial produces an operator-visible ApprovalRequest. Only an appropriate trusted operator authority may create the Grant.

A Grant is scoped across as many dimensions as practical:

```text
principal / Agent / Attempt / Run / Task
canonical action
resource
argument constraints
expiry
maximum uses
restoreGeneration
```

Persistent operator decisions become explicit user/project configuration rather than an immortal runtime Grant.

## Restore-generation fencing

Runtime Grants and capability tickets are authority minted within one installation `RestoreGeneration`. The generation is a fresh unpredictable value that survives ordinary daemon restart and is rotated as the first durable authority transition after disaster restore.

A restored historical Grant is evidence of a prior approval, not automatically current authority. Redemption requires:

```text
grant.restoreGeneration == current RestoreGeneration
```

and capability-ticket redemption requires the same generation match. A mismatch fails closed before use-count mutation, broker-operation creation, secret retrieval or external effect.

Operators do not reactivate an old-generation Grant in place. If the same authority is still desired after restore, the operator explicitly re-affirms it, creating a new Grant under the current RestoreGeneration. This prevents one human approval or one-use Grant from becoming reusable because SQLite was rewound behind an already-applied external effect.

Broker operations created by redemption also record the current RestoreGeneration. After restore, an old-generation broker operation may be inspected/reconciled under its original stable external identity, but its restored `PENDING`/incomplete state is never authority to issue the external effect again. `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md` defines the reconciliation-only restore rule.

## Atomic Grant use-count redemption

A `uses: N` Grant is concurrency-sensitive authority and must be consumed transactionally.

Correct redemption for a consequential operation:

```text
BEGIN IMMEDIATE

re-read:
  current Attempt/Run/Task authority
  current ConfigurationRevision/authz policy
  current RestoreGeneration
  Grant state + scope + expiry + restoreGeneration
  remaining uses
  exact normalized action/resource/args hash

require Grant.restoreGeneration == current RestoreGeneration
re-evaluate authorization under current policy

if operation is credential-bearing:
  load Run.ExecutionBinding.credentialBindingRegistryDigest
  resolve exact normalized action/resource in frozen CredentialBindingRegistry
  resolve same exact normalized action/resource in current CredentialBindingRegistry
  require both bindings exist
  require frozen credentialBindingAuthorityDigest == current credentialBindingAuthorityDigest
  authorize secret.use for the resolved logical SecretRef/operation
  require SecretDescriptor currently usable/reconciled

CAS decrement/increment Grant use accounting exactly once
create/transition exact broker-operation authority record under current RestoreGeneration
for credential-bearing operation, persist non-secret binding provenance used for this authority decision
append authorization/audit Event

COMMIT

only then retrieve transient secret material if required
external effect
```

The broker-operation provenance for a credential-bearing effect includes at least the Run's frozen registry digest, the exact `credentialBindingAuthorityDigest`, current configuration/registry identity used for the compatibility decision, and the logical SecretRef/SecretDescriptor identity. The factual `SecretVersionId` actually used is recorded after material resolution/effect observation; it is not part of the frozen authority digest.

Two concurrent requests cannot both consume the last remaining use. Retries of the same idempotent operation return/reconcile the already-created broker operation rather than consuming another use or re-resolving it into a different credential authority.

A broker operation from an older RestoreGeneration is not eligible for this normal retry path. It is reconciliation-only until its external outcome is established or explicitly force-resolved; Pantheon never changes its idempotency identity merely to make a restored operation executable again.

## Capability tickets

Pantheon may use an internal short-lived `CapabilityTicket` as an implementation reference binding:

```text
Attempt/Run/Task
exact action/resource
argsHash
Grant/decision refs
restoreGeneration
expiry/single-use state
```

But a ticket is **not durable bearer authority that bypasses current policy or credential-binding compatibility**.

Before redemption/external effect, the broker transaction must revalidate current authority, require the ticket's RestoreGeneration to equal the current generation, re-run the exact frozen/current credential-binding check when the operation is credential-bearing, and atomically mark/consume the ticket. A ticket issued before security tightening, credential remapping, or recovered from an older disaster-restore generation can therefore be denied at redemption.

No Agent may mint a ticket. Ticket IDs/bytes are never equivalent to Operator Control authority.

## Secret authorization

Keep distinct:

```text
semantic authorization
  may this principal perform git.push on repo://... now?

credential binding
  which logical credential authority may satisfy that exact operation for this Run?

secret.use
  may Pantheon's broker use the resolved credential material for this exact authorized operation?

secret.read
  disclose raw secret material to principal-controlled code/context
```

For Agent principals, v1 built-in hard policy denies `secret.read` non-approvably. Injecting a raw secret into an arbitrary Agent-controlled shell/environment/file is therefore not permitted as `secret.use`; it would be disclosure.

Credential-requiring semantic actions normally trigger `secret.use` as an internal sub-authorization. Before that sub-authorization/retrieval, an existing Run must resolve the exact action/resource through its frozen `credentialBindingRegistryDigest` and the current active CredentialBindingRegistry. V1 requires equality of the exact `credentialBindingAuthorityDigest`; whole-registry equality is not required.

CredentialBinding authority covers the canonical action/resource scope, logical SecretRef, broker mechanism class and credential-use constraints. It excludes SecretVersionId and secret bytes. Therefore material rotation behind the same logical SecretRef may affect a still-authorized Run after SecretProvider reconciliation, while remapping to a different SecretRef or mechanism denies the old Run rather than broadening it.

Secret material remains in the SecretProvider/Credential Broker boundary.

## Delegation and Task spawning

Delegation is canonical authority, not an unmanaged backend sub-process feature. `task.spawn`/`task.graph.propose` are authorized through Agent Control and Graph Controller.

Child Tasks inherit ceilings, never privileges. A child cannot broaden access by recursion, and it does not inherit credential material.

## Sandbox and ambient authority

Cedar/PDP controls semantic authority; it cannot intercept arbitrary syscalls made by shell code. Therefore Sandbox enforcement is mandatory for ambient actions.

For every Run:

```text
physical Sandbox ambient capability
<= effective frozen ambient authority envelope
```

Examples:

- workspace filesystem access -> Sandbox mounts;
- process spawning -> Sandbox process boundary;
- direct network -> Sandbox network controls;
- Git push/integration -> brokered;
- secret use -> brokered;
- Artifact seal/Task spawn -> Agent Control + controllers.

Hard security exclusions for untrusted Agent Sandboxes include:

```text
Pantheon Operator Control socket
pantheon.db
raw Artifact CAS
active policy/configuration storage
SecretProvider administration
peer Task workspaces
authoritative shared Git ref/common-dir authority
host container/hypervisor runtime socket
host credential agents
```

Same-user filesystem permissions alone are not a sufficient Agent/operator boundary.

## Sandbox classes

Canonical security classes are defined by `docs/architecture/security/sandbox-broker-and-isolation.md`:

```text
TRUSTED_HOST
CONTAINER
HARDENED
```

The older `native/workspace/isolated` terminology is not a security-class contract. Worktree/Workspace isolation is separate from Sandbox isolation.

## Brokered privileged infrastructure

Agents never receive direct ambient control of privileged host systems such as container runtime sockets, host SSH agents, secret stores, cloud control planes or shared Git integration authority.

They request semantic operations; Pantheon authorizes and a privileged broker performs the exact effect.

## Backend-native enforcement

Execution adapters may compile Pantheon restrictions into native harness controls as defense in depth. Adapter-native controls may tighten but never broaden Pantheon authority.

If a required policy cannot be enforced by native controls **and** Sandbox/Broker boundaries cannot compensate, the execution configuration is incompatible and fails closed.

## Audit

Audit/Event records identify, without sensitive material:

- principal/Attempt/Run/Task where relevant;
- normalized action/resource/argument hash;
- decision and reason;
- exact ConfigurationRevision/authz digest;
- Run `credentialBindingRegistryDigest` and exact `credentialBindingAuthorityDigest` for credential-bearing effects;
- current credential-binding registry/config identity compared at redemption;
- logical SecretRef/SecretDescriptor identity and factual SecretVersionId actually used, when applicable;
- Grant/ticket/broker-operation refs actually involved;
- RestoreGeneration involved in authority redemption;
- redemption/use-count transition;
- external outcome/reconciliation state.

Secrets/tokens/raw bearer material never enter Events/logs.

## Core invariants

1. Pantheon, not model/backend, is authorization authority.
2. Authorization is `PERMIT|DENY`; approval creates a Grant then re-evaluates.
3. Hard forbid/default deny are mandatory.
4. Existing Runs never gain broader authority from later policy relaxation or credential-binding remapping.
5. Credential-bearing Run operations require exact frozen/current `credentialBindingAuthorityDigest` equality in addition to semantic authorization; unrelated registry changes do not invalidate the Run.
6. CredentialBinding authority excludes SecretVersionId/secret bytes, so material rotation behind the same logical SecretRef is possible without broadening the Run.
7. Grants/tickets are revalidated against current authority **and current RestoreGeneration** at redemption; restored old-generation authority cannot be redeemed until an operator creates new current-generation authority.
8. Grant `uses` accounting and exact operation creation are one transactional CAS boundary, and credential-bearing broker operations persist the exact non-secret binding authority that was redeemed.
9. Old-generation broker operations are reconciliation evidence after restore, not authority to repeat an external effect.
10. Agent `secret.read` is hard-denied in v1; `secret.use` is brokered use only.
11. Sandbox ambient capability is no broader than semantic authority and excludes Pantheon/peer/host privileged state.
12. Provider-native security is defense in depth, never the source of truth.
13. Failure to enforce required policy or credential-binding compatibility fails closed.
