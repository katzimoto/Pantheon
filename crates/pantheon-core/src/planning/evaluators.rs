//! Resolving evaluator references out of a stored configuration component.
//!
//! Issue #24 requires Task materialization to resolve evaluator refs "from
//! the active ConfigurationRevision". The configuration component is stored
//! as canonical JSON and content-addressed by #23, so resolution reads that
//! stored text rather than a separately compiled copy — which is what makes
//! the pinned `(registry digest, version id)` coordinate recover the exact
//! evaluator definition later.
//!
//! Only the `ref -> currentVersion` hop is implemented, because that is the
//! only hop pinning needs. Reconstituting the whole component would be a
//! decompiler this mission has no use for.

use crate::config::canonical::Value;
use crate::config::parse;
use crate::planning::validate::EvaluatorResolver;

/// Resolves logical evaluator refs from a stored evaluators component.
#[derive(Debug, Clone)]
pub struct RegistryResolver {
    entries: Vec<(String, String)>,
}

impl RegistryResolver {
    /// Reads the `refs` table out of an evaluators component's canonical JSON.
    ///
    /// # Errors
    ///
    /// Returns the parse failure when the stored text is not the canonical
    /// component shape. Failing here is correct: a component Pantheon cannot
    /// interpret must not silently resolve to nothing, because "no evaluator"
    /// and "unknown evaluator" have different meanings at validation.
    pub fn from_canonical_json(text: &str) -> Result<Self, parse::ParseError> {
        let value = parse::parse(text)?;
        let mut entries = Vec::new();
        if let Some(Value::Array(refs)) = value.get("refs") {
            for entry in refs {
                let (Some(Value::String(reference)), Some(Value::String(version))) =
                    (entry.get("ref"), entry.get("currentVersion"))
                else {
                    continue;
                };
                entries.push((reference.clone(), version.clone()));
            }
        }
        Ok(Self { entries })
    }

    /// How many logical refs this registry resolves.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl EvaluatorResolver for RegistryResolver {
    fn resolve(&self, reference: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == reference)
            .map(|(_, version)| version.clone())
    }
}
