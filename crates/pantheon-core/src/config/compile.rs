//! Compiling source text into an internally consistent configuration.
//!
//! `docs/architecture/operations/configuration-and-policy-revisions.md` §8
//! defines the pipeline a candidate must complete before it may activate:
//!
//! ```text
//! read source bytes -> parse -> schema validate -> domain composition
//!   -> validate all cross-references -> canonicalize -> hash -> candidate
//! ```
//!
//! [`compile`] is that pipeline, and a [`CompiledConfiguration`] is the only
//! evidence that it completed. Nothing here touches the filesystem or the
//! database: compilation is pure computation, so a candidate can be validated
//! without any possibility of disturbing the active revision.

use crate::config::canonical::Value;
use crate::config::error::ConfigError;
use crate::config::model::{
    Agent, AgentComponent, AuthorizationComponent, AuthorizationRule, BackendRegistration,
    ContextComponent, EvaluatorComponent, EvaluatorKind, EvaluatorRef, EvaluatorVersion,
    ExecutionComponent, IsolationClass, LogicalAgentVersion, NetworkMode, RoutePolicy,
    RoutingComponent, RuleEffect, SandboxProfile,
};
use crate::config::parse;
use crate::config::reader::{
    as_array, as_bool, as_i64, as_str, field, non_empty, path, positive, string_list, unique,
};
use crate::config::revision::CompiledConfiguration;
use crate::config::validate;

/// The actions that require control-plane isolation.
///
/// `sandbox-broker-and-isolation.md` ("Mandatory control-plane isolation"):
/// "Model-driven arbitrary shell/process execution requires
/// `isolation.control-plane` by default." Both halves of "shell/process" are
/// listed here — an Agent that can spawn arbitrary processes is not meaningfully
/// less dangerous than one that can run arbitrary shell commands, and reading
/// the rule as covering only the shell would leave the same escape open under
/// a different action name.
pub const CONTROL_PLANE_ACTIONS: &[&str] = &["shell.execute", "process.spawn"];

/// The guarantee that names control-plane isolation.
pub const CONTROL_PLANE_GUARANTEE: &str = "isolation.control-plane";

/// The canonical action vocabulary.
///
/// These names are the ones `permissions-and-capabilities.md` ("Canonical
/// actions and resources") defines — not a local invention. The subset is the
/// surface the MVP slice uses; anything outside it is rejected rather than
/// compiled into a policy whose meaning Pantheon cannot check, and the same
/// vocabulary governs Agent action declarations and authorization rules so the
/// two cannot describe different worlds.
pub const ACTIONS: &[&str] = &[
    "filesystem.read",
    "filesystem.write",
    "filesystem.delete",
    "shell.execute",
    "process.spawn",
    "artifact.read",
    "artifact.seal",
    "secret.read",
    "secret.use",
];

/// The action v1 built-in hard policy denies to Agent principals
/// non-approvably.
///
/// `agent-manifest.md`: "Agent `secret.read` is hard-denied by v1 built-in
/// policy even if a malformed manifest attempted to permit it."
/// `permissions-and-capabilities.md`: "For Agent principals, v1 built-in hard
/// policy denies `secret.read` non-approvably."
pub const HARD_DENIED_ACTION: &str = "secret.read";

/// The v0.1.0 route preference vocabulary. Unknown keys are rejected at
/// activation rather than being silently ignored by selection.
///
/// `featureMatch` is deliberately absent: after the fail-closed feature
/// checks every validated candidate already supports every required feature,
/// so a match count cannot discriminate and would be a preference key with no
/// semantics.
pub const ROUTE_PREFERENCE_KEYS: &[&str] = &["contextCapacity"];

/// The stable candidate identity keys accepted as route tie-breaks.
pub const ROUTE_TIE_BREAK_KEYS: &[&str] = &["backendId", "agentId"];

/// Compiles configuration source text.
///
/// # Errors
///
/// [`ConfigError`] when the source is malformed, a field is missing or
/// invalid, an identity is declared twice, a reference names something the
/// configuration does not declare, or two declarations cannot hold together.
pub fn compile(source: &str) -> Result<CompiledConfiguration, ConfigError> {
    let root = parse::parse(source)?;

    let compiled = CompiledConfiguration {
        agents: agents(field(&root, "", "agents")?)?,
        routing: routing(field(&root, "", "routing")?)?,
        execution: execution(field(&root, "", "execution")?)?,
        evaluators: evaluators(field(&root, "", "evaluators")?)?,
        context: context(field(&root, "", "context")?)?,
        authorization: authorization(field(&root, "", "authorization")?)?,
    };

    // Cross-reference validation runs over the whole compiled candidate, not
    // per component: a reference is only checkable once every component it
    // could name has been compiled.
    validate::cross_references(&compiled)?;

    Ok(compiled)
}

