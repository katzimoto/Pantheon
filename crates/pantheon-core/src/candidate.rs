//! The immutable [`CandidateResult`](self::CandidateResult): a Run's proposed
//! answer to its Task, content-addressed over canonical semantic content.
//!
//! `docs/architecture/artifacts-and-workspaces/artifact-model.md`
//! ("CandidateResult") and
//! `docs/architecture/evaluation-and-acceptance/task-acceptance-and-completion.md`
//! ("CandidateResult") are canonical for what a Candidate means. This module
//! holds only the provider-neutral vocabulary and the pure rules identity
//! depends on; whether a particular submission is authorized is lifecycle and
//! persistence work that lives elsewhere.
//!
//! Three properties carry the weight here:
//!
//! - **Canonical identity.** A Candidate's digest is taken over this module's
//!   canonical encoding — object keys sorted by the canonical value form,
//!   outputs ordered by raw slot-name bytes — so identity never depends on
//!   insertion order or map iteration order.
//! - **Structural honesty.** Duplicate output slots are refused at
//!   construction, so a caller cannot build an ambiguous mapping and hash it.
//! - **Nothing but semantics.** There is no field a credential, path, host
//!   location, verdict or self-assessment could hide in. Provenance (who
//!   produced an Artifact) and evidence (whether it is good) are separate
//!   graphs by contract and are deliberately absent here.

use std::fmt;

use crate::config::Digest;
use crate::config::canonical::Value;
use crate::planning::TaskSpec;
use crate::planning::task::TaskOutput;

/// The longest output-slot name a Candidate output may carry.
///
/// Mirrors the durable bound the store enforces, so an impossible mapping is
/// refused before anything is hashed or persisted.
pub const MAX_OUTPUT_SLOT_BYTES: usize = 128;

/// One normalized output-slot-to-Artifact mapping inside a Candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateOutput {
    /// The TaskSpec output slot this Artifact fills.
    pub slot: String,
    /// The immutable content identity of the produced Artifact.
    ///
    /// Pantheon computed this digest when the Artifact was sealed; a submitter
    /// can only name existing content, never mint identity.
    pub artifact: Digest,
}

impl CandidateOutput {
    fn to_value(&self) -> Value {
        Value::object([
            ("artifact", Value::string(self.artifact.to_string())),
            ("slot", Value::string(&self.slot)),
        ])
    }
}

/// Why a CandidateResult could not be built or validated against its Task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateError {
    /// The same output slot was supplied twice. One Candidate maps each slot
    /// to exactly one Artifact.
    DuplicateOutputSlot { slot: String },
    /// An output slot name is outside the bounded domain.
    InvalidOutputSlot { slot: String, reason: &'static str },
    /// The submission names a slot the immutable specification does declare,
    /// but the submitted Artifact's kind differs from what that slot permits.
    OutputKindMismatch {
        slot: String,
        permitted: String,
        submitted: String,
    },
    /// The submission names an output slot the immutable specification does
    /// not declare.
    UndeclaredOutputSlot { slot: String },
    /// The specification requires an output slot the submission omits.
    MissingRequiredOutput { slot: String },
}

impl fmt::Display for CandidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateOutputSlot { slot } => {
                write!(f, "output slot {slot:?} was supplied more than once")
            }
            Self::InvalidOutputSlot { slot, reason } => {
                write!(f, "output slot {slot:?} is invalid: {reason}")
            }
            Self::OutputKindMismatch {
                slot,
                permitted,
                submitted,
            } => write!(
                f,
                "output slot {slot:?} permits {permitted}, but the submitted \
                 Artifact kind is {submitted}"
            ),
            Self::UndeclaredOutputSlot { slot } => {
                write!(
                    f,
                    "output slot {slot:?} is not declared by the specification"
                )
            }
            Self::MissingRequiredOutput { slot } => write!(
                f,
                "required output slot {slot:?} is missing from the submission"
            ),
        }
    }
}

