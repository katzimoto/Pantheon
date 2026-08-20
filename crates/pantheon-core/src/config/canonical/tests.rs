use std::collections::BTreeMap;

use super::Value;

fn canonical(value: &Value) -> String {
    String::from_utf8(value.to_canonical_bytes()).expect("canonical bytes are utf-8")
}

#[test]
fn object_keys_are_emitted_in_sorted_order_regardless_of_insertion_order() {
    // The property Issue #23 turns on: identity must not depend on the order
    // the caller happened to build the value in.
    let forwards = Value::object([
        ("alpha", Value::Integer(1)),
        ("beta", Value::Integer(2)),
        ("gamma", Value::Integer(3)),
    ]);
    let backwards = Value::object([
        ("gamma", Value::Integer(3)),
        ("beta", Value::Integer(2)),
        ("alpha", Value::Integer(1)),
    ]);

    assert_eq!(canonical(&forwards), r#"{"alpha":1,"beta":2,"gamma":3}"#);
    assert_eq!(canonical(&forwards), canonical(&backwards));
    assert_eq!(forwards.digest(), backwards.digest());
}

#[test]
fn array_order_is_significant() {
    // Arrays carry meaning in their order, so unlike object keys they must not
    // be normalised. This is the other half of the same decision.
    let one = Value::array([Value::Integer(1), Value::Integer(2)]);
    let other = Value::array([Value::Integer(2), Value::Integer(1)]);
    assert_ne!(one.digest(), other.digest());
}

#[test]
fn a_semantic_change_changes_the_digest() {
    let base = Value::object([("limit", Value::Integer(10))]);
    let changed = Value::object([("limit", Value::Integer(11))]);
    assert_ne!(base.digest(), changed.digest());
}

#[test]
fn encoding_has_no_insignificant_whitespace() {
    let value = Value::object([
        ("nested", Value::object([("k", Value::string("v"))])),
        ("list", Value::array([Value::Null, Value::Bool(true)])),
    ]);
    assert_eq!(
        canonical(&value),
        r#"{"list":[null,true],"nested":{"k":"v"}}"#
    );
}

#[test]
fn strings_use_one_fixed_escape_form() {
    let value = Value::string("quote\" back\\ nl\n tab\t bell\u{7} unicode\u{00e9}");
    assert_eq!(
        canonical(&value),
        "\"quote\\\" back\\\\ nl\\n tab\\t bell\\u0007 unicode\u{00e9}\""
    );
}

#[test]
fn distinct_values_that_render_alike_are_not_conflated() {
    // A string "1" and an integer 1 are different configuration, and the
    // encoding must keep them apart.
    assert_ne!(
        Value::string("1").digest(),
        Value::Integer(1).digest(),
        "type must be part of identity"
    );
    assert_ne!(Value::Null.digest(), Value::string("null").digest());
}

#[test]
fn nested_objects_are_sorted_at_every_level() {
    let mut inner = BTreeMap::new();
    inner.insert("z".to_string(), Value::Integer(1));
    inner.insert("a".to_string(), Value::Integer(2));
    let value = Value::object([("outer", Value::Object(inner))]);
    assert_eq!(canonical(&value), r#"{"outer":{"a":2,"z":1}}"#);
}

#[test]
fn negative_and_extreme_integers_round_trip_exactly() {
    for probe in [0, -1, i64::MIN, i64::MAX] {
        let value = Value::Integer(probe);
        assert_eq!(canonical(&value), probe.to_string());
    }
}

#[test]
fn display_and_the_digest_encoding_agree() {
    // One rendering only: a diagnostic can never describe different bytes than
    // the ones that were digested.
    let value = Value::object([("a", Value::string("b"))]);
    assert_eq!(value.to_string(), canonical(&value));
}
