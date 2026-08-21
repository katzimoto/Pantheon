//! The provider-neutral [`ContextPlan`] vocabulary and the pure rules that
//! deterministically select it from a Run's frozen source universe.
//!
//! `docs/architecture/agents-and-context/context-builder.md` is canonical for
//! the three-layer separation this module implements one layer of:
//!
//! ```text
//! ContextSourceSnapshot   what immutable sources are eligible (T3 freezes it)
//! ContextPlan             what semantic information was selected (this module)
//! backend context package how one provider represents it (adapters, later)
//! ```
//!
//! Everything here is pure computation over Pantheon's canonical values: a
//! plan's identity is the SHA-256 digest of its canonical encoding, exactly
//! like every other content-addressed control-plane document. Nothing reads
//! durable state, renders a prompt, measures tokens for a provider, touches a
//! repository, or grants authority. Loading the frozen sources and committing
//! the built plan belong to `pantheon-engine` and `pantheon-store`.
//!
//! # What is deliberately absent
//!
//! A plan carries no credentials, no Capability Grants, no host paths beyond
//! the Task-declared opaque workspace reference, no hidden model reasoning, no
//! provider session state, and no mutable latest pointers. Knowledge that an
//! operation exists (the Task scope, the Agent action surface) never appears
//! here as permission to perform it.

use std::cmp::Ordering;

use crate::config::Digest;
use crate::config::canonical::Value;
use crate::config::model::{ContextComponent, LogicalAgentVersion};
use crate::planning::TaskSpec;
use crate::planning::task::TaskInput;
use crate::scheduling::ContextSourceSnapshot;

/// Identifies the construction semantics that produced a plan.
///
/// Part of every plan's canonical identity through the `builder` object, so a
/// future change to how Pantheon selects context produces different plans
/// rather than silently reinterpreting existing ones.
pub const CONTEXT_BUILDER_VERSION: &str = "context-builder-v1";

/// Canonical section-kind names. The frozen [`ContextComponent`]'s
/// `mandatorySections` / `preloadPriority` / `optionalDropOrder` lists refer
/// to sections by these names, and preparation fails closed on a mandatory
/// name this build cannot produce.
pub const SECTION_TASK_CONTRACT: &str = "task-contract";
pub const SECTION_GOAL_CONTRACT: &str = "goal-contract";
pub const SECTION_AGENT_SOUL: &str = "agent-soul";
pub const SECTION_AGENT_BEHAVIOR: &str = "agent-behavior";
pub const SECTION_WORKSPACE_ORIENTATION: &str = "workspace-orientation";
pub const SECTION_REFERENCE_INPUT: &str = "reference-input";

/// Whether a selected section must be present at launch, was preloaded
/// because it is likely useful, or is referenced for on-demand discovery.
///
/// `context-builder.md` ("Inclusion classes") fixes these three classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InclusionClass {
    Mandatory,
    Preload,
    OnDemand,
}

impl InclusionClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mandatory => "mandatory",
            Self::Preload => "preload",
            Self::OnDemand => "on-demand",
        }
    }
}

/// The canonical trust/precedence stratum a section belongs to.
///
/// `context-builder.md` ("Trust and precedence strata") orders these five;
/// lower strata can never become higher-level control-plane authority merely
/// because their text looks like instructions. Backend renderers must preserve
/// the distinction; this type is what makes it expressible before rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrecedenceStratum {
    ExecutionProtocol,
    GoalTaskContract,
    AgentGuidance,
    ContinuationEvidence,
    ReferenceData,
}

impl PrecedenceStratum {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExecutionProtocol => "execution-protocol",
            Self::GoalTaskContract => "goal-task-contract",
            Self::AgentGuidance => "agent-guidance",
            Self::ContinuationEvidence => "continuation-evidence",
            Self::ReferenceData => "reference-data",
        }
    }

    /// The authority rank of this stratum, in canonical precedence order.
    ///
    /// `context-builder.md` ("Trust and precedence strata") orders these five
    /// and places GOAL / TASK CONTRACT above AGENT GUIDANCE above REFERENCE
    /// DATA; the rank is what makes a plan's section order state the same
    /// thing the field says instead of leaving hierarchy to each consumer.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::ExecutionProtocol => 0,
            Self::GoalTaskContract => 1,
            Self::AgentGuidance => 2,
            Self::ContinuationEvidence => 3,
            Self::ReferenceData => 4,
        }
    }
}

