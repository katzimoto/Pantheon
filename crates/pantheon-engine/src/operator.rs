//! The operations Operator Control serves.
//!
//! Every operator-visible read and mutation is a method here, and transport
//! owns none of it. `pantheon-operator-api` translates HTTP to one of these
//! calls and translates the result back; a decision made in a handler would be
//! a decision no other entry point could reach and no test could exercise
//! without a socket.
//!
//! # What this layer is allowed to know
//!
//! It composes durable state into operator-facing views. It does not invent
//! state: where the architecture names a fact Pantheon does not yet hold — a
//! recovery barrier, an installation identity distinct from the
//! RestoreGeneration — this layer reports the absence rather than
//! substituting something that looks like the missing fact.
//!
//! # Mutations
//!
//! Every mutation goes through the #18 command kernel via `pantheon-store`,
//! under the caller's `(commandEpoch, commandId)`. There is no second write
//! path, and this layer opens no transaction of its own.

use std::sync::Arc;

use pantheon_core::config::canonical::Value;
use pantheon_store::{Command, CursorError, Store, StoreError};

use crate::configuration::ConfigurationAuthority;

/// A durable position in the Event Journal.
///
/// Re-exported rather than redefined: the cursor a client resumes from must be
/// the position the store actually recorded, and a parallel type would be one
/// conversion away from disagreeing with it.
pub use pantheon_store::Cursor;

mod goals;
mod system;

mod events;

pub use events::{EventPage, EventView, MAX_EVENTS};
pub use goals::{GoalSummary, GoalView, GoalsPage, TaskView};
pub use system::{
    ActiveConfigurationView, ComponentState, JournalView, ReadinessComponent, ReadinessReport,
    SystemView,
};

/// The API versions this build serves.
pub const API_VERSIONS: &[&str] = &["v1"];

/// The identity one operator mutation is issued under.
///
/// Owned, and separate from the store's borrowed command envelope, for two
/// reasons. A transport hands this across a task boundary that outlives the
/// request it was parsed from, and — more importantly — the *event type* a
/// mutation records is not the transport's to choose. Letting a handler name
/// the Event would let the wire decide what durable history says happened.
#[derive(Debug, Clone)]
pub struct CommandIdentity {
    /// The `commandEpoch` the caller believes is current.
    pub epoch: String,
    /// Single-use within its epoch.
    pub id: String,
    /// A non-sensitive digest of the request, computed by the transport.
    pub request_hash: [u8; 32],
}

impl CommandIdentity {
    /// A derived identity for one step of a multi-step operation.
    ///
    /// Same epoch and same request hash, a derived id. See
    /// [`derive_command_id`] for why the derivation is what makes a retry
    /// safe.
    #[must_use]
    fn step(&self, step: &str) -> Self {
        Self {
            epoch: self.epoch.clone(),
            id: derive_command_id(&self.epoch, &self.id, step),
            request_hash: self.request_hash,
        }
    }

    fn command<'a>(&'a self, event_type: &'a str) -> Command<'a> {
        Command {
            epoch: &self.epoch,
            id: &self.id,
            request_hash: &self.request_hash,
            event_type,
        }
    }
}

/// Everything the operator surface serves from, under one owner.
///
/// The store and the configuration authority share one `Arc` rather than one
/// borrowing the other, so a long-lived server can hold this without a
/// self-referential lifetime and without leaking either for the life of the
/// process.
#[derive(Debug)]
pub struct OperatorRuntime {
    store: Arc<Store>,
    configuration: ConfigurationAuthority<Arc<Store>>,
}

impl OperatorRuntime {
    #[must_use]
    pub const fn new(store: Arc<Store>, configuration: ConfigurationAuthority<Arc<Store>>) -> Self {
        Self {
            store,
            configuration,
        }
    }

    /// The operations one request may perform.
    #[must_use]
    pub fn service(&self) -> RuntimeService<'_> {
        OperatorService::new(&self.store, &self.configuration)
    }
}

/// The service a long-lived runtime hands out.
///
/// An alias so a transport can name the type without naming `pantheon-store`
/// — which it is not allowed to depend on, and has no business knowing about.
pub type RuntimeService<'a> = OperatorService<'a, Arc<Store>>;

/// The operations the operator surface is allowed to perform.
///
/// It borrows the durable store and the process-local configuration
/// authority rather than owning either: `pantheond` is the composition root,
/// and a second owner of authority is a second authority.
#[derive(Debug)]
pub struct OperatorService<'a, S> {
    store: &'a Store,
    configuration: &'a ConfigurationAuthority<S>,
}

impl<'a, S> OperatorService<'a, S> {
    #[must_use]
    pub const fn new(store: &'a Store, configuration: &'a ConfigurationAuthority<S>) -> Self {
        Self {
            store,
            configuration,
        }
    }
}

