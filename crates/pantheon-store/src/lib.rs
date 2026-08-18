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
//! that creates only the schema this mission requires, and establishes the
//! installation's stable [`RestoreGeneration`] identity.
//!
//! It does not yet implement the reusable state-dependent authoritative
//! write/CAS transaction mechanism (serialized writer, bounded read pool,
//! revision/CAS primitives) — that is a later mission's scope. See
//! `docs/development/implementation.md`.

mod error;
mod migrations;
mod policy;
mod store;

#[cfg(test)]
mod test_support;

pub use error::StoreError;
pub use store::{RestoreGeneration, Store};