/// One selected semantic section of a plan.
///
/// `provenance` is the canonical object naming the immutable refs/digests the
/// section was selected from; `instruction` carries a bounded trusted
/// instruction body where the section's semantics are textual (static approved
/// Agent guidance, the Task objective). Reference-shaped sections carry only
/// their reference — large content stays where it lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSection {
    /// One of the canonical `SECTION_*` names.
    pub kind: &'static str,
    /// Stable identity within the kind (an input name, for example). Empty
    /// for kinds that appear at most once in a plan.
    pub key: String,
    pub inclusion: InclusionClass,
    pub precedence: PrecedenceStratum,
    pub provenance: Value,
    pub instruction: Option<String>,
}

impl ContextSection {
    /// The total-order key selection sorts by. Deterministic by construction,
    /// and aligned with the canonical trust hierarchy: inclusion class first
    /// (mandatory, then preload, then on-demand), then the authority stratum
    /// rank — so walking `sections` in listed order never presents
    /// lower-authority content above higher-authority content — then the
    /// canonical kind order (stable under renames), then the within-kind key.
    /// Never insertion order.
    #[must_use]
    pub fn order_key(&self) -> (u8, u8, u8, &'static str, &str) {
        (
            match self.inclusion {
                InclusionClass::Mandatory => 0,
                InclusionClass::Preload => 1,
                InclusionClass::OnDemand => 2,
            },
            self.precedence.rank(),
            kind_rank(self.kind),
            self.kind,
            self.key.as_str(),
        )
    }

    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            ("kind", Value::string(self.kind)),
            ("key", Value::string(&self.key)),
            ("inclusion", Value::string(self.inclusion.as_str())),
            ("precedence", Value::string(self.precedence.as_str())),
            ("provenance", self.provenance.clone()),
            (
                "instruction",
                match &self.instruction {
                    Some(text) => Value::string(text),
                    None => Value::Null,
                },
            ),
        ])
    }
}

/// An eligible preload section the frozen policy's drop order removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedSection {
    pub kind: &'static str,
    pub key: String,
    /// Why it was dropped. v1 has exactly one reason: the frozen policy's
    /// deterministic optional drop order under a measured capacity.
    pub reason: &'static str,
}

impl DroppedSection {
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            ("kind", Value::string(self.kind)),
            ("key", Value::string(&self.key)),
            ("reason", Value::string(self.reason)),
        ])
    }
}

/// The immutable provider-neutral selection a Run's frozen sources produce.
///
/// Not a prompt, transcript, session, rendered package, authorization grant or
/// mutable runtime context — see the canonical contract's three-layer model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPlan {
    /// The exact source universe this plan was selected from. Must equal the
    /// Run's frozen snapshot; attachment enforces it relationally.
    pub source_snapshot_digest: Digest,
    pub context_policy_digest: Digest,
    pub task_spec_digest: Digest,
    pub goal_id: String,
    pub goal_revision: i64,
    pub graph_revision: i64,
    pub agent: LogicalAgentVersion,
    pub agent_soul_digest: Digest,
    pub agent_behavior_digest: Digest,
    pub workspace_id: String,
    pub workspace_resolved_base: String,
    /// Selected sections in deterministic total order.
    pub sections: Vec<ContextSection>,
    /// Eligible preload sections the frozen policy dropped, in drop order.
    pub dropped: Vec<DroppedSection>,
}

impl ContextPlan {
    /// The canonical encoding the digest is taken over.
    ///
    /// Stored verbatim beside the digest, so a persisted plan can be re-hashed
    /// and compared against its own recorded identity.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::object([
            (
                "builder",
                Value::object([
                    ("version", Value::string(CONTEXT_BUILDER_VERSION)),
                    (
                        "contextPolicyDigest",
                        Value::string(self.context_policy_digest.to_string()),
                    ),
                ]),
            ),
            (
                "sourceSnapshot",
                Value::string(self.source_snapshot_digest.to_string()),
            ),
            (
                "task",
                Value::object([
                    (
                        "specDigest",
                        Value::string(self.task_spec_digest.to_string()),
                    ),
                    ("goalId", Value::string(&self.goal_id)),
                    ("goalRevision", Value::Integer(self.goal_revision)),
                    ("graphRevision", Value::Integer(self.graph_revision)),
                ]),
            ),
            (
                "agent",
                Value::object([
                    ("name", Value::string(&self.agent.name)),
                    ("version", Value::Integer(i64::from(self.agent.version))),
                    ("soul", Value::string(self.agent_soul_digest.to_string())),
                    (
                        "behavior",
                        Value::string(self.agent_behavior_digest.to_string()),
                    ),
                ]),
            ),
            (
                "workspace",
                Value::object([
                    ("id", Value::string(&self.workspace_id)),
                    ("resolvedBase", Value::string(&self.workspace_resolved_base)),
                ]),
            ),
            (
                "sections",
                Value::array(self.sections.iter().map(ContextSection::to_value)),
            ),
            (
                "dropped",
                Value::array(self.dropped.iter().map(DroppedSection::to_value)),
            ),
        ])
    }

    /// The immutable plan identity.
    #[must_use]
    pub fn digest(&self) -> Digest {
        self.to_value().digest()
    }
}

