//! Dispatch commands: the operator's durable desired state.

use pantheon_operator_protocol::dispatch::DispatchResponse;

use crate::args::Invocation;
use crate::client::Client;
use crate::commands::{Failure, command_epoch, command_id, emit, path};
use crate::render;

pub(crate) async fn status(invocation: &Invocation, client: &Client<'_>) -> Result<(), Failure> {
    let dispatch: DispatchResponse = client.get(&path("/dispatch")).await?;
    emit(invocation, &dispatch, || render::dispatch(&dispatch));
    Ok(())
}

/// Pauses or resumes dispatch.
///
/// The CLI mirrors the daemon's optimistic concurrency: it reads the current
/// representation for its ETag and echoes that back as `If-Match`. A lost
/// race surfaces as the daemon's stale-revision problem rather than a silent
/// overwrite of a concurrent pause/resume.
async fn set_mode(
    invocation: &Invocation,
    client: &Client<'_>,
    action: &str,
) -> Result<(), Failure> {
    let current: DispatchResponse = client.get(&path("/dispatch")).await?;
    let if_match = format!("\"dispatch-{}\"", current.revision);
    let epoch = command_epoch(client).await?;
    let id = command_id(invocation)?;
    let dispatch: DispatchResponse = client
        .post(
            &path(&format!("/dispatch/actions/{action}")),
            &epoch,
            &id,
            Some(&if_match),
            None,
        )
        .await?;
    emit(invocation, &dispatch, || render::dispatch(&dispatch));
    Ok(())
}

pub(crate) async fn pause(invocation: &Invocation, client: &Client<'_>) -> Result<(), Failure> {
    set_mode(invocation, client, "pause").await
}

pub(crate) async fn resume(invocation: &Invocation, client: &Client<'_>) -> Result<(), Failure> {
    set_mode(invocation, client, "resume").await
}
