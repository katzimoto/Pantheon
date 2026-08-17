//! The Operator Control wire contract.
//!
//! # Owns
//!
//! The types that cross Pantheon's public operator boundary: request and
//! response bodies, public resource representations, error envelopes, and query
//! and pagination structures.
//!
//! # Must not own
//!
//! Anything else. This crate has no internal dependencies and must keep none:
//! it is a deliberate compatibility membrane, not a re-export of persistence
//! rows or of every domain type in `pantheon-core`. Sharing a type with the
//! core domain or the database would make any internal refactor a public
//! breaking change, which is exactly what the separate crate prevents.
//!
//! It also owns no transport. Serving these types over HTTP is
//! `pantheon-operator-api`; consuming them is `pantheon-cli`.
//!
//! Nothing is implemented yet, and no serialization framework is a dependency
//! yet — one arrives with the first real wire type. See
//! `docs/development/implementation.md`.
