# Workspace and Git Integration

## Status

Draft design — Pantheon workspace and Git integration subsystem specification.

## Purpose

This document defines how Pantheon gives Tasks isolated mutable repository state, how Runs and Attempts operate inside that state, how code candidates are sealed into immutable Artifacts, and how accepted code is later integrated without allowing workers to mutate shared repository history directly.

The central rule is:

> **A Task owns mutable workspace state. Runs and Attempts operate inside it. Pantheon alone seals immutable candidate state, and only an authorized Integration Controller may mutate shared repository refs.**

See also:

- `docs/architecture/task-object.md`
- `docs/architecture/run-and-attempt.md`
- `docs/architecture/artifact-model.md`
- `docs/architecture/task-acceptance-and-completion.md`
- `docs/architecture/recovery-policy.md`
- `docs/architecture/scheduler-reservations-ownership-and-leases.md`
- `docs/architecture/permissions-and-capabilities.md`

## Architectural boundary

```text
Repository
    │
    ▼
Task Workspace
mutable / isolated
    │
    ├── Run 1
    │    ├── Attempt 1
    │    └── Attempt 2
    │
    └── Run 2
         └── Attempt 1
                │
                ▼
             SEAL
                │
                ▼
       code.changeset Artifact
                │
                ▼
          CandidateResult
                │
                ▼
           Acceptance
                │
                ▼
      optional IntegrationIntent
```

Task success and repository integration are intentionally separate.

## 1. Workspace ownership is Task-scoped

A normal code Task has one durable mutable Workspace that may survive multiple Runs and Attempts.

```text
Task
  └── Workspace
        ├── Run 1
        │    ├── Attempt 1
        │    └── Attempt 2
        └── Run 2
             └── Attempt 1
```

This allows execution retry and semantic retry to preserve useful work rather than discarding it automatically.

A fresh/reset workspace is an explicit Recovery decision or project policy choice, not the default consequence of creating a new Run.

Workspace reservation is therefore normally Task-scoped, as defined by the scheduler reservation model.

## 2. Workspace pins an immutable repository base

Workspace creation distinguishes the requested human/project ref from the resolved immutable Git object.

Conceptually:

```yaml
workspace:
  id: workspace_123
  task: task_456

  repository: repo://Pantheon

  requestedBase:
    ref: refs/heads/main

  resolvedBase:
    commit: 7d882a...

  isolation:
    kind: git-worktree
```

`requestedBase.ref` expresses intent. `resolvedBase.commit` is execution truth.

Once materialized, the workspace does not automatically follow movement of the requested branch/ref. If `main` advances after workspace creation, the Task workspace remains pinned to its resolved base until explicit recovery/rebase/re-materialization policy changes it.

## 3. Detached HEAD is the default linked-worktree mode

For normal Git repositories, Pantheon should create linked worktrees in detached-HEAD mode at the resolved base commit.

Conceptually:

```text
git worktree add --detach --lock <workspace-path> <baseCommit>
```

Detached mode permits normal file editing and commits without giving the worker ownership of a named shared branch.

```text
shared main ref
     │
     X     worker does not own this ref

Task worktree
HEAD detached at base
     │
     ├── optional worker commit
     └── optional worker commit
```

Worker commits are development history and checkpoints. They are not the authoritative semantic identity of a candidate.

## 4. Shared repository refs are Pantheon-controlled

Executors may receive canonical Git/file actions needed to work inside their Task workspace, but they must not obtain unrestricted authority over shared repository refs or remotes.

The effective boundary is:

```text
Task working tree
  writable when Workspace = READY

Task-specific Git state
  writable where required

shared repository refs
  controller-owned

push credentials / remote mutation
  separately authorized
```

A worker must not be able to bypass Pantheon's control plane merely by using a shell command such as `git update-ref`, directly editing shared Git metadata, or using hidden credentials.

This requires filesystem/sandbox enforcement in addition to prompt/tool-level policy.

Canonical actions should distinguish worker actions from integration authority, for example:

```text
git.read
git.commit
filesystem.write

versus

git.update-ref
git.integrate
git.push
```

The latter remain broker/controller operations unless explicitly and safely delegated.

## 5. Git worktree lock is defensive metadata, not ownership truth

Pantheon should normally lock active linked worktrees so Git housekeeping cannot prune them accidentally.

However:

> **Git worktree lock state is not Pantheon authority.**