/// One Run's immutable, content-addressed proposed answer to one Task.
///
/// Identity is the SHA-256 of [`to_canonical_json`](Self::to_canonical_json):
/// the exact Task id, the exact Run id, and the normalized output mapping —
/// nothing else. Two Candidates with equal content are the same Candidate
/// regardless of the order a caller listed the outputs in; changing any bound
/// member changes the identity.
pub struct CandidateResult {
    task_id: String,
    run_id: String,
    /// Sorted by raw slot-name bytes, duplicates already refused.
    outputs: Vec<CandidateOutput>,
}

impl CandidateResult {
    /// Builds a Candidate from an unordered set of output mappings.
    ///
    /// The mappings are normalized (sorted by slot-name bytes) before they can
    /// reach identity, so the same logical submission always produces the same
    /// Candidate regardless of iteration order.
    ///
    /// # Errors
    ///
    /// [`CandidateError::DuplicateOutputSlot`] when one slot appears twice;
    /// [`CandidateError::InvalidOutputSlot`] when a slot name is empty or
    /// longer than [`MAX_OUTPUT_SLOT_BYTES`] bytes.
    pub fn new<I: Into<String>>(
        task_id: impl Into<String>,
        run_id: impl Into<String>,
        outputs: impl IntoIterator<Item = (I, Digest)>,
    ) -> Result<Self, CandidateError> {
        let mut normalized: Vec<CandidateOutput> = outputs
            .into_iter()
            .map(|(slot, artifact)| CandidateOutput {
                slot: slot.into(),
                artifact,
            })
            .collect();
        for output in &normalized {
            if output.slot.is_empty() {
                return Err(CandidateError::InvalidOutputSlot {
                    slot: output.slot.clone(),
                    reason: "the slot name is empty",
                });
            }
            if output.slot.len() > MAX_OUTPUT_SLOT_BYTES {
                return Err(CandidateError::InvalidOutputSlot {
                    slot: output.slot.clone(),
                    reason: "the slot name exceeds the bounded length",
                });
            }
        }
        normalized.sort_by(|a, b| a.slot.as_bytes().cmp(b.slot.as_bytes()));
        for pair in normalized.windows(2) {
            if pair[0].slot == pair[1].slot {
                return Err(CandidateError::DuplicateOutputSlot {
                    slot: pair[0].slot.clone(),
                });
            }
        }
        Ok(Self {
            task_id: task_id.into(),
            run_id: run_id.into(),
            outputs: normalized,
        })
    }

    /// The Task this Candidate proposes to satisfy.
    #[must_use]
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    /// The Run that produced this Candidate. At most one Candidate may ever
    /// exist per Run; enforcing that is authoritative-submission work.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The normalized output mapping, sorted by slot-name bytes.
    #[must_use]
    pub fn outputs(&self) -> &[CandidateOutput] {
        &self.outputs
    }

    /// The canonical value this Candidate's identity is taken over.
    ///
    /// Exactly three keys exist: `task`, `run` and `outputs`. Every entry of
    /// `outputs` carries exactly `slot` and `artifact`. There is nowhere else
    /// for content — or credential material — to hide.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            (
                "outputs",
                Value::array(self.outputs.iter().map(|output| output.to_value())),
            ),
            ("run", Value::string(&self.run_id)),
            ("task", Value::string(&self.task_id)),
        ])
    }

    /// The canonical JSON document the Candidate digest is taken over.
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        self.to_value().to_string()
    }

    /// The immutable content identity: `candidate://sha256/<digest>` names
    /// this exact proposal.
    #[must_use]
    pub fn digest(&self) -> Digest {
        Digest::of(self.to_canonical_json().as_bytes())
    }

    /// Validates the output mapping against the immutable TaskSpec the
    /// authenticated execution lineage runs under.
    ///
    /// Pure computation over two immutable values: every *required* declared
    /// slot must be present and no undeclared slot may be named. An optional
    /// slot may legitimately be omitted.
    ///
    /// Kind agreement needs the referenced Artifacts' stored kinds, which a
    /// submission does not carry; that check runs where both the specification
    /// and the Artifact rows are readable ([`kind_permitted`](kind_permitted),
    /// called by the authoritative submission transaction).
    ///
    /// # Errors
    ///
    /// [`CandidateError::MissingRequiredOutput`] for the first required slot
    /// the submission omits, then [`CandidateError::UndeclaredOutputSlot`]
    /// for the first submitted slot the specification does not declare.
    pub fn validate_against_spec(&self, spec: &TaskSpec) -> Result<(), CandidateError> {
        let declared = |name: &str| spec.outputs.iter().find(|output| output.name == name);
        // Declared-but-missing first, so the report describes the Task's own
        // contract rather than the submission's extras.
        for required in spec.outputs.iter().filter(|output| output.required) {
            if !self.outputs.iter().any(|o| o.slot == required.name) {
                return Err(CandidateError::MissingRequiredOutput {
                    slot: required.name.clone(),
                });
            }
        }
        for output in &self.outputs {
            if declared(&output.slot).is_none() {
                return Err(CandidateError::UndeclaredOutputSlot {
                    slot: output.slot.clone(),
                });
            }
        }
        Ok(())
    }
}

