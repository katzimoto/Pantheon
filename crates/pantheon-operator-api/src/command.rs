//! The command identity a mutation must carry.

use axum::http::HeaderMap;
use pantheon_engine::operator::CommandIdentity;
use pantheon_operator_protocol::{COMMAND_EPOCH_HEADER, COMMAND_ID_HEADER};

use crate::problem::{ProblemResponse, precondition_required};

/// Reads the `(commandEpoch, commandId)` a mutation was issued under.
///
/// Both headers are mandatory, and a missing one is 428 rather than 400: it is
/// a precondition the client failed to supply, and the contract maps a missing
/// mandatory precondition to `precondition-required`. Defaulting the epoch to
/// whatever is currently durable would defeat the entire fence — a client that
/// slept through a disaster restore would have its retry silently accepted as
/// new.
///
/// The transport supplies the request hash and nothing else; which Event a
/// mutation records is the engine's decision, not the wire's.
pub(crate) fn identity_from_headers(
    headers: &HeaderMap,
    request_hash: [u8; 32],
) -> Result<CommandIdentity, ProblemResponse> {
    Ok(CommandIdentity {
        epoch: required(headers, COMMAND_EPOCH_HEADER)?,
        id: required(headers, COMMAND_ID_HEADER)?,
        request_hash,
    })
}

fn required(headers: &HeaderMap, name: &'static str) -> Result<String, ProblemResponse> {
    let value = headers
        .get(name)
        .ok_or_else(|| precondition_required(format!("{name} is required for a mutation")))?;
    let text = value
        .to_str()
        .map_err(|_| precondition_required(format!("{name} must be ASCII")))?;
    if text.is_empty() {
        return Err(precondition_required(format!("{name} must not be empty")));
    }
    Ok(text.to_string())
}
