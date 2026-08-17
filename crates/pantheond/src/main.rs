//! `pantheond`: the Pantheon daemon and the workspace's composition root.
//!
//! # Owns
//!
//! Wiring, and only wiring. Process bootstrap, observability initialization,
//! constructing the store and engine, choosing the concrete backends that
//! satisfy the engine's abstract ports, starting the operator server,
//! supervising controllers, and orderly shutdown.
//!
//! Being the single place that names concrete implementations is what lets
//! every other crate stay independent of them.
//!
//! # Must not own
//!
//! Domain rules, persistence details, orchestration logic or request handling.
//! Anything worth testing without starting a process belongs in the crate that
//! owns it, not here.
//!
//! Bootstrap is not implemented yet; this entry point deliberately does
//! nothing. See `docs/development/implementation.md`.

fn main() {}