SQLite/Pantheon durable state remains canonical for Workspace identity, ownership and lifecycle. Git worktree inventory is observed external state reconciled against it.

## 6. Isolation strategy may vary by repository

Linked worktrees are the preferred v1 strategy for normal repositories because they cheaply isolate working trees while sharing object storage.

Some repository structures may be unsafe or operationally awkward under linked worktrees. Repositories with problematic submodule semantics are an important example.

Pantheon therefore supports at least:

```text
workspace isolation = worktree
workspace isolation = copy / isolated clone
```

The Workspace Controller selects or validates a strategy capable of satisfying the Task/Agent/project requirement. It must not silently choose a known-unsafe layout.

## 7. Worker staging state is not authoritative candidate state

Workers may edit, stage, unstage and commit according to their workflow.

Pantheon must not define the candidate as:

```text
whatever happens to be staged
```

or:

```text
HEAD only
```

because valid final edits may remain unstaged or uncommitted.

Pantheon instead captures the actual working-tree state through an immutable `WorkspaceRevision`.

## 8. WorkspaceRevision

A `WorkspaceRevision` is an immutable checkpoint describing the exact Git tree state observed in a Task workspace at a specific control-plane boundary.

Conceptually:

```yaml
workspaceRevision:
  id: workspace-rev_01K...
  workspace: workspace_123

  baseCommit: 7d882a...
  tree: a8193c...

  observedHead: f902ab...
  createdAt: ...
```

The Git `tree` is the important immutable content state. `observedHead` is provenance/debug information and may or may not correspond to the final working-tree content.

## 9. WorkspaceRevision capture must not mutate the worker's index

Pantheon should use a temporary, controller-owned Git index when constructing a checkpoint.

Conceptually:

```text
create temporary GIT_INDEX_FILE
      ↓
load baseline/index state
      ↓
overlay current working-tree additions/modifications/deletions
      ↓
write-tree
      ↓
immutable Git tree OID
```

The worker's ordinary staging/index state remains untouched.

This allows Pantheon to capture actual workspace content without imposing a Git workflow on the Agent.

## 10. Ignored files are excluded from code snapshots by default

Code WorkspaceRevisions and `code.changeset` candidates normally include:

```text
tracked files
non-ignored new files
tracked deletions
```

Ignored files are excluded unless an explicit project/output policy says otherwise.

This avoids silently packaging build output, dependencies, caches, local environment state, or potential secrets into code candidates.

If an ignored/generated file is a genuine deliverable, it should normally be sealed explicitly as its own Artifact.

## 11. Checkpoint boundaries

Pantheon need not snapshot the workspace after every file edit.

Useful immutable checkpoints include:

```text
workspace materialized
Run starts
Attempt starts where useful
Attempt terminates where useful
candidate sealing
explicit recovery/reset
```

At minimum every Run records its starting WorkspaceRevision, allowing later audit and learning to establish what code state the Run inherited.

## 12. Worker commits do not define the final candidate

A worker may create multiple commits or no commits at all.

Example:

```text
commit A
commit B
uncommitted final edit C
```

Candidate sealing includes C.

Therefore:

> **Pantheon seals the actual Task workspace tree, not merely HEAD or staged state.**

Worker commits may still be useful for debugging, recovery checkpoints, human review and provenance, but they are not required for candidate validity.

## 13. Candidate sealing requires a settled Git state

Before Pantheon creates a final `code.changeset`, the workspace must not contain unresolved repository operations that make its semantic state ambiguous.

Examples include:

- unresolved merge conflicts / unmerged index entries;
- unfinished rebase/cherry-pick state where result semantics are unresolved;
- other repository states that the Workspace Controller cannot safely represent as a settled candidate.

If the workspace is unsettled, sealing fails closed and execution/recovery must resolve or explicitly abandon the state.

## 14. Scope is revalidated at seal time

Runtime permissions are not the only guard.

Pantheon also computes the effective change set between the immutable base and final result tree and validates it against the Task's scope/effects.

Conceptually:

```text
baseCommit / base tree
        ↓
     diff
        ↓
resultTree
        ↓
changed paths/effects
        ↓
Task scope validation
```

A candidate modifying paths outside the Task's allowed scope is rejected at sealing even if runtime isolation failed to prevent the mutation.

This provides deterministic defense in depth:

```text
runtime sandbox
+
sealed-result scope validation
```

