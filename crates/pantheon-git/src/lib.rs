//! Isolated Git materialization of Task-owned Workspaces.
//!
//! # Owns
//!
//! The concrete implementation of `pantheon-engine`'s
//! [`RepositoryMaterializer`](pantheon_engine::workspace::RepositoryMaterializer)
//! port: how a Task-local repository is actually created, observed and
//! discarded on a local filesystem, and the sterile, non-interactive profile
//! every Git process it starts runs under.
//!
//! # Must not own
//!
//! Durable authority or orchestration. It never opens the database, never
//! decides whether a Workspace may exist, and never decides what happens
//! after a failure — `pantheon-engine` owns the ordering between durable
//! state and external effect, and `pantheon-store` owns the state itself.
//! It also owns no Sandbox behaviour: the Workspace it produces is safe
//! *input* to a later isolation layer, not that layer.
//!
//! # Why this is a separate crate
//!
//! `docs/development/implementation.md` names two boundaries that each
//! justify one, and this is both: a concrete platform implementation behind
//! an abstract port, and a trust boundary. Everything here spawns processes
//! against repository state, which is exactly the authority the engine must
//! not be able to reach directly.
//!
//! # The security model, in one paragraph
//!
//! A Task Workspace is an *independent* repository. It is created empty, is
//! given exactly the objects reachable from one immutable commit, and is
//! never connected to the source repository by a remote, an alternate object
//! store or a shared common directory. Nothing the worker does inside it can
//! reach the source repository's refs or object database, because no path
//! from one to the other exists. That structural absence — not a deny-list of
//! dangerous settings — is the boundary. [`GitMaterializer`] documents what
//! that costs and what it buys, and
//! `docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md`
//! for the contract it implements.

mod materializer;
mod sterile;

#[cfg(test)]
mod tests;

pub use materializer::{GitMaterializer, PANTHEON_BASE_REF};
