//! Evidence for the v1 scope matcher: component boundaries, wildcard
//! semantics, and fail-closed compilation.

use super::*;
use crate::artifact::RepositoryPath;

fn scope(patterns: &[&str]) -> WorkspaceScope {
    let owned: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
    WorkspaceScope::compile(&owned).expect("valid fixture scope")
}

fn authorizes(scope: &WorkspaceScope, path: &str) -> bool {
    scope.authorizes(&RepositoryPath::from_bytes(path.as_bytes()).expect("fixture path"))
}

#[test]
fn literals_match_whole_paths_only() {
    let s = scope(&["workspace://src/checkout/main.rs"]);
    assert!(authorizes(&s, "src/checkout/main.rs"));
    // No prefix or suffix leakage through component boundaries.
    assert!(!authorizes(&s, "src/checkout/main.rs.bak"));
    assert!(!authorizes(&s, "src/checkout"));
    assert!(!authorizes(&s, "src"));
}

#[test]
fn single_star_is_confined_to_one_component() {
    let s = scope(&["workspace://src/*"]);
    assert!(authorizes(&s, "src/a.txt"));
    assert!(authorizes(&s, "src/.hidden"));
    assert!(!authorizes(&s, "src/a/b.txt"), "* must not cross a slash");
    assert!(!authorizes(&s, "src"));
    let nested = scope(&["workspace://tests/checkout/*"]);
    assert!(authorizes(&nested, "tests/checkout/x.py"));
    assert!(!authorizes(&nested, "tests/checkout/sub/y.py"));
}

#[test]
fn double_star_spans_components_including_zero() {
    let s = scope(&["workspace://src/**"]);
    assert!(authorizes(&s, "src/a.txt"));
    assert!(authorizes(&s, "src/a/b/c.txt"));
    assert!(authorizes(&s, "src"), "** matches zero segments");
    assert!(!authorizes(&s, "other/a.txt"));

    let mid = scope(&["workspace://a/**/z"]);
    assert!(authorizes(&mid, "a/z"));
    assert!(authorizes(&mid, "a/b/c/z"));
    assert!(!authorizes(&mid, "a/b/z/y"));
    assert!(!authorizes(&mid, "z"));

    let tail = scope(&["workspace://tests/**"]);
    assert!(authorizes(&tail, "tests/unit/a.rs"));
    assert!(!authorizes(&tail, "tests_extra/a.rs"));
}

#[test]
fn any_declared_pattern_authorizes_and_empty_authorizes_nothing() {
    let empty = scope(&[]);
    assert!(!authorizes(&empty, "anything"));
    // Files merely present in the Workspace are not automatically output.
    let unrelated = scope(&["workspace://docs/**"]);
    assert!(!authorizes(&unrelated, "secrets/key.pem"));
    assert!(authorizes(&unrelated, "docs/a.md"));
}

#[test]
fn non_utf8_paths_fail_closed_rather_than_lossily_matching() {
    let s = scope(&["workspace://src/**"]);
    let raw = [b's', b'r', b'c', b'/', 0xE9];
    let p = RepositoryPath::from_bytes(&raw).expect("representable");
    assert!(!s.authorizes(&p), "an inexpressible path must not match");
}

#[test]
fn unsupported_patterns_are_refused_at_compile_time() {
    for bad in [
        "repo://x/**",
        "src/**",
        "workspace://",
        "workspace://",
        "workspace://a//b",
        "workspace:///abs",
        "workspace://a**b",
        "workspace://**b",
        "workspace://a*/b",
        "workspace://a/",
    ] {
        let err = WorkspaceScope::compile(&[bad.to_string()])
            .err()
            .unwrap_or_else(|| panic!("{bad} should be refused"));
        assert!(!err.reason.is_empty(), "{bad}");
    }
}

#[test]
fn multiple_patterns_are_disjunctive() {
    let s = scope(&[
        "workspace://src/checkout/**",
        "workspace://tests/checkout/*",
    ]);
    assert!(authorizes(&s, "src/checkout/deep/file.rs"));
    assert!(authorizes(&s, "tests/checkout/one.py"));
    assert!(!authorizes(&s, "tests/other/one.py"));
}