## 15. `code.changeset` Artifact semantics

Pantheon represents the immutable code candidate as a `code.changeset` Artifact.

Conceptually:

```yaml
artifactKind: code.changeset

git:
  repository: repo://Pantheon
  baseCommit: 7d882a...
  resultTree: a8193c...

contents:
  - name: changes.patch
    mediaType: application/x-git-diff
    digest: sha256:...
    size: ...
```

The semantic core is:

```text
repository
baseCommit
resultTree
```

A stable patch representation is included for transport/review/materialization convenience.

Pantheon should invoke Git with controlled options/configuration so user-local diff settings do not alter canonical Artifact construction.

Worker commit history is not required in the Artifact identity and may be recorded separately in production/provenance metadata.

## 16. Candidate submission freezes mutable output

For a code Task:

```text
Task Workspace
     │
     ▼
capture final resultTree
     │
scope validation
     │
seal code.changeset Artifact
     │
     ▼
CandidateResult
     │
     ▼
Task → Evaluating
Workspace → FROZEN
Run → Finalizing
```

When the Workspace is `FROZEN`, worker mutation is prohibited while the submitted candidate is under evaluation.

If Acceptance later rejects the candidate and Recovery chooses `REQUEUE_TASK`, the same Task workspace may be made writable again for the next Run.

## 17. Acceptance verifies the sealed candidate, not a live workspace

Acceptance evaluators must operate against immutable candidate state.

Incorrect:

```text
submit candidate
→ run tests in still-mutable producer workspace
```

Preferred:

```text
code.changeset Artifact / resultTree
        ↓
verification workspace/container
        ↓
Pantheon-controlled evaluator
        ↓
Evidence
```

This ensures that Evidence binds to the exact Candidate/Artifact digest and cannot be invalidated by concurrent producer edits.

## 18. Downstream Tasks need not wait for shared-branch integration

TaskGraph data dependencies and Git integration are separate.

An accepted `code.changeset` Artifact may become a downstream Task input even if it has not yet been merged into a shared branch.

```text
Task A
  ↓ accepted changeset Artifact
Task B input binding
```

The Workspace Controller may materialize accepted upstream changes into Task B's pinned workspace according to dependency/materialization policy.

Therefore:

> **Task dependency is not equivalent to shared-branch merge dependency.**

Complex multi-changeset composition policy is deferred, but this separation is fundamental.

## 19. Acceptance and integration are separate contracts

A Task may succeed because its `code.changeset` candidate passed Acceptance.

That does not by itself authorize:

- advancing `main` or another shared ref;
- pushing to a remote;
- opening/merging a pull request;
- deploying anything.

Shared repository mutation is represented by a separate `IntegrationIntent` under explicit policy/authorization.

## 20. IntegrationIntent

Conceptually:

```yaml
integration:
  id: integration_123

  candidate: candidate://sha256/...
  changeset: artifact://sha256/...

  target:
    repository: repo://Pantheon
    ref: refs/heads/main

  expectedTarget: 83ca12...

  policyHash: sha256:...
  desired: applied
```

An IntegrationIntent may be created by Goal/project finalization policy or explicit human/controller action.

The worker does not create authoritative integration simply by claiming success.

## 21. Integration uses three-way semantics against the current target

A candidate is defined relative to its immutable base:

```text
base B
  ↓
result tree C
```

Meanwhile the target may have advanced:

```text
B → X → Y
```

Pantheon must not overwrite Y with C.

Integration evaluates a three-way merge:

```text
             candidate C
            /
base B ----
            \
             X → Y current target
```

Git plumbing such as `merge-tree --write-tree` is a strong fit because it can calculate a merge result without mutating an ordinary worktree/index.

## 22. Synthetic candidate commits are integration mechanics only

When Git merge machinery needs a commit object, Pantheon may construct an internal synthetic candidate commit:

```text
SyntheticCandidate
  tree = candidate resultTree
  parent = baseCommit
```

using Git plumbing.

This object exists for merge computation and does not redefine the `code.changeset` Artifact or require that the synthetic commit appear in final user-facing history.

## 23. Integration conflict does not invalidate accepted work

An Artifact can be correct and accepted yet conflict with a target that moved later.

Therefore:

```text
Acceptance = PASS
Integration = CONFLICT
```

is valid.

The accepted candidate remains immutable and accepted. The IntegrationIntent records conflict evidence/status.