// --- component compilation ----------------------------------------------

fn agents(value: &Value) -> Result<AgentComponent, ConfigError> {
    let entries = as_array(value, "agents")?;
    let mut seen = Vec::new();
    let mut agents = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let prefix = format!("agents[{index}]");
        reject_unknown_fields(
            entry,
            &prefix,
            &[
                "name",
                "version",
                "enabled",
                "current",
                "accepts",
                "competencies",
                "routePolicy",
                "executionFeatures",
                "minContextTokens",
                "sandboxProfile",
                "sandboxRequirements",
                "actions",
            ],
        )?;
        let name = as_str(field(entry, &prefix, "name")?, &path(&prefix, "name"))?.to_string();
        non_empty(&name, &path(&prefix, "name"))?;

        let version = as_i64(field(entry, &prefix, "version")?, &path(&prefix, "version"))?;
        positive(version, &path(&prefix, "version"))?;
        let identity = format!("{name}@{version}");
        unique(&mut seen, "Agent version", &identity)?;
        let min_context_tokens = as_i64(
            field(entry, &prefix, "minContextTokens")?,
            &path(&prefix, "minContextTokens"),
        )?;
        positive(min_context_tokens, &path(&prefix, "minContextTokens"))?;

        let actions = string_list(entry, &prefix, "actions")?;
        unique_list(&actions, "agent action")?;
        for action in &actions {
            if !ACTIONS.contains(&action.as_str()) {
                return Err(ConfigError::InvalidValue {
                    path: path(&prefix, "actions"),
                    detail: format!("{action:?} is not a canonical Pantheon action"),
                });
            }
            // Built-in hard policy is not something an Agent manifest can opt
            // out of, so declaring the action at all is rejected rather than
            // silently compiled and denied later.
            if action == HARD_DENIED_ACTION {
                return Err(ConfigError::HardPolicyViolation {
                    detail: format!(
                        "agent {name:?} declares {HARD_DENIED_ACTION}, which v1 built-in hard policy denies to Agent principals non-approvably"
                    ),
                });
            }
        }

        let accepts = string_list(entry, &prefix, "accepts")?;
        non_empty_list(&accepts, &path(&prefix, "accepts"))?;
        unique_list(&accepts, "accepts entry")?;
        let competencies = string_list(entry, &prefix, "competencies")?;
        non_empty_list(&competencies, &path(&prefix, "competencies"))?;
        unique_list(&competencies, "competency")?;

        agents.push(Agent {
            name,
            version: u32::try_from(version).map_err(|_| ConfigError::InvalidValue {
                path: path(&prefix, "version"),
                detail: "does not fit a 32-bit version".to_string(),
            })?,
            enabled: optional_bool(entry, &prefix, "enabled", true)?,
            current: optional_bool(entry, &prefix, "current", true)?,
            accepts,
            competencies,
            route_policy: as_str(
                field(entry, &prefix, "routePolicy")?,
                &path(&prefix, "routePolicy"),
            )?
            .to_string(),
            execution_features: {
                let features = string_list(entry, &prefix, "executionFeatures")?;
                unique_list(&features, "execution feature")?;
                features
            },
            min_context_tokens,
            sandbox_profile: as_str(
                field(entry, &prefix, "sandboxProfile")?,
                &path(&prefix, "sandboxProfile"),
            )?
            .to_string(),
            sandbox_requirements: {
                let requirements = string_list(entry, &prefix, "sandboxRequirements")?;
                unique_list(&requirements, "sandbox requirement")?;
                requirements
            },
            actions,
        });
    }
    Ok(AgentComponent { agents })
}

