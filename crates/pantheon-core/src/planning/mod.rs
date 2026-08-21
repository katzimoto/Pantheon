//! Goal, Task and DIRECT planning semantics.
//!
//! This module holds the provider-neutral vocabulary for the first real
//! semantic path Pantheon can execute: a bounded coding Goal becomes one
//! immutable Task in a Goal-owned TaskGraph.
//!
//! # The boundary this module exists to hold
//!
//! `docs/architecture/goals-and-planning/planner-and-task-decomposition.md`
//! states the rule in one sentence:
//!
//! > Planner proposes structure; Pantheon validates/materializes it. Planner
//! > never assigns concrete execution backends/models, grants permissions,
//! > creates Runs or directly mutates lifecycle state.
//!
//! and the persistence contract makes the consequence explicit:
//!
//! > A PlanningRecord is not lifecycle authority and is never proof that its
//! > proposal was materialized. Graph Controller separately rechecks
//! > GoalRevision/GraphRevision/current policy before GraphPatch commit.
//!
//! So everything here is pure computation over values. A [`Proposal`] is
//! evidence, not authority: it cannot reach the database, and the type that
//! *can* be materialized — [`validate::Materializable`] — is constructible
//! only by validation against authoritative state that the caller supplies.

pub mod direct;
pub mod evaluators;
pub mod goal;
pub mod proposal;
pub mod scope;
pub mod task;
pub mod validate;

pub use goal::{GoalPhase, GoalSpec};
pub use proposal::Proposal;
pub use task::{TaskDecodeError, TaskPhase, TaskSpec};
