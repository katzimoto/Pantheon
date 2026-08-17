//! `pantheon`: the operator command-line client.
//!
//! Built from package `pantheon-cli`; the binary is `pantheon`.
//!
//! # Owns
//!
//! The operator's command-line experience: argument parsing, output rendering,
//! exit codes, and speaking the Operator Control protocol to `pantheond`.
//!
//! # Must not own
//!
//! Control-plane authority of any kind. The CLI is a client, never a second
//! path into Pantheon's state, so it must not reach the database, the engine or
//! the daemon's internals. Its only permitted internal dependency is
//! `pantheon-operator-protocol`, which leaves it structurally unable to bypass
//! the daemon rather than merely discouraged from doing so —
//! `scripts/check-crate-deps.sh` enforces that ceiling.
//!
//! No commands are implemented yet; this entry point deliberately does nothing.
//! See `docs/development/implementation.md`.

fn main() {}
