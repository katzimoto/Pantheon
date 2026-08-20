//! Evidence for the Server-Sent Events reader.

use crate::args::{Command, Invocation};
use crate::commands::events::{emit_message, find_separator};

fn invocation() -> Invocation {
    Invocation {
        socket: std::path::PathBuf::from("/unused.sock"),
        json: false,
        command_id: None,
        command: Command::EventsWatch { after: None },
    }
}

#[test]
fn both_legal_message_separators_are_understood() {
    // A client that understood only `\n\n` would stall forever against a
    // perfectly conformant server that used CRLF.
    assert_eq!(find_separator(b"id: a\n\nrest"), Some((5, 2)));
    assert_eq!(find_separator(b"id: a\r\n\r\nrest"), Some((5, 4)));
    assert_eq!(
        find_separator(b"id: a\n"),
        None,
        "an incomplete message waits"
    );
}

#[test]
fn a_message_split_across_frames_is_not_read_twice_or_dropped() {
    // The reader drains only complete messages, so a partial tail must stay
    // buffered rather than being parsed as a whole message.
    let mut buffer = Vec::from(&b"id: e:1\ndata: {}"[..]);
    assert_eq!(find_separator(&buffer), None);
    buffer.extend_from_slice(b"\n\nid: e:2\n");
    let (length, separator) = find_separator(&buffer).expect("now complete");
    assert_eq!(&buffer[..length], b"id: e:1\ndata: {}");
    assert_eq!(&buffer[length + separator..], b"id: e:2\n");
}

#[test]
fn the_cursor_reported_for_resumption_is_the_events_own_cursor() {
    // The whole point of the id line is that handing it back as `after`
    // resumes exactly where the stream stopped.
    let message = concat!(
        "id: epoch:7\n",
        "event: goal.created\n",
        r#"data: {"eventId":"e1","cursor":"epoch:7","eventType":"goal.created","recordedAt":1}"#,
    );
    let cursor = emit_message(&invocation(), message.as_bytes());
    assert_eq!(cursor.as_deref(), Some("epoch:7"));
}

#[test]
fn a_keep_alive_comment_carries_no_cursor_and_is_not_an_event() {
    assert_eq!(emit_message(&invocation(), b":\n"), None);
}

#[test]
fn an_unreadable_payload_still_yields_its_cursor_rather_than_losing_the_position() {
    // Dropping the position would make the next resume silently skip
    // everything between here and wherever the client restarted.
    let message = "id: epoch:9\ndata: not json";
    assert_eq!(
        emit_message(&invocation(), message.as_bytes()).as_deref(),
        Some("epoch:9")
    );
}