/// The frozen source material preparation reconstructed and digest-verified
/// before this pure builder runs.
///
/// Every field names content that was loaded through an immutable identity of
/// the Run's source snapshot — never through a current active pointer. The
/// builder trusts nothing and re-derives each section's provenance from the
/// frozen identities on the snapshot itself.
#[derive(Debug, Clone)]
pub struct FrozenSources<'a> {
    pub task_spec: &'a TaskSpec,
    /// The content digest of the exact Goal revision the Task was created
    /// from, as recorded on the durable immutable revision row.
    pub goal_content_digest: Digest,
    /// The SOUL guidance body of the frozen Agent version.
    pub soul: &'a str,
    /// The BEHAVIOR guidance body of the frozen Agent version.
    pub behavior: &'a str,
    /// The Task-declared opaque repository reference of the frozen Workspace.
    /// Never a controller-side host path.
    pub workspace_repository: &'a str,
}

/// Why a plan cannot be built from otherwise-valid frozen sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextPlanError {
    /// The frozen policy names a mandatory section this builder cannot
    /// produce from the frozen sources. Fail closed: a mandatory section is
    /// never silently omitted, and an unknown name may mean the Run froze
    /// policy semantics this build does not implement.
    MandatorySectionUnsatisfiable { section: String },
}

impl std::fmt::Display for ContextPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MandatorySectionUnsatisfiable { section } => write!(
                f,
                "the frozen context policy requires section {section:?}, which \
                 cannot be produced from the frozen sources"
            ),
        }
    }
}

impl std::error::Error for ContextPlanError {}

