# Workspace and Git Integration

## Status

Canonical Pantheon Workspace/Git specification.

## Purpose

> **A Task owns mutable Workspace state. Runs/Attempts operate inside it. Pantheon seals immutable candidate state into CAS-complete Artifacts, and only Integration Controller may mutate authoritative shared repository refs.**

Worktree isolation and security Sandbox isolation are distinct.

See also:

- `docs/architecture/artifacts-and-workspaces/artifact-model.md`
- `docs/architecture/security/sandbox-broker-and-isolation.md`
- `docs/architecture/execution/run-and-attempt.md`
- `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`

## Workspace ownership

A normal code Task owns one durable mutable Workspace that may survive multiple Runs/Attempts:

```text
Task
  └─ Workspace
       ├─ Run A / Attempt(s)
       └─ Run B / Attempt(s)
```

A fresh/reset Workspace is explicit Recovery/project policy, not an automatic consequence of a new Run.

Workspace capacity is therefore normally Task-scoped and reused across Runs rather than re-reserved each time.

## Immutable base

Workspace records both requested human/project ref and resolved immutable base commit:

```yaml
workspace:
  repository: repo://Pantheon
  requestedBase: refs/heads/main
  resolvedBase: <immutable commit OID>
```

The Workspace does not silently follow movement of `main` or another requested ref.

## Workspace strategies

Pantheon supports at least:

```text
isolated-clone
linked-worktree
copy/other repository-safe materialization
```

The Workspace/Sandbox planners choose only strategies capable of satisfying the actual security/integration requirements.

### Isolated Git state is preferred for untrusted shell

For strongly sandboxed model-driven coding Agents that need direct Git commands, v1 prefers a Task-scoped isolated clone/repository state.

The Task owns writable:

```text
working tree
index
local HEAD/local refs
Task-local Git metadata
```

but does not own:

```text
authoritative repository shared refs
credentialed remote mutation
host repository common-dir
```

The worker may commit locally for development/checkpointing. Those commits/branch names are not semantic Candidate identity.

### Linked worktrees

Linked worktrees remain useful for trusted controller operations, trusted-host workloads, verification/materialization and future safe projections.

However Git linked worktrees share administrative/common-dir state. An untrusted Sandbox **must never receive writable access to the authoritative shared Git common directory/ref store**. Therefore a plain linked worktree bind-mounted with its shared common-dir writable is not an acceptable security boundary for an arbitrary shell Agent.

Worktree lock is defensive Git housekeeping metadata, not Pantheon ownership truth; SQLite remains authority.

## Shared refs and remote mutation

Worker actions may include local Git read/commit behavior inside Task-owned state. Shared repository mutations remain broker/controller operations:

```text
git.read / git.commit       may be worker-local

git.integrate
git.update-ref shared target
git.push                    controller/broker authority
```

An Agent Sandbox receives neither ambient push credentials nor host SSH/GPG credential agents.

## Remote configuration

A Task-local Git repository has no credential-bearing remote authority. It may retain sanitized remote URLs/history metadata where policy permits, but credentialed push/fetch requiring secrets is brokered.

## Hostile repository state and controller-side Git execution

Repository state writable by an Agent is **untrusted input**, even when the files are owned by the same operating-system user that runs Pantheon and even when Git would consider the repository safe by ownership policy.

This includes, without limitation:

```text
working-tree .gitattributes / .gitmodules and repository control files
.git/config and recursively included configuration
.git/hooks/** and configured hooks paths
.git/info/**
index, local refs and local object database
.git gitfiles / gitdir indirection
commondir indirection
objects/info/alternates and alternate-object indirection
repository-configured filters, diff/textconv, merge drivers, fsmonitor,
credential/transport helpers, submodule commands and equivalent extension points
```

The list is threat documentation, not a complete security blacklist. A future Git extension point remains untrusted by default.

Pantheon therefore has a separate controller-side execution boundary:

> **Pantheon never executes Git or another repository-configurable tool with ambient daemon/control-plane authority against Agent-writable repository control state.**

