# Sandbox Broker and Isolation Classes

## Status

Accepted architecture correction — physical execution containment and ambient-authority specification.

## Purpose

Pantheon authorization determines what an Agent may do. The Sandbox Broker determines what arbitrary Agent-controlled machine code can physically reach without asking Pantheon.

The central invariant is:

> **Any ambient capability exposed inside a Sandbox must be no broader than the Run's effective authority ceiling. Brokered authority remains subject to current Pantheon authorization at use time.**

Worktree isolation and Sandbox isolation are separate concepts. A Git worktree does not provide a security boundary.

## Isolation classes

Pantheon uses three conceptual classes:

- `TRUSTED_HOST` — no meaningful security boundary between workload and Pantheon host user. Intended only for trusted helpers/adapters/operator-controlled processes.
- `CONTAINER` — normal v1 coding/research containment class with explicit filesystem, process, privilege, network and resource isolation.
- `HARDENED` — stronger VM/microVM or equivalent boundary for deliberately adversarial workloads such as CTFs, malware/unknown binaries and sandbox-escape research.

Profile names are desired policy, not proof. A SandboxBackend publishes factual mechanisms/guarantees and Pantheon verifies whether a concrete SandboxPlan can satisfy the Run.

## Mandatory control-plane isolation

Model-driven arbitrary shell/process execution requires `isolation.control-plane` by default.

A compliant Sandbox must prevent direct worker access to at least:

- the Operator Control API;
- `pantheon.db`;
- active configuration/policy storage;
- SecretProvider administrative authority;
- Credential Broker administrative authority;
- raw Artifact CAS storage;
- other Task workspaces;
- host container/hypervisor runtime sockets;
- host SSH/GPG/cloud credential agents;
- authoritative shared Git ref storage;
- unrestricted Pantheon daemon internals.

If required guarantees cannot be established, preparation fails closed. Pantheon never silently falls back to same-user host execution.

## Ambient versus brokered authority

Pantheon distinguishes:

```text
AMBIENT SANDBOX AUTHORITY
  capabilities arbitrary workload syscalls can exercise directly

BROKERED AUTHORITY
  semantic operations that require Agent Control -> authorization -> broker/controller
```

Typical ambient authority includes writable mounted paths, process spawning, allotted CPU/RAM and any direct network exposure.

Typical brokered authority includes `git.push`, repository integration, secret-backed operations, external service mutation, `container.run`, Artifact sealing, Task spawning and result submission.

Every Run freezes an `AmbientAuthorityEnvelope` derived from Task scope, Agent ceiling, frozen Run security ceiling, current configuration constraints and the SandboxProfile. Sandbox preparation must prove:

```text
physical ambient capability <= AmbientAuthorityEnvelope
```

Temporary Grants never broaden an existing Sandbox ambient envelope. Broader ambient rights require a new Run. Security tightening applies monotonically where enforceable; otherwise the Run is stopped/finalized and later work requires a new Run.

## SandboxPlan

Before execution Pantheon creates a provider-neutral immutable SandboxPlan containing at least:

- SandboxProfile ref + digest;
- selected SandboxBackend/placement;
- required and verified isolation guarantees;
- filesystem mount plan;
- network mode;
- process/privilege constraints;
- resource claims;
- immutable rootfs/image identity where applicable;
- Agent Control exposure plan.

Sandbox feasibility is controller-owned. ExecutorBackends may not self-award isolation guarantees.

`ExecutionCandidate` remains `Logical Agent + ExecutionOffer` in v1; the Sandbox Planner validates and resolves a feasible SandboxPlan before the Binding is committed.

## Network modes

V1 semantic modes:

- `NONE` — no external network; the narrowly scoped Agent Control transport may still be reachable.
- `BROKERED` — arbitrary worker egress unavailable; authorized external operations run through Pantheon-owned brokers.
- `DIRECT` — worker receives direct network only where the SandboxBackend can actually enforce the requested scope.

If the runtime can enforce only all-or-none egress, it cannot truthfully satisfy a host/port allowlist. Pantheon must select brokered networking, choose another SandboxBackend or fail closed. Prompt instructions are never network enforcement.

## Filesystem exposure

A normal Agent Sandbox exposes only explicit mounts, conceptually:

```text
/workspace      Task-owned mutable Workspace
/inputs         approved immutable/read-only Artifact materializations
/tmp            ephemeral scratch
/run/pantheon   Attempt-bound Agent Control material
```

The host home, `~/.pantheon`, operator socket, DB, raw CAS, SecretProvider authority, peer workspaces, runtime sockets and host credential agents are not mounted.

Writable mounts are ambient filesystem authority and must appear in the SandboxPlan. Raw Artifact CAS is never mounted; approved Artifact content is broker-materialized read-only.

## Process and privilege requirements

Normal Agent Sandboxes are non-privileged and must not expose generic host-escape controls such as privileged mode, host PID/network/IPC namespaces, runtime-management sockets or broad capabilities.

Where supported, require:

