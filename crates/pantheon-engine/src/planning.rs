//! The Goal-to-Ready-Task control path.
//!
//! This is where the semantic path is assembled: create a Goal, plan its
//! current revision, validate the proposal against authoritative state, and
//! ask the store to materialize it.
//!
//! # The boundary, restated in code
//!
//! [`PlanningController::plan`] produces durable *evidence* — a
//! PlanningOperation and its immutable PlanningRecord. It patches nothing.
//! [`PlanningController::materialize`] validates the proposal and hands the
//! result to the store, which re-reads the Goal revision, graph revision and
//! active configuration inside its write transaction before committing.
//!
//! Validation runs against the Goal *Pantheon durably holds*, read back from
//! the immutable revision — never a copy supplied by the caller. Otherwise the
//! scope ceiling and deliverable coverage would be checked against whatever
//! the caller claimed the Goal was, and fencing the revision number would not
//! notice. The store then re-checks the Goal content digest as well, so the
//! two layers fail closed independently. A
//! proposal is therefore never authority at any point in this file: the only
//! thing that can move the graph is a store transaction that has just
//! confirmed the world still matches what the planner saw.

use pantheon_core::config::Digest;
use pantheon_core::planning::direct::{self, PlanningInput, Trigger};
use pantheon_core::planning::evaluators::RegistryResolver;
use pantheon_core::planning::goal::GoalSpec;
use pantheon_core::planning::proposal::Proposal;
use pantheon_core::planning::validate::{self, Authority, Materializable, PlanError};
use pantheon_store::{
    Command, Committed, MaterializedPlan, PlanningDecision, PlanningOperationRecord,
    ProposalRecord, Store, StoreError,
};

/// A failure along the planning control path.
#[derive(Debug)]
pub enum PlanningError {
    /// The proposal cannot become authority. Nothing was written.
    Invalid(PlanError),
    /// Durable state rejected or could not perform the mutation. On a
    /// conflict the existing Goal and graph remain authoritative.
    Store(StoreError),
    /// Configuration required for planning is missing or uninterpretable.
    Configuration(String),
}

impl std::fmt::Display for PlanningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(err) => write!(f, "invalid plan: {err}"),
            Self::Store(err) => write!(f, "planning store failure: {err}"),
            Self::Configuration(detail) => write!(f, "configuration unusable: {detail}"),
        }
    }
}

impl std::error::Error for PlanningError {}

impl From<PlanError> for PlanningError {
    fn from(err: PlanError) -> Self {
        Self::Invalid(err)
    }
}

impl From<StoreError> for PlanningError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

/// Runs the Goal-to-Ready-Task path against durable authority.
#[derive(Debug)]
pub struct PlanningController<'store> {
    store: &'store Store,
}

impl<'store> PlanningController<'store> {
    #[must_use]
    pub const fn new(store: &'store Store) -> Self {
        Self { store }
    }

    /// Creates a Goal in `Planning` with its first immutable revision.
    ///
    /// # Errors
    ///
    /// [`PlanningError::Store`] when the mutation is rejected.
    pub fn create_goal(
        &self,
        command: &Command<'_>,
        goal_id: &str,
        spec: &GoalSpec,
    ) -> Result<Committed<pantheon_store::GoalRecord>, PlanningError> {
        Ok(self.store.create_goal(command, goal_id, spec)?)
    }

    /// Plans the Goal's current revision and records the decision.
    ///
    /// Deterministic and local: no external planner is contacted, so no
    /// PlanningAttempt and no contact state exist. That follows from this
    /// planner having no external backend, not from the mode being DIRECT.
    ///
    /// # Errors
    ///
    /// [`PlanningError::Configuration`] when there is no active configuration
    /// to plan against, or [`PlanningError::Store`] when the record is
    /// rejected.
    pub fn plan(
        &self,
        command: &Command<'_>,
        operation_id: &str,
        goal_id: &str,
    ) -> Result<Committed<PlanningOperationRecord>, PlanningError> {
        let context = self.context(goal_id)?;

        let input = PlanningInput {
            goal_id,
            goal_revision: context.goal_revision,
            goal: &context.goal,
            expected_graph_revision: context.graph_revision,
            configuration_activation_sequence: context.configuration_activation_sequence,
            trigger: Trigger::Initial,
        };
        let proposal = direct::plan(&input);
        let canonical = String::from_utf8(proposal.to_value().to_canonical_bytes())
            .map_err(|_| PlanningError::Configuration("proposal is not utf-8".to_string()))?;

        Ok(self.store.record_direct_planning(
            command,
            &PlanningDecision {
                operation_id,
                goal_id,
                goal_revision: context.goal_revision,
                expected_graph_revision: context.graph_revision,
                configuration_activation_sequence: context.configuration_activation_sequence,
                planning_input_digest: input.digest(),
                trigger_kind: Trigger::Initial.as_str(),
                planner_implementation: direct::PLANNER_IMPLEMENTATION,
                planner_version: direct::PLANNER_VERSION,
            },
            &ProposalRecord {
                digest: proposal.digest(),
                canonical: &canonical,
                normalization_provenance: "direct/v1",
            },
        )?)
    }

