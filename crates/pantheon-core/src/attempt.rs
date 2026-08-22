//! Attempt-lineage vocabulary: normalized execution observations and the
//! durable launch-contact boundary.
//!
//! `docs/architecture/execution/run-and-attempt.md` is canonical for what an
//! Attempt is and why these two facts exist. This module holds only the
//! provider-neutral words for them; it performs no lifecycle, no persistence
//! and no external contact.
//!
//! # Normalized observations
//!
//! [`Observation`] is what Pantheon records about one external execution
//! lineage after normalizing a backend's report. It is deliberately
//! independent of any controller phase: the same lineage may be observed in
//! any of these states in any order, including first contact after it already
//! exited.
//!
//! # Launch-contact state
//!
//! [`LaunchContactState`] separates "Pantheon's launch path definitely never
//! crossed the external call boundary" (`NotContacted`) from "the call may
//! have happened" (`ContactMayHaveOccurred`). The transition between them is
//! committed durably before the first `ensureExecution` contact; after it,
//! credential material freezes and recovery reconciles the same lineage.

/// What Pantheon observed about one external execution lineage.
///
/// Canonical equivalents of ABSENT / STARTING / RUNNING / EXITED / UNKNOWN,
/// per `docs/architecture/execution/run-and-attempt.md` ("UNKNOWN
/// execution"). `Unknown` means Pantheon cannot establish whether the
/// execution still exists; it fences rather than authorizes replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// The backend can prove the lineage does not exist.
    Absent,
    /// The lineage exists but has not begun running.
    Starting,
    /// The lineage is running.
    Running,
    /// The lineage ran to a definitive end.
    Exited,
    /// Whether the lineage exists cannot be established.
    Unknown,
}

impl Observation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "ABSENT",
            Self::Starting => "STARTING",
            Self::Running => "RUNNING",
            Self::Exited => "EXITED",
            Self::Unknown => "UNKNOWN",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "ABSENT" => Self::Absent,
            "STARTING" => Self::Starting,
            "RUNNING" => Self::Running,
            "EXITED" => Self::Exited,
            "UNKNOWN" => Self::Unknown,
            _ => return None,
        })
    }
}

/// Whether Pantheon's launch path has possibly crossed the external-call
/// boundary for one Attempt.
///
/// The value is monotonic in one direction: `NotContacted` ->
/// `ContactMayHaveOccurred`. A crash while `NotContacted` proves no launch
/// call was made; once `ContactMayHaveOccurred` is durable, a lost
/// acknowledgement is ambiguity (`Observation::Unknown`), never proof of
/// absence, and same-Attempt credential rekeying is permanently forbidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchContactState {
    /// No launch-capable external call may have been made yet.
    NotContacted,
    /// The launch call may have been made; absence cannot be presumed.
    ContactMayHaveOccurred,
}

impl LaunchContactState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotContacted => "NOT_CONTACTED",
            Self::ContactMayHaveOccurred => "CONTACT_MAY_HAVE_OCCURRED",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "NOT_CONTACTED" => Self::NotContacted,
            "CONTACT_MAY_HAVE_OCCURRED" => Self::ContactMayHaveOccurred,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests;
