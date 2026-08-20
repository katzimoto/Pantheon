//! The valid MVP configuration source the compilation tests build on.
//!
//! Each binary compiles this module separately and uses a subset, so unused
//! items here are expected rather than dead code.
#![allow(dead_code)]

/// A complete, internally consistent v0.1.0 configuration.
///
/// It declares exactly the surfaces Issue #23 names: one Logical Agent, a
/// deterministic route policy, the strict local container profile plus a
/// separate verification profile, the fake and production-local backend
/// registrations, one deterministic evaluator, the context policy, and the
/// minimal authorization rules.
pub(crate) const VALID_SOURCE: &str = r#"{
  "agents": [
    {
      "name": "builder",
      "version": 1,
      "accepts": ["code-change"],
      "competencies": ["rust"],
      "routePolicy": "default",
      "executionFeatures": ["exec.shell"],
      "minContextTokens": 8000,
      "sandboxProfile": "strict-local-container",
      "sandboxRequirements": ["isolation.control-plane"],
      "actions": ["shell.execute", "filesystem.read", "filesystem.write"]
    }
  ],
  "routing": {
    "policies": [
      { "name": "default", "ordering": ["contextCapacity"], "tieBreak": "backendId" }
    ]
  },
  "execution": {
    "profiles": [
      {
        "name": "strict-local-container",
        "isolationClass": "CONTAINER",
        "guarantees": ["isolation.control-plane", "isolation.peer-workspaces"],
        "networkMode": "NONE",
        "environmentIdentity": "sha256:aaaa0000000000000000000000000000000000000000000000000000000000001"
      },
      {
        "name": "verification-default",
        "isolationClass": "CONTAINER",
        "guarantees": ["isolation.control-plane"],
        "networkMode": "NONE",
        "environmentIdentity": "sha256:bbbb0000000000000000000000000000000000000000000000000000000000002"
      }
    ],
    "backends": [
      { "backendId": "fake-local", "enabled": true, "selector": "fake" },
      { "backendId": "local-container", "enabled": true, "selector": "local-container" }
    ]
  },
  "evaluators": {
    "versions": [
      {
        "id": "unit-tests-v1",
        "kind": "check",
        "argv": ["/usr/bin/pantheon-check", "--suite", "unit"],
        "timeoutMs": 60000,
        "sandboxProfile": "verification-default",
        "resultProtocol": "pantheon-check-v1"
      }
    ],
    "refs": [
      { "ref": "check://project/unit-tests", "currentVersion": "unit-tests-v1" }
    ]
  },
  "context": {
    "schemaVersion": 1,
    "mandatorySections": ["task", "acceptance"],
    "preloadPriority": ["task", "workspace"],
    "memoryLimitTokens": 4000,
    "workspaceOrientationLimitTokens": 2000,
    "safetyMarginTokens": 512,
    "optionalDropOrder": ["workspace", "memory"]
  },
  "authorization": {
    "schemaVersion": 1,
    "rules": [
      { "action": "shell.execute", "effect": "permit" },
      { "action": "secret.read", "effect": "forbid" }
    ]
  }
}"#;

/// Replaces the first occurrence of `from` with `to`, asserting it was present.
///
/// Asserting the substitution happened is what stops a renamed fixture field
/// from silently turning a rejection test into a re-test of the valid source.
#[must_use]
pub(crate) fn variant(from: &str, to: &str) -> String {
    assert!(
        VALID_SOURCE.contains(from),
        "fixture does not contain {from:?}; the variant would test nothing"
    );
    VALID_SOURCE.replacen(from, to, 1)
}
