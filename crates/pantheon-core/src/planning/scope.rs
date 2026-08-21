//! Deterministic matching of Task scope resource patterns against
//! repository-relative changed paths.
//!
//! `docs/architecture/tasks/task-object.md` defines `scope.resources` as the
//! Task's least-privilege ceiling over Workspace paths; sealing must reject
//! changed paths outside it. That only works if "inside" means one fixed
//! thing, so this module *is* the v1 matcher: a small segment grammar,
//! compiled once, matched deterministically, failing closed on anything it
//! cannot express exactly.
//!
//! # The grammar (recorded in the canonical Task contract)
//!
//! A pattern is `workspace://` followed by `/`-separated segments:
//!
//! - a literal segment matches itself exactly;
//! - `*` matches exactly one segment, whatever its contents;
//! - `**` matches zero or more segments;
//!
//! and nothing else. Patterns that are not `workspace://`, name no segments,
//! contain an empty segment, or bury wildcards inside a larger segment (`a**`,
//! `**b`, `x*y`) are refused at compile time rather than guessed at — an
//! ambiguous grant must fail closed, because every reading of an ambiguous
//! grant is dangerous.

use std::fmt;

use crate::artifact::RepositoryPath;

/// Why a scope pattern could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeError {
    pub pattern: String,
    pub reason: &'static str,
}

impl fmt::Display for ScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "scope pattern {:?} is unusable: {}",
            self.pattern, self.reason
        )
    }
}

impl std::error::Error for ScopeError {}

/// One compiled pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pattern {
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    One,
    Many,
}

/// The compiled resource scope of one Task.
///
/// A Task that declares no patterns compiles to an empty scope, and an empty
/// scope authorizes nothing: files merely present in a Workspace are not
/// automatically authorized output.
#[derive(Debug, Clone)]
pub struct WorkspaceScope {
    patterns: Vec<Pattern>,
}

impl WorkspaceScope {
    /// Compiles the Task's declared resource patterns.
    ///
    /// # Errors
    ///
    /// [`ScopeError`] for any pattern outside the documented grammar.
    pub fn compile(patterns: &[String]) -> Result<Self, ScopeError> {
        let mut compiled = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let Some(rest) = pattern.strip_prefix("workspace://") else {
                return Err(ScopeError {
                    pattern: pattern.clone(),
                    reason: "it does not begin with workspace://",
                });
            };
            if rest.is_empty() {
                return Err(ScopeError {
                    pattern: pattern.clone(),
                    reason: "it names no path",
                });
            }
            let mut segments = Vec::new();
            for piece in rest.split('/') {
                let segment = match piece {
                    "" => {
                        return Err(ScopeError {
                            pattern: pattern.clone(),
                            reason: "it has an empty path component",
                        });
                    }
                    "**" => Segment::Many,
                    "*" => Segment::One,
                    _ if piece.contains('*') => {
                        return Err(ScopeError {
                            pattern: pattern.clone(),
                            reason: "a wildcard must be a whole component",
                        });
                    }
                    _ => Segment::Literal(piece.to_string()),
                };
                segments.push(segment);
            }
            compiled.push(Pattern { segments });
        }
        Ok(Self { patterns: compiled })
    }

    /// Whether this scope authorizes nothing at all.
    ///
    /// A Task that declares no resource patterns has an empty ceiling, and
    /// sealing against it must refuse up front rather than discover the
    /// refusal per changed path.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether a repository-relative changed path is inside this scope.
    ///
    /// Component boundaries are load-bearing: a segment matches one whole
    /// path component or none of it, so `src/*` authorizes `src/a.txt` but
    /// never `src/a/b.txt`, and `src/**` authorizes everything beneath
    /// `src/` — including `src/` itself matching the zero-segment reading.
    ///
    /// Matching is byte-exact per component. Patterns are UTF-8 strings by
    /// way of the Task spec's canonical JSON, but paths are raw bytes: a
    /// non-UTF-8 component compares byte-wise against literal segments
    /// (and can only ever match a wildcard), which is what makes the
    /// lossless manifest encoding reachable for authorized trees instead
    /// of dead on arrival. No comparison ever decodes or normalizes.
    #[must_use]
    pub fn authorizes(&self, path: &RepositoryPath) -> bool {
        let components: Vec<&[u8]> = path.as_bytes().split(|byte| *byte == b'/').collect();
        self.patterns
            .iter()
            .any(|pattern| matches_from(&pattern.segments, &components))
    }
}

/// Matches `pattern` against `path`, both as whole segments. Components are
/// raw bytes; literals compare through their UTF-8 bytes exactly.
fn matches_from(pattern: &[Segment], path: &[&[u8]]) -> bool {
    match (pattern.split_first(), path.split_first()) {
        (None, None) => true,
        // A literal or `*` consumes exactly one component; literals compare
        // byte-exactly.
        (Some((Segment::Literal(literal), prest)), Some((component, crest))) => {
            *component == literal.as_bytes() && matches_from(prest, crest)
        }
        (Some((Segment::One, prest)), Some((_component, crest))) => matches_from(prest, crest),
        // `**` matches zero components here, or one component and retries
        // with itself still at the head.
        (Some((Segment::Many, _)), _) => {
            matches_from(&pattern[1..], path)
                || (!path.is_empty() && matches_from(pattern, &path[1..]))
        }
        // One side ran out before the other.
        _ => false,
    }
}

#[cfg(test)]
mod tests;