fn routing(value: &Value) -> Result<RoutingComponent, ConfigError> {
    let entries = as_array(field(value, "routing", "policies")?, "routing.policies")?;
    let mut seen = Vec::new();
    let mut policies = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let prefix = format!("routing.policies[{index}]");
        let name = as_str(field(entry, &prefix, "name")?, &path(&prefix, "name"))?.to_string();
        non_empty(&name, &path(&prefix, "name"))?;
        unique(&mut seen, "route policy", &name)?;
        let priority = optional_i64(entry, &prefix, "priority", 0)?;
        let ordering = string_list(entry, &prefix, "ordering")?;
        for key in &ordering {
            if !ROUTE_PREFERENCE_KEYS.contains(&key.as_str()) {
                return Err(ConfigError::InvalidValue {
                    path: path(&prefix, "ordering"),
                    detail: format!("unknown route preference key {key:?}"),
                });
            }
        }
        let tie_break = as_str(
            field(entry, &prefix, "tieBreak")?,
            &path(&prefix, "tieBreak"),
        )?
        .to_string();
        non_empty(&tie_break, &path(&prefix, "tieBreak"))?;
        if !ROUTE_TIE_BREAK_KEYS.contains(&tie_break.as_str()) {
            return Err(ConfigError::InvalidValue {
                path: path(&prefix, "tieBreak"),
                detail: format!("unknown route tie-break key {tie_break:?}"),
            });
        }
        policies.push(RoutePolicy {
            name,
            priority,
            ordering,
            tie_break,
            requires_keyed_launch: optional_bool(entry, &prefix, "requiresKeyedLaunch", true)?,
        });
    }
    Ok(RoutingComponent {
        policies,
        agent_pins: agent_references(value, "agentPins")?,
        agent_exclusions: agent_references(value, "agentExclusions")?,
    })
}

fn execution(value: &Value) -> Result<ExecutionComponent, ConfigError> {
    let profile_entries = as_array(field(value, "execution", "profiles")?, "execution.profiles")?;
    let mut seen_profiles = Vec::new();
    let mut profiles = Vec::with_capacity(profile_entries.len());
    for (index, entry) in profile_entries.iter().enumerate() {
        let prefix = format!("execution.profiles[{index}]");
        let name = as_str(field(entry, &prefix, "name")?, &path(&prefix, "name"))?.to_string();
        non_empty(&name, &path(&prefix, "name"))?;
        unique(&mut seen_profiles, "sandbox profile", &name)?;

        let class_at = path(&prefix, "isolationClass");
        let isolation_class = match as_str(field(entry, &prefix, "isolationClass")?, &class_at)? {
            "TRUSTED_HOST" => IsolationClass::TrustedHost,
            "CONTAINER" => IsolationClass::Container,
            other => {
                return Err(ConfigError::InvalidValue {
                    path: class_at,
                    detail: format!("unknown isolation class {other:?}"),
                });
            }
        };
        let network_at = path(&prefix, "networkMode");
        let network_mode = match as_str(field(entry, &prefix, "networkMode")?, &network_at)? {
            "NONE" => NetworkMode::None,
            "BROKERED" => NetworkMode::Brokered,
            other => {
                return Err(ConfigError::InvalidValue {
                    path: network_at,
                    detail: format!("unknown network mode {other:?}"),
                });
            }
        };
        let environment_identity = as_str(
            field(entry, &prefix, "environmentIdentity")?,
            &path(&prefix, "environmentIdentity"),
        )?
        .to_string();
        non_empty(&environment_identity, &path(&prefix, "environmentIdentity"))?;

        let guarantees = string_list(entry, &prefix, "guarantees")?;
        unique_list(&guarantees, "sandbox guarantee")?;

        profiles.push(SandboxProfile {
            name,
            isolation_class,
            guarantees,
            network_mode,
            environment_identity,
        });
    }

    let backend_entries = as_array(field(value, "execution", "backends")?, "execution.backends")?;
    let mut seen_backends = Vec::new();
    let mut backends = Vec::with_capacity(backend_entries.len());
    for (index, entry) in backend_entries.iter().enumerate() {
        let prefix = format!("execution.backends[{index}]");
        let backend_id = as_str(
            field(entry, &prefix, "backendId")?,
            &path(&prefix, "backendId"),
        )?
        .to_string();
        non_empty(&backend_id, &path(&prefix, "backendId"))?;
        unique(&mut seen_backends, "backend", &backend_id)?;
        let selector = as_str(
            field(entry, &prefix, "selector")?,
            &path(&prefix, "selector"),
        )?
        .to_string();
        non_empty(&selector, &path(&prefix, "selector"))?;
        backends.push(BackendRegistration {
            backend_id,
            enabled: as_bool(field(entry, &prefix, "enabled")?, &path(&prefix, "enabled"))?,
            selector,
        });
    }

    Ok(ExecutionComponent { profiles, backends })
}

