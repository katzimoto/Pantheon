//! Typed failures at the configuration boundary.

use std::fmt;

use crate::config::parse::ParseError;

/// Why a configuration candidate cannot become authority.
///
/// Issue #23 requires rejection to be "a typed configuration-level
/// failure/diagnostic" rather than an opaque error, because the operator has
/// to be able to tell a malformed file from an internally inconsistent one —
/// and because the caller must be able to prove the active revision was left
/// alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The source text is not well-formed.
    Malformed(ParseError),
    /// A required field is absent at `path`.
    MissingField { path: String },
    /// A field at `path` has the wrong shape or an unacceptable value.
    InvalidValue { path: String, detail: String },
    /// Two declarations claim the same identity.
    DuplicateIdentity { kind: &'static str, id: String },
    /// A declaration references something no component declares. This is the
    /// "syntactically valid but internally inconsistent" case: every field
    /// parses, and the configuration still cannot mean anything.
    UnknownReference {
        from: String,
        kind: &'static str,
        id: String,
    },
    /// Two individually valid declarations cannot hold at once.
    IncompatibleCombination { detail: String },
    /// The candidate attempts to weaken Pantheon's compiled-in hard policy.
    ///
    /// Distinct from an ordinary invalid value: this is configuration trying
    /// to grant authority that no configuration scope may grant, and the
    /// contract requires it to fail even when the rest of the candidate is
    /// well-formed.
    HardPolicyViolation { detail: String },
}

impl ConfigError {
    /// A short stable label for diagnostics and tests.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "malformed",
            Self::MissingField { .. } => "missing-field",
            Self::InvalidValue { .. } => "invalid-value",
            Self::DuplicateIdentity { .. } => "duplicate-identity",
            Self::UnknownReference { .. } => "unknown-reference",
            Self::IncompatibleCombination { .. } => "incompatible-combination",
            Self::HardPolicyViolation { .. } => "hard-policy-violation",
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(err) => write!(f, "{err}"),
            Self::MissingField { path } => write!(f, "missing required field {path}"),
            Self::InvalidValue { path, detail } => write!(f, "invalid value at {path}: {detail}"),
            Self::DuplicateIdentity { kind, id } => {
                write!(f, "duplicate {kind} {id:?}")
            }
            Self::UnknownReference { from, kind, id } => write!(
                f,
                "{from} references {kind} {id:?}, which this configuration does not declare"
            ),
            Self::IncompatibleCombination { detail } => {
                write!(f, "incompatible configuration: {detail}")
            }
            Self::HardPolicyViolation { detail } => {
                write!(
                    f,
                    "configuration may not weaken built-in hard policy: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<ParseError> for ConfigError {
    fn from(err: ParseError) -> Self {
        Self::Malformed(err)
    }
}
