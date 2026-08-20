//! Evidence that the command line means what it says.

use std::path::PathBuf;

use super::{Command, Invocation};

fn parse(args: &[&str]) -> Result<Option<Invocation>, super::ArgsError> {
    Invocation::parse_with(args.iter().map(|arg| (*arg).to_string()), |_| None)
}

fn command(args: &[&str]) -> Command {
    parse(args).expect("parses").expect("not help").command
}

#[test]
fn the_socket_comes_from_the_flag_then_the_environment_then_the_default() {
    let flagged = Invocation::parse_with(
        ["--socket", "/flag.sock", "status"]
            .iter()
            .map(|a| (*a).to_string()),
        |_| Some("/env.sock".to_string()),
    )
    .expect("parses")
    .expect("not help");
    assert_eq!(flagged.socket, PathBuf::from("/flag.sock"));

    let from_env = Invocation::parse_with(["status"].iter().map(|a| (*a).to_string()), |name| {
        (name == "PANTHEON_SOCKET").then(|| "/env.sock".to_string())
    })
    .expect("parses")
    .expect("not help");
    assert_eq!(from_env.socket, PathBuf::from("/env.sock"));

    let defaulted = parse(&["status"]).expect("parses").expect("not help");
    assert_eq!(
        defaulted.socket,
        PathBuf::from("pantheon-data/run/pantheond.sock")
    );
}

#[test]
fn every_command_the_mission_requires_parses() {
    assert_eq!(command(&["status"]), Command::Status);
    assert_eq!(command(&["version"]), Command::Version);
    assert_eq!(command(&["goal", "list"]), Command::GoalList);
    assert_eq!(
        command(&["goal", "get", "goal-1"]),
        Command::GoalGet {
            id: "goal-1".to_string()
        }
    );
    assert_eq!(
        command(&["goal", "cancel", "goal-1"]),
        Command::GoalCancel {
            id: "goal-1".to_string()
        }
    );
    assert_eq!(
        command(&["events", "watch"]),
        Command::EventsWatch { after: None }
    );
    assert_eq!(
        command(&["events", "watch", "--after", "e:4"]),
        Command::EventsWatch {
            after: Some("e:4".to_string())
        }
    );
    assert_eq!(
        command(&["events", "list", "--limit", "5"]),
        Command::EventsList {
            after: None,
            limit: Some(5)
        }
    );
}

#[test]
fn a_deliverable_is_required_unless_it_says_otherwise() {
    // The safe reading has to be the default: an accidentally optional
    // deliverable would let a plan that cannot produce it validate.
    let Command::GoalCreate(request) = command(&[
        "goal",
        "create",
        "--objective",
        "ship it",
        "--deliverable",
        "changeset:code.changeset",
        "--deliverable",
        "notes:doc.markdown:optional",
    ]) else {
        panic!("expected goal create");
    };
    assert_eq!(
        request.deliverables,
        vec![
            ("changeset".to_string(), "code.changeset".to_string(), true),
            ("notes".to_string(), "doc.markdown".to_string(), false),
        ]
    );
}

#[test]
fn a_deliverable_suffix_that_is_not_optional_is_refused_rather_than_ignored() {
    let err = parse(&[
        "goal",
        "create",
        "--objective",
        "ship it",
        "--deliverable",
        "changeset:code.changeset:maybe",
    ])
    .expect_err("must refuse");
    assert!(err.0.contains("optional"), "unexpected: {err}");
}

#[test]
fn goal_constraints_accumulate_in_the_order_given() {
    let Command::GoalCreate(request) = command(&[
        "goal",
        "create",
        "--objective",
        "ship it",
        "--permit",
        "filesystem.read",
        "--permit",
        "filesystem.write",
        "--forbid",
        "git.push",
        "--resource",
        "workspace://src/**",
        "--input",
        "repository=repo://shop",
    ]) else {
        panic!("expected goal create");
    };
    assert_eq!(
        request.permitted_effects,
        ["filesystem.read", "filesystem.write"]
    );
    assert_eq!(request.forbidden_effects, ["git.push"]);
    assert_eq!(request.permitted_resources, ["workspace://src/**"]);
    assert_eq!(
        request.inputs,
        vec![("repository".to_string(), "repo://shop".to_string())]
    );
}

#[test]
fn an_input_without_a_reference_is_refused() {
    let err = parse(&[
        "goal",
        "create",
        "--objective",
        "x",
        "--input",
        "repository",
    ])
    .expect_err("must refuse");
    assert!(err.0.contains("<name>=<reference>"), "unexpected: {err}");
}

#[test]
fn goal_create_without_an_objective_is_refused_before_any_request_is_built() {
    let err = parse(&["goal", "create", "--permit", "filesystem.read"]).expect_err("must refuse");
    assert!(err.0.contains("--objective"), "unexpected: {err}");
}

#[test]
fn an_unknown_command_is_refused_rather_than_guessed_at() {
    assert!(parse(&["goals"]).is_err());
    assert!(parse(&["goal", "delete", "goal-1"]).is_err());
    assert!(parse(&["events", "tail"]).is_err());
    assert!(parse(&[]).is_err());
}

#[test]
fn a_flag_that_needs_a_value_and_has_none_is_refused() {
    assert!(parse(&["--socket"]).is_err());
    assert!(parse(&["goal", "get"]).is_err());
    assert!(parse(&["events", "list", "--limit"]).is_err());
    assert!(parse(&["events", "list", "--limit", "soon"]).is_err());
}

#[test]
fn help_is_not_a_command() {
    assert!(parse(&["--help"]).expect("parses").is_none());
    assert!(parse(&["-h"]).expect("parses").is_none());
}

#[test]
fn a_global_flag_is_recognized_wherever_it_appears() {
    // `pantheon status --json` is what an operator types. A parser that only
    // accepted globals before the command would ignore it silently, which is
    // worse than refusing it.
    let trailing = parse(&["status", "--json"])
        .expect("parses")
        .expect("not help");
    assert!(trailing.json);
    assert_eq!(trailing.command, Command::Status);

    let leading = parse(&["--json", "status"])
        .expect("parses")
        .expect("not help");
    assert_eq!(leading, trailing);
}

#[test]
fn a_command_that_takes_no_arguments_refuses_extra_words() {
    // Otherwise a mistyped option looks like it took effect.
    assert!(parse(&["status", "--verbose"]).is_err());
    assert!(parse(&["goal", "list", "goal-1"]).is_err());
    assert!(parse(&["goal", "get", "goal-1", "goal-2"]).is_err());
}