Two implementation patterns satisfy this rule:

1. **Sterile controller projection.** Operations that only need logical repository content, such as authoritative WorkspaceRevision/candidate capture, use controller-owned Git control state anchored to the durable immutable base. The controller may read the quiesced Workspace's permitted file bytes as untrusted data, but it does not use the Agent's `.git` as `GIT_DIR`, common directory, configuration source, hooks directory or object-store authority. A temporary index, object database or scratch repository used for capture is controller-created and sterile.
2. **Confined hostile-repository inspection.** An operation that genuinely must interpret Agent-owned Git metadata executes inside the Agent Sandbox or an equivalently confined controller-owned helper whose ambient authority is no greater than the hostile Workspace requires. That helper has no access to Operator Control, `pantheon.db`, active configuration, raw CAS, SecretProvider/Credential Broker administration, host credential agents, runtime-management sockets, peer workspaces or unrelated authoritative repositories.

Controller-owned Git execution also uses a sterile, non-interactive execution profile as defense in depth: system/global configuration is replaced by controller-owned empty configuration; repository-local configuration is controller-owned for sterile projections; hooks and external helpers are disabled unless a specific trusted controller operation explicitly requires one; interactive pager/editor/askpass/credential prompting is disabled; and remote/submodule/transport execution is unavailable unless separately brokered and authorized.

These configuration controls are **defense in depth, not the security boundary**. Pantheon must remain safe if Git gains another repository-configurable execution mechanism.

Pantheon does not follow an Agent-controlled `gitdir`, `commondir`, alternates path, config include, remote/helper declaration or similar indirection into a more privileged host location and then treat the target as trusted. Controller-trusted repository paths and roots come from durable Pantheon/controller state. An operation that cannot establish the required projection or confinement fails closed with `workspace.hostile-repository-state` and fences/quarantines the affected Workspace rather than executing with greater authority.

## Hostile filesystem state and privileged capture

Agent-writable filesystem structure is untrusted input independently of Git metadata. A path that appears to be beneath the Workspace must never cause a privileged Pantheon process to dereference Agent-controlled filesystem indirection using ambient daemon/control-plane authority.

The capture invariant is:

> **Privileged Workspace/Artifact capture reaches source objects only through a trusted, root-confined, no-follow traversal rooted in durable Pantheon Workspace state. Agent-created symlinks are captured as symlinks; they are never followed to obtain payload bytes.**

Conceptually:

```text
durable Workspace capture root
        ↓
pin/open trusted root object
        ↓
enumerate descendants relative to trusted directory objects
        ↓
inspect each entry itself without following filesystem indirection
        ↓
regular/executable file → read that opened object
symlink                 → read/capture link target bytes as data
directory               → descend through a confined child-directory object
unsupported special     → fail closed
        ↓
CAS + canonical manifest
```

This is a semantic portability requirement, not a Linux syscall contract. A platform implementation may use `openat2`-style beneath/no-symlink resolution, directory-handle-relative APIs, or another mechanism only if it provides the same no-escape/no-follow guarantee and binds validation to the exact object being read.

Pantheon does **not** use a `realpath`/prefix check followed by a later path reopen as the security boundary. Validation and payload reads must refer to the same confined object identity so an Agent cannot win a path-replacement or symlink TOCTOU race between check and use.

V1 source-object treatment is:

```text
regular file      allowed
executable file   allowed
symlink           allowed as repository data; never dereferenced
directory         traversed only through root-confined handles/objects
FIFO              rejected
Unix socket       rejected
block device      rejected
character device  rejected
undeclared mount / filesystem escape
                  rejected
```

A symlink target may itself contain relative or absolute path text. Those bytes are repository content, not capture instructions. Whether the target is semantically acceptable is repository/project policy; privileged capture still never follows it.

Declared gitlinks/submodules or other repository structures are handled only by an explicit supported materialization policy. Pantheon never silently traverses them into another filesystem/repository tree because an Agent-created path points there.

