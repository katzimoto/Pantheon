//! The compiled configuration and its revision identity.

use crate::config::Digest;
use crate::config::canonical::Value;
use crate::config::model::{
    AgentComponent, AuthorizationComponent, Component, ContextComponent, EvaluatorComponent,
    ExecutionComponent, RoutingComponent,
};

/// Identifies the compilation semantics that produced a revision.
///
/// It participates in the revision digest, so a future change to how Pantheon
/// compiles configuration produces a different identity rather than silently
/// reinterpreting an existing one.
pub const COMPILER_VERSION: &str = "pantheon-config-v1";

/// The distinct component digests an immutable decision may bind.
///
/// Named individually rather than collected in a map because these are the
/// exact bindings the configuration contract requires downstream decisions to
/// record — `routePolicyDigest`, `executionProfileDigest`,
/// `evaluatorRegistryDigest`, `contextPolicyDigest`, `authzPolicyDigest` — and
/// a map would let a caller ask for one that does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentDigests {
    pub agents: Digest,
    pub routing: Digest,
    pub execution_profile: Digest,
    pub evaluator_registry: Digest,
    pub context_policy: Digest,
    pub authorization: Digest,
}

impl ComponentDigests {
    /// The revision identity these components produce under `compiler_version`.
    ///
    /// Taking the compiler version as a parameter rather than reading the
    /// current constant is what lets a stored revision be re-derived and
    /// checked on its own terms: the question at load time is whether the row
    /// is internally consistent, not whether this build would have produced
    /// it.
    #[must_use]
    pub fn revision_digest(self, compiler_version: &str) -> Digest {
        Value::object([
            ("compilerVersion", Value::string(compiler_version)),
            ("components", self.to_value()),
        ])
        .digest()
    }

    /// The canonical value the revision digest is taken over.
    fn to_value(self) -> Value {
        Value::object([
            ("agents", Value::string(self.agents.to_string())),
            ("routing", Value::string(self.routing.to_string())),
            (
                "executionProfiles",
                Value::string(self.execution_profile.to_string()),
            ),
            (
                "evaluators",
                Value::string(self.evaluator_registry.to_string()),
            ),
            ("context", Value::string(self.context_policy.to_string())),
            (
                "authorization",
                Value::string(self.authorization.to_string()),
            ),
        ])
    }
}

/// A fully compiled, internally consistent configuration.
///
/// Holding one of these is the evidence that the candidate passed the whole
/// pipeline: parsed, locally validated, cross-reference checked and
/// canonicalized. Nothing constructs it except
/// [`crate::config::compile::compile`], so an unvalidated candidate cannot be
/// mistaken for an activatable one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledConfiguration {
    pub(crate) agents: AgentComponent,
    pub(crate) routing: RoutingComponent,
    pub(crate) execution: ExecutionComponent,
    pub(crate) evaluators: EvaluatorComponent,
    pub(crate) context: ContextComponent,
    pub(crate) authorization: AuthorizationComponent,
}

impl CompiledConfiguration {
    pub const fn agents(&self) -> &AgentComponent {
        &self.agents
    }

    pub const fn routing(&self) -> &RoutingComponent {
        &self.routing
    }

    pub const fn execution(&self) -> &ExecutionComponent {
        &self.execution
    }

    pub const fn evaluators(&self) -> &EvaluatorComponent {
        &self.evaluators
    }

    pub const fn context(&self) -> &ContextComponent {
        &self.context
    }

    pub const fn authorization(&self) -> &AuthorizationComponent {
        &self.authorization
    }

    /// The immutable component records, as `(domain, digest, canonical JSON)`.
    ///
    /// The domain names match the component keys in the configuration
    /// contract's revision manifest, so a stored row is readable against the
    /// document that defines it.
    #[must_use]
    pub fn component_records(&self) -> Vec<(&'static str, Digest, String)> {
        fn record<C: Component>(
            domain: &'static str,
            component: &C,
        ) -> (&'static str, Digest, String) {
            let value = component.to_value();
            let bytes = value.to_canonical_bytes();
            let digest = Digest::of(&bytes);
            // Canonical bytes are UTF-8 by construction.
            let text = String::from_utf8(bytes).unwrap_or_default();
            (domain, digest, text)
        }
        vec![
            record("agents", &self.agents),
            record("routing", &self.routing),
            record("executionProfiles", &self.execution),
            record("evaluators", &self.evaluators),
            record("context", &self.context),
            record("authorization", &self.authorization),
        ]
    }

    /// The distinct per-domain digests.
    #[must_use]
    pub fn component_digests(&self) -> ComponentDigests {
        ComponentDigests {
            agents: self.agents.digest(),
            routing: self.routing.digest(),
            execution_profile: self.execution.digest(),
            evaluator_registry: self.evaluators.digest(),
            context_policy: self.context.digest(),
            authorization: self.authorization.digest(),
        }
    }

    /// The whole-revision content identity.
    ///
    /// Taken over the component digest set plus the compiler version, so it
    /// changes when any component changes and when the compilation semantics
    /// change — and not otherwise.
    #[must_use]
    pub fn revision_digest(&self) -> Digest {
        self.component_digests().revision_digest(COMPILER_VERSION)
    }
}

/// The digest of the source set a revision was compiled from.
///
/// Recorded as provenance, never as semantic identity: the configuration
/// contract keeps `sourceSetDigest` and the compiled digest separate precisely
/// so reformatting a file does not look like a semantic change.
#[must_use]
pub fn source_set_digest(sources: &[(String, String)]) -> Digest {
    // Sorted by name so the digest does not depend on directory listing order.
    let mut sorted: Vec<&(String, String)> = sources.iter().collect();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    Value::array(sorted.iter().map(|(name, text)| {
        Value::object([
            ("name", Value::string(name)),
            (
                "contentDigest",
                Value::string(Digest::of(text.as_bytes()).to_string()),
            ),
        ])
    }))
    .digest()
}
