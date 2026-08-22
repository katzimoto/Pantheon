# Implementation Map

## Status

**Canonical for where Rust code belongs and which crate may depend on which.**
It has no authority over what Pantheon *is* — that is `docs/architecture/` — and
none over how an agent operates, which is `AGENTS.md`. Where this document and a
canonical architecture contract disagree about a subsystem's responsibilities,
the architecture contract wins and the disagreement is a defect to report.

## Current state

Every crate exists, compiles, documents its own boundary and is covered by the
dependency checker. Several now carry real behaviour and the rest are still
scaffolding; the paragraphs below are where that split is kept current, and
`AGENTS.md` points here rather than restating it.
`pantheon-store` implements the authoritative
SQLite store kernel described in
`docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`,
together with that contract's state-dependent authoritative write mechanism:
one serialized authoritative writer connection, `BEGIN IMMEDIATE`
transactions, the revision/CAS primitive for mutable authoritative rows, and
read access separated onto a read-only connection. On top of that it owns the
durable command mutation kernel — restore-generation-scoped command identity,
idempotent replay, deterministic conflict on a reused identity, and the Event
Journal append and its sequence allocation committing in the same
authoritative transaction.
`pantheon-core` holds the configuration vocabulary: the canonical value form
whose encoding defines configuration identity, content digests, the immutable
compiled components, and the compile/validate pipeline — all pure computation.
`pantheon-engine` owns configuration publication: compiling a candidate,
activating it through the store's command envelope, and keeping the
process-local snapshot consistent with the durable active pointer. Those are
the workspace's first internal crate edges: `pantheon-store -> pantheon-core`
and `pantheon-engine -> pantheon-core, pantheon-store`.

On top of that the three crates carry the first real semantic path: a bounded
coding Goal, deterministic DIRECT planning into a durable PlanningOperation
and immutable PlanningRecord, and validated materialization of one Goal-owned
TaskGraph revision containing exactly one immutable Task in the canonical
`Ready` phase. `pantheon-core` holds the Goal/Task/proposal vocabulary and the
validation that turns a proposal into something materializable;
`pantheon-store` owns the Goal, graph, Task and planning tables and the
authoritative mutations; `pantheon-engine` runs the control path.

On top of that the four crates carry the Task-owned coding Workspace:
`pantheon-core` holds the Workspace phase domain, the separate materialization
observation and the two base identities; `pantheon-store` owns the `workspaces`
family and its four authoritative transitions; `pantheon-engine` owns the
ordering between durable state and external effect, together with the abstract
`RepositoryMaterializer` port; and `pantheon-git` is the first concrete
implementation behind one of the engine's ports, materializing a Workspace as
an independent local Git repository under a sterile execution profile.

On top of that the four crates carry the single-slot scheduler and the durable
Run-intent boundary: `pantheon-core` holds the dispatch-mode vocabulary, the
pure Goal-fairness ordering decision, and the immutable `ExecutionBinding` /
`ContextSourceSnapshot` identities; `pantheon-store` owns the scheduler state,
Run-intent and Run/`run_status` families — including the T3 transaction that
commits Binding + source snapshot + Run + Task activation + the one global
execution slot + the fairness charge atomically, with partial unique indexes as
the database backstop for one-Run-per-Task and the single slot — plus durable
dispatch pause/resume through the command envelope; `pantheon-engine` keeps the
four scheduling stages distinct (eligibility read → deterministic ordering →
admission/routing via the side-effect-free `RoutingController` → T3) in
`SchedulingController`, and serves dispatch status/pause/resume on the Operator
surface; `pantheond` supervises the tick loop that calls it.