If safe object resolution cannot be established, capture fails closed with `workspace.hostile-filesystem-state`. The Workspace remains fenced/quarantined as appropriate; Pantheon does not fall back to an ambient-privilege pathname read.

## Workspace phases

Workspace lifecycle can remain small, for example:

```text
REQUESTED
MATERIALIZING
READY
FROZEN
RELEASING
RELEASED
ERROR
```

`READY` allows the current Task execution owner to mutate within the Workspace. `FROZEN` is used while authoritative candidate/yield/finalization state must not change without a new controller transition.

Task Waiting after blocking yield normally keeps the Task-scoped Workspace reservation but freezes mutation authority until a later Run becomes responsible.

## WorkspaceRevision

A `WorkspaceRevision` is an immutable controller checkpoint of exact logical repository state at a control-plane boundary.

Conceptually:

```yaml
workspaceRevision:
  id: workspace-rev_...
  workspace: workspace_123
  baseCommit: ...
  tree: <Git tree OID when applicable>
  observedHead: ...
  createdAt: ...
```

`tree`/observed Git IDs provide immutable repository-state metadata; they are not the sole portable Artifact payload.

Pantheon captures WorkspaceRevision without mutating the worker's normal staging/index workflow. For Git implementations this may use a controller-owned **sterile repository projection and temporary index** anchored to the durable immutable base to construct the exact resulting tree. Agent-writable `.git` state is never the privileged controller's Git control plane for this capture. Any Workspace bytes supplied to that sterile projection are obtained through the root-confined/no-follow filesystem capture boundary above.

Ignored/ephemeral build output is excluded from code candidate snapshots by default unless the Task explicitly declares it as an output.

## Candidate sealing

Workers may edit/stage/commit however they prefer. Candidate identity is never simply:

```text
HEAD
whatever is staged
last worker commit
```

On `task.submit_result`, Pantheon captures the actual permitted Workspace state and seals a `code.changeset` Artifact.

Canonical flow:

```text
quiesce/fence Workspace mutation for submission transaction
  ↓
pin trusted Workspace capture root
  ↓
capture WorkspaceRevision through sterile projection/confined inspection
  ↓
validate allowed path/scope + filesystem-object types
  ↓
compare immutable trusted base to final logical state
  ↓
for each changed path, derive authoritative before state
from the resolved immutable base through controller-owned/sterile repository state
  ↓
for each changed path, capture authoritative after state
from the settled Workspace through root-confined no-follow reads
  ↓
copy required before/after regular/executable bytes and symlink-target bytes into Pantheon CAS
  ↓
build canonical ordered code.changeset before/after manifest
  ↓
verify changed-path preimage/result completeness and digests
  ↓
optional controller-owned Git object pins for efficiency
  ↓
commit Artifact + Candidate + Task/Run lifecycle transition
```

The authoritative changeset payload is CAS-complete as defined by `docs/architecture/artifacts-and-workspaces/artifact-model.md`; it does not rely solely on Task Git ODB objects that later GC could prune.

The worker cannot supply or override authoritative preimage state. An Agent-local patch, commit, index entry, claimed old blob or Agent-writable Git object database is untrusted input. The `before` side is derived from the durable resolved base using controller-owned/trusted Git state or another equally authoritative immutable source.

The `after` side remains subject to the existing Workspace capture boundary: source objects are read from the exact settled Workspace through root-confined/no-follow traversal, with symlinks represented as link-target bytes and unsupported special objects rejected.

Only changed-path preimages are copied into CAS. Pantheon does not duplicate the entire base repository merely to make a small changeset self-contained.

## Path/scope validation

Before sealing, Pantheon validates that changed paths/effects fit Task/Run authority and repository rules. A worker cannot submit changes outside the Task Workspace by naming arbitrary host paths, and an in-Workspace pathname does not authorize following a symlink, mount, device, socket or other filesystem indirection into a different authority domain.

Path/scope validation and filesystem-object confinement are distinct checks: lexical/canonical repository path validity does not substitute for no-follow object resolution.

