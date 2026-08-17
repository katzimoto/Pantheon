# Research: High-Value Rust-Specific Agent Skills for Pantheon

## Status

**Not canonical.** This is a research/decision result for Issue #22. It ranks
Rust-specific agent-skill candidates and records why each was accepted,
deferred, or rejected. It adds no skill, hook, dependency, crate, or runtime
behavior; it is the artifact the mission's acceptance criteria require to
preserve the decision. A skill becomes real only when a later Engineering
Mission creates it inside whatever canonical skill location Issue #21
("Establish portable agent skills and lifecycle hooks") establishes. Until
then this document is the disposition record; do not treat it as a skill
catalog.

## Mission and question

Issue #22 asked for an evidence-based recommendation on which Rust-specific
agent skills, if any, Pantheon's repository agent layer should add, with each
candidate justified by a repeated Pantheon-specific failure mode or workflow
rather than by generic Rust knowledge a model already has. It named six
candidates to evaluate at minimum: dependency changes, persistence/SQLite/
recovery review, deterministic concurrency testing, public/protocol type
evolution, error-boundary design, and async/task-cancellation/resource-
lifetime review. It asked the result to rank recommendations `add now`,
`defer until evidence`, `do not add`, and to judge "add now" against the
active MVP sequence: #16 (authoritative SQLite store), #17 (revisioned
authoritative write transactions), #18 (command/Event Journal mutation
kernel) — all under Milestone `v0.1.0 — MVP`.

## Method

**Repository evidence consulted:**

- `AGENTS.md`, `docs/development/implementation.md` — crate boundaries,
  dependency policy, toolchain/lint policy, `./scripts/verify.sh` contents.
- `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`
  and `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`
  — the two architecture entry points the issue named.
- Issues #16, #17, #18 — the concrete near-term missions the issue asked
  candidates to be judged against.
- Issue #21 — the sibling mission establishing the canonical skill mechanism
  itself; this research does not duplicate or pre-empt it.
- Current workspace state (`crates/`, `Cargo.toml`, `rust-toolchain.toml`):
  every crate is scaffolding with zero third-party dependencies, no async
  runtime, and no schema. This is load-bearing for several dispositions
  below — several "generic-sounding" Rust concerns are not yet real problems
  in this repository because the code that would exhibit them does not exist.

**External evidence consulted** (primary/official sources preferred; used to
confirm current guidance and to identify real agent/community failure modes
rather than to substitute for repository evidence):

