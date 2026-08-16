# Permissions and Capabilities

## Status

Canonical Pantheon authorization specification.

## Purpose

Pantheon is the canonical authorization authority independent of execution backend/model/harness. The subsystem answers:

> May principal X perform action Y on resource Z under context C **now**?

Authorization is binary: `PERMIT` or `DENY`. Approval is not a third authorization outcome; approval creates a narrowly scoped Grant and the action is re-evaluated.

See also:

- `agent-control-channel.md`
- `configuration-and-policy-revisions.md`
- `sandbox-broker-and-isolation.md`
- `secret-store-and-credential-brokering.md`

## Foundational principles

1. **Pantheon owns authorization.** Models/backends may never broaden authority.
2. **Default deny.** No applicable permit means DENY.
3. **Hard forbid wins.** Lower scopes/Grants cannot bypass hard policy.
4. **Approval creates a scoped Grant, not broad trust.**
5. **Authorization and physical containment are separate.** Sandbox ambient authority must already be no broader than the semantic ceiling.
6. **Fail closed.** If Pantheon cannot enforce required authority, work does not start/continue.
7. **Every consequential privileged operation is auditable.**
8. **Authorization is checked at redemption/use time, not only when a request was first proposed.**

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
currentAuthzPolicyDigest
frozen Run authorization ceiling digest (for Run principals)
Task/Goal restrictions
Grant refs actually consumed
```

For a live Run, effective semantic authority is bounded by:

```text
built-in hard policy
∩ frozen Run authorization ceiling
∩ current active authorization policy
∩ current Task/Goal restrictions
+ applicable narrowly scoped Grant where the action is approvable
```

A policy relaxation never silently broadens an existing Run. A tightening may deny future operations immediately and may require Run termination if its Sandbox cannot physically enforce the new ceiling.

## Cedar policy engine

Pantheon embeds Cedar (or an equivalent deterministic PDP matching this contract) for `principal/action/resource/context` evaluation. User/project configuration compiles into validated policy; invalid policy never activates.

Normal users need not author raw Cedar. Configuration composition/revision semantics are defined by `configuration-and-policy-revisions.md`.

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
```

Persistent operator decisions become explicit user/project configuration rather than an immortal runtime Grant.

## Atomic Grant use-count redemption

A `uses: N` Grant is concurrency-sensitive authority and must be consumed transactionally.

Correct redemption for a consequential operation:

```text
BEGIN IMMEDIATE

re-read:
  current Attempt/Run/Task authority
  current ConfigurationRevision/authz policy
  Grant state + scope + expiry
  remaining uses
  exact normalized action/resource/args hash

re-evaluate authorization under current policy

CAS decrement/increment Grant use accounting exactly once
create/transition exact broker-operation authority record
append authorization/audit Event

COMMIT

external effect
```

Two concurrent requests cannot both consume the last remaining use. Retries of the same idempotent operation return/reconcile the already-created broker operation rather than consuming another use.

## Capability tickets

Pantheon may use an internal short-lived `CapabilityTicket` as an implementation reference binding:

```text
Attempt/Run/Task
exact action/resource
argsHash
Grant/decision refs
expiry/single-use state
```

But a ticket is **not durable bearer authority that bypasses current policy**.

Before redemption/external effect, the broker transaction must revalidate current authority and atomically mark/consume the ticket. A ticket issued before security tightening can therefore be denied at redemption.

No Agent may mint a ticket. Ticket IDs/bytes are never equivalent to Operator Control authority.

## Secret authorization

Keep distinct:

```text
secret.use
  authorize a Pantheon-owned broker to use credential material on behalf of the principal

secret.read
  disclose raw secret material to principal-controlled code/context
```

For Agent principals, v1 built-in hard policy denies `secret.read` non-approvably. Injecting a raw secret into an arbitrary Agent-controlled shell/environment/file is therefore not permitted as `secret.use`; it would be disclosure.

Credential-requiring semantic actions normally trigger `secret.use` as an internal sub-authorization. Secret material remains in the SecretProvider/Credential Broker boundary.

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

Canonical security classes are defined by `sandbox-broker-and-isolation.md`:

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
- Grant/ticket/broker-operation refs actually involved;
- redemption/use-count transition;
- external outcome/reconciliation state.

Secrets/tokens/raw bearer material never enter Events/logs.

## Core invariants

1. Pantheon, not model/backend, is authorization authority.
2. Authorization is `PERMIT|DENY`; approval creates a Grant then re-evaluates.
3. Hard forbid/default deny are mandatory.
4. Existing Runs never gain broader authority from later policy relaxation.
5. Grants/tickets are revalidated against current authority at redemption.
6. Grant `uses` accounting and exact operation creation are one transactional CAS boundary.
7. Agent `secret.read` is hard-denied in v1; `secret.use` is brokered use only.
8. Sandbox ambient capability is no broader than semantic authority and excludes Pantheon/peer/host privileged state.
9. Provider-native security is defense in depth, never the source of truth.
10. Failure to enforce required policy fails closed.
