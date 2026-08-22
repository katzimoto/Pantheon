//! Pantheon's effectful control-plane orchestration.
//!
//! # Owns
//!
//! The controllers that drive state forward: scheduling, run and attempt
//! control, recovery, authorization decisions, evaluation, configuration
//! publication, artifact and integration workflows. It also owns the *abstract*
//! ports through which external systems are reached.
//!
//! # Must not own
//!
//! Concrete implementations behind its own ports. No provider adapter, executor
//! backend, sandbox backend or database driver lives here; those are separate
//! crates that `pantheond` wires in. Nor does it own transport: HTTP routing
//! and wire formats belong to `pantheon-operator-api`.
//!
//! Configuration publication, Goal-to-Ready-Task planning, pre-Run Agent /
//! Execution Fabric routing, the single-slot scheduling path up to the T3
//! Run-intent commit, and deterministic Run context preparation from the
//! frozen source snapshot are implemented here. Attempt creation and concrete
//! backends remain later boundaries. See
//! `docs/development/implementation.md`.

pub mod configuration;
pub mod context;
pub mod operator;
pub mod planning;
pub mod routing;
pub mod scheduling;
pub mod sealing;
pub mod workspace;