- [The Cargo Book: Workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html) (official Cargo reference) — workspace dependency mechanics.
- [The Rust Edition Guide: Rust 2024](https://doc.rust-lang.org/edition-guide/rust-2024/index.html) (official) — 2024 edition is stable since Rust 1.85; Pantheon already pins `1.97.1`/edition 2024, so this is settled, not a gap.
- [rusqlite](https://docs.rs/rusqlite) / [SQLx](https://sqlx.dev/) / [refinery](https://github.com/rust-db/refinery) documentation, alongside SQLite's own [`lang_transaction.html`](https://www.sqlite.org/lang_transaction.html), [`wal.html`](https://www.sqlite.org/wal.html), and [`pragma.html`](https://www.sqlite.org/pragma.html) (official SQLite documentation, authoritative for the transaction-mode/WAL/PRAGMA behavior `sqlite-persistence-and-transactions.md` builds on) — SQLite driver and migration-tooling landscape for the crate #16 will need to choose.
- [tokio-rs/loom](https://github.com/tokio-rs/loom) and [awslabs/shuttle](https://github.com/awslabs/shuttle) — deterministic/permutation concurrency testing tools and what class of bug they target (hand-rolled unsafe/lock-free primitives and arbitrary interleavings), via their own docs and [Properly Testing Concurrent Data Structures](https://matklad.github.io/2024/07/05/properly-testing-concurrent-data-structures.html).
- [cargo-semver-checks](https://github.com/obi1kenobi/cargo-semver-checks) and the [Rust Project Goals page for it](https://rust-lang.github.io/rust-project-goals/2025h2/cargo-semver-checks.html) — automated public-API semver diffing. It supports comparing against a published registry version, but also a `--baseline-rev` (Git revision), `--baseline-root` (local manifest), or `--baseline-rustdoc` (arbitrary rustdoc JSON) — publication to crates.io is not required.
- [Tokio's own `select!` documentation](https://docs.rs/tokio/latest/tokio/macro.select.html) (official, "Cancellation safety" section) — the authoritative definition of cancellation safety and how to reason about it, preferred here over secondary commentary for the core claim. [Oxide RFD 400, "Dealing with cancel safety in async Rust"](https://rfd.shared.oxide.computer/rfd/0400) supplements it with concrete failure-mode case studies: `select!` loops, `tokio::sync::Mutex` held across `.await`, task aborts, and mitigations (message-passing/actors, `reserve`-then-send splits, background tasks for cancel-unsafe work).
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) — official categories (naming, documentation, predictability, flexibility, type safety, dependability, debuggability, future-proofing); used to identify what is *already* well-covered, general model knowledge.
- cargo-deny / cargo-vet / cargo-audit landscape (via the [Rust Engineering Practices supply-chain chapter](https://microsoft.github.io/RustTraining/engineering-book/ch06-dependency-management-and-supply-chain-s.html) and the [cargo-vet project](https://github.com/mozilla/cargo-vet)) — current supply-chain tooling, none of which Pantheon's `verify.sh` currently runs.
- Community/industry commentary on LLM/agent-generated Rust ([Rust and LLMs: The Compiler Does What Code Review Shouldn't Have To](https://blog.rezvov.com/rust-and-llms-the-compiler-does-what-code-review-shouldnt-have-to), [How Rust's Compiler Catches What Coding Agents Get Wrong](https://marclove.com/blog/2025-12-13-rust-feedback-loop-catches-claude-code-hallucinations-dead-code-bugs/)) — used only as evidence of a general failure *pattern* (agents reach for `unsafe`, `Box::leak`, `Arc<Mutex<T>>`, or premature `.clone()` to escape borrow-checker rejections instead of fixing ownership), not as justification by itself. Pantheon already forecloses the most dangerous instance of this pattern by denying `unsafe_code` workspace-wide and by not blanket-banning `unwrap`/`expect` (see `docs/development/implementation.md`), so this generic pattern is explicitly **not** proposed as a skill below — it is a lint-policy and code-review concern, not a progressive-disclosure procedural gap.
- General guidance on skills vs. hooks vs. documentation (progressive disclosure, "one skill, one verb", keeping always-loaded files short) — used to weigh recommended mechanism, not to justify any specific candidate.

**Evaluation criteria**, applied identically to every candidate: Pantheon-
specific value, repetition frequency, correctness/safety impact, context
cost, overlap with `AGENTS.md`/architecture/`./scripts/verify.sh`,
determinism, and whether a skill, hook, validator, documentation, or no new
mechanism fits best.

## Generic knowledge rejected outright

Per the issue's explicit instruction, the following are **not** proposed as
skills anywhere in this document, regardless of how often they occur,
because nothing in the repository evidence shows a Pantheon-specific
procedural gap in them: basic ownership/borrowing, ordinary iterator usage,
formatting (`cargo fmt` is mechanical and already enforced), basic unit-test
syntax, standard Git usage, and broad "write idiomatic Rust" advice. The
agent-generated-Rust failure pattern of reaching for `unsafe`/`Arc<Mutex<T>>`/
premature `.clone()` under borrow-checker pressure is a real, sourced failure
mode, but Pantheon already has a mechanical guardrail for its worst form
(`unsafe_code` denied workspace-wide, checked by `./scripts/verify.sh`); the
residual concern is ordinary code-review judgment, not a packageable
procedure.

## Candidate evaluation

### 1. Dependency-change procedure

```text
candidate: dependency-change-procedure
repeated Pantheon-specific workflow: yes
failure impact: medium
compiler/CI already catches it: partly
progressive-disclosure value: medium
recommended mechanism: skill
trigger examples:
  - adding rusqlite (or an alternative SQLite driver) for #16
  - adding any new crates.io dependency to any workspace crate
  - adding a new crate under crates/
non-trigger examples:
  - editing code within an already-declared dependency's usage
  - upgrading only the toolchain pin (a different, narrower procedure)
add now / defer / reject: add now
```

Pantheon's dependency policy (`docs/development/implementation.md`) is
unusually strict and mechanically partial: wildcard versions and mutable-
branch Git dependencies are prohibited, `Cargo.lock` is committed, and
`./scripts/verify.sh` runs everything `--locked`. A new crate additionally
requires a `//!` boundary statement and an entry in
`scripts/check-crate-deps.sh`'s allowlist, checked by
`check-crate-deps.sh` itself. That mechanical layer catches a **forbidden
edge** or an **unregistered new crate**, but it does not catch a floating
version pinned loosely enough to drift, an unmaintained/abandoned crate, or a
crate with an open RUSTSEC advisory — Pantheon runs no `cargo audit` or
`cargo deny` step today. Issue #16 is the first mission that will actually
need a new third-party dependency (a SQLite driver), so this candidate has
immediate, concrete near-term value rather than being speculative.

- **Inputs/authorities**: `docs/development/implementation.md` (`## When a
  dependency is justified`, `## When a new crate is justified`),
  `scripts/check-crate-deps.sh`, the committed `Cargo.lock`.
- **Workflow/output**: confirm the dependency is justified by real code that
  needs it now (not speculative); reject wildcard versions and mutable-branch
  Git sources. Before adding it, perform targeted due diligence the current
  mechanical checks do not cover: check for open RUSTSEC security advisories
  against the exact version being pinned; check upstream maintenance and
  provenance (is it actively maintained, does the source repository match
  the published crate, is the maintainer/publishing history consistent);
  check license compatibility with Pantheon's own license and its other
  dependencies; check the crate's minimum supported Rust version against
  Pantheon's pinned `1.97.1`/edition 2024; check which features are
  required versus enabled by default, and disable defaults that pull in
  unneeded surface area; check what the dependency itself transitively
  pulls in, since a small direct addition can be a large transitive one.
  This is procedural diligence the skill performs by hand — it does not
  require adding `cargo-deny`/`cargo-vet`/`cargo-audit` to CI as part of
  this mission, and does not block on tooling that does not exist yet. For
  a new crate, add its `//!` boundary statement and its allowlist entry
  together in the same change; run `cargo update -p <crate>` (not a broad
  `cargo update`) so the lockfile diff stays minimal and reviewable; finish
  by running `./scripts/verify.sh` (its `--locked` invocations are internal
  to the script, not a separate flag the caller supplies) and treating a
  failure there as the actual gate, not the skill's own judgment.
- **Must not duplicate/decide**: it does not decide whether a new crate
  boundary is architecturally justified — `docs/development/implementation.md`
  and the acceptance criteria of the mission that requested the dependency
  decide that. It does not replace `check-crate-deps.sh` or `verify.sh`; it
  is the checklist that gets an agent to a clean run of them on the first
  attempt instead of a failed one.

### 2. SQLite persistence and recovery transaction review

```text
candidate: persistence-and-recovery-transaction-review
repeated Pantheon-specific workflow: yes
failure impact: high
compiler/CI already catches it: no
progressive-disclosure value: high
recommended mechanism: skill
trigger examples:
  - implementing or modifying any pantheon-store transaction that mutates
    a revisioned/authoritative row (#16, #17, #18 and essentially every
    later pantheon-store mission)
  - adding a new table family from sqlite-persistence-and-transactions.md
  - implementing a recovery/reconciliation pass from
    global-recovery-and-crash-reconciliation.md
non-trigger examples:
  - a pure read-only query against existing schema
  - a change confined to pantheon-core's provider-neutral types with no
    persistence involvement
  - CLI output formatting in pantheon-cli
add now / defer / reject: add now
```

This is the strongest candidate in the set. The two named architecture
documents specify invariants that are unusual, precise, and easy to violate
in a way no compiler or Clippy lint can detect: `BEGIN IMMEDIATE` rather than
deferred transactions for state-dependent writes; revisioned rows updated
only via `WHERE id = ? AND revision = ?` with an exact-one-row-affected
check rather than a bare `UPDATE`; holder-safety enforced through composite
foreign keys rather than a bare cross-table pointer; cardinality invariants
("at most one nonterminal Run per Task") expressed as partial unique indexes
layered under controller logic; `RestoreGeneration` fencing so a restored
snapshot's negative observations are never treated as current proof; and the
`NOT_CONTACTED` / `CONTACT_MAY_HAVE_OCCURRED` launch-contact discipline that
makes "UNKNOWN never authorizes replacement work" a hard rule. Getting any of
these wrong produces exactly the class of bug Pantheon exists to prevent
(duplicate side effects, lost writes, or silently wrong recovery after a
crash), and #17's and #18's own acceptance criteria already require tests
that prove several of these properties directly (concurrent same-revision
mutation, injected mid-transaction failure, replay of the same command
identity). A skill earns its keep here specifically because the two source
documents combined are long (over 3,300 lines together); progressive
disclosure — a short trigger description plus a compact checklist that
points at the exact invariant and its anchor in the canonical document,
loaded only when a change actually touches store transactions — is the
textbook case this mechanism exists for.

- **Inputs/authorities**:
  `docs/architecture/persistence-and-recovery/sqlite-persistence-and-transactions.md`,
  `docs/architecture/persistence-and-recovery/global-recovery-and-crash-reconciliation.md`,
  the crate boundary for `pantheon-store` in
  `docs/development/implementation.md`.
- **Workflow/output**: before writing or reviewing a store transaction,
  identify which invariant families the change touches (revision CAS,
  holder-safety FK, cardinality partial index, idempotent command identity,
  restore-generation fencing, launch-contact certainty) and re-read only
  those sections of the two canonical documents rather than the whole file;
  confirm the transaction begins `BEGIN IMMEDIATE` when it is a
  state-dependent authoritative write; confirm mutation results are typed so
  a caller can distinguish "stale/conflict" from other failure, matching
  what #17 already requires; produce (or verify presence of) the required
  test shapes — concurrent-CAS race, injected-failure rollback, idempotent
  replay — rather than leaving them to incidental coverage.
- **Must not duplicate/decide**: it must not restate the architecture
  documents' content as a second copy of the rule; it points at the exact
  section instead. It does not decide *what* the schema or transaction
  should be for a feature not yet specified — that remains the owning
  mission's acceptance criteria and the canonical architecture. It is not a
  substitute for `./scripts/verify.sh`, which remains the actual pass/fail
  gate.

### 3. Deterministic concurrency testing (loom/shuttle-style)

```text
candidate: deterministic-concurrency-testing
repeated Pantheon-specific workflow: no
failure impact: low today
compiler/CI already catches it: n/a (nothing to catch yet)
progressive-disclosure value: low today
recommended mechanism: none for now; revisit as a skill or docs note later
trigger examples (future, not current):
  - Pantheon introduces a hand-rolled concurrent in-memory structure (for
    example an in-process scheduler queue shared across threads/tasks)
    outside SQLite's own serialization
non-trigger examples (current state):
  - the concurrent-CAS races #17 requires, which are ordinary multi-
    connection SQLite integration tests, not hand-rolled unsafe primitives
add now / defer / reject: defer
```

`loom` and `shuttle` exist to find bugs in hand-rolled concurrent/lock-free
primitives by exploring or randomizing thread interleavings — their own
documentation frames them around `unsafe`-adjacent atomics and shared-memory
data structures. Pantheon's v1 concurrency-safety model is the opposite of
that: a single serialized authoritative-writer SQLite connection plus
`BEGIN IMMEDIATE` and relational CAS *is* the concurrency-safety mechanism
(`sqlite-persistence-and-transactions.md`), and the workspace denies
`unsafe_code` entirely. The concurrent-write races #17 requires proving are
ordinary integration tests against a real SQLite connection pool, already
covered procedurally by the persistence-review skill's test-shape checklist
above. There is no evidence anywhere in the current workspace, `#16`–`#18`,
or the named architecture documents of a hand-rolled concurrent data
structure that would need loom/shuttle-style exploration. Adding this skill
now would be speculative packaging of a tool for a problem Pantheon does not
yet have — exactly what the issue asked to avoid. Revisit when the
in-process Scheduler domain (`docs/architecture/scheduling/`) is
implemented with genuine shared mutable state across concurrent
threads/tasks rather than delegating serialization to SQLite.

### 4. Public/protocol type evolution

```text
candidate: public-protocol-type-evolution
repeated Pantheon-specific workflow: not yet
failure impact: high, once real
compiler/CI already catches it: no (there is no public API surface yet for
  any tool, `cargo-semver-checks` included, to diff against)
progressive-disclosure value: medium, once real
recommended mechanism: skill, deferred
trigger examples (future, not current):
  - the first Operator API mission adds real request/response types,
    resource representations, or error envelopes to
    pantheon-operator-protocol
non-trigger examples (current state):
  - pantheon-operator-protocol today declares no types at all; there is
    nothing to evolve
add now / defer / reject: defer until pantheon-operator-protocol gains a
  first real wire type
```

`docs/development/implementation.md` states the crate's purpose precisely:
it is a "compatibility membrane" — public types must not be aliases of core
domain types or persistence rows, specifically so an internal refactor does
not become a public breaking change. That is a genuine Pantheon-specific
invariant, not generic semver advice, and `cargo-semver-checks` is real,
maintained, official-adjacent tooling that automates exactly this kind of
diff. It does not require a crates.io publication to do so — it also
supports a `--baseline-rev` (Git revision), `--baseline-root` (local
manifest), or `--baseline-rustdoc` (arbitrary rustdoc JSON) baseline, any of
which would work against an unpublished crate like
`pantheon-operator-protocol`. The reason to defer is not a tooling
limitation: the crate currently declares zero types, so there is no API
surface yet for any baseline strategy to diff against, and a skill written
now would either restate generic semver knowledge (rejected by the issue) or
invent a protocol shape ahead of the mission that actually defines it. This
is the clearest "defer until evidence" case in the set: revisit at the first
mission that adds real wire types to `pantheon-operator-protocol`, and
consider wiring `cargo-semver-checks` with a `--baseline-rev` pinned to the
last agreed revision into that mission's verification rather than only into
a skill.

### 5. Error-boundary design

```text
candidate: error-boundary-design
repeated Pantheon-specific workflow: partly (only at specific seams)
failure impact: medium-high at the store CAS boundary specifically
compiler/CI already catches it: no
progressive-disclosure value: low as a standalone skill
recommended mechanism: none as a standalone skill; fold the store-specific
  slice into persistence-and-recovery-transaction-review (candidate 2) and
  the protocol-envelope slice into public-protocol-type-evolution
  (candidate 4, deferred)
add now / defer / reject: do not add (as a standalone skill)
```

A generic "how to design Rust error types" skill would mostly restate the
Rust API Guidelines' "Dependability" category and the well-known
thiserror-for-libraries / anyhow-for-applications split — exactly the kind
of broad, framework-agnostic advice the issue and Issue #21 both explicitly
reject. There is a real Pantheon-specific slice: #17 already requires a
"typed stale/conflict result" rather than a boolean or a stringly-typed
error for revisioned-mutation failures, so callers can distinguish CAS
conflict from other failure. That requirement is naturally part of the
persistence-transaction-review workflow above, not a separate concern —
splitting it out would duplicate trigger conditions for no benefit. The
`pantheon-operator-protocol` error-envelope shape is a second real slice, but
it depends on the crate having real content, so it belongs with candidate 4,
deferred for the same reason. No standalone skill is justified.

### 6. Async task-cancellation / resource-lifetime review

```text
candidate: async-cancellation-and-resource-lifetime-review
repeated Pantheon-specific workflow: not yet
failure impact: high, once real
compiler/CI already catches it: no (rustc/Clippy cannot detect
  cancel-unsafety)
progressive-disclosure value: high, once real
recommended mechanism: skill, deferred
trigger examples (future, not current):
  - pantheond or pantheon-operator-api adopts an async runtime (tokio or
    equivalent) for the daemon/server or Agent Control surface
non-trigger examples (current state):
  - no crate in the workspace has an async dependency or an async fn today
add now / defer / reject: defer until an async runtime is introduced
```

No crate in the workspace depends on `tokio` or any async runtime today;
every crate is synchronous scaffolding. Async cancellation is nonetheless
worth flagging now as a **strong** future candidate, because the failure
mode it addresses is not generic — it is structurally the same discipline
Pantheon already imposes on itself for external operations. Tokio's own
`select!` documentation states the core definition authoritatively: a future
is cancellation safe when dropping it before completion and recreating it is
a no-op, and cancellation always happens at an `.await` point. Oxide's RFD
400 supplements that definition with concrete case studies: `tokio::select!`
silently drops in-flight work from a non-cancel-safe branch, `tokio::sync::Mutex`
held across an `.await` can leave shared state invalid under cancellation,
and task aborts cancel at an arbitrary await point. Pantheon's own launch-contact model
(`NOT_CONTACTED` / `CONTACT_MAY_HAVE_OCCURRED`, "`UNKNOWN` never authorizes
duplicate replacement work") is the same "did the operation actually
complete, and can I safely retry" question the async-cancellation-safety
literature is asking, just phrased for external backend calls instead of
`.await` points. When Pantheon adopts an async runtime for the daemon, this
skill should be written to make that connection explicit rather than
teaching generic async Rust — and it should not be written before that
runtime exists, per the issue's instruction and the current absence of any
async code to review.

## Ranking

| Skill | Decision | Trigger to revisit |
|---|---|---|
| `dependency-change-procedure` | **Add now** | N/A — needed starting at #16 |
| `persistence-and-recovery-transaction-review` | **Add now** | N/A — needed starting at #16/#17/#18 |
| `public-protocol-type-evolution` | Defer until evidence | First real wire type added to `pantheon-operator-protocol` |
| `async-cancellation-and-resource-lifetime-review` | Defer until evidence | First `tokio`/async-runtime dependency introduced anywhere in the workspace |
| `deterministic-concurrency-testing` | Defer until evidence | First hand-rolled concurrent in-memory structure outside SQLite's own serialization (e.g., in-process Scheduler state) |
| `error-boundary-design` (standalone) | **Do not add** | Absorbed into the two "add now" skills and the deferred protocol skill; a standalone version would restate the Rust API Guidelines |
| generic Rust basics (ownership, iterators, formatting, unit-test syntax, git, "idiomatic Rust") | **Do not add** | Not revisited; explicitly out of scope per Issue #22 and Issue #21 |

Both "add now" candidates map onto Milestone `v0.1.0 — MVP`'s active
sequence: #16 needs `dependency-change-procedure` for its first third-party
SQLite driver dependency, and #16/#17/#18 all need
`persistence-and-recovery-transaction-review` because each one implements or
extends authoritative SQLite transactions against the exact invariants that
skill operationalizes. The three deferred candidates are explicitly *later*
surfaces the issue itself named — async runtimes, protocol/API evolution,
and (potentially) hand-rolled concurrency — none of which the workspace has
adopted yet.

## Skill evaluation strategy

For the two "add now" skills, evaluate with real Pantheon missions rather
than synthetic trivia, consistent with the issue's instruction:

- **Representative scenarios**: #16 (first SQLite dependency and schema),
  #17 (revisioned CAS transaction), #18 (command/Event Journal atomic
  commit). Each is a real upcoming mission with its own acceptance criteria
  already written, so a with/without comparison has a concrete pass/fail
  target instead of a synthetic one.
- **Trigger cases**: a change to any file under `crates/pantheon-store/`
  touching a `BEGIN IMMEDIATE` transaction, a `Cargo.toml` diff adding a new
  dependency, a new crate under `crates/`.
- **Non-trigger cases**: a change confined to `pantheon-cli` output
  formatting; a change to `pantheon-core` pure domain types with no
  persistence involvement; a documentation-only change.
- **Comparison method**: run the same mission's acceptance criteria twice —
  once with the skill available and once without — and compare whether the
  required test shapes (concurrent-CAS race, injected-failure rollback,
  idempotent replay for the persistence skill; a clean first-attempt
  `./scripts/verify.sh` pass with no allowlist/lockfile round-trip for the
  dependency skill) were produced correctly on the first attempt.
  This reuses real mission acceptance criteria as the evaluation harness
  instead of inventing a parallel benchmark.

For the three deferred skills, the same method applies once their trigger
condition (see the Ranking table) is met; there is no useful with/without
comparison before the triggering code exists, because there is nothing yet
for the skill to have prevented.

## Relationship to Issue #21

This document specifies skills in mechanism-neutral form (name, trigger,
non-trigger, inputs/authorities, workflow/output, what it must not
duplicate) rather than as ready-to-drop files, because Issue #21 — still
open — is what defines the canonical agent-neutral skill location, its
frontmatter/triggering conventions, and its relationship to `AGENTS.md` and
vendor adapters. A future mission implementing `dependency-change-procedure`
and `persistence-and-recovery-transaction-review` should do so inside
whatever mechanism #21 establishes, using the specifications above as its
input rather than re-deriving them.

## Outcome

No skill, hook, validator, dependency, or crate was added by this mission.
This document is the decision record the mission's acceptance criteria
require. As a non-canonical research result, it does not itself authorize
anything; it recommends `dependency-change-procedure` and
`persistence-and-recovery-transaction-review` as candidates for a later
Engineering Mission to implement, scoped as specified above, and leaves the
remaining four candidates on explicit, evidence-based hold rather than
reopening the research question
each time they resurface.
