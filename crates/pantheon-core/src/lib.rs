//! Pantheon's provider-neutral semantic vocabulary and pure domain rules.
//!
//! # Owns
//!
//! The types that name what Pantheon reasons about, and the rules that follow
//! from those types by pure computation.
//!
//! # Must not own
//!
//! Persistence, transport, process, filesystem or network effects. Daemon
//! startup. Sandbox runtime behaviour. Any concrete provider, model or harness
//! name — a backend called "Claude Code" is a fact about one deployment, and
//! this crate must stay true across all of them.
//!
//! A rule that has to read the database or call a backend to reach its answer
//! is not a pure domain rule; it belongs in `pantheon-engine`.
//!
//! Nothing is implemented yet. The crate map, the allowed dependency graph and
//! the rule for when a new crate is justified are in
//! `docs/development/implementation.md`.