/// Deterministically builds the provider-neutral plan for one committed Run.
///
/// The same frozen snapshot, the same digest-verified sources and the same
/// frozen policy always produce byte-for-byte identical canonical content and
/// therefore the same digest — regardless of the order the caller collected
/// anything in. Every section's provenance derives from identities frozen at
/// T3, never from current pointers.
///
/// # Errors
///
/// [`ContextPlanError::MandatorySectionUnsatisfiable`] when the frozen policy
/// demands a mandatory section the frozen sources cannot yield.
pub fn build_context_plan(
    snapshot: &ContextSourceSnapshot,
    sources: &FrozenSources<'_>,
    policy: &ContextComponent,
) -> Result<ContextPlan, ContextPlanError> {
    let mut sections = Vec::with_capacity(6 + sources.task_spec.inputs.len());

    sections.push(ContextSection {
        kind: SECTION_TASK_CONTRACT,
        key: String::new(),
        inclusion: InclusionClass::Mandatory,
        precedence: PrecedenceStratum::GoalTaskContract,
        provenance: Value::object([
            (
                "specDigest",
                Value::string(snapshot.task_spec_digest.to_string()),
            ),
            (
                "acceptanceDigest",
                Value::string(sources.task_spec.acceptance_digest().to_string()),
            ),
            ("goalId", Value::string(&snapshot.goal_id)),
            ("goalRevision", Value::Integer(snapshot.goal_revision)),
        ]),
        instruction: Some(sources.task_spec.objective.clone()),
    });
    sections.push(ContextSection {
        kind: SECTION_GOAL_CONTRACT,
        key: String::new(),
        inclusion: InclusionClass::Mandatory,
        precedence: PrecedenceStratum::GoalTaskContract,
        provenance: Value::object([
            ("goalId", Value::string(&snapshot.goal_id)),
            ("goalRevision", Value::Integer(snapshot.goal_revision)),
            (
                "contentDigest",
                Value::string(sources.goal_content_digest.to_string()),
            ),
        ]),
        instruction: None,
    });
    sections.push(ContextSection {
        kind: SECTION_AGENT_SOUL,
        key: String::new(),
        inclusion: InclusionClass::Mandatory,
        precedence: PrecedenceStratum::AgentGuidance,
        provenance: Value::object([
            ("agentName", Value::string(&snapshot.agent.name)),
            (
                "agentVersion",
                Value::Integer(i64::from(snapshot.agent.version)),
            ),
            (
                "digest",
                Value::string(snapshot.agent_soul_digest.to_string()),
            ),
        ]),
        instruction: Some(sources.soul.to_string()),
    });
    sections.push(ContextSection {
        kind: SECTION_AGENT_BEHAVIOR,
        key: String::new(),
        inclusion: InclusionClass::Mandatory,
        precedence: PrecedenceStratum::AgentGuidance,
        provenance: Value::object([
            ("agentName", Value::string(&snapshot.agent.name)),
            (
                "agentVersion",
                Value::Integer(i64::from(snapshot.agent.version)),
            ),
            (
                "digest",
                Value::string(snapshot.agent_behavior_digest.to_string()),
            ),
        ]),
        instruction: Some(sources.behavior.to_string()),
    });
    // Bounded orientation metadata only: the Workspace's durable identity, its
    // Task-declared repository reference and the immutable base. No file tree,
    // no captured bytes, no host path — runtime exploration is Workspace
    // authority governed separately, not initial-plan content.
    sections.push(ContextSection {
        kind: SECTION_WORKSPACE_ORIENTATION,
        key: String::new(),
        inclusion: InclusionClass::Preload,
        precedence: PrecedenceStratum::ReferenceData,
        provenance: Value::object([
            ("workspaceId", Value::string(&snapshot.workspace_id)),
            ("repository", Value::string(sources.workspace_repository)),
            (
                "resolvedBase",
                Value::string(&snapshot.workspace_resolved_base),
            ),
        ]),
        instruction: None,
    });

    // Required Task inputs become on-demand references, never embedded bodies:
    // large inputs stay where they live and are discovered during execution.
    let mut inputs: Vec<&TaskInput> = sources.task_spec.inputs.iter().collect();
    inputs.sort_by(|left, right| left.name.cmp(&right.name));
    for input in inputs {
        sections.push(ContextSection {
            kind: SECTION_REFERENCE_INPUT,
            key: input.name.clone(),
            inclusion: InclusionClass::OnDemand,
            precedence: PrecedenceStratum::ReferenceData,
            provenance: Value::object([("ref", Value::string(&input.reference))]),
            instruction: None,
        });
    }

    // A mandatory section the frozen policy names must be producible. This
    // closes the gap between "policy demands X" and "this build produces X":
    // an unknown or unproducible name fails closed instead of silently
    // shipping a plan that violates its own policy.
    for required in &policy.mandatory_sections {
        if !sections.iter().any(|section| section.kind == *required) {
            return Err(ContextPlanError::MandatorySectionUnsatisfiable {
                section: required.clone(),
            });
        }
    }

    // No factual measurement exists until a backend renderer can measure a
    // rendered package, so this composed path applies no capacity budget and
    // drops nothing. The drop machinery itself is deterministic and proven by
    // the pure-domain tests with synthetic measurements.
    let (mut ordered, dropped) =
        apply_optional_drop(&sections, &policy.optional_drop_order, |_| false);
    ordered.sort_by(compare_sections);

    Ok(ContextPlan {
        source_snapshot_digest: snapshot.digest(),
        context_policy_digest: snapshot.context_policy_digest,
        task_spec_digest: snapshot.task_spec_digest,
        goal_id: snapshot.goal_id.clone(),
        goal_revision: snapshot.goal_revision,
        graph_revision: snapshot.graph_revision,
        agent: snapshot.agent.clone(),
        agent_soul_digest: snapshot.agent_soul_digest,
        agent_behavior_digest: snapshot.agent_behavior_digest,
        workspace_id: snapshot.workspace_id.clone(),
        workspace_resolved_base: snapshot.workspace_resolved_base.clone(),
        sections: ordered,
        dropped,
    })
}

/// Total order over sections: inclusion class, then authority stratum, then
/// canonical kind order, then within-kind key. A pure function of the two
/// sections alone — the frozen policy contributes to selection through the
/// mandatory-section satisfiability check and the optional drop order, never
/// through this ordering.
fn compare_sections(left: &ContextSection, right: &ContextSection) -> Ordering {
    left.order_key().cmp(&right.order_key())
}

/// The stable ordinal of a section kind inside the total order.
///
/// Fixed by the canonical contract's own enumerations rather than by spelling,
/// so renaming a kind cannot silently reorder plan identity: task-contract
/// before goal-contract (the trust strata list objective/constraints/acceptance
/// before Goal constraints is their shared stratum; within it the Task's own
/// contract leads), SOUL before BEHAVIOR (`agent-genome.md`'s layer order),
/// then workspace orientation and input references. Unknown kinds sort after
/// every known one, deterministically by spelling, so a future kind extends
/// the order without reordering existing sections.
fn kind_rank(kind: &str) -> u8 {
    match kind {
        SECTION_TASK_CONTRACT => 0,
        SECTION_GOAL_CONTRACT => 1,
        SECTION_AGENT_SOUL => 2,
        SECTION_AGENT_BEHAVIOR => 3,
        SECTION_WORKSPACE_ORIENTATION => 4,
        SECTION_REFERENCE_INPUT => 5,
        _ => u8::MAX,
    }
}

