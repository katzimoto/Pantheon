//! Evidence that startup options resolve predictably and refuse what cannot
//! work.

use std::path::PathBuf;

use super::Options;

fn parse(args: &[&str]) -> Result<Option<Options>, super::OptionsError> {
    Options::parse(args.iter().map(|arg| (*arg).to_string()), |_| None)
}

#[test]
fn the_socket_and_configuration_default_under_the_data_directory() {
    let options = parse(&["--data-dir", "/srv/pantheon"])
        .expect("parses")
        .expect("not help");
    assert_eq!(
        options.database(),
        PathBuf::from("/srv/pantheon/pantheon.db")
    );
    assert_eq!(
        options.socket,
        PathBuf::from("/srv/pantheon/run/pantheond.sock")
    );
    assert_eq!(
        options.configuration,
        PathBuf::from("/srv/pantheon/configuration.json")
    );
}

#[test]
fn an_explicit_flag_beats_the_environment() {
    let options = Options::parse(
        ["--data-dir", "/from/flag"]
            .iter()
            .map(|a| (*a).to_string()),
        |name| (name == "PANTHEON_DATA_DIR").then(|| "/from/env".to_string()),
    )
    .expect("parses")
    .expect("not help");
    assert_eq!(options.data_dir, PathBuf::from("/from/flag"));
}

#[test]
fn the_environment_supplies_the_data_directory_when_no_flag_does() {
    let options = Options::parse(std::iter::empty(), |name| {
        (name == "PANTHEON_DATA_DIR").then(|| "/from/env".to_string())
    })
    .expect("parses")
    .expect("not help");
    assert_eq!(options.data_dir, PathBuf::from("/from/env"));
}

#[test]
fn a_socket_path_too_long_for_a_unix_address_is_refused_at_startup() {
    // Left to the kernel this surfaces as `InvalidInput`, which tells an
    // operator nothing about what to change.
    let long = format!("/{}/pantheond.sock", "d".repeat(120));
    let err = parse(&["--socket", &long]).expect_err("must refuse");
    assert!(err.0.contains("shorter --socket"), "unexpected: {err}");
}

#[test]
fn an_unrecognized_argument_is_refused_rather_than_ignored() {
    let err = parse(&["--listen", "0:8080"]).expect_err("must refuse");
    assert!(err.0.contains("--listen"), "unexpected: {err}");
}

#[test]
fn help_is_not_a_startup() {
    assert!(parse(&["--help"]).expect("parses").is_none());
}