- non-root or mapped/non-host-root execution as appropriate;
- no privilege escalation / no-new-privileges equivalent;
- minimal Linux capabilities (drop-all then add only required capabilities);
- runtime syscall confinement (for example seccomp or platform-equivalent);
- explicit resource limits;
- no host runtime socket.

Agent requests do not choose Linux capabilities, privileged flags or host namespaces.

## Git authority boundary

Linked Git worktrees share authoritative repository state through Git's common directory. Therefore:

> **An untrusted shell may not receive writable access to the authoritative repository's shared Git common directory.**

For strongly sandboxed coding Agents that require direct Git commands, v1 prefers Task-scoped isolated Git state/isolated clones. The Task owns its working tree, index and local refs/commits without owning shared repository refs or credentialed remotes.

Linked worktrees remain useful where shared Git authority can be kept inaccessible/safe: trusted controller work, trusted-host workloads, verification/materialization and future safe projections.

Worker-local commits and branch names remain development history only. Candidate identity remains the sealed WorkspaceRevision/code.changeset, and only Integration Controller may mutate shared refs.

## Controller-side execution boundary

Control-plane isolation is bidirectional. It is not enough to prevent an Agent process from reaching Pantheon directly; Pantheon must also avoid acting as a confused deputy by interpreting Agent-controlled repository state with greater ambient authority.

Repository configuration, hooks, attributes, Git metadata, helper declarations and repository indirections inside an Agent-writable Workspace are untrusted input. Pantheon must not execute Git or any other repository-configurable tool against that state as an ambiently privileged daemon/controller process.

Where controller logic only needs logical file content, it uses controller-owned sterile repository/control state. Where it must interpret the hostile repository itself, the operation executes inside the Agent Sandbox or an equally confined controller-owned helper whose filesystem/network/credential/control-plane authority is no broader than required for that inspection.

A deny-list of known Git execution surfaces is defense in depth only. The security property is the authority boundary: repository-controlled behavior can never inherit Pantheon daemon, operator, raw-CAS, secret, credential-agent, runtime-management or unrelated authoritative-repository authority.

See `workspace-and-git-integration.md` for the canonical hostile-repository contract.

## Credential isolation

Agent Sandboxes never receive host `SSH_AUTH_SOCK`, GPG agents, cloud credential agents, platform keychain authority or hidden credentialed Git remotes. Credentialed operations use the Secret/Credential Broker.

## Agent Control exposure

Agent Control is the only Pantheon control-plane ingress exposed to an untrusted workload. The workload cannot reach Operator Control, other Agent sessions, controller administrative channels or SQLite.

The concrete transport (dedicated socket proxy, vsock, private endpoint, native backend tool bridge, etc.) is SandboxBackend/private implementation. Agent Control reachability is not general internet/network authority.

## Ownership and lifetime

Workspace is normally Task-scoped. SandboxInstance has an explicit durable holder and is normally either Run-scoped or control-operation-scoped:

```text
Task
  └─ Workspace
       ├─ Run A -> SandboxInstance A -> Attempt(s)
       └─ Run B -> SandboxInstance B -> Attempt(s)

EvaluationOperation
  └─ verification SandboxInstance
       ├─ EvaluationAttempt 1
       └─ EvaluationAttempt 2  # only after attempt 1 is terminal
```

For Run execution, the Run is the Sandbox holder. Sequential Attempts under the same Run may reuse that SandboxInstance only while its SandboxKey/identity/state are known and policy still permits reuse. A new Run normally gets a fresh SandboxInstance because Binding, configuration, ContextPlan or security envelope may have changed.

For evaluation, the **EvaluationOperation** is the control-operation holder. The EvaluationAttempt does not own the Sandbox because the verification Sandbox must be prepared and verified before an EvaluationAttempt crosses its external launch boundary. Bounded sequential EvaluationAttempts may reuse the same verification Sandbox only while the SandboxKey, immutable materialization/environment identity, verification result, resource reservation and current hard-policy constraints remain valid.

V1 permits at most one current/non-RELEASED SandboxInstance for a given Run holder and at most one current/non-RELEASED SandboxInstance for a given EvaluationOperation holder. A Sandbox in `UNKNOWN`, `PREPARING`, `READY` or `RELEASING` state cannot be bypassed by provisioning an overlapping replacement for the same holder. Replacement requires the prior Sandbox to be definitively absent/released or explicitly force-resolved under recovery policy.

EvaluationOperations use separate verification Sandboxes and never the producer Run Sandbox.

## Sandbox lifecycle and external identity

Desired lifecycle remains small:

```text
REQUESTED -> PREPARING -> READY -> RELEASING -> RELEASED
                         \-> ERROR
```

External observation is separate: `PRESENT | ABSENT | UNKNOWN`.

Each SandboxInstance has an immutable `SandboxKey` created durably before provisioning side effects and an immutable holder binding (`Run` or v1 `EvaluationOperation` control operation). SandboxBackend provides idempotent `ensureSandbox(SandboxKey, SandboxPlan)` / inspect semantics where possible.