    /// Validates a proposal and materializes it into the Goal's TaskGraph.
    ///
    /// # Errors
    ///
    /// [`PlanningError::Invalid`] when validation refuses the proposal —
    /// nothing is written, because validation is pure. [`PlanningError::Store`]
    /// when the store refuses it, in which case the existing Goal and graph
    /// remain completely authoritative.
    pub fn materialize(
        &self,
        command: &Command<'_>,
        operation_id: &str,
        task_id: &str,
        goal_id: &str,
        proposal: &Proposal,
    ) -> Result<Committed<MaterializedPlan>, PlanningError> {
        let plan = self.validate(goal_id, proposal)?;
        Ok(self
            .store
            .materialize_plan(command, operation_id, task_id, &plan)?)
    }

    /// The DIRECT proposal for the Goal's current authoritative state.
    ///
    /// Materialization takes the proposal that planning recorded, and this is
    /// how a later transaction obtains it without carrying it in process
    /// memory across a restart: DIRECT planning is deterministic, so the same
    /// authoritative state yields the same proposal. The caller is expected
    /// to check the result against the recorded `proposalDigest` rather than
    /// trust that determinism held.
    ///
    /// # Errors
    ///
    /// [`PlanningError::Configuration`] when the Goal, its graph or the
    /// active configuration cannot be read.
    pub fn proposal(&self, goal_id: &str) -> Result<Proposal, PlanningError> {
        let context = self.context(goal_id)?;
        Ok(direct::plan(&PlanningInput {
            goal_id,
            goal_revision: context.goal_revision,
            goal: &context.goal,
            expected_graph_revision: context.graph_revision,
            configuration_activation_sequence: context.configuration_activation_sequence,
            trigger: Trigger::Initial,
        }))
    }

    /// Validates a proposal against current authoritative state.
    ///
    /// Exposed so a caller can check a candidate without any possibility of
    /// mutating anything — validation reads, and the store re-reads again
    /// under its own transaction before committing.
    ///
    /// # Errors
    ///
    /// [`PlanningError`] when configuration is unusable or the proposal is
    /// refused.
    pub fn validate(
        &self,
        goal_id: &str,
        proposal: &Proposal,
    ) -> Result<Materializable, PlanningError> {
        let context = self.context(goal_id)?;
        Ok(validate::validate(
            proposal,
            &Authority {
                goal: &context.goal,
                goal_id,
                goal_revision: context.goal_revision,
                evaluators: &context.evaluators,
                evaluator_registry_digest: context.evaluator_registry_digest,
                configuration_activation_sequence: context.configuration_activation_sequence,
            },
        )?)
    }

    /// The authoritative facts planning and validation depend on.
    fn context(&self, goal_id: &str) -> Result<PlanningContext, PlanningError> {
        let goal = self.store.goal(goal_id)?.ok_or_else(|| {
            PlanningError::Configuration(format!("goal {goal_id} does not exist"))
        })?;
        let graph = self.store.task_graph(goal_id)?.ok_or_else(|| {
            PlanningError::Configuration(format!("goal {goal_id} has no task graph"))
        })?;
        let active =
            self.store.configuration_pointer()?.active.ok_or_else(|| {
                PlanningError::Configuration("no active configuration".to_string())
            })?;
        let component = self
            .store
            .active_evaluator_component_json()?
            .ok_or_else(|| {
                PlanningError::Configuration("active configuration has no evaluators".to_string())
            })?;
        let evaluators = RegistryResolver::from_canonical_json(&component)
            .map_err(|err| PlanningError::Configuration(err.to_string()))?;

        let stored = self
            .store
            .goal_revision_json(goal_id, goal.current_revision)?
            .ok_or_else(|| {
                PlanningError::Configuration(format!(
                    "goal {goal_id} revision {} is not stored",
                    goal.current_revision
                ))
            })?;
        let stored = GoalSpec::from_canonical_json(&stored)
            .map_err(|err| PlanningError::Configuration(err.to_string()))?;

        Ok(PlanningContext {
            goal: stored,
            goal_revision: goal.current_revision,
            graph_revision: graph.revision.get(),
            configuration_activation_sequence: active.activation_sequence,
            evaluator_registry_digest: active.components.evaluator_registry,
            evaluators,
        })
    }
}

struct PlanningContext {
    /// The Goal as Pantheon durably holds it, read back from the immutable
    /// revision rather than accepted from the caller.
    goal: GoalSpec,
    goal_revision: i64,
    graph_revision: i64,
    configuration_activation_sequence: i64,
    evaluator_registry_digest: Digest,
    evaluators: RegistryResolver,
}

#[cfg(test)]
mod tests;
