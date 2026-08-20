//! The operator workflow as an operator performs it: the real `pantheon`
//! binary against a real `pantheond`, over a real Unix socket.
//!
//! This is the only place the mission's acceptance path is exercised end to
//! end through the command line. Everything else tests a layer.
//!
//! # Locating the daemon
//!
//! `CARGO_BIN_EXE_*` names only this package's binaries, so `pantheond` is
//! located beside `pantheon` in the same target directory. `cargo test
//! --workspace` — which is what `./scripts/verify.sh` runs — builds both. A
//! run that has not built the daemon fails with that instruction rather than
//! skipping: a test that silently does not run is worse than one that fails.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

const CONFIGURATION: &str = r#"{
  "agents": [{"name":"builder","version":1,"accepts":["code-change"],"competencies":["rust"],
    "routePolicy":"default","executionFeatures":["exec.shell"],"minContextTokens":8000,
    "sandboxProfile":"strict","sandboxRequirements":["isolation.control-plane"],
    "actions":["filesystem.read"]}],
  "routing": {"policies":[{"name":"default","ordering":["featureMatch"],"tieBreak":"backendId"}]},
  "execution": {
    "profiles":[{"name":"strict","isolationClass":"CONTAINER",
      "guarantees":["isolation.control-plane"],"networkMode":"NONE",
      "environmentIdentity":"sha256:image"}],
    "backends":[{"backendId":"fake-local","enabled":true,"selector":"fake"}]},
  "evaluators": {
    "versions":[{"id":"unit-v1","kind":"check","argv":["/bin/check"],"timeoutMs":1000,
      "sandboxProfile":"strict","resultProtocol":"p-v1"}],
    "refs":[{"ref":"check://project/unit-tests","currentVersion":"unit-v1"}]},
  "context": {"schemaVersion":1,"mandatorySections":["task"],"preloadPriority":["task"],
    "memoryLimitTokens":4000,"workspaceOrientationLimitTokens":2000,
    "safetyMarginTokens":512,"optionalDropOrder":["memory"]},
  "authorization": {"schemaVersion":1,"rules":[{"action":"filesystem.read","effect":"permit"}]}
}"#;

struct Fixture {
    dir: PathBuf,
    daemon: Option<Child>,
}

