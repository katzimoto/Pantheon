//! Cross-reference validation.
//!
//! Local field validation can only prove a declaration is well-formed. These
//! rules prove the declarations mean something *together* — the case Issue #23
//! calls "syntactically valid but internally inconsistent", where every field
//! parses and the configuration still cannot be acted on.
//!
//! Each rule traces to a contract:
//!
//! - an Agent's `routePolicy` "names a logical configured route policy
//!   resolved through ConfigurationRevision" (`agent-manifest.md`);
//! - an Agent's `sandbox.profile` names "a logical SandboxProfile from
//!   ConfigurationRevision", and its `sandbox.requirements` are guarantees the
//!   profile must actually assert (`agent-manifest.md`,
//!   `sandbox-broker-and-isolation.md`: "Profile names are desired policy, not
//!   proof");
//! - model-driven arbitrary shell *or process* execution requires
//!   `isolation.control-plane`, which a trusted-host profile cannot assert
//!   (`sandbox-broker-and-isolation.md`);
//! - an evaluator version's profile must resolve, and every logical evaluator
//!   ref must resolve to a declared immutable version
//!   (`evaluation-and-evaluator-registry.md`);
//! - the context component's drop order and preload priority may only name
//!   sections the component declares (`context-builder.md`).
//!
//! Applying the shell rule at activation is stricter than the sandbox contract,
//! which states it as a preparation-time fail-closed rule. Rejecting the
//! combination here is a deliberate choice: configuration that could never
//! place its own Agent is not authority worth activating.

use crate::config::compile::{CONTROL_PLANE_ACTIONS, CONTROL_PLANE_GUARANTEE};
use crate::config::error::ConfigError;
use crate::config::model::IsolationClass;
use crate::config::revision::CompiledConfiguration;

/// Checks every cross-component reference in a compiled candidate.
///
/// # Errors
///
/// [`ConfigError::UnknownReference`] when a declaration names something no
/// component declares, or [`ConfigError::IncompatibleCombination`] when two
/// individually valid declarations cannot hold at once.
pub fn cross_references(compiled: &CompiledConfiguration) -> Result<(), ConfigError> {
    let route_policies: Vec<&str> = compiled
        .routing()
        .policies
        .iter()
        .map(|policy| policy.name.as_str())
        .collect();

    for agent in &compiled.agents().agents {
        // Rule 1: the Agent's route policy must exist.
        if !route_policies.contains(&agent.route_policy.as_str()) {
            return Err(ConfigError::UnknownReference {
                from: format!("agent {:?}", agent.name),
                kind: "route policy",
                id: agent.route_policy.clone(),
            });
        }

        // Rule 2: the Agent's sandbox profile must exist, and must assert every
        // guarantee the Agent requires. A profile that cannot satisfy its own
        // Agent is dead configuration.
        let profile = compiled
            .execution()
            .profiles
            .iter()
            .find(|profile| profile.name == agent.sandbox_profile)
            .ok_or_else(|| ConfigError::UnknownReference {
                from: format!("agent {:?}", agent.name),
                kind: "sandbox profile",
                id: agent.sandbox_profile.clone(),
            })?;

        for required in &agent.sandbox_requirements {
            if !profile.guarantees.contains(required) {
                return Err(ConfigError::IncompatibleCombination {
                    detail: format!(
                        "agent {:?} requires sandbox guarantee {:?}, which profile {:?} does not assert",
                        agent.name, required, profile.name
                    ),
                });
            }
        }

        // Rule 3: an Agent that can run arbitrary shell commands or spawn
        // arbitrary processes must do so under control-plane isolation.
        if let Some(action) = agent
            .actions
            .iter()
            .find(|action| CONTROL_PLANE_ACTIONS.contains(&action.as_str()))
        {
            if !profile.isolation_class.can_isolate_control_plane() {
                return Err(ConfigError::IncompatibleCombination {
                    detail: format!(
                        "agent {:?} may run {action} but profile {:?} is {}, which cannot isolate the control plane",
                        agent.name,
                        profile.name,
                        profile.isolation_class.as_str()
                    ),
                });
            }
            if !profile
                .guarantees
                .iter()
                .any(|guarantee| guarantee == CONTROL_PLANE_GUARANTEE)
            {
                return Err(ConfigError::IncompatibleCombination {
                    detail: format!(
                        "agent {:?} may run {action} but profile {:?} does not assert {CONTROL_PLANE_GUARANTEE}",
                        agent.name, profile.name
                    ),
                });
            }
        }
    }

    // Rule 4: every evaluator version's sandbox profile must exist.
    for version in &compiled.evaluators().versions {
        if !compiled
            .execution()
            .profiles
            .iter()
            .any(|profile| profile.name == version.sandbox_profile)
        {
            return Err(ConfigError::UnknownReference {
                from: format!("evaluator version {:?}", version.id),
                kind: "sandbox profile",
                id: version.sandbox_profile.clone(),
            });
        }
    }

    // Rule 5: every logical evaluator ref must resolve to a declared version.
    for reference in &compiled.evaluators().refs {
        if !compiled
            .evaluators()
            .versions
            .iter()
            .any(|version| version.id == reference.current_version)
        {
            return Err(ConfigError::UnknownReference {
                from: format!("evaluator ref {:?}", reference.reference),
                kind: "evaluator version",
                id: reference.current_version.clone(),
            });
        }
    }

    // Rule 6: context selection may only name sections the component declares,
    // and a mandatory section can never be in the optional drop order —
    // mandatory context is never silently truncated.
    let context = compiled.context();
    for section in &context.optional_drop_order {
        if context.mandatory_sections.contains(section) {
            return Err(ConfigError::IncompatibleCombination {
                detail: format!(
                    "context section {section:?} is mandatory and cannot appear in the optional drop order"
                ),
            });
        }
    }
    for section in &context.preload_priority {
        if !context.mandatory_sections.contains(section)
            && !context.optional_drop_order.contains(section)
        {
            return Err(ConfigError::UnknownReference {
                from: "context preload priority".to_string(),
                kind: "context section",
                id: section.clone(),
            });
        }
    }

    Ok(())
}

/// Whether `class` is the strict local container class the MVP profile uses.
#[must_use]
pub const fn is_container(class: IsolationClass) -> bool {
    matches!(class, IsolationClass::Container)
}
