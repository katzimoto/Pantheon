//! Goal commands.

use pantheon_operator_protocol::goals::{
    CreateGoalRequest, DeliverablePayload, GoalConstraintsPayload, GoalInputPayload,
    GoalListResponse, GoalResponse, GoalSpecPayload,
};

use crate::args::{GoalRequest, Invocation};
use crate::client::Client;
use crate::commands::{Failure, command_epoch, command_id, emit, path};
use crate::render;

pub(crate) async fn create(
    invocation: &Invocation,
    client: &Client<'_>,
    request: &GoalRequest,
) -> Result<(), Failure> {
    let body = serde_json::to_vec(&CreateGoalRequest {
        goal: payload(request),
    })
    .map_err(|err| Failure::Usage(format!("could not encode the goal: {err}")))?;

    // Read the epoch, then mutate. Both go over the same socket to the same
    // daemon, so a restore between the two is precisely the case the daemon's
    // epoch fence rejects — which is the intended outcome, not a race to
    // paper over.
    let epoch = command_epoch(client).await?;
    let id = command_id(invocation)?;
    let goal: GoalResponse = client
        .post(&path("/goals"), &epoch, &id, Some(body))
        .await?;
    emit(invocation, &goal, || render::goal(&goal));
    Ok(())
}

pub(crate) async fn get(
    invocation: &Invocation,
    client: &Client<'_>,
    goal_id: &str,
) -> Result<(), Failure> {
    let goal: GoalResponse = client.get(&path(&format!("/goals/{goal_id}"))).await?;
    emit(invocation, &goal, || render::goal(&goal));
    Ok(())
}

pub(crate) async fn list(invocation: &Invocation, client: &Client<'_>) -> Result<(), Failure> {
    let goals: GoalListResponse = client.get(&path("/goals")).await?;
    emit(invocation, &goals, || render::goals(&goals));
    Ok(())
}

pub(crate) async fn cancel(
    invocation: &Invocation,
    client: &Client<'_>,
    goal_id: &str,
) -> Result<(), Failure> {
    let epoch = command_epoch(client).await?;
    let id = command_id(invocation)?;
    let goal: GoalResponse = client
        .post(
            &path(&format!("/goals/{goal_id}/actions/cancel")),
            &epoch,
            &id,
            None,
        )
        .await?;
    emit(invocation, &goal, || render::cancelled(&goal));
    Ok(())
}

fn payload(request: &GoalRequest) -> GoalSpecPayload {
    GoalSpecPayload {
        objective: request.objective.clone(),
        inputs: request
            .inputs
            .iter()
            .map(|(name, reference)| GoalInputPayload {
                name: name.clone(),
                reference: reference.clone(),
            })
            .collect(),
        deliverables: request
            .deliverables
            .iter()
            .map(|(name, kind, required)| DeliverablePayload {
                name: name.clone(),
                kind: kind.clone(),
                required: *required,
            })
            .collect(),
        constraints: GoalConstraintsPayload {
            permitted_effects: request.permitted_effects.clone(),
            forbidden_effects: request.forbidden_effects.clone(),
            permitted_resources: request.permitted_resources.clone(),
        },
    }
}
