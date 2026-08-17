//! The Operator Control transport adapter.
//!
//! # Owns
//!
//! Serving the operator surface: routes, the Unix-socket HTTP server,
//! middleware, conversion between `pantheon-operator-protocol` wire types and
//! domain types, handling of sensitive requests, and API description assembly.
//!
//! # Must not own
//!
//! Business decisions. A handler validates and translates a request, then calls
//! an operation on `pantheon-engine`; logic that lives in a handler is logic
//! that no other entry point can reuse and no test can reach without HTTP.
//!
//! It does not own persistence either: it must reach durable state through the
//! engine rather than talking to `pantheon-store` directly.
//!
//! Nothing is implemented yet, and no HTTP framework is a dependency yet. See
//! `docs/development/implementation.md`.