/// A failure an operator can be told about without leaking internals.
///
/// Each variant maps onto exactly one of the problem codes the public API
/// contract enumerates, which is why the set is closed rather than a string:
/// a handler that had to classify by message text would be one refactor away
/// from returning the wrong status.
#[derive(Debug)]
pub enum OperatorError {
    /// The named resource does not exist.
    NotFound { resource: &'static str, id: String },
    /// The request is well-formed but the current state forbids it.
    Conflict(String),
    /// The command carried an epoch that is not the current
    /// RestoreGeneration. Fails closed; see the command kernel.
    StaleCommandEpoch { supplied: String, current: String },
    /// This command identity was already used with a different request.
    CommandConflict { command_id: String },
    /// The requested journal position cannot be resumed from.
    CursorGone(String),
    /// The control plane cannot safely serve this yet.
    NotReady(String),
    /// The request itself is unacceptable.
    Invalid(String),
    /// Something failed that the operator can do nothing about.
    Internal(String),
}

impl std::fmt::Display for OperatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { resource, id } => write!(f, "no such {resource}: {id}"),
            Self::Conflict(detail) | Self::CursorGone(detail) => f.write_str(detail),
            Self::StaleCommandEpoch { supplied, current } => write!(
                f,
                "command epoch {supplied} is not the current command epoch {current}"
            ),
            Self::CommandConflict { command_id } => write!(
                f,
                "command {command_id} was already used for a different request"
            ),
            Self::NotReady(detail) | Self::Invalid(detail) | Self::Internal(detail) => {
                f.write_str(detail)
            }
        }
    }
}

impl std::error::Error for OperatorError {}

impl From<StoreError> for OperatorError {
    /// Maps durable outcomes onto operator-visible ones.
    ///
    /// The typed store variants exist precisely so this mapping is total and
    /// mechanical. Anything else becomes [`OperatorError::Internal`]: an
    /// operator cannot act on a SQLite error, and repeating one would put
    /// storage detail on a public surface.
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::StaleCommandEpoch { supplied, current } => {
                Self::StaleCommandEpoch { supplied, current }
            }
            StoreError::CommandConflict { command_id } => Self::CommandConflict { command_id },
            StoreError::GoalNotCancellable { .. } | StoreError::RevisionConflict { .. } => {
                Self::Conflict(err.to_string())
            }
            other => Self::Internal(other.to_string()),
        }
    }
}

impl From<CursorError> for OperatorError {
    fn from(err: CursorError) -> Self {
        Self::CursorGone(err.to_string())
    }
}

/// Derives the daemon-internal command identity for one step of a multi-step
/// operator command.
///
/// A single operator request can require more than one authoritative
/// transaction — creating a Goal, recording its planning decision and
/// materializing the plan are three. Each needs its own durable command
/// identity, and each must be *the same* identity on a retry, or a retry
/// would execute a step that already committed. Deriving them from the
/// operator's own `(commandEpoch, commandId)` makes the whole operation
/// idempotent without inventing an in-process cache, which the mission
/// forbids.
///
/// Derivation is over the canonical encoding of the three parts rather than a
/// concatenation, so no choice of `command_id` and `step` can collide with a
/// different pair.
fn derive_command_id(epoch: &str, command_id: &str, step: &str) -> String {
    let value = Value::object([
        ("commandEpoch", Value::string(epoch)),
        ("commandId", Value::string(command_id)),
        ("step", Value::string(step)),
    ]);
    format!("{step}-{}", short(&value.digest().to_hex()))
}

/// How much of a derived digest an identifier carries.
///
/// 128 bits. Long enough that two different commands cannot collide by
/// accident, short enough that an operator can read an id off a terminal and
/// type it back. The full digest is not an identifier anyone benefits from.
const IDENTIFIER_HEX: usize = 32;

fn short(hex: &str) -> &str {
    &hex[..IDENTIFIER_HEX.min(hex.len())]
}

/// The Goal identity one create-Goal command produces.
///
/// Deterministic for the same reason: a retry of `POST /goals` must be able
/// to find what the first attempt created. The command kernel's replay result
/// deliberately carries no value — the durable ledger records where the Event
/// landed, not what some earlier process returned — so the only way to
/// reconcile a retry is to re-read the resource, and that requires knowing its
/// identity from the command alone.
fn derive_goal_id(epoch: &str, command_id: &str) -> String {
    let value = Value::object([
        ("commandEpoch", Value::string(epoch)),
        ("commandId", Value::string(command_id)),
    ]);
    format!("goal-{}", short(&value.digest().to_hex()))
}

#[cfg(test)]
mod tests;
