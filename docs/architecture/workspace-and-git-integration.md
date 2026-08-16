# Workspace and Git Integration

## Status

Canonical Pantheon Workspace/Git specification.

## Purpose

> **A Task owns mutable Workspace state. Runs/Attempts operate inside it. Pantheon seals immutable candidate state into CAS-complete Artifacts, and only Integration Controller may mutate authoritative shared repository refs.**

Worktree isolation and security Sandbox isolation are distinct.

See also:

- `artifact-model.md`
- `sandbox-broker-and-isolation.md`
- `run-and-attempt.md`
- `global-recovery-and-crash-reconciliation.md`

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

Pantheon captures WorkspaceRevision without mutating the worker's normal staging/index workflow. For Git implementations this may use a controller-owned temporary index to construct the exact resulting tree.

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
capture WorkspaceRevision
  ↓
validate allowed path/scope changes
  ↓
compare immutable base to final logical state
  ↓
copy changed-file payload bytes into Pantheon CAS
  ↓
build canonical ordered code.changeset manifest
  ↓
optional controller-owned Git object pins for efficiency
  ↓
commit Artifact + Candidate + Task/Run lifecycle transition
```

The authoritative changeset payload is CAS-complete as defined by `artifact-model.md`; it does not rely solely on Task Git ODB objects that later GC could prune.

## Path/scope validation

Before sealing, Pantheon validates that changed paths/effects fit Task/Run authority and repository rules. A worker cannot submit changes outside the Task Workspace by naming arbitrary host paths.

Repository-submodule layouts or other Git structures that make isolation ambiguous may require isolated clone/copy or may be rejected/fail closed; Pantheon does not silently choose a known-unsafe layout.

## Workspace settle/quiescence

Candidate/yield checkpointing requires a settled Workspace boundary. Pantheon prevents new semantic Agent actions and ensures the controller observes a stable filesystem state before computing the WorkspaceRevision/changeset.

This is a controller/Sandbox responsibility, not a request that the model promise it stopped writing.

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

Integration Controller materializes the accepted CAS-complete changeset and computes a controlled three-way/application result against the intended repository state.

Default v1 may use squash-style integration regardless of worker-local commit history. Worker commits remain provenance/development checkpoints, not authoritative branch history.

Conflict means the current target state cannot satisfy the recorded integration preconditions; it does not invalidate the accepted Artifact.

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

If integration/materialization temporarily relies on repository Git objects, controller may create refs under a Pantheon-owned namespace to pin those objects before committing a DB obligation that assumes continued availability.

Those Git pins are storage optimization/retention, not `code.changeset` identity. The canonical Artifact remains reconstructable from Pantheon CAS.

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

## Cleanup

Workspace cleanup occurs only after required Candidate/Artifact/Integration data is durably preserved. Releasing the Workspace never deletes the only copy of accepted Task output.

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

Workspace policy and Sandbox policy jointly enforce these boundaries.

## Core invariants

1. Task owns mutable Workspace; Runs/Attempts use it.
2. Workspace base commit is immutable unless explicit Recovery/rematerialization changes the Workspace.
3. Untrusted shell does not get writable authoritative shared Git common-dir/ref authority.
4. Isolated Task Git state is preferred for sandboxed coding Agents in v1.
5. Worker commits/staging are not Candidate identity.
6. WorkspaceRevision captures exact logical state without mutating worker staging semantics.
7. `code.changeset` is CAS-complete and remains valid even if Task Git objects are later GC'd.
8. Acceptance uses immutable sealed content, not live producer Workspace.
9. Task success does not imply merge/push.
10. IntegrationIntent precedes external shared-ref mutation and Git target update is CAS-protected/reconciled.
11. Sandbox and Workspace isolation are distinct and both are required where applicable.