Recovery/Planner/human policy may create conflict-resolution work or leave the accepted candidate unapplied.

The original Task is not retroactively marked failed merely because later integration conflicts.

## 24. v1 integration history is controlled and squash-style

V1 should not automatically preserve arbitrary worker-created commit history when integrating.

Given:

```text
current target = Y
clean merged tree = M
```

Pantheon creates one controlled integration commit:

```text
IntegrationCommit
  tree = M
  parent = Y
```

This preserves deterministic/shared history while allowing Agents to use whatever local commit style helps them reason.

Preserving curated worker commit history can be added later as an explicit integration policy.

## 25. Shared ref updates use compare-and-swap

Integration is computed against an exact target OID.

Before advancing a ref, Pantheon verifies that it still points to the expected old OID.

Conceptually:

```text
compute merge against Y
      ↓
create integration commit Z
      ↓
CAS update:
refs/heads/main Y → Z
      │
      ├── success → APPLIED
      └── ref moved → STALE; recompute/reconcile
```

Git `update-ref` with an expected old OID provides the necessary compare-and-swap semantics.

No integration may blindly overwrite a ref that changed since the merge calculation.

## 26. Never move a branch behind an unmanaged checked-out worktree

Pantheon must not silently advance a branch that is currently checked out in a user/unmanaged worktree where moving the ref would make that worktree's index/working tree inconsistent.

Before target mutation, the Integration Controller reconciles Git worktree inventory and determines whether the target ref is safe/managed for automatic update.

If not, v1 should stage the result under a Pantheon-controlled integration ref such as:

```text
refs/pantheon/integration/<integration-id>
```

and report a handoff-ready state, or use an explicitly configured managed/remote integration surface.

Core safety rule:

> **Pantheon never modifies the user's ordinary working checkout behind their back.**

## 27. Parallel Tasks remain isolated until composition/integration

Example:

```text
main = B

Task A workspace ← B
Task B workspace ← B
Task C workspace ← B
```

Each worker edits only its own mutable workspace and produces an immutable changeset Artifact.

Integration then serializes/refences actual target movement:

```text
integrate A → target A'
integrate B against A' → clean/conflict
integrate C against latest target → clean/conflict
```

Agents do not share mutable working directories as a synchronization mechanism.

## 28. Workspace recovery and reconciliation

SQLite/Pantheon state is authoritative; Git filesystem/worktree state is external observed state.

On startup/reconciliation:

```text
load non-released WorkspaceRecords
        ↓
query Git worktree inventory
        ↓
compare durable intent vs observed state
```

Representative outcomes:

```text
expected workspace exists
→ inspect/recover

DB workspace exists, path/Git registration missing
→ workspace.missing / recovery policy

Git/Pantheon-looking worktree exists without durable owner
→ dangling/quarantine
```

Pantheon must not blindly prune all apparently stale Git worktrees. Ownership and durable output preservation are established before destructive cleanup.

## 29. Workspace cleanup occurs after durable preservation

Before removing a Task workspace, Pantheon verifies that all state policy requires to survive has been sealed or otherwise durably recorded.

Checks may include:

- candidate Artifacts sealed;
- required diagnostic WorkspaceRevisions retained;
- Acceptance/finalization no longer needs mutable workspace state;
- Integration/recovery no longer depends on the workspace;
- retention policy permits removal.

Then:

```text
Workspace → RELEASING
      ↓
remove Git worktree/copy
      ↓
verify Git/admin state
      ↓
release Task-scoped reservation
      ↓
Workspace → RELEASED
```

Forced removal is permitted only after Pantheon has explicitly decided how to handle any unsealed local state.

## 30. Workspace lifecycle

V1 phases:

```text
REQUESTED
    ↓
PREPARING
    ↓
READY
    ↕
FROZEN
    ↓
RELEASING
    ↓
RELEASED

exception:
ERROR
```

Semantics:

- `REQUESTED` — durable workspace intent exists;
- `PREPARING` — external Git/filesystem materialization is being reconciled;
- `READY` — worker mutation is permitted according to policy;
- `FROZEN` — workspace retained but producer mutation is prohibited;
- `RELEASING` — cleanup is desired/in progress;
- `RELEASED` — workspace reservation/external state has been safely released;
- `ERROR` — controller cannot currently satisfy/reconcile the desired workspace state.

Detailed observations belong in conditions rather than phase explosion, for example:

