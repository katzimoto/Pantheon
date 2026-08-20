//! Structured errors, per RFC 9457 `application/problem+json`.
//!
//! `docs/architecture/operations/public-daemon-api-and-cli.md` ("Errors")
//! enumerates the stable problem codes and states that "clients must not parse
//! human detail text". [`Problem::code`] is therefore the only member a client
//! branches on; [`Problem::detail`] is for a human reading a terminal.

use serde::{Deserialize, Serialize};

/// The stable problem codes this surface can return.
///
/// A closed enum rather than a string: the codes are a compatibility promise,
/// and a typo in one would otherwise be indistinguishable from a code a client
/// has not learned yet. Codes the wider contract lists but this surface cannot
/// reach are deliberately absent — a code no path returns is not a promise
/// worth making.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProblemCode {
    NotFound,
    Validation,
    PreconditionRequired,
    StaleRevision,
    StaleCommandEpoch,
    Conflict,
    CursorGone,
    TemporarilyUnavailable,
    Internal,
}

impl ProblemCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not-found",
            Self::Validation => "validation",
            Self::PreconditionRequired => "precondition-required",
            Self::StaleRevision => "stale-revision",
            Self::StaleCommandEpoch => "stale-command-epoch",
            Self::Conflict => "conflict",
            Self::CursorGone => "cursor-gone",
            Self::TemporarilyUnavailable => "temporarily-unavailable",
            Self::Internal => "internal",
        }
    }

    /// The HTTP status this code is served with.
    ///
    /// Three of these are fixed by the canonical contract: 428 for
    /// `precondition-required`, 412 for `stale-revision`, 410 for
    /// `cursor-gone`. The rest the contract leaves undefined, and this
    /// function is where #26 decides them. The two worth stating a reason for:
    ///
    /// - `stale-command-epoch` is **409**, not 412. 412 belongs to a failed
    ///   `If-Match` precondition the client supplied; a stale command epoch is
    ///   a fail-closed authority conflict that occurs whether or not any
    ///   precondition was sent.
    /// - `temporarily-unavailable` is **503**, matching the readiness
    ///   endpoint, so a client sees the same status for "not ready yet"
    ///   however it discovers it.
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::NotFound => 404,
            Self::Validation => 400,
            Self::CursorGone => 410,
            Self::StaleRevision => 412,
            Self::PreconditionRequired => 428,
            Self::StaleCommandEpoch | Self::Conflict => 409,
            Self::Internal => 500,
            Self::TemporarilyUnavailable => 503,
        }
    }

    /// The stable `type` URI for this code.
    ///
    /// A URN, not an `https://` URL. RFC 9457 wants a stable identifier, not a
    /// dereferenceable one, and minting an `https://` type would claim a
    /// domain Pantheon does not own and cannot promise to serve.
    #[must_use]
    pub fn type_uri(self) -> String {
        format!("urn:pantheon:problem:{}", self.as_str())
    }

    /// The short, stable human title. Not the detail text.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::NotFound => "Resource not found",
            Self::Validation => "Request is not valid",
            Self::PreconditionRequired => "Required precondition is missing",
            Self::StaleRevision => "Revision precondition failed",
            Self::StaleCommandEpoch => "Command epoch is not current",
            Self::Conflict => "Current state forbids this request",
            Self::CursorGone => "Requested journal position is unreachable",
            Self::TemporarilyUnavailable => "Control plane is not ready",
            Self::Internal => "Internal error",
        }
    }
}

/// A structured error body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    /// The stable type URI. Present because RFC 9457 defines it; clients
    /// branch on [`Problem::code`], which is shorter and cannot drift from
    /// the enum.
    #[serde(rename = "type")]
    pub type_uri: String,
    pub title: String,
    pub status: u16,
    /// Human-readable. Clients must not parse this.
    pub detail: String,
    /// The Pantheon problem code. This is the machine-readable member.
    pub code: ProblemCode,
}

impl Problem {
    #[must_use]
    pub fn new(code: ProblemCode, detail: impl Into<String>) -> Self {
        Self {
            type_uri: code.type_uri(),
            title: code.title().to_string(),
            status: code.status(),
            detail: detail.into(),
            code,
        }
    }
}