/// Whether one declared output slot permits an Artifact of `artifact_kind`.
///
/// Pure ceiling check over the immutable specification. The authoritative
/// caller supplies the kind it read from the stored, digest-keyed Artifact
/// row — never a kind the submitter claimed.
///
/// # Errors
///
/// [`CandidateError::OutputKindMismatch`] when the kinds disagree.
pub fn kind_permitted(slot: &TaskOutput, artifact_kind: &str) -> Result<(), CandidateError> {
    if slot.kind == artifact_kind {
        Ok(())
    } else {
        Err(CandidateError::OutputKindMismatch {
            slot: slot.name.clone(),
            permitted: slot.kind.clone(),
            submitted: artifact_kind.to_string(),
        })
    }
}

impl fmt::Debug for CandidateResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CandidateResult")
            .field("task_id", &self.task_id)
            .field("run_id", &self.run_id)
            .field("outputs", &self.outputs)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::task::{AcceptanceContract, TaskOutput, TaskScope};

    fn digest(seed: u8) -> Digest {
        Digest::of(&[seed; 32])
    }

    fn spec(outputs: &[(&str, &str, bool)]) -> TaskSpec {
        TaskSpec {
            task_type: "code.change".to_string(),
            objective: "prove the vocabulary".to_string(),
            inputs: Vec::new(),
            outputs: outputs
                .iter()
                .map(|(name, kind, required)| TaskOutput {
                    name: (*name).to_string(),
                    kind: (*kind).to_string(),
                    required: *required,
                })
                .collect(),
            competencies: Vec::new(),
            scope: TaskScope {
                resources: Vec::new(),
                permitted_effects: Vec::new(),
                forbidden_effects: Vec::new(),
            },
            acceptance: AcceptanceContract {
                criteria: Vec::new(),
                evaluator_registry_digest: digest(9),
                configuration_activation_sequence: 1,
            },
            goal_id: "goal-1".to_string(),
            goal_revision: 1,
        }
    }

    #[test]
    fn canonical_identity_is_independent_of_insertion_order() {
        let forward = CandidateResult::new(
            "task-1",
            "run-1",
            [("changeset", digest(1)), ("report", digest(2))],
        )
        .expect("valid mapping");
        let reversed = CandidateResult::new(
            "task-1",
            "run-1",
            [("report", digest(2)), ("changeset", digest(1))],
        )
        .expect("valid mapping");

        assert_eq!(forward.digest(), reversed.digest());
        assert_eq!(forward.to_canonical_json(), reversed.to_canonical_json());
        assert_eq!(forward.outputs()[0].slot, "changeset");
        assert_eq!(forward.outputs()[1].slot, "report");
    }

    #[test]
    fn identity_binds_every_semantic_member() {
        let base =
            CandidateResult::new("task-1", "run-1", [("changeset", digest(1))]).expect("valid");
        let other_task =
            CandidateResult::new("task-2", "run-1", [("changeset", digest(1))]).expect("valid");
        let other_run =
            CandidateResult::new("task-1", "run-2", [("changeset", digest(1))]).expect("valid");
        let other_slot =
            CandidateResult::new("task-1", "run-1", [("diagnosis", digest(1))]).expect("valid");
        let other_artifact =
            CandidateResult::new("task-1", "run-1", [("changeset", digest(3))]).expect("valid");

        for variant in [&other_task, &other_run, &other_slot, &other_artifact] {
            assert_ne!(base.digest(), variant.digest());
        }
    }

    #[test]
    fn duplicate_output_slots_are_refused_before_identity_exists() {
        let error = CandidateResult::new(
            "task-1",
            "run-1",
            [("changeset", digest(1)), ("changeset", digest(2))],
        )
        .expect_err("duplicates are ambiguous");
        assert_eq!(
            error,
            CandidateError::DuplicateOutputSlot {
                slot: "changeset".to_string()
            }
        );
    }

    #[test]
    fn empty_slot_names_are_refused() {
        let empty = CandidateResult::new("task-1", "run-1", [("", digest(1))])
            .expect_err("empty slot names are meaningless");
        assert!(matches!(empty, CandidateError::InvalidOutputSlot { .. }));
    }

    #[test]
    fn overlong_slot_names_are_refused() {
        let long_name = "x".repeat(MAX_OUTPUT_SLOT_BYTES + 1);
        let error = CandidateResult::new("task-1", "run-1", [(long_name, digest(1))])
            .expect_err("over-long slot names cannot be stored");
        assert!(matches!(error, CandidateError::InvalidOutputSlot { .. }));
    }

    #[test]
    fn spec_validation_reports_missing_required_before_undeclared() {
        let contract = spec(&[("changeset", "code.changeset", true)]);
        let submission =
            CandidateResult::new("task-1", "run-1", [("diagnosis", digest(1))]).expect("valid");

        assert_eq!(
            submission.validate_against_spec(&contract),
            Err(CandidateError::MissingRequiredOutput {
                slot: "changeset".to_string()
            })
        );
    }

    #[test]
    fn undeclared_slots_are_refused_and_optional_slots_may_be_absent() {
        let contract = spec(&[
            ("changeset", "code.changeset", true),
            ("notes", "text.note", false),
        ]);

        let minimal =
            CandidateResult::new("task-1", "run-1", [("changeset", digest(1))]).expect("valid");
        assert_eq!(minimal.validate_against_spec(&contract), Ok(()));

        let extra = CandidateResult::new(
            "task-1",
            "run-1",
            [("changeset", digest(1)), ("bonus", digest(2))],
        )
        .expect("valid");
        assert_eq!(
            extra.validate_against_spec(&contract),
            Err(CandidateError::UndeclaredOutputSlot {
                slot: "bonus".to_string()
            })
        );
    }

    #[test]
    fn kind_permission_is_a_pure_ceiling_check() {
        let contract = spec(&[("changeset", "code.changeset", true)]);
        let slot = &contract.outputs[0];

        assert_eq!(kind_permitted(slot, "code.changeset"), Ok(()));
        assert_eq!(
            kind_permitted(slot, "research.report"),
            Err(CandidateError::OutputKindMismatch {
                slot: "changeset".to_string(),
                permitted: "code.changeset".to_string(),
                submitted: "research.report".to_string(),
            })
        );
    }

    #[test]
    fn the_canonical_form_carries_exactly_the_three_semantic_keys() {
        let candidate =
            CandidateResult::new("task-1", "run-1", [("changeset", digest(1))]).expect("valid");
        let json = candidate.to_canonical_json();

        // Nothing but task/run/outputs exists — there is no field a
        // credential or verdict could hide in.
        assert_eq!(
            json,
            format!(
                "{{\"outputs\":[{{\"artifact\":\"{}\",\"slot\":\"changeset\"}}],\
                 \"run\":\"run-1\",\"task\":\"task-1\"}}",
                digest(1)
            )
        );
    }
}