Repository-submodule layouts or other Git structures that make isolation ambiguous may require isolated clone/copy or may be rejected/fail closed; Pantheon does not silently choose a known-unsafe layout.

## Workspace settle/quiescence

Candidate/yield checkpointing requires a settled Workspace boundary. Pantheon prevents new semantic Agent actions and ensures the controller observes a stable filesystem state before computing the WorkspaceRevision/changeset.

This is a controller/Sandbox responsibility, not a request that the model promise it stopped writing. Quiescence reduces mutation races but is not the filesystem security boundary; root-confined no-follow object capture remains mandatory even for a Workspace believed to be settled.

## Acceptance independence

Acceptance evaluates immutable Candidate/Artifact materialization in an independent verification Sandbox. It never trusts the producer's still-mutable Workspace as authoritative evidence.

## Blocking yield

A blocking child yield retains Task Workspace state but releases Run-scoped execution/Sandbox resources.

Before committing `Run -> Yielded` / `Task -> Waiting`, Pantheon captures a WorkspaceRevision and freezes mutation authority. Later continuation creates a new Run/ContextPlan against that Task Workspace checkpoint.

## IntegrationIntent

Task success and repository integration are separate.

After accepted `code.changeset`, an authorized operation may create immutable/durable IntegrationIntent containing at least:

```text
candidate/changeset digest
repository
target ref
expected target OID
intended result/result commit identity where known
current integration policy/config digest
state/revision
```

IntegrationIntent is persisted before mutating shared Git refs.

## Controlled integration

Integration Controller materializes the accepted CAS-complete changeset and computes a controlled changed-path three-way/application result against the intended repository state.

Default v1 may use squash-style integration regardless of worker-local commit history. Worker commits remain provenance/development checkpoints, not authoritative branch history.

The canonical changeset itself supplies each changed path's immutable trusted `before` state and proposed `after` state. Integration observes the authorized current target state independently.

Per changed path, useful deterministic cases are:

```text
current == before
→ apply after cleanly

current == after
→ already applied / no-op for that path

current differs from before and after
→ use the configured permitted three-way/conflict strategy
```

For mergeable text/content, the semantic three-way inputs are:

```text
base    = Artifact.before
current = current authorized target state
other   = Artifact.after
```

For binary content, incompatible mode/type transitions, structural conflicts, unsupported gitlink/submodule semantics or any other case where current policy cannot establish a deterministic safe result, Integration Controller records a conflict rather than guessing.

This changed-path self-containment does not promise bit-for-bit reproduction of every Git history-aware merge algorithm. When the trusted integration repository still contains `baseCommit`/tree objects, Integration Controller may use controller-owned Git machinery for richer repository-level verification/merge behavior. Those Git objects are optional acceleration/context; absence of Task-local base objects cannot erase the accepted Artifact's before/after changed-path semantics.

Conflict means the current target state cannot satisfy the recorded integration preconditions or cannot be reconciled safely under current integration policy; it does not invalidate the accepted Artifact.

Integration Git execution uses controller-owned/trusted repository state or another explicitly confined projection. Accepted Artifact bytes are data; they do not make repository-controlled configuration from a producer Workspace trustworthy.

Materializing a symlink `before`/`after` state uses the recorded link-target bytes as repository content. It never dereferences that target on the materializing host to obtain replacement content.

## Git ref CAS

Shared ref update uses compare-and-swap semantics equivalent to:

```text
update target only if current OID == expected_target_oid
```

No silent target drift.

Correct ordering:

```text
commit IntegrationIntent in SQLite
  ↓
external Git operation/ref CAS
  ↓
inspect/reconcile actual ref
  ↓
persist integration result/Event
```

Crash between Git mutation and DB result is recovered by comparing expected target, intended result and actual current ref.

## Git object retention

If integration/materialization temporarily benefits from repository Git objects, controller may create refs under a Pantheon-owned namespace to pin those objects.

Pins are created only through controller-owned/trusted Git control state. Pantheon does not obtain host authority merely by pointing a privileged Git process at an Agent-writable object database or repository configuration.