fn evaluators(value: &Value) -> Result<EvaluatorComponent, ConfigError> {
    let version_entries = as_array(
        field(value, "evaluators", "versions")?,
        "evaluators.versions",
    )?;
    let mut seen_versions = Vec::new();
    let mut versions = Vec::with_capacity(version_entries.len());
    for (index, entry) in version_entries.iter().enumerate() {
        let prefix = format!("evaluators.versions[{index}]");
        let id = as_str(field(entry, &prefix, "id")?, &path(&prefix, "id"))?.to_string();
        non_empty(&id, &path(&prefix, "id"))?;
        unique(&mut seen_versions, "evaluator version", &id)?;

        let kind_at = path(&prefix, "kind");
        let kind = match as_str(field(entry, &prefix, "kind")?, &kind_at)? {
            "check" => EvaluatorKind::Check,
            "schema" => EvaluatorKind::Schema,
            other => {
                return Err(ConfigError::InvalidValue {
                    path: kind_at,
                    detail: format!("unknown evaluator kind {other:?}"),
                });
            }
        };

        let argv = string_list(entry, &prefix, "argv")?;
        if argv.is_empty() {
            return Err(ConfigError::InvalidValue {
                path: path(&prefix, "argv"),
                detail:
                    "must name an executable; an evaluator is an argv vector, never a shell string"
                        .to_string(),
            });
        }
        let timeout_ms = as_i64(
            field(entry, &prefix, "timeoutMs")?,
            &path(&prefix, "timeoutMs"),
        )?;
        positive(timeout_ms, &path(&prefix, "timeoutMs"))?;
        let result_protocol = as_str(
            field(entry, &prefix, "resultProtocol")?,
            &path(&prefix, "resultProtocol"),
        )?
        .to_string();
        non_empty(&result_protocol, &path(&prefix, "resultProtocol"))?;

        versions.push(EvaluatorVersion {
            id,
            kind,
            argv,
            timeout_ms,
            sandbox_profile: as_str(
                field(entry, &prefix, "sandboxProfile")?,
                &path(&prefix, "sandboxProfile"),
            )?
            .to_string(),
            result_protocol,
        });
    }

    let ref_entries = as_array(field(value, "evaluators", "refs")?, "evaluators.refs")?;
    let mut seen_refs = Vec::new();
    let mut refs = Vec::with_capacity(ref_entries.len());
    for (index, entry) in ref_entries.iter().enumerate() {
        let prefix = format!("evaluators.refs[{index}]");
        let reference = as_str(field(entry, &prefix, "ref")?, &path(&prefix, "ref"))?.to_string();
        non_empty(&reference, &path(&prefix, "ref"))?;
        unique(&mut seen_refs, "evaluator ref", &reference)?;
        refs.push(EvaluatorRef {
            reference,
            current_version: as_str(
                field(entry, &prefix, "currentVersion")?,
                &path(&prefix, "currentVersion"),
            )?
            .to_string(),
        });
    }

    Ok(EvaluatorComponent { refs, versions })
}

fn context(value: &Value) -> Result<ContextComponent, ConfigError> {
    let schema_version = as_i64(
        field(value, "context", "schemaVersion")?,
        "context.schemaVersion",
    )?;
    positive(schema_version, "context.schemaVersion")?;
    let memory_limit_tokens = as_i64(
        field(value, "context", "memoryLimitTokens")?,
        "context.memoryLimitTokens",
    )?;
    positive(memory_limit_tokens, "context.memoryLimitTokens")?;
    let workspace_orientation_limit_tokens = as_i64(
        field(value, "context", "workspaceOrientationLimitTokens")?,
        "context.workspaceOrientationLimitTokens",
    )?;
    positive(
        workspace_orientation_limit_tokens,
        "context.workspaceOrientationLimitTokens",
    )?;
    let safety_margin_tokens = as_i64(
        field(value, "context", "safetyMarginTokens")?,
        "context.safetyMarginTokens",
    )?;
    positive(safety_margin_tokens, "context.safetyMarginTokens")?;

    Ok(ContextComponent {
        schema_version: u32::try_from(schema_version).map_err(|_| ConfigError::InvalidValue {
            path: "context.schemaVersion".to_string(),
            detail: "does not fit a 32-bit version".to_string(),
        })?,
        mandatory_sections: string_list(value, "context", "mandatorySections")?,
        preload_priority: string_list(value, "context", "preloadPriority")?,
        memory_limit_tokens,
        workspace_orientation_limit_tokens,
        safety_margin_tokens,
        optional_drop_order: string_list(value, "context", "optionalDropOrder")?,
    })
}

