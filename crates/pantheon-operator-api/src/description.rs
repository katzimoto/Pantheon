//! Keeping the published API description honest.
//!
//! `schemas/operator-control-v1.openapi.json` is hand-written and subordinate
//! to the canonical architecture, exactly as Issue #26 requires: it describes
//! what this crate serves, and it is the defect when it and the architecture
//! disagree.
//!
//! Hand-written means it can rot. The test below closes the direction that
//! rots silently — a route added to [`crate::router`] with no entry in the
//! document. The other direction, a documented operation that does not
//! actually route, is proved over a real socket by the daemon's end-to-end
//! tests, because only a real request can establish that.

/// The published description, relative to this crate.
///
/// Only the drift tests read it. The path is stated here rather than inside
/// them so the description has one name in the source that owns it.
#[cfg(test)]
pub(crate) const DESCRIPTION: &str = "../../schemas/operator-control-v1.openapi.json";

#[cfg(test)]
mod tests;
