# Implementation Map

## Status

**Canonical for where Rust code belongs and which crate may depend on which.**
It has no authority over what Pantheon *is* — that is `docs/architecture/` — and
none over how an agent operates, which is `AGENTS.md`. Where this document and a
canonical architecture contract disagree about a subsystem's responsibilities,
the architecture contract wins and the disagreement is a defect to report.

## Current state

Most of the workspace is still scaffolding: every crate exists, compiles,
documents its own boundary and is covered by the dependency checker.
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

There is no scheduler, no backend and no endpoint yet, and nothing creates a
Run, an Attempt, a Workspace or a Sandbox. The store's schema is limited to
migration bookkeeping, installation identity, the command ledger, Event
Journal and journal epoch/sequence state, the configuration
component/revision/active-pointer families, and the Goal, planning, TaskGraph
and Task families that path requires; the future conceptual production
schema is not implemented ahead of the behaviour that needs it. Revisioned
mutation and the command envelope are exercised against test-only fixture
tables for that reason.

`pantheon-store` depends on `rusqlite` (bundled SQLite) and `pantheon-core`
depends on `sha2` for the SHA-256 configuration digests the configuration
contract names; every other crate still has no third-party dependencies. That remains a deliberate default for
a crate until real code needs one, not an oversight.

## Crate map

```text
crates/
├── pantheon-core/                semantic vocabulary and pure domain rules
├── pantheon-store/               authoritative persistence
├── pantheon-engine/              effectful control-plane orchestration
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
pantheon-operator-protocol    -> (none)
pantheon-operator-api         -> pantheon-core, pantheon-engine,
                                 pantheon-operator-protocol
pantheond                     -> pantheon-core, pantheon-store,
                                 pantheon-engine, pantheon-operator-api,
                                 pantheon-operator-protocol
pantheon-cli                  -> pantheon-operator-protocol
```

These are ceilings, not requirements. An allowed edge is declared in a manifest
when real code needs it and not before, which is why no crate currently declares
an internal edge to another workspace crate — `pantheon-store`'s dependency on
`rusqlite` (see "Current state") is a third-party edge this graph does not
govern. The graph above is enforced by `scripts/check-crate-deps.sh`,
which reads Cargo's own resolved dependencies rather than grepping manifests, and
covers dev and build dependencies and every target platform. Adding a forbidden
edge fails verification; so does adding a crate the allowlist does not mention.

Three edges carry most of the architectural weight:

- **`pantheon-core` depends on nothing.** The semantic vocabulary stays true
  regardless of how state is stored, which provider runs a workload, or which
  platform the daemon is on.
- **`pantheon-cli` depends only on `pantheon-operator-protocol`.** Operator
  Control is the sole control-plane authority, so the CLI must be a client and
  never a second path into Pantheon's state. Depending on the wire contract
  alone makes bypassing the daemon a compile error rather than a code-review
  question.
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

Applies each single-line edit in `tests/mutants.txt` to a scratch copy of the
workspace and requires the named test to fail. A surviving mutant means that
test does not check the property it claims to — strengthen the test, never the
mutant.

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
