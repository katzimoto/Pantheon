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
//! # What is deliberately not here
//!
//! No database row, no SQL, no digest type, no `pantheon-core` domain enum.
//! Every field below is a primitive or another type in this crate, so the
//! internal representation of a Goal can change without changing the wire.
//!
//! Cursors are opaque strings on the wire even though they have structure
//! durably. `docs/architecture/operations/public-daemon-api-and-cli.md` calls
//! them opaque, and one spelling — `<journalEpoch>:<sequence>` — is used for
//! the `snapshotCursor` field, the `after` query parameter and the SSE `id`
//! line alike, so a client can round-trip a cursor without parsing it.

pub mod dispatch;
pub mod events;
pub mod goals;
pub mod problem;
pub mod system;

/// The path prefix every versioned resource is served under.
pub const API_PREFIX: &str = "/api/v1";

/// The media type of a structured error body, per RFC 9457.
pub const PROBLEM_MEDIA_TYPE: &str = "application/problem+json";

/// The media type of an Event stream.
pub const EVENT_STREAM_MEDIA_TYPE: &str = "text/event-stream";

/// The header carrying an operator command's single-use identity.
///
/// A header rather than a body field so that it is identical across every
/// mutation regardless of body shape, and so a `GET` can never carry one by
/// accident.
pub const COMMAND_ID_HEADER: &str = "pantheon-command-id";

/// The header carrying the `commandEpoch` the client believes is current.
///
/// The client reads it from `GET /api/v1/system` before issuing a mutation. A
/// request carrying an old epoch fails closed, so this is not a formality: it
/// is what stops a client that slept through a disaster restore from having
/// its retry treated as new.
pub const COMMAND_EPOCH_HEADER: &str = "pantheon-command-epoch";