Those Git pins are storage optimization/retention, not `code.changeset` identity or a correctness prerequisite. A retained changeset's changed-path preimages and result payloads remain reconstructable from Pantheon CAS even after the optional Git pins or Task-local objects disappear.

A controller must not commit an Integration obligation whose correctness depends on optional Git objects unless it either pins them under trusted controller state for the lifetime of that narrower operation or has a fail-closed fallback to the canonical CAS before/after semantics. Optional richer Git behavior may disappear; Artifact correctness may not.

## Startup reconciliation

Workspace Controller reconciles SQLite inventory with actual Workspace/Git materialization. Host paths/PIDs/Git worktree lists are observations, not authority.

Examples:

```text
DB Workspace active + materialization missing
→ RecoveryFinding / reconcile/rematerialize if safe

orphan Task-local worktree/clone without durable owner
→ quarantine, not silently adopt

shared target ref differs from pending IntegrationIntent
→ integration reconciliation
```

Reconciliation of Agent-writable Git state follows the hostile-repository rule above. It never promotes paths/configuration discovered by following Agent-controlled Git metadata into controller authority. Filesystem inspection/capture during reconciliation follows the same trusted-root/no-follow rule; an observed symlink or special file is data/finding, never permission for privileged dereference.

## Cleanup

Workspace cleanup occurs only after required Candidate/Artifact/Integration data is durably preserved. Releasing the Workspace never deletes the only copy of accepted Task output or the only changed-path preimage required by a retained accepted changeset.

Task terminalization, explicit reset/recovery and retention policy decide Workspace release. Run terminalization alone normally does not release a Task Workspace if Task may continue.

## Security exclusions

Untrusted Sandbox never receives ambient access to:

```text
Pantheon operator socket/DB/config
raw CAS
peer Task workspaces
authoritative repository common-dir/ref store
host credential agents
host container runtime socket
```

The inverse boundary is equally mandatory: Agent-writable Workspace/repository state never causes Pantheon to execute repository-configurable behavior **or dereference filesystem indirection** with ambient control-plane authority.

Workspace policy and Sandbox policy jointly enforce these boundaries.

## Core invariants

1. Task owns mutable Workspace; Runs/Attempts use it.
2. Workspace base commit is immutable unless explicit Recovery/rematerialization changes the Workspace.
3. Untrusted shell does not get writable authoritative shared Git common-dir/ref authority.
4. Isolated Task Git state is preferred for sandboxed coding Agents in v1.
5. Worker commits/staging are not Candidate identity.
6. WorkspaceRevision captures exact logical state without mutating worker staging semantics.
7. `code.changeset` is CAS-complete and remains valid even if Task Git objects are later GC'd.
8. Every changed path's `before` state is derived from the trusted immutable base, never from Agent-claimed Git/preimage state; every `after` state is captured from the settled Workspace through the confined capture boundary.
9. Retained changesets preserve required changed-path preimage and result payload in CAS, so integration correctness does not depend on Task-local base Git objects surviving.
10. Acceptance uses immutable sealed content, not live producer Workspace.
11. Task success does not imply merge/push.
12. IntegrationIntent precedes external shared-ref mutation and Git target update is CAS-protected/reconciled.
13. Integration may use optional trusted Git objects for richer behavior, but unsupported/ambiguous CAS-only reconciliation fails as an Integration conflict rather than guessing.
14. Sandbox and Workspace isolation are distinct and both are required where applicable.
15. Agent-writable repository state is untrusted input; Pantheon never executes a repository-configurable tool against it with ambient daemon/control-plane authority.
16. Agent-writable filesystem structure is untrusted input; privileged capture is rooted in durable Workspace state, root-confined and no-follow, and never dereferences Agent-created indirection with ambient authority.
17. Symlinks may be repository content but are captured as link-target bytes, never as dereferenced target content; unsupported special filesystem objects fail closed in v1.
18. Sterile Git configuration is defense in depth; confinement or controller-owned Git control state is the security boundary.
