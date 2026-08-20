//! Configuration semantics: canonical form, content identity, and the
//! immutable compiled components that make up a ConfigurationRevision.
//!
//! `docs/architecture/operations/configuration-and-policy-revisions.md` states
//! the rule this module exists to serve:
//!
//! > Source files are desired configuration inputs. Runtime authority is an
//! > immutable, validated ConfigurationRevision stored in Pantheon state and
//! > activated atomically.
//!
//! Everything here is pure computation over provider-neutral values: parsing,
//! validation, canonicalization and digesting. Persisting a revision belongs to
//! `pantheon-store`, and compiling one from a source set and activating it
//! belongs to `pantheon-engine`.

pub mod canonical;
pub mod compile;
pub mod digest;
pub mod error;
pub mod model;
pub mod parse;
pub mod reader;
pub mod revision;
pub mod validate;

pub use compile::compile;
pub use digest::Digest;
pub use error::ConfigError;
pub use revision::{CompiledConfiguration, ComponentDigests};