fn authorization(value: &Value) -> Result<AuthorizationComponent, ConfigError> {
    let schema_version = as_i64(
        field(value, "authorization", "schemaVersion")?,
        "authorization.schemaVersion",
    )?;
    positive(schema_version, "authorization.schemaVersion")?;

    let entries = as_array(
        field(value, "authorization", "rules")?,
        "authorization.rules",
    )?;
    let mut rules = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let prefix = format!("authorization.rules[{index}]");
        let action_at = path(&prefix, "action");
        let action = as_str(field(entry, &prefix, "action")?, &action_at)?.to_string();
        if !ACTIONS.contains(&action.as_str()) {
            return Err(ConfigError::InvalidValue {
                path: action_at,
                detail: format!("{action:?} is not a canonical Pantheon action"),
            });
        }
        let effect_at = path(&prefix, "effect");
        let effect = match as_str(field(entry, &prefix, "effect")?, &effect_at)? {
            "permit" => RuleEffect::Permit,
            "forbid" => RuleEffect::Forbid,
            other => {
                return Err(ConfigError::InvalidValue {
                    path: effect_at,
                    detail: format!("unknown effect {other:?}"),
                });
            }
        };
        // A configured rule cannot weaken built-in hard policy. Permitting the
        // hard-denied action is rejected outright; an explicit forbid is
        // redundant but harmless and stays compilable.
        if action == HARD_DENIED_ACTION && effect == RuleEffect::Permit {
            return Err(ConfigError::HardPolicyViolation {
                detail: format!(
                    "an authorization rule permits {HARD_DENIED_ACTION}, which v1 built-in hard policy denies non-approvably"
                ),
            });
        }
        rules.push(AuthorizationRule { action, effect });
    }

    Ok(AuthorizationComponent {
        schema_version: u32::try_from(schema_version).map_err(|_| ConfigError::InvalidValue {
            path: "authorization.schemaVersion".to_string(),
            detail: "does not fit a 32-bit version".to_string(),
        })?,
        rules,
    })
}

fn optional_bool(
    parent: &Value,
    prefix: &str,
    key: &str,
    default: bool,
) -> Result<bool, ConfigError> {
    parent
        .get(key)
        .map_or(Ok(default), |value| as_bool(value, &path(prefix, key)))
}

fn optional_i64(parent: &Value, prefix: &str, key: &str, default: i64) -> Result<i64, ConfigError> {
    parent
        .get(key)
        .map_or(Ok(default), |value| as_i64(value, &path(prefix, key)))
}

fn non_empty_list(values: &[String], at: &str) -> Result<(), ConfigError> {
    if values.is_empty() {
        return Err(ConfigError::InvalidValue {
            path: at.to_string(),
            detail: "must contain at least one value".to_string(),
        });
    }
    Ok(())
}

/// Rejects duplicate entries in one set-valued list.
///
/// The set-semantics fields canonicalize as sorted sets for the component
/// digest, and the manifest schema declares them `uniqueItems`, so a repeated
/// entry is a malformed declaration, not a set with multiplicity.
fn unique_list(values: &[String], kind: &'static str) -> Result<(), ConfigError> {
    let mut seen = Vec::new();
    for value in values {
        unique(&mut seen, kind, value)?;
    }
    Ok(())
}

fn reject_unknown_fields(value: &Value, prefix: &str, allowed: &[&str]) -> Result<(), ConfigError> {
    let Value::Object(fields) = value else {
        return Ok(());
    };
    if let Some(unknown) = fields.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(ConfigError::InvalidValue {
            path: prefix.to_string(),
            detail: format!("unknown field {unknown:?}"),
        });
    }
    Ok(())
}

fn agent_references(value: &Value, key: &str) -> Result<Vec<LogicalAgentVersion>, ConfigError> {
    let at = path("routing", key);
    let Some(raw) = value.get(key) else {
        return Ok(Vec::new());
    };
    let entries = as_array(raw, &at)?;
    let mut seen = Vec::new();
    let mut references = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let prefix = format!("routing.{key}[{index}]");
        let name = as_str(field(entry, &prefix, "name")?, &path(&prefix, "name"))?.to_string();
        non_empty(&name, &path(&prefix, "name"))?;
        let version = as_i64(field(entry, &prefix, "version")?, &path(&prefix, "version"))?;
        positive(version, &path(&prefix, "version"))?;
        let version = u32::try_from(version).map_err(|_| ConfigError::InvalidValue {
            path: path(&prefix, "version"),
            detail: "does not fit a 32-bit version".to_string(),
        })?;
        let identity = format!("{name}@{version}");
        unique(&mut seen, "agent reference", &identity)?;
        references.push(LogicalAgentVersion::new(name, version));
    }
    Ok(references)
}
