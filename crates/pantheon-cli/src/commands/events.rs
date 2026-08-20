//! Event commands, including the Server-Sent Events watch.

use http_body_util::BodyExt;
use pantheon_operator_protocol::events::{EventListResponse, EventResponse};

use crate::args::Invocation;
use crate::client::{Client, ClientError};
use crate::commands::{Failure, emit, path};
use crate::render;

pub(crate) async fn list(
    invocation: &Invocation,
    client: &Client<'_>,
    after: Option<&str>,
    limit: Option<i64>,
) -> Result<(), Failure> {
    let mut query = Vec::new();
    if let Some(after) = after {
        query.push(format!("after={after}"));
    }
    if let Some(limit) = limit {
        query.push(format!("limit={limit}"));
    }
    let suffix = if query.is_empty() {
        "/events".to_string()
    } else {
        format!("/events?{}", query.join("&"))
    };
    let page: EventListResponse = client.get(&path(&suffix)).await?;
    emit(invocation, &page, || render::events(&page));
    Ok(())
}

/// Follows the Event stream until the operator stops it or the daemon closes
/// the connection.
///
/// The last `id` line seen is reported on exit, because that is the cursor a
/// resumed watch must pass back as `--after`. Losing it would force a relist,
/// which is the very gap the cursor exists to avoid.
pub(crate) async fn watch(
    invocation: &Invocation,
    client: &Client<'_>,
    after: Option<&str>,
) -> Result<(), Failure> {
    let suffix = match after {
        Some(cursor) => format!("/events/watch?after={cursor}"),
        None => "/events/watch".to_string(),
    };
    let response = client.stream(&path(&suffix)).await?;
    let mut body = response.into_body();
    let mut pending: Vec<u8> = Vec::new();
    let mut last_cursor: Option<String> = None;

    loop {
        let frame = match body.frame().await {
            Some(Ok(frame)) => frame,
            Some(Err(err)) => {
                report_resume(last_cursor.as_deref());
                return Err(Failure::Unreachable(format!(
                    "the event stream failed: {err}"
                )));
            }
            None => break,
        };
        let Ok(chunk) = frame.into_data() else {
            continue;
        };
        pending.extend_from_slice(&chunk);

        // SSE separates messages with a blank line. Anything after the last
        // separator is an incomplete message and stays buffered.
        while let Some((length, separator)) = find_separator(&pending) {
            let message: Vec<u8> = pending.drain(..length).collect();
            pending.drain(..separator);
            if let Some(cursor) = emit_message(invocation, &message) {
                last_cursor = Some(cursor);
            }
        }
    }

    report_resume(last_cursor.as_deref());
    Ok(())
}

/// Prints one SSE message, returning its cursor if it carried one.
pub(crate) fn emit_message(invocation: &Invocation, message: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(message);
    let mut id = None;
    let mut data = String::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push_str(value.trim());
        }
        // `event:` names the Event type, which is also inside the payload.
        // Keep-alive comment lines start with `:` and carry nothing.
    }
    if data.is_empty() {
        return id;
    }
    match serde_json::from_str::<EventResponse>(&data) {
        Ok(event) => {
            emit(invocation, &event, || render::event(&event));
            Some(event.cursor)
        }
        // An unparsable message is reported rather than dropped: silently
        // skipping it would look identical to no Event having happened.
        Err(err) => {
            eprintln!("pantheon: skipped an unreadable event: {err}");
            id
        }
    }
}

fn report_resume(cursor: Option<&str>) {
    if let Some(cursor) = cursor {
        eprintln!("pantheon: resume with --after {cursor}");
    }
}

/// Finds the end of the first complete SSE message.
///
/// Returns the message length and the separator length. Both `\n\n` and
/// `\r\n\r\n` are legal separators, and a client that understood only one
/// would stall against a perfectly conformant server.
pub(crate) fn find_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len().saturating_sub(1) {
        if index + 3 < buffer.len() && &buffer[index..index + 4] == b"\r\n\r\n" {
            return Some((index, 4));
        }
        if buffer[index] == b'\n' && buffer[index + 1] == b'\n' {
            return Some((index, 2));
        }
    }
    None
}

/// Named so the client's failure type is part of this module's surface.
const _: fn(ClientError) -> Failure = Failure::from;

#[cfg(test)]
mod tests;