/// Applies the frozen policy's deterministic optional drop order.
///
/// Mandatory sections are never dropped. On-demand references are never
/// dropped — they are pointers, not payload. When `capacity_exceeded` holds,
/// preload sections leave in `drop_order`: whole kinds in the listed sequence,
/// tail-of-the-total-order first inside a kind. The same eligible set, the
/// same policy and the same measurement always drop the same sections in the
/// same order, whatever order the caller supplied them in.
///
/// Returns the survivors (unsorted; callers apply the total order) and the
/// dropped sections in the order they left.
pub fn apply_optional_drop(
    sections: &[ContextSection],
    drop_order: &[String],
    capacity_exceeded: impl Fn(&[ContextSection]) -> bool,
) -> (Vec<ContextSection>, Vec<DroppedSection>) {
    let mut kept: Vec<ContextSection> = sections.to_vec();
    let mut dropped = Vec::new();

    for kind in drop_order {
        while capacity_exceeded(&kept) {
            // The last remaining preload section of this kind in the total
            // order is the next to go: the order runs from most-authoritative
            // and most-prioritized content to least, so its tail is the least
            // important survivor. Stable even among same-kind siblings.
            let victim = kept
                .iter()
                .filter(|section| {
                    section.inclusion == InclusionClass::Preload && section.kind == kind.as_str()
                })
                .max_by(|left, right| compare_sections(left, right))
                .cloned();
            let Some(section) = victim else {
                // No droppable section of this kind remains; move to the next
                // drop tier rather than touching mandatory content.
                break;
            };
            kept.retain(|remaining| remaining.kind != section.kind || remaining.key != section.key);
            dropped.push(DroppedSection {
                kind: section.kind,
                key: section.key,
                reason: "policy-drop-order",
            });
        }
    }

    (kept, dropped)
}

/// The static approved SOUL/BEHAVIOR bodies of one Agent version, extracted
/// from an immutable agents component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentGuidance {
    pub soul: String,
    pub behavior: String,
}

/// Why the frozen guidance could not be reconstructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuidanceSourceError {
    /// The component does not contain this exact Agent version.
    VersionAbsent { agent: LogicalAgentVersion },
    /// The stored component is malformed for this build.
    Malformed(String),
}

impl std::fmt::Display for GuidanceSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionAbsent { agent } => write!(
                f,
                "agents component does not contain agent {}@{}",
                agent.name, agent.version
            ),
            Self::Malformed(detail) => {
                write!(f, "stored agents component is not readable: {detail}")
            }
        }
    }
}

impl std::error::Error for GuidanceSourceError {}

/// Reads the approved guidance bodies of exactly `agent` out of a parsed
/// agents configuration component.
///
/// Used identically by T3 validation and later preparation, so both derive
/// the expected frozen digests through this one extraction rule. The lookup
/// is by exact name and version: a newer version of the same Logical Agent is
/// a different source, not a substitute.
///
/// # Errors
///
/// [`GuidanceSourceError`] when the component lacks the version or cannot be
/// interpreted.
pub fn frozen_agent_guidance(
    agents_component: &Value,
    agent: &LogicalAgentVersion,
) -> Result<AgentGuidance, GuidanceSourceError> {
    let error = |detail: String| GuidanceSourceError::Malformed(detail);
    let Some(Value::Array(entries)) = agents_component.get("agents") else {
        return Err(error("missing or malformed agents array".to_string()));
    };
    let entry = entries
        .iter()
        .find(|entry| {
            matches!(
                (entry.get("name"), entry.get("version")),
                (Some(Value::String(name)), Some(Value::Integer(version)))
                    if name.as_str() == agent.name && *version == i64::from(agent.version)
            )
        })
        .ok_or(GuidanceSourceError::VersionAbsent {
            agent: agent.clone(),
        })?;
    let text = |name: &str| -> Result<String, GuidanceSourceError> {
        match entry.get(name) {
            Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
            _ => Err(error(format!("missing or empty {name}"))),
        }
    };
    Ok(AgentGuidance {
        soul: text("soul")?,
        behavior: text("behavior")?,
    })
}

/// The content digest of one bounded guidance body.
///
/// Taken over the canonical encoding of the text, so guidance identity uses
/// Pantheon's canonical digest mechanism like every other frozen source.
#[must_use]
pub fn guidance_digest(text: &str) -> Digest {
    Value::string(text).digest()
}