impl Fixture {
    /// Short by necessity: a Unix socket address is bounded by `SUN_LEN`.
    fn start(label: &str) -> Self {
        let dir = PathBuf::from(format!("/tmp/pw-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the installation directory");
        std::fs::write(dir.join("configuration.json"), CONFIGURATION).expect("write configuration");

        let daemon = Self::start_daemon(&dir);

        let fixture = Self {
            dir,
            daemon: Some(daemon),
        };
        for _ in 0..200 {
            // `status` is used rather than `version` deliberately: `version`
            // answers without the daemon, so it would report success against
            // a socket file a killed daemon left behind.
            if fixture.run(&["status"]).status.success() {
                return fixture;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("pantheond did not start serving");
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("s.sock")
    }

    /// Stops the daemon and starts a new one over the same installation.
    fn restart(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        let replacement = Self::start_daemon(&self.dir);
        self.daemon = Some(replacement);
        for _ in 0..200 {
            if self.run(&["status"]).status.success() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("pantheond did not resume serving");
    }

    /// Runs `pantheon` against this daemon.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pantheon"))
            .arg("--socket")
            .arg(self.socket())
            .args(args)
            .output()
            .expect("run pantheon")
    }

    /// The returned child is stored in a `Fixture`, which kills and waits on
    /// it in `Drop`. Returning it rather than spawning inline is what lets
    /// `restart` reuse the same command.
    fn start_daemon(dir: &Path) -> Child {
        Command::new(daemon_binary())
            .arg("--data-dir")
            .arg(dir)
            .arg("--socket")
            .arg(dir.join("s.sock"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start pantheond")
    }

    fn stdout(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "pantheon {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf-8")
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        serde_json::from_str(&self.stdout(&all)).expect("JSON")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn daemon_binary() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_pantheon"))
        .parent()
        .expect("the binary has a directory")
        .join("pantheond");
    assert!(
        path.exists(),
        "{} is not built. Run `cargo test --workspace` (what ./scripts/verify.sh runs), \
         or `cargo build -p pantheond` first.",
        path.display()
    );
    path
}

const GOAL_ARGS: &[&str] = &[
    "goal",
    "create",
    "--objective",
    "Fix the checkout timeout with the smallest safe change.",
    "--input",
    "repository=repo://whiskyshop",
    "--deliverable",
    "changeset:code.changeset",
    "--permit",
    "filesystem.read",
    "--permit",
    "filesystem.write",
    "--forbid",
    "git.push",
    "--resource",
    "workspace://src/**",
];

#[test]
fn the_operator_reaches_a_ready_task_and_then_cancels_it() {
    let fixture = Fixture::start("workflow");

    let status = fixture.stdout(&["status"]);
    assert!(status.contains("ready          yes"), "{status}");
    assert!(
        status.contains("recovery-barrier       unimplemented"),
        "readiness must show what it cannot establish: {status}"
    );

    let goal = fixture.json(GOAL_ARGS);
    let goal_id = goal["id"].as_str().expect("id").to_string();
    assert_eq!(goal["phase"], "Active");
    assert_eq!(goal["tasks"][0]["phase"], "Ready");

    let listed = fixture.json(&["goal", "list"]);
    assert_eq!(listed["goals"].as_array().expect("goals").len(), 1);
    let cursor = listed["snapshotCursor"]
        .as_str()
        .expect("cursor")
        .to_string();

    let fetched = fixture.json(&["goal", "get", &goal_id]);
    assert_eq!(fetched, goal);

    let cancelled = fixture.stdout(&["goal", "cancel", &goal_id]);
    assert!(cancelled.contains("cancellation accepted"), "{cancelled}");
    assert!(
        cancelled.contains("Finalizing"),
        "the rendered result must not claim the Goal is Cancelled: {cancelled}"
    );

    // The cursor taken before the cancellation still reaches its Event, which
    // is the gap-free property the snapshot cursor exists for.
    let events = fixture.json(&["events", "list", "--after", &cursor]);
    let events = events["events"].as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["eventType"], "goal.cancel.requested");
}

#[test]
fn repeating_a_create_with_the_same_command_id_is_the_same_command() {
    // This is the only safe way to retry a mutation whose outcome is unknown,
    // and the CLI has to make it reachable or an operator cannot do it.
    let fixture = Fixture::start("retry");

    let mut args = vec!["--command-id", "operator-retry"];
    args.extend_from_slice(GOAL_ARGS);
    let first = fixture.json(&args);
    let second = fixture.json(&args);

    assert_eq!(first, second);
    assert_eq!(
        fixture.json(&["goal", "list"])["goals"]
            .as_array()
            .expect("goals")
            .len(),
        1,
        "a retry must not create a second Goal"
    );
}

#[test]
fn two_invocations_without_a_command_id_are_two_different_commands() {
    // The other half: a fresh id per invocation, or every second `goal create`
    // would silently replay the first.
    let fixture = Fixture::start("distinct");

    let first = fixture.json(GOAL_ARGS);
    let second = fixture.json(GOAL_ARGS);

    assert_ne!(first["id"], second["id"]);
    assert_eq!(
        fixture.json(&["goal", "list"])["goals"]
            .as_array()
            .expect("goals")
            .len(),
        2
    );
}

#[test]
fn a_refusal_and_an_unreachable_daemon_have_different_exit_codes() {
    // A script has to be able to tell "the daemon said no" from "there was no
    // daemon" without parsing text.
    let fixture = Fixture::start("exit-codes");

    let refused = fixture.run(&["goal", "get", "goal-nope"]);
    assert_eq!(refused.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("not-found (404)"), "{stderr}");

    let unreachable = Command::new(env!("CARGO_BIN_EXE_pantheon"))
        .args(["--socket", "/tmp/pantheon-no-such.sock", "status"])
        .output()
        .expect("run pantheon");
    assert_eq!(unreachable.status.code(), Some(4));

    let misused = fixture.run(&["goal", "frobnicate"]);
    assert_eq!(misused.status.code(), Some(2));
}

#[test]
fn the_cli_cannot_answer_from_anywhere_but_the_daemon() {
    // There is no offline mode. With the daemon stopped, a read that could in
    // principle be answered from the database on disk must still fail — the
    // CLI has no way to reach it, and that is the point of the crate boundary.
    let mut fixture = Fixture::start("no-offline");
    assert!(fixture.run(&["goal", "list"]).status.success());

    if let Some(mut daemon) = fixture.daemon.take() {
        let _ = daemon.kill();
        let _ = daemon.wait();
    }
    // The database is still right there in the installation directory.
    assert!(fixture.dir.join("pantheon.db").exists());

    let offline = fixture.run(&["goal", "list"]);
    assert_eq!(
        offline.status.code(),
        Some(4),
        "a stopped daemon must make every read fail, not fall back to the store"
    );

    // `version` is the one exception, because it is a fact about the binary.
    let version = fixture.run(&["version"]);
    assert!(version.status.success());
}

#[test]
fn a_restart_leaves_the_cli_reading_and_watching_the_same_durable_goal() {
    // The acceptance property a restart has to preserve: the Goal, the
    // command epoch, and the client's journal position. None of it lives in
    // either process's memory, so all three survive.
    let mut fixture = Fixture::start("restart");

    let before = fixture.json(GOAL_ARGS);
    let goal_id = before["id"].as_str().expect("id").to_string();
    let epoch_before = fixture.json(&["status"])["commandEpoch"].clone();
    let cursor = fixture.json(&["goal", "list"])["snapshotCursor"]
        .as_str()
        .expect("cursor")
        .to_string();

    fixture.restart();

    let epoch_after = fixture.json(&["status"])["commandEpoch"].clone();
    assert_eq!(
        epoch_before, epoch_after,
        "an ordinary restart must not rotate the command epoch"
    );
    assert_eq!(
        fixture.json(&["goal", "get", &goal_id]),
        before,
        "the Goal is exactly as it was"
    );

    // The cursor taken before the restart still resumes: a mutation after the
    // restart is reachable from it, with nothing between lost.
    fixture.stdout(&["goal", "cancel", &goal_id]);
    let events = fixture.json(&["events", "list", "--after", &cursor]);
    let events = events["events"].as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["eventType"], "goal.cancel.requested");

    // And the retry fence still holds across the restart.
    let mut args = vec!["--command-id", "across-restart"];
    args.extend_from_slice(GOAL_ARGS);
    let first = fixture.json(&args);
    fixture.restart();
    assert_eq!(fixture.json(&args), first);
}
