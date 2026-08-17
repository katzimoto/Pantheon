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
//! Nothing is implemented yet, and no database driver is a dependency yet. See
//! `docs/development/implementation.md`.
