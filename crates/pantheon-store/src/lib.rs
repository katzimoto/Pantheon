//! Authoritative persistence for Pantheon's control-plane state.
//!
//! # Owns
//!
//! Everything about how durable state is stored and retrieved: connection
//! policy, schema migrations, transaction boundaries, queries and row/domain
//! mapping, invariant checks over stored state, and the database mechanics of
//! backup and restore.
//!
//! # Must not own
//!
//! Any external effect. No network, Git, process spawning, container, executor,
//! sandbox or secret-provider work happens here — that separation is what keeps
//! external effects out of transactions.
//!
//! Deciding *what* should be persisted is orchestration and belongs in
//! `pantheon-engine`; this crate decides how it is persisted durably and
//! correctly.
//!
//! # Current state
//!
//! `pantheon-store` exclusively owns opening, migrating, and closing
//! Pantheon's authoritative local SQLite database (see [`Store`]), using a
//! vetted bundled SQLite rather than an arbitrary host library. It applies
//! and verifies the v1 connection policy, runs the ordered migration set
//! that creates only the schema this behaviour requires, and establishes the
//! installation's stable [`RestoreGeneration`] identity.
//!
//! It also owns the reusable state-dependent authoritative write mechanism:
//! one serialized authoritative writer connection reached only through
//! [`Store::write`], `BEGIN IMMEDIATE` transactions, the revision/CAS
//! primitive [`Writer::update_revisioned`], and read access separated onto a
//! connection SQLite itself treats as read-only. A process may hold only one
//! [`Store`] per database file, so there is no second writer connection for
//! a caller to reach; excluding other *processes* is the daemon's
//! operating-system installation lock and is not this crate's concern.
//!
//! On top of that it owns the durable command mutation kernel: an
//! authoritative mutation executed under `(commandEpoch, commandId)`
//! identity, where the epoch is the installation RestoreGeneration, with
//! restore-aware idempotent replay, deterministic conflict on a reused
//! identity, and the Event Journal append and its journal sequence
//! allocation committing in that same transaction. See
//! [`Store::execute_command`].
//!
//! The canonical contract also names a small bounded read pool; there are no
//! concurrent readers to pool yet, so read access is one read-only
//! connection. The disaster-restore authority fence that rotates the
//! RestoreGeneration and the JournalEpoch, Event streaming, export and
//! pruning are later missions' scope and are not implemented here. See
//! `docs/development/implementation.md`.

mod artifacts;
mod command;
mod configuration;
mod error;
mod migrations;
mod operator;
mod planning;
mod policy;
mod scheduling;
mod store;
/// Assumptions about SQLite that Pantheon's correctness rests on.
#[cfg(test)]
mod substrate_tests;
mod transaction;
mod workspace;

#[cfg(test)]
mod test_support;

pub use artifacts::{ArtifactRecord, SealOutcome, SealedChangeset};
pub use command::{Command, Committed, JournalCursor};
pub use configuration::{ActiveConfiguration, ConfigurationPointer};
pub use error::StoreError;
pub use operator::{Cursor, CursorError, EventRecord, GoalDetail, GoalSnapshot};
pub use planning::{
    Cancellation, GoalRecord, GraphRecord, MaterializedPlan, PlanningDecision,
    PlanningOperationRecord, PlanningState, ProposalRecord, TaskRecord,
};
pub use scheduling::{
    DispatchCandidate, GoalSchedulingRow, RunIntent, RunIntentCommit, SchedulerStateRecord,
    SchedulingSnapshot,
};
pub use store::{RestoreGeneration, Store};
pub use transaction::{Revision, Value, Writer};
pub use workspace::{WorkspaceBinding, WorkspaceRecord};