```text
GitRegistered
BaseMaterialized
SandboxReady
CandidateSealed
Missing
Corrupt
```

## Git safety model

```text
                  SHARED REPOSITORY
                refs / remote authority
                         ▲
                         │
                 Integration Controller
                         │
             ┌───────────┴────────────┐
             │                        │
          Task A                   Task B
       detached worktree        detached worktree
             │                        │
          mutable                   mutable
             │                        │
           seal                     seal
             │                        │
             ▼                        ▼
       Changeset A              Changeset B
             │                        │
             └──────────┬─────────────┘
                        ▼
                    Acceptance
                        │
                        ▼
                IntegrationIntent
                        │
               three-way / CAS
                        │
                        ▼
                 controlled ref
```

## v1 scope

Include:

- Task-scoped Workspace resource;
- immutable base commit pinning;
- detached linked worktrees by default;
- clone/copy fallback where linked worktrees are unsuitable;
- protection of shared refs/remotes from worker mutation;
- immutable WorkspaceRevision checkpoints;
- candidate capture from actual working-tree state using controller-owned index state;
- ignored-file exclusion by default;
- settled-state and scope validation before sealing;
- `code.changeset` Artifact based on base commit + result tree;
- Workspace freezing during Acceptance;
- verification against immutable sealed state;
- separate IntegrationIntent;
- three-way integration;
- controlled squash-style integration commit;
- compare-and-swap ref updates;
- protection of unmanaged checked-out branches;
- startup/recovery reconciliation;
- safe cleanup after output preservation.

Defer:

- preserving arbitrary worker commit history during integration;
- advanced stacked-branch/patch-stack semantics;
- complex automatic composition of many independent changesets;
- distributed multi-host Git workspace ownership;
- automatic destructive cleanup of ambiguous dangling worktrees;
- specialized large-monorepo virtual filesystem integrations;
- arbitrary SCM systems other than Git.

## Key decisions

1. **A Git Workspace is Task-owned and normally survives multiple Runs and Attempts.**
2. **Workspace creation resolves a requested ref to an immutable base commit and never automatically follows later ref movement.**
3. **Linked worktrees use detached HEAD by default.**
4. **Git worktree locks are defensive; SQLite remains ownership truth.**
5. **Shared Git refs/remotes are controller-owned and protected from executor mutation at the enforcement layer.**
6. **Worker Git commits may be used for development but do not define candidate identity.**
7. **Repositories unsuitable for safe linked worktrees use clone/copy isolation.**
8. **WorkspaceRevision captures immutable Git tree state without changing the worker's actual staging index.**
9. **Ignored files are excluded from code snapshots by default and explicit deliverables are sealed separately.**
10. **Every Run records its starting WorkspaceRevision; additional checkpointing is policy-driven.**
11. **The authoritative candidate is sealed from actual worktree state, not merely HEAD or staged files.**
12. **Candidate sealing requires settled Git state and revalidates the complete changeset against Task scope.**
13. **A `code.changeset` Artifact binds repository, base commit and result tree, with a stable patch representation.**
14. **Candidate submission freezes worker mutation; Acceptance verifies immutable sealed state in an independent verification context.**
15. **Accepted changesets may feed downstream Tasks without first being merged into a shared branch.**
16. **Acceptance and Git integration are separate; Task success does not inherently mutate repository refs or remotes.**
17. **Shared repository mutation is expressed through a separately authorized IntegrationIntent.**
18. **Integration uses three-way semantics against the current target rather than overwriting it.**
19. **Integration conflict does not invalidate an already accepted candidate Artifact.**
20. **V1 integrates using a controlled squash-style commit rather than arbitrary worker commit history.**
21. **Target refs are advanced using compare-and-swap against the exact target OID used for calculation.**
22. **Pantheon does not silently advance a branch currently checked out by an unmanaged worktree.**
23. **Parallel Tasks never share mutable working trees; conflicts are handled at Artifact composition/integration boundaries.**
24. **Workspace recovery reconciles durable Pantheon state against Git/worktree reality and quarantines ambiguous dangling state before cleanup.**
25. **Workspace cleanup occurs only after immutable outputs and required diagnostics are durably preserved.**

## Core invariant

> **Agents may freely reason and edit inside their authorized Task workspace, but only Pantheon can turn those edits into an immutable candidate and only an authorized integration transaction can affect shared repository history.**