Crash recovery inventories non-RELEASED SandboxInstances directly, resolves each durable holder, and re-inspects the same SandboxKey. `UNKNOWN` never authorizes blind duplicate/replacement provisioning while a previous sandbox may contain a live Attempt/EvaluationAttempt or still hold accounted capacity.

Sandbox destruction and Attempt/EvaluationAttempt termination are separate observations/resources; neither is inferred solely from the other.

## Resource accounting

Sandbox requirements participate in the existing effective Resource Ledger claim set before Run or control-operation commitment, for example container/VM slots, disk, memory and CPU. Agents cannot create unaccounted nested containers/VMs by receiving the host runtime socket.

Verification Sandbox claims are owned by the same `control-operation` holder as the EvaluationOperation. Sandbox lifecycle must therefore remain consistent with the corresponding ResourceReservation lifecycle; uncertain Sandbox existence keeps the relevant capacity charged.

## Immutable environment identity

Container/rootfs configuration is resolved to immutable content identity before execution (for example an image digest rather than a mutable tag). Image/rootfs acquisition is controller-owned and may use brokered credentials; workers do not receive registry/runtime administration authority.

## Sandbox verification before Attempt

`SandboxReady=True` means Pantheon verified the SandboxInstance against the SandboxPlan, including at least:

- SandboxKey identity;
- immutable holder identity (Run or EvaluationOperation);
- immutable environment identity;
- expected mount set and absence of forbidden mounts;
- expected network mode;
- privilege/capability/no-escalation configuration;
- Agent Control route scope where applicable;
- Workspace/Candidate materialization binding as applicable;
- resource limits.

Only after that verification may a Run become LaunchReady and create a normal Attempt. Likewise, a verification Sandbox must be READY and verified for its EvaluationOperation before that operation creates/launches an externally executing EvaluationAttempt.

V1 uses factual local `SandboxVerification`; cryptographic remote attestation is deferred.

## Failure handling

Detected violations such as unexpected privileged configuration, forbidden host mount, runtime socket exposure, wrong network mode, peer Workspace access or SandboxKey mismatch are `SYSTEM` failures with namespaced codes such as `sandbox.invariant-violation`.

If Pantheon cannot establish the hostile-repository execution boundary before a controller operation would interpret Agent-writable repository state, the operation fails closed as `workspace.hostile-repository-state` in the same system/security severity class. The affected Workspace/Run is fenced or quarantined rather than falling back to privileged host Git execution.

Affected Runs/control operations are stopped/fenced and the SandboxBackend may be quarantined from new work. These are not normal Agent/evaluator failures.

## Enforcement mapping

Policy compilation must know which effects are physically Sandbox-enforced and which are broker-enforced. Typical mapping:

- Task-workspace filesystem access -> Sandbox mounts;
- process spawning -> Sandbox process boundary;
- direct network -> Sandbox network policy;
- Git push/integration -> broker/controller;
- secret use -> Credential Broker;
- Artifact seal -> Agent Control + Artifact Controller;
- Task spawn -> Agent Control + Graph Controller.

The combined security model is:

```text
PHYSICAL POSSIBILITY
  = Sandbox ambient envelope

SEMANTIC PERMISSION
  = hard policy
    ∩ frozen Run ceiling
    ∩ current policy
    ∩ Task scope
    ∩ valid scoped Grant
```

For brokered actions both layers must permit. For ambient syscalls the Sandbox itself must already be no broader than the semantic ceiling.

## v1 scope

Architecturally support `TRUSTED_HOST`, `CONTAINER`, and `HARDENED` profiles. The first implementation may initially implement `TRUSTED_HOST` and a strict local container SandboxBackend; the HARDENED VM/microVM backend may follow when adversarial CTF execution is enabled.

Do not add Kubernetes, distributed sandbox fleets, service meshes, complex SDN, remote hardware attestation or multi-tenant orchestration to v1.

## Core invariants

1. Worktree isolation is not security isolation.
2. Model-driven arbitrary shell requires proven control-plane isolation by default.
3. Ambient capabilities are frozen and must be no broader than effective authority.
4. Temporary grants authorize brokered operations; they never broaden ambient Sandbox authority.
5. Untrusted workers cannot access Operator Control, Pantheon state, secrets infrastructure, peer workspaces, authoritative Git refs or host runtime sockets.
6. SandboxInstance ownership is durably explicit: a v1 Sandbox belongs to exactly one Run or one EvaluationOperation control operation, and a holder has at most one current/non-RELEASED Sandbox.
7. A new Run normally gets a fresh SandboxInstance; bounded EvaluationAttempts may reuse their EvaluationOperation's verification Sandbox only while its identity/verification/policy remain valid.
8. Sandbox provisioning is durable/idempotent/reconciled like every other external side effect, and recovery inventories SandboxInstances independently of Run traversal.
9. Sandbox invariant violations are system/security failures and fail closed.
10. Pantheon never executes a repository-configurable tool with ambient control-plane authority against Agent-writable repository state; hostile inspection is confined or uses controller-owned sterile control state.