On top of that the same crates carry authoritative Workspace capture and
`code.changeset` sealing (#32, #76): `pantheon-core` holds the Artifact
vocabulary — lossless repository paths, canonical entry kinds, the
deterministic revision-state digest and the canonical changeset manifest — plus the Task
scope matcher; `pantheon-store` owns the `blobs`, `artifacts`,
`artifact_members` and `workspace_revisions` families, the `Ready -> Frozen`
fence, and the single-transaction seal publication that revalidates authority.
Since #76 every seal runs under a Run-backed authority: the freeze, the
already-frozen revalidation, and the publication each re-prove inside their own
transaction that the claimed Run is the Task's current responsible Run —
nonterminal, at the claimed revision, bound to exactly this Workspace at its
immutable base, under a specification whose requested output slot permits a
`code.changeset`. There is no zero-Run seal authority: a Ready Task has no
execution owner, so nothing settled exists to seal. `pantheon-engine` owns the
sealing order (freeze or revalidate under Run authority, confined capture,
CAS-first publication, trusted-base preimages, scope enforcement) and the three ports it
needs; `pantheon-git` implements root-confined no-follow capture and sterile
base reads — including the Git object names of captured bytes, computed under
the source repository's own object format, that decide changed paths without
reading unchanged same-size base blobs (#75); and `pantheon-cas` is the concrete local content-addressed store
behind the CAS port. There is still no Candidate and no acceptance: sealing
produces durable output, and what accepts it is a later boundary.

On top of that the same crates carry deterministic Run context preparation
(#30): `pantheon-core` holds the provider-neutral `ContextPlan` vocabulary —
inclusion classes, precedence strata, canonical section identity — plus the
pure rules that deterministically select one plan from a frozen source
snapshot and the frozen ContextPolicy; the Agent configuration now carries
each version's bounded static SOUL/BEHAVIOR guidance as compiled component
content, so its digests are content-addressed with everything else. T3 freezes
those guidance digests into the source snapshot and validates them against the
stored immutable agents component before committing. `pantheon-store` owns the
`context_plans` / `run_context_plans` families and the one-time T3a attachment
transaction, whose composite foreign keys prove that an attached plan was
built from exactly the source snapshot its Run froze; `pantheon-engine` owns
the `ContextPreparationController`, which reconstructs every frozen source by
immutable identity (never through an active pointer), digest-verifies it,
builds the plan deterministically, attaches it exactly once, and reconciles a
same-plan retry after restart.

On top of that the same crates carry the restart-safe Run/Attempt lineage
(#31): `pantheon-core` holds the Attempt-lineage vocabulary — normalized
execution `Observation`s (ABSENT/STARTING/RUNNING/EXITED/UNKNOWN) and the
monotonic launch-contact states. `pantheon-store` owns the `attempts`,
`attempt_status` and `agent_control_sessions` families plus the holder-safe
`run_status.current_attempt_id` pointer (composite FK; `run_status` was
rebuilt in migration 13 to attach it), and the T4/T4a/T4b transaction set:
T4 creates one Attempt with a Run-local ordinal, a globally unique LaunchKey
and an Attempt-scoped session bound to the transaction's RestoreGeneration,
under a command identity whose request hash deliberately excludes random
launch material so lost-response retries replay instead of minting lineage;
T4a rotates only a session's verifier/revision while its Attempt is durably
NOT_CONTACTED in the current generation under current Run authority; T4b is
the monotonic contact boundary, bound to the exact current credential
revision; terminalization requires durable contact; one nonterminal Attempt
per Run is both a controller check and a partial unique index.
`pantheon-engine` owns the `RunController` — preparation gates
(WorkspaceReady against the frozen snapshot's own Workspace, fake-only
SandboxReadiness behind an explicit port, ContextReady via #30's controller,
PolicyReady re-deriving the Binding's sandbox identity from the frozen
execution-profiles component), bearer memory behind an injectable entropy
port (`OsRandom` reads the kernel CSPRNG at `/dev/urandom`; tests substitute
deterministic sources), post-contact reconciliation by LaunchKey alone,
UNKNOWN fencing without replacement or slot release, deliberate
new-attempt retry under an unchanged Binding/plan through a minimum
deterministic recovery policy, and the raw-bearer secrecy rule (bearer lives
only in process-local transient state and the launch package; only SHA-256
verifiers persist). `pantheond` composes the deterministic fake executor
behind `--executor fake`: routing descriptor, keyed-idempotent launcher and
fake Sandbox gate in one object making no production claim (strict container
backend remains #34, production local executor #35). There is still no
Candidate submission (#33), no evaluation/acceptance, no cancellation
surface, no ControlLease rotation, and no startup recovery barrier (#38):
conclusion records a durable terminal target directly, and restart
reconciliation is the ordinary controller path over durable inventory.

There is still no endpoint surface beyond Goals/dispatch/events, no
SandboxInstance table family and no concrete production execution backend:
T3 creates durable responsibility, and #31's fake backend exercises lineage
semantics without changing that. The store's schema is limited to
migration bookkeeping, installation identity, the command ledger, Event
Journal and journal epoch/sequence state, the configuration
component/revision/active-pointer families, the Goal, planning, TaskGraph
and Task families, the `workspaces` family, the scheduler/Run-intent families
that path requires, the `blobs`/`artifacts`/`artifact_members`/
`workspace_revisions` families sealing publishes into, the
`context_plans`/`run_context_plans` families context preparation publishes
into, and the `attempts`/`attempt_status`/`agent_control_sessions` families
the Attempt lineage publishes into; the future conceptual production schema is
not implemented ahead of the behaviour that needs it.

`pantheon-store` depends on `rusqlite` (bundled SQLite), `pantheon-core`
depends on `sha2` for the SHA-256 digests identity requires, and
`pantheon-git` drives the `git` executable through `std` plus `rustix`: the
root-confined capture boundary needs `openat`/`statat`/`readlinkat` with
`O_NOFOLLOW`, which `std` does not expose, and re-opening by pathname would
reintroduce exactly the check-then-reopen race the capture contract forbids.
`rustix` provides those syscalls as safe wrappers, so no `unsafe` exists in
the workspace. Every other crate still has no third-party dependencies. That remains a deliberate default for
a crate until real code needs one, not an oversight.

## Crate map

```text
crates/
├── pantheon-core/                semantic vocabulary and pure domain rules
├── pantheon-store/               authoritative persistence
├── pantheon-engine/              effectful control-plane orchestration
├── pantheon-git/                 isolated Git Workspace materialization and capture
├── pantheon-cas/                 local content-addressed object store
├── pantheon-operator-protocol/   Operator Control wire contract
├── pantheon-operator-api/        Operator Control transport adapter
├── pantheond/                    daemon, and the composition root
└── pantheon-cli/                 operator CLI; binary `pantheon`
```

Two packages produce binaries: `pantheond` builds `pantheond`, and
`pantheon-cli` builds `pantheon`. The CLI's package and binary names differ on
purpose — the package name keeps its role legible inside the workspace, and the
binary name is the operator-facing command.

Each crate root carries a `//!` statement of what it owns and what it must not
own. That statement is the short version of this document and is the first thing
to read when placing new code.

## Dependency graph

Allowed direct internal edges:

```text
pantheon-core                 -> (none)
pantheon-store                -> pantheon-core
pantheon-engine               -> pantheon-core, pantheon-store
pantheon-git                  -> pantheon-core, pantheon-engine
pantheon-cas                  -> pantheon-core, pantheon-engine
pantheon-operator-protocol    -> (none)
pantheon-operator-api         -> pantheon-core, pantheon-engine,
                                 pantheon-operator-protocol
pantheond                     -> pantheon-core, pantheon-store,
                                 pantheon-engine, pantheon-git,
                                 pantheon-operator-api,
                                 pantheon-operator-protocol
pantheon-cli                  -> pantheon-operator-protocol
```

These are ceilings, not requirements. An allowed edge is declared in a manifest
when real code needs it and not before, so a crate's declared dependencies are
normally narrower than its line here, and an edge listed above may not exist yet.
`pantheon-store`'s dependency on `rusqlite` (see "Current state") is a
third-party edge this graph does not govern. The graph above is enforced by
`scripts/check-crate-deps.sh`, which reads Cargo's own resolved dependencies
rather than grepping manifests, and
covers dev and build dependencies and every target platform. Adding a forbidden
edge fails verification; so does adding a crate the allowlist does not mention.

Four edges carry most of the architectural weight:

- **`pantheon-core` depends on nothing.** The semantic vocabulary stays true
  regardless of how state is stored, which provider runs a workload, or which
  platform the daemon is on.
- **`pantheon-cli` depends only on `pantheon-operator-protocol`.** Operator
  Control is the sole control-plane authority, so the CLI must be a client and
  never a second path into Pantheon's state. Depending on the wire contract
  alone makes bypassing the daemon a compile error rather than a code-review
  question.
- **`pantheon-git` is a crate, not a module in the engine.** Two of the
  boundaries below apply at once: it is a concrete platform implementation
  behind an abstract port, which is what keeps that port abstract, and it is a
  trust boundary — everything in it spawns processes against repository state,
  which is exactly the authority the engine's own code must not be able to
  reach directly. It depends on `pantheon-engine` because that is where the
  port it implements is declared, and it deliberately cannot reach
  `pantheon-store`: a materializer that could open the database would be a
  second path into authoritative state.
- **`pantheon-operator-protocol` depends on nothing.** It is a compatibility
  membrane. If public types were aliases of core domain types or persistence
  rows, every internal refactor would become a public breaking change.

`pantheond` is the composition root and the only crate allowed to know concrete
implementations. Everything else names an abstraction and lets the daemon supply
the implementation.

## Responsibilities

| Crate | Owns | Must not own |
|---|---|---|
| `pantheon-core` | Provider-neutral semantic types; rules that are pure computation | Persistence, transport, process, filesystem or network effects; concrete provider, model or harness names; daemon startup; sandbox behaviour |
| `pantheon-store` | Connection policy, migrations, transaction boundaries, queries, row/domain mapping, invariant checks, backup/restore database mechanics | Any external effect: network, Git, process, container, executor, sandbox, secret provider |
| `pantheon-engine` | Scheduling, run/attempt control, recovery, authorization, evaluation, configuration, artifact and integration workflows; the abstract ports to outside systems | Concrete implementations behind its own ports; HTTP routing and wire formats |
| `pantheon-git` | Concrete Git materialization of a Task Workspace: repository creation, observation, discard, and the sterile non-interactive execution profile every Git process runs under; root-confined no-follow capture of a settled Workspace's logical tree; sterile reads of the trusted immutable base, and the sterile identity computations over captured bytes those reads compare against | Durable authority; orchestration; deciding what happens after a failure; Sandbox behaviour |
| `pantheon-cas` | The concrete local content-addressed store behind the engine's CAS port: hash, stage durably, publish atomically into the digest namespace, verify what lands | Durable authority; ordering between CAS durability and DB commits; any orchestration |
| `pantheon-operator-protocol` | Request/response bodies, public resource representations, error envelopes, query and pagination types | Any internal dependency; transport; persistence shapes |
| `pantheon-operator-api` | Routes, Unix-socket HTTP, middleware, wire/domain conversion, sensitive request handling, API description assembly | Business decisions; direct persistence access |
| `pantheond` | Bootstrap, observability init, constructing store and engine, choosing concrete backends, server startup, controller supervision, shutdown | Domain rules, persistence details, orchestration logic, request handling |
| `pantheon-cli` | Argument parsing, output rendering, exit codes, speaking Operator Control | Control-plane authority; any access to store, engine or daemon internals |

## When a new crate is justified

> Architecture domains are not automatically Rust crates.

`docs/architecture/` has far more domains than this workspace has crates, and
that is the intended shape. A domain is a unit of *contract ownership*; a crate
is a unit of *compilation and dependency isolation*. Mirroring one onto the other
produces crates that exist only because a document does, with all the cost of a
boundary and none of the benefit. New work goes into a module inside the crate
that owns the concern.

A new crate requires a real boundary. Any one of these is enough:

- **Dependency isolation** — it pulls in dependencies the parent crate should
  not carry, such as a provider SDK or a database driver.
- **Trust or security isolation** — it handles material or authority that should
  not be reachable from the parent's code.
- **An independently consumable protocol or API** — something outside Pantheon
  compiles against it, so its surface must version separately.
- **A provider or platform implementation** — a concrete backend behind an
  abstract port, which is also what keeps the port abstract.
- **Measured compile or test isolation** — a demonstrated build or test-cycle
  problem, with the measurement, not a prediction of one.
- **Dependency-cycle pressure** — two modules need each other, which usually
  means ownership is confused and extracting the shared concept fixes it.

"This file got large" is not a boundary; a module split solves it. "There is an
architecture document with this name" is not a boundary either.

When a crate is justified, add it under `crates/`, give it a `//!` boundary
statement, add its name to the explicit `members` list in the workspace root
`Cargo.toml`, add its line to the allowlist in `scripts/check-crate-deps.sh`,
and add it here. Verification fails until the workspace and the allowlist both
know about it, which is deliberate: a new crate must state its boundary and
join the workspace explicitly rather than quietly inherit none.

## When a dependency is justified

A dependency enters with the first code that uses it — not because the
architecture is expected to need it later. An unused dependency is a supply-chain
liability, a compile-time cost and a claim about the design that no code backs.

| Kind | Policy |
|---|---|
| Ordinary crates.io dependency | Normal semver requirement, resolved through the committed `Cargo.lock` |
| Wildcard `*` version | Prohibited |
| Git dependency on a mutable branch | Prohibited by default |
| Unavoidable Git dependency | Requires explicit mission justification and an exact immutable revision |

`Cargo.lock` is committed because this workspace builds applications, not
libraries: every developer, every CI run and every release should resolve the
same dependency versions. Verification runs with `--locked`, so a change that
would silently move a dependency fails instead.

## Toolchain

Stable Rust only, pinned in `rust-toolchain.toml`. No nightly channel, no
`#![feature(...)]`, no `cargo -Z` and no `rustc -Z`.

| | |
|---|---|
| Toolchain | `1.97.1`, stable, pinned |
| Edition | 2024 |
| Resolver | 3 |
| `Cargo.lock` | Committed |

Lint policy lives once, in `[workspace.lints]` at the workspace root, and every
package inherits it with `[lints] workspace = true`. It is deliberately small:
`unsafe_code` is denied, along with a few high-signal Clippy lints. Blanket
`pedantic`, `restriction` and `nursery` groups and a global
`unwrap_used`/`expect_used` ban are not used, because lint output that is mostly
noise gets ignored.

## Verifying a change

```sh
./scripts/verify.sh
```

That is the whole interface, it runs from anywhere in the repository, and it is
exactly what CI runs on Linux. It needs the pinned toolchain and ordinary OS
build prerequisites — no task runner, no test runner, no auditing tool, no other
language runtime.

It runs the GitHub Actions pin check, the documentation checks, the crate
dependency check, the store read-path check, `cargo fmt --check`, workspace
Clippy with warnings denied, workspace tests, and rustdoc with warnings denied,
all with `--locked`.

One test surface needs something beyond the toolchain: `pantheon-git`'s tests
and the Workspace end-to-end test drive the `git` executable against
repositories they create in a temporary directory. That is not a new
prerequisite in practice — obtaining this repository requires `git` — and the
alternative, asserting Git's behaviour in comments, is what `AGENTS.md`
forbids.

`scripts/check-store-read-paths.sh` refuses a public method on `Store` that
nothing outside test code calls. Twice a review has found a durable fact stored,
a read path for it present, and no caller — so the fence the schema implied did
not exist. A `pub fn` with a caller in another crate is not dead code, and tests
are not callers for this purpose. An unconsumed read path belongs in that
script's allowlist with the reason it still exists, which turns an oversight into
tracked debt.

It does not use `--all-features`. A union of every feature is a configuration
nobody ships: it hides real conflicts and reports failures in code paths no
build produces. When Pantheon has features worth testing, each gets an explicit
CI job naming its combination.

CI adds one job beyond this: `cargo test --workspace --locked` on macOS, which
is a different OS and a different architecture. That is the portability claim
Pantheon makes today. Windows is not supported.

### Proving the tests are load-bearing

```sh
./scripts/check-mutants.sh
```

Applies each record in `tests/mutants.txt` to a scratch copy of the workspace
and requires the named test to fail. Every record is validated structurally
first — shape, target file, anchor resolution at the requested occurrence, and
that the mutation changes bytes — without compiling anything, so a stale or
drifted anchor fails in seconds instead of surfacing partway through a full
run. Anchors match whitespace-normalized source, so a rustfmt reflow cannot
invalidate a record whose token sequence is unchanged (#82). A surviving
mutant means that test does not check the property it claims to — strengthen
the test, never the mutant.

This is the one thing deliberately outside `verify.sh`, and `AGENTS.md` records
why: it answers whether the tests would notice a regression, not whether the
tree is currently sound, and it costs a scratch rebuild per mutant. It has its
own CI workflow on pull requests touching `crates/`.

Every entry exists because a real surviving mutant exposed a test that could not
fail. Adding one is how a mission makes a claim about a fence durable rather
than a claim in a pull request body that decays.

## Where to look

| For | Read |
|---|---|
| What Pantheon is, and the contracts code must satisfy | `docs/architecture/README.md`, then the domain it routes you to |
| Where code belongs, and what may depend on what | This document |
| How to operate in this repository, and how to finish | `AGENTS.md` |
| What the system currently does | The code |
| Executable evidence of behaviour | The tests |

Authority runs in that order: a mission Issue sets the outcome, architecture sets
the contracts and invariants, this document places the code, and the code and
tests describe what is actually true today.
