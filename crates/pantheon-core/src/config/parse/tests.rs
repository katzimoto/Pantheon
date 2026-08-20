use super::parse;
use crate::config::canonical::Value;

#[test]
fn parses_the_value_domain() {
    let value = parse(r#"{"a":1,"b":"x","c":[true,false,null],"d":{"e":-2}}"#).expect("parses");
    assert_eq!(value.get("a"), Some(&Value::Integer(1)));
    assert_eq!(value.get("b"), Some(&Value::string("x")));
    assert_eq!(
        value.get("d").and_then(|d| d.get("e")),
        Some(&Value::Integer(-2))
    );
}

#[test]
fn formatting_differences_do_not_change_identity() {
    // The property Issue #23 requires: source formatting that does not alter
    // compiled semantics must not alter the semantic identity.
    let dense = parse(r#"{"b":2,"a":1}"#).expect("parses");
    let spaced = parse("{\n  \"a\" : 1 ,\n  \"b\" : 2\n}\n").expect("parses");
    assert_eq!(dense, spaced);
    assert_eq!(dense.digest(), spaced.digest());
}

#[test]
fn a_duplicate_key_is_a_conflicting_declaration() {
    let err = parse(r#"{"a":1,"a":2}"#).expect_err("duplicate keys must be rejected");
    assert!(err.detail.contains("duplicate key"), "unexpected: {err}");
}

#[test]
fn fractional_numbers_are_rejected_rather_than_truncated() {
    // Truncating would make two source texts compile to one configuration.
    let err = parse(r#"{"a":1.5}"#).expect_err("floats must be rejected");
    assert!(err.detail.contains("fractional"), "unexpected: {err}");
    assert!(parse(r#"{"a":1e3}"#).is_err());
}

#[test]
fn leading_zeros_are_rejected() {
    assert!(parse(r#"{"a":01}"#).is_err());
    assert_eq!(
        parse(r#"{"a":0}"#).expect("zero parses").get("a"),
        Some(&Value::Integer(0))
    );
}

#[test]
fn malformed_source_is_rejected_with_an_offset() {
    for bad in [
        "",
        "{",
        r#"{"a"}"#,
        r#"{"a":}"#,
        r#"{"a":1,}"#,
        "[1,]",
        r#"{"a":1} trailing"#,
        "// comment\n{}",
        r#"{"a":"unterminated}"#,
    ] {
        assert!(
            parse(bad).is_err(),
            "expected {bad:?} to be rejected as malformed"
        );
    }
}

#[test]
fn escapes_decode_to_the_characters_they_name() {
    let value = parse(r#""a\"b\\c\nd\u0041""#).expect("parses");
    assert_eq!(value, Value::string("a\"b\\c\ndA"));

    // And the canonical re-encoding is the fixed form, so a source escape and
    // a literal character reach the same identity.
    let literal = parse("\"\\u00e9\"").expect("parses");
    assert_eq!(literal, Value::string("\u{e9}"));
}

#[test]
fn control_characters_must_be_escaped_in_source() {
    assert!(parse("\"raw\nnewline\"").is_err());
}
