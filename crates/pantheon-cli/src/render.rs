//! Turning a response into something an operator can read.
//!
//! Rendering only. Nothing here decides anything, and no field is computed:
//! every line is a value the daemon sent. `--json` bypasses this entirely.

use std::fmt::Write as _;

use pantheon_operator_protocol::dispatch::DispatchResponse;
use pantheon_operator_protocol::events::{EventListResponse, EventResponse};
use pantheon_operator_protocol::goals::{GoalListResponse, GoalResponse};
use pantheon_operator_protocol::problem::Problem;
use pantheon_operator_protocol::system::SystemResponse;

/// A refusal, as one line.
///
/// The code comes first because it is the stable part; the detail is the
/// human half and may change without notice.
#[must_use]
pub(crate) fn problem(problem: &Problem) -> String {
    format!(
        "{} ({}): {}",
        problem.code.as_str(),
        problem.status,
        problem.detail
    )
}

#[must_use]
pub(crate) fn system(system: &SystemResponse) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "daemon         {}", system.daemon_version);
    let _ = writeln!(out, "api            {}", system.api_versions.join(", "));
    let _ = writeln!(out, "schema         {}", system.schema_version);
    let _ = writeln!(out, "command epoch  {}", system.command_epoch);
    let _ = writeln!(
        out,
        "journal        {} @ {}",
        system.journal.epoch,
        system
            .journal
            .latest_sequence
            .map_or_else(|| "empty".to_string(), |sequence| sequence.to_string())
    );
    match &system.active_configuration {
        Some(active) => {
            let _ = writeln!(
                out,
                "configuration  revision {} ({}{})",
                active.activation_sequence,
                &active.content_digest[..12.min(active.content_digest.len())],
                if active.semantics_loaded {
                    ""
                } else {
                    ", source drifted"
                }
            );
        }
        None => {
            let _ = writeln!(out, "configuration  none active");
        }
    }
    let _ = writeln!(
        out,
        "ready          {}",
        if system.readiness.ready { "yes" } else { "no" }
    );
    for component in &system.readiness.components {
        let _ = write!(out, "  {:<22} {}", component.name, component.state);
        if let Some(detail) = &component.detail {
            let _ = write!(out, " — {detail}");
        }
        let _ = writeln!(out);
    }
    out.trim_end().to_string()
}

#[must_use]
pub(crate) fn goal(goal: &GoalResponse) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "goal      {}", goal.id);
    let _ = writeln!(out, "phase     {}", goal.phase);
    let _ = writeln!(
        out,
        "revision  {} (goal revision {})",
        goal.revision, goal.goal_revision
    );
    let _ = writeln!(out, "objective {}", goal.goal.objective);
    if goal.tasks.is_empty() {
        let _ = writeln!(out, "tasks     none");
    } else {
        let _ = writeln!(out, "tasks");
        for task in &goal.tasks {
            let _ = writeln!(out, "  {:<10} {}", task.phase, task.id);
        }
    }
    out.trim_end().to_string()
}

/// A cancellation result.
///
/// Says what actually happened rather than "cancelled": the Goal is
/// finalizing toward `Cancelled`, and it reaches that phase only once its
/// obligations are safely finalized.
#[must_use]
pub(crate) fn cancelled(goal: &GoalResponse) -> String {
    format!(
        "cancellation accepted\n{}\n\nThe Goal reaches Cancelled once its obligations are finalized.",
        self::goal(goal)
    )
}

#[must_use]
pub(crate) fn goals(goals: &GoalListResponse) -> String {
    let mut out = String::new();
    if goals.goals.is_empty() {
        let _ = writeln!(out, "no goals");
    }
    for goal in &goals.goals {
        let _ = writeln!(out, "{:<12} {:<4} {}", goal.phase, goal.revision, goal.id);
    }
    // Printed so an operator can hand it straight to `events watch --after`,
    // which is the whole reason the list carries it.
    let _ = write!(out, "\nsnapshot cursor {}", goals.snapshot_cursor);
    out
}

#[must_use]
pub(crate) fn events(page: &EventListResponse) -> String {
    let mut out = String::new();
    if page.events.is_empty() {
        let _ = writeln!(out, "no events");
    }
    for record in &page.events {
        let _ = writeln!(out, "{}", event(record));
    }
    let _ = write!(out, "\nnext cursor {}", page.next_cursor);
    out
}

#[must_use]
pub(crate) fn event(event: &EventResponse) -> String {
    let cause = match (&event.command_epoch, &event.command_id) {
        (Some(_), Some(id)) => format!(" (command {id})"),
        _ => String::new(),
    };
    format!("{:<24} {}{}", event.cursor, event.event_type, cause)
}

/// The dispatch view: desired state first, then the factual gates.
#[must_use]
pub(crate) fn dispatch(dispatch: &DispatchResponse) -> String {
    let mut text = format!(
        "dispatch {} (revision {})\n",
        dispatch.desired_mode, dispatch.revision
    );
    if dispatch.effective_can_dispatch {
        text.push_str("  new Run intents: permitted\n");
    } else {
        text.push_str("  new Run intents: blocked by\n");
        for gate in &dispatch.blocked_by {
            text.push_str(&format!("    {gate}\n"));
        }
    }
    text
}
