//! Evidence against real repositories.
//!
//! These tests need the `git` executable, which every environment that can
//! check this repository out already has, and they run against repositories
//! they create themselves in a temporary directory.
//!
//! Three of them are *substrate* tests in the sense `AGENTS.md` means: they
//! establish how Git actually behaves rather than asserting a comment about
//! it. `a_linked_worktree_shares_the_source_repositorys_git_authority` is the
//! reason this crate does not use a worktree;
//! `a_workspace_fetch_is_pinned_to_an_object_not_a_ref` is the reason the
//! fetch names an object identity; and
//! `host_git_configuration_reaches_a_repository_created_without_the_sterile_profile`
//! is the reason the sterile profile exists at all. Each one fails if Git
//! stops behaving that way, which is when Pantheon would need to know.

use std::path::{Path, PathBuf};
use std::process::Command;

use pantheon_core::workspace::{Materialization, RequestedBase, ResolvedBase};
use pantheon_engine::workspace::{MaterializationTarget, RepositoryMaterializer};

use super::{GitMaterializer, PANTHEON_BASE_REF};

/// Names a destination the way `WorkspaceController::path_of` does, so these
/// tests exercise the same layout the engine asks for.
fn workspace_repo(root: &Path, workspace_id: &str) -> PathBuf {
    root.join(workspace_id).join("repo")
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pantheon-git-test-{label}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs `git` for the *test's* own purposes — building fixtures and
/// inspecting results — with a committer identity and without the host's
/// configuration. Deliberately not the crate's sterile profile: a test that
/// used the code under test to observe the code under test would agree with
/// itself for free.
fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
    command.output().expect("run git")
}

fn ok(dir: &Path, args: &[&str]) -> String {
    let output = git(dir, args);
    assert!(
        output.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn fails(dir: &Path, args: &[&str]) -> String {
    let output = git(dir, args);
    assert!(
        !output.status.success(),
        "git {args:?} unexpectedly succeeded in {}",
        dir.display()
    );
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

/// A source repository with one commit on `main`.
fn source_repository(root: &Path) -> PathBuf {
    let source = root.join("source");
    std::fs::create_dir_all(&source).expect("create source");
    ok(&source, &["init", "--quiet", "-b", "main"]);
    std::fs::write(source.join("app.txt"), b"original\n").expect("write");
    ok(&source, &["add", "-A"]);
    ok(&source, &["commit", "--quiet", "-m", "base"]);
    source
}

fn refs_of(repo: &Path) -> String {
    ok(repo, &["show-ref"])
}

fn main_ref() -> RequestedBase {
    RequestedBase::parse("refs/heads/main").expect("valid ref name")
}

struct Fixture {
    _dir: TempDir,
    materializer: GitMaterializer,
    source: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let dir = TempDir::new(label);
        let source = source_repository(&dir.0);
        let control = dir.0.join("control");
        std::fs::create_dir_all(&control).expect("create control root");
        let materializer = GitMaterializer::new(&control).expect("create materializer");
        let root = dir.0.join("workspaces");
        Self {
            _dir: dir,
            materializer,
            source,
            root,
        }
    }

    fn destination(&self) -> PathBuf {
        workspace_repo(&self.root, "workspace-1")
    }

    fn resolve(&self) -> ResolvedBase {
        self.materializer
            .resolve_base(&self.source, &main_ref())
            .expect("the requested base resolves")
    }

    fn materialize(&self, base: &ResolvedBase) -> PathBuf {
        let destination = self.destination();
        let verified = self
            .materializer
            .materialize(&MaterializationTarget {
                workspace_id: "workspace-1",
                source: &self.source,
                destination: &destination,
                base,
            })
            .expect("the workspace materializes");
        assert_eq!(
            verified.as_str(),
            base.as_str(),
            "materialization must verify the base it was asked for"
        );
        destination
    }

    fn observe(&self, base: &ResolvedBase) -> Materialization {
        let destination = self.destination();
        self.materializer
            .observe(&MaterializationTarget {
                workspace_id: "workspace-1",
                source: &self.source,
                destination: &destination,
                base,
            })
            .expect("observation succeeds")
    }
}

#[test]
fn a_materialized_workspace_is_checked_out_at_the_immutable_base() {
    let fixture = Fixture::new("materialize");
    let base = fixture.resolve();
    let workspace = fixture.materialize(&base);

    assert_eq!(ok(&workspace, &["rev-parse", "HEAD"]), base.as_str());
    assert_eq!(
        std::fs::read_to_string(workspace.join("app.txt")).expect("worktree file"),
        "original\n",
        "the working tree is materialized, not just the object store"
    );
    // Pinned under Pantheon's own namespace so an ordinary `git gc` inside
    // the Workspace cannot prune the base out from under it.
    assert_eq!(
        ok(&workspace, &["rev-parse", PANTHEON_BASE_REF]),
        base.as_str()
    );
    // Detached: no branch exists that anything could mistake for Candidate
    // identity.
    assert!(
        !git(&workspace, &["symbolic-ref", "--quiet", "HEAD"])
            .status
            .success(),
        "HEAD must be detached, not on a branch"
    );
    assert_eq!(ok(&workspace, &["for-each-ref", "refs/heads"]), "");
    assert_eq!(fixture.observe(&base), Materialization::Present);
}

#[test]
fn a_workspace_shares_no_git_authority_with_the_source_repository() {
    let fixture = Fixture::new("isolation");
    let base = fixture.resolve();
    let workspace = fixture.materialize(&base);

    // Its own repository, not a view onto another one.
    assert!(
        workspace.join(".git").is_dir(),
        "the workspace must own its repository directory, not point at one"
    );
    assert_eq!(
        ok(
            &workspace,
            &["rev-parse", "--path-format=absolute", "--git-dir"]
        ),
        ok(
            &workspace,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"]
        ),
        "a shared common directory is exactly what an untrusted shell may not have"
    );
    // No borrowed object database.
    assert!(
        !workspace.join(".git/objects/info/alternates").exists(),
        "the workspace must not borrow the source's objects"
    );
    // No remote: nothing to push to, and nothing that could carry a
    // credential.
    assert_eq!(ok(&workspace, &["remote"]), "");
    let stderr = fails(&workspace, &["push"]);
    assert!(
        stderr.contains("No configured push destination"),
        "unexpected push failure: {stderr}"
    );
}

#[test]
fn ordinary_workspace_mutation_cannot_change_the_source_repository() {
    let fixture = Fixture::new("mutation");
    let base = fixture.resolve();
    let workspace = fixture.materialize(&base);
    let source_refs_before = refs_of(&fixture.source);

    // Everything a coding worker normally does.
    std::fs::write(workspace.join("app.txt"), b"changed\n").expect("write");
    std::fs::write(workspace.join("new.txt"), b"added\n").expect("write");
    ok(&workspace, &["add", "-A"]);
    ok(&workspace, &["commit", "--quiet", "-m", "worker change"]);
    let worker_commit = ok(&workspace, &["rev-parse", "HEAD"]);
    ok(&workspace, &["checkout", "--quiet", "-b", "worker/feature"]);
    ok(
        &workspace,
        &["update-ref", "refs/heads/whatever", &worker_commit],
    );

    assert_ne!(worker_commit, base.as_str(), "the worker really did commit");
    assert_eq!(
        refs_of(&fixture.source),
        source_refs_before,
        "no authoritative source ref moved"
    );
    // Not even the objects reached the source: there is no path between the
    // two object databases at all.
    assert!(
        !git(&fixture.source, &["cat-file", "-e", &worker_commit])
            .status
            .success(),
        "the worker's commit must not exist in the source object database"
    );
    // The sterile profile removes the host user's Git identity along with
    // the rest of HOME, so the Workspace carries its own: a worker can commit
    // without being told to configure one first.
    assert_eq!(
        ok(&workspace, &["config", "--get", "user.name"]),
        "Pantheon Task Workspace"
    );
    std::fs::write(workspace.join("app.txt"), b"changed again\n").expect("write");
    let committed = Command::new("git")
        .arg("-C")
        .arg(&workspace)
        .args(["commit", "--quiet", "-a", "-m", "no ambient identity"])
        .env_clear()
        .envs(std::env::var_os("PATH").map(|path| ("PATH", path)))
        .output()
        .expect("run git");
    assert!(
        committed.status.success(),
        "a worker with no ambient Git identity must still be able to commit: {}",
        String::from_utf8_lossy(&committed.stderr)
    );
    assert_eq!(
        ok(&workspace, &["log", "-1", "--format=%an <%ae>"]),
        "Pantheon Task Workspace <workspace@pantheon.invalid>"
    );
}

#[test]
fn a_linked_worktree_shares_the_source_repositorys_git_authority() {
    // The substrate fact behind the choice of strategy. If this ever stops
    // being true, the reasoning in `materializer` needs revisiting; until
    // then it is why a linked worktree is not a boundary for an untrusted
    // shell, and why `verify_isolation` refuses one.
    let dir = TempDir::new("worktree-contrast");
    let source = source_repository(&dir.0);
    let worktree = dir.0.join("linked");
    ok(
        &source,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "linked",
            worktree.to_str().expect("utf-8 path"),
        ],
    );

    std::fs::write(worktree.join("app.txt"), b"from the worktree\n").expect("write");
    ok(&worktree, &["add", "-A"]);
    ok(&worktree, &["commit", "--quiet", "-m", "worktree change"]);
    let commit = ok(&worktree, &["rev-parse", "HEAD"]);

    assert!(
        git(&source, &["cat-file", "-e", &commit]).status.success(),
        "a linked worktree writes into the source object database"
    );
    ok(&worktree, &["update-ref", "refs/heads/main", &commit]);
    assert_eq!(
        ok(&source, &["rev-parse", "refs/heads/main"]),
        commit,
        "and can move the source repository's authoritative refs"
    );

    // And Pantheon refuses to treat such a repository as an isolated
    // Workspace.
    let control = dir.0.join("control");
    std::fs::create_dir_all(&control).expect("create control root");
    let materializer = GitMaterializer::new(&control).expect("create materializer");
    let base = ResolvedBase::parse(&commit).expect("object name");
    assert_eq!(
        materializer
            .observe(&MaterializationTarget {
                workspace_id: "workspace-1",
                source: &source,
                destination: &worktree,
                base: &base,
            })
            .expect("observation succeeds"),
        Materialization::Unknown,
        "a linked worktree must never be observed as a valid materialization"
    );
}

#[test]
fn a_workspace_fetch_is_pinned_to_an_object_not_a_ref() {
    let fixture = Fixture::new("moving-ref");
    let base = fixture.resolve();

    // The source's `main` advances after resolution and before
    // materialization — the exact race a ref-named fetch would lose.
    std::fs::write(fixture.source.join("app.txt"), b"moved on\n").expect("write");
    ok(&fixture.source, &["add", "-A"]);
    ok(&fixture.source, &["commit", "--quiet", "-m", "moved"]);
    let moved = ok(&fixture.source, &["rev-parse", "refs/heads/main"]);
    assert_ne!(moved, base.as_str(), "the source ref really moved");

    let workspace = fixture.materialize(&base);

    assert_eq!(
        ok(&workspace, &["rev-parse", "HEAD"]),
        base.as_str(),
        "the workspace is bound to the base that was resolved, not to where the ref went"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("app.txt")).expect("worktree file"),
        "original\n"
    );
    assert!(
        !git(&workspace, &["cat-file", "-e", &moved])
            .status
            .success(),
        "the later commit must not even be present in the workspace"
    );
}

#[test]
fn hostile_source_repository_configuration_does_not_become_worker_authority() {
    let fixture = Fixture::new("hostile-source");

    // Everything the canonical contract lists as credential-bearing or
    // execution-bearing repository control state, in the source.
    ok(
        &fixture.source,
        &[
            "config",
            "credential.helper",
            "!echo password=hunter2; echo",
        ],
    );
    ok(
        &fixture.source,
        &[
            "config",
            "url.https://token:secret@example.invalid/.insteadOf",
            "https://example.invalid/",
        ],
    );
    ok(
        &fixture.source,
        &["config", "core.hooksPath", ".hostile-hooks"],
    );
    ok(
        &fixture.source,
        &["config", "uploadpack.packObjectsHook", "/bin/false"],
    );
    let hooks = fixture.source.join(".git/hooks");
    std::fs::create_dir_all(&hooks).expect("create hooks");
    std::fs::write(hooks.join("post-checkout"), b"#!/bin/sh\nexit 9\n").expect("write hook");

    let base = fixture.resolve();
    let workspace = fixture.materialize(&base);

    for setting in [
        "credential.helper",
        "url.https://token:secret@example.invalid/.insteadOf",
        "core.hooksPath",
        "uploadpack.packObjectsHook",
    ] {
        let output = git(&workspace, &["config", "--get", setting]);
        assert!(
            !output.status.success(),
            "{setting} leaked into the workspace: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    // Not filtered out afterwards — never copied. The workspace's whole
    // configuration is what `git init` wrote plus the commit identity.
    let config = std::fs::read_to_string(workspace.join(".git/config")).expect("read config");
    assert!(
        !config.contains("hunter2"),
        "config carries a credential: {config}"
    );
    assert!(
        !config.contains("secret"),
        "config carries a credential: {config}"
    );
    assert!(
        !config.contains("remote"),
        "config carries a remote: {config}"
    );
    // And no hook came with it. The controller-owned empty template means
    // the directory is either absent or empty; both are "no hooks".
    let entries: Vec<_> = std::fs::read_dir(workspace.join(".git/hooks"))
        .map(|dir| {
            dir.map(|entry| entry.expect("entry").file_name())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(
        entries.is_empty(),
        "workspace hooks are not empty: {entries:?}"
    );
}

#[test]
fn host_git_configuration_reaches_a_repository_created_without_the_sterile_profile() {
    // The substrate fact behind the sterile profile: an ambient
    // `init.templateDir` installs an executable hook into every new
    // repository, and an ambient `credential.helper` is visible from it. This
    // is what the next test proves Pantheon prevents.
    let dir = TempDir::new("ambient-injection");
    let home = dir.0.join("home");
    let template_hooks = home.join("template/hooks");
    std::fs::create_dir_all(&template_hooks).expect("create template");
    std::fs::write(template_hooks.join("post-commit"), b"#!/bin/sh\nexit 0\n").expect("write hook");
    std::fs::write(
        home.join(".gitconfig"),
        format!(
            "[init]\n\ttemplateDir = {}\n[credential]\n\thelper = !echo password=hunter2; echo\n",
            home.join("template").display()
        ),
    )
    .expect("write gitconfig");

    let victim = dir.0.join("victim");
    std::fs::create_dir_all(&victim).expect("create victim");
    let output = Command::new("git")
        .arg("-C")
        .arg(&victim)
        .args(["init", "--quiet"])
        .env("HOME", &home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git");
    assert!(output.status.success());

    assert!(
        victim.join(".git/hooks/post-commit").exists(),
        "ambient init.templateDir must install a hook, or this test proves nothing"
    );
    let helper = Command::new("git")
        .arg("-C")
        .arg(&victim)
        .args(["config", "--get", "credential.helper"])
        .env("HOME", &home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git");
    assert!(
        helper.status.success(),
        "ambient credential.helper must be visible, or this test proves nothing"
    );
}

#[test]
fn ambient_configuration_placed_where_git_would_look_does_not_reach_a_workspace() {
    let fixture = Fixture::new("sterile-home");

    // The sterile profile points HOME and XDG_CONFIG_HOME at a
    // controller-owned directory. Plant exactly the injection the previous
    // test proved works, in that directory, and it still has no effect —
    // because GIT_CONFIG_GLOBAL names a controller-owned empty file and
    // `git init` is given a controller-owned empty template.
    let sterile_home = fixture
        .destination()
        .parent()
        .expect("workspace directory")
        .parent()
        .expect("workspace root")
        .parent()
        .expect("temporary root")
        .join("control/sterile/home");
    assert!(
        sterile_home.is_dir(),
        "expected the controller-owned sterile home at {}",
        sterile_home.display()
    );
    // Two injections at once. `hooks/` is what Git seeds a new repository
    // from when the template directory *is* this directory, and
    // `init.templateDir` is what a global configuration would point at.
    for hooks in ["hooks", "template/hooks"] {
        let dir = sterile_home.join(hooks);
        std::fs::create_dir_all(&dir).expect("create template");
        std::fs::write(dir.join("post-commit"), b"#!/bin/sh\nexit 0\n").expect("write hook");
    }
    std::fs::write(
        sterile_home.join(".gitconfig"),
        format!(
            "[init]\n\ttemplateDir = {}\n[credential]\n\thelper = !echo password=hunter2; echo\n",
            sterile_home.join("template").display()
        ),
    )
    .expect("write gitconfig");

    let base = fixture.resolve();
    let workspace = fixture.materialize(&base);

    assert!(
        !workspace.join(".git/hooks/post-commit").exists(),
        "no template outside the controller-owned empty one may seed workspace hooks"
    );
    // The repository's own configuration is what `git init` wrote plus the
    // commit identity — a global helper left nothing durable behind in it.
    let config = std::fs::read_to_string(workspace.join(".git/config")).expect("read config");
    assert!(
        !config.contains("hunter2"),
        "a global credential helper reached the workspace configuration: {config}"
    );
}

#[test]
fn observation_reports_unknown_rather_than_absent_for_state_that_is_not_a_workspace() {
    let fixture = Fixture::new("observe");
    let base = fixture.resolve();
    let destination = fixture.destination();

    assert_eq!(
        fixture.observe(&base),
        Materialization::Absent,
        "nothing exists yet"
    );

    // Something is there, but it is not this Workspace. Reporting `Absent`
    // would let a caller conclude the path is free.
    std::fs::create_dir_all(&destination).expect("create directory");
    std::fs::write(destination.join("stray"), b"x").expect("write");
    assert_eq!(fixture.observe(&base), Materialization::Unknown);

    // A dangling symlink is also *something*: `symlink_metadata` must not
    // follow it into a NotFound.
    std::fs::remove_dir_all(&destination).expect("remove");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(destination.join("nowhere"), &destination)
            .expect("create dangling symlink");
        assert_eq!(fixture.observe(&base), Materialization::Unknown);
        std::fs::remove_file(&destination).expect("remove symlink");
    }

    assert_eq!(fixture.observe(&base), Materialization::Absent);
}

#[test]
fn discarding_removes_workspace_state_and_is_idempotent() {
    let fixture = Fixture::new("discard");
    let base = fixture.resolve();
    fixture.materialize(&base);
    let destination = fixture.destination();
    let target = MaterializationTarget {
        workspace_id: "workspace-1",
        source: &fixture.source,
        destination: &destination,
        base: &base,
    };

    fixture.materializer.discard(&target).expect("discards");
    assert_eq!(fixture.observe(&base), Materialization::Absent);
    // Discard runs on every retry, including one where nothing survived.
    fixture
        .materializer
        .discard(&target)
        .expect("discarding nothing is not a failure");
}

#[test]
fn a_failed_materialization_leaves_the_source_repository_untouched() {
    let fixture = Fixture::new("failure");
    let refs_before = refs_of(&fixture.source);
    // An object name that is well formed and not in the source.
    let absent = ResolvedBase::parse(&"1".repeat(40)).expect("object name");
    let destination = fixture.destination();

    let error = fixture
        .materializer
        .materialize(&MaterializationTarget {
            workspace_id: "workspace-1",
            source: &fixture.source,
            destination: &destination,
            base: &absent,
        })
        .expect_err("fetching an absent base fails");
    assert_eq!(error.code, "workspace.materialization-failed");

    assert_eq!(
        refs_of(&fixture.source),
        refs_before,
        "a failed materialization must not touch the source"
    );
    // And what is left behind is reported honestly: something exists at the
    // path, so it is Unknown rather than Absent.
    assert_eq!(fixture.observe(&absent), Materialization::Unknown);
}

#[test]
fn an_unresolvable_requested_base_fails_before_anything_is_created() {
    let fixture = Fixture::new("unresolvable");
    let missing = RequestedBase::parse("refs/heads/no-such-branch").expect("valid ref name");

    let error = fixture
        .materializer
        .resolve_base(&fixture.source, &missing)
        .expect_err("an absent ref does not resolve");
    assert_eq!(error.code, "workspace.base-unresolvable");
    assert!(
        !fixture.root.exists(),
        "resolution must not create workspace state"
    );
}

#[test]
fn the_sterile_profile_passes_only_an_allowlisted_environment() {
    // The mission's requirement is an allowlist, not a deny-list: the child
    // environment is cleared and rebuilt, so a credential variable Git or a
    // helper invents tomorrow is excluded without this code being changed.
    let dir = TempDir::new("environment");
    let control = dir.0.join("control");
    std::fs::create_dir_all(&control).expect("create control root");
    let profile = crate::sterile::SterileProfile::create(&control).expect("create profile");
    let command = profile.command();

    let mut names: Vec<String> = command
        .get_envs()
        .map(|(key, value)| {
            assert!(value.is_some(), "{key:?} is removed rather than set");
            key.to_string_lossy().into_owned()
        })
        .collect();
    names.sort();

    let mut expected = vec![
        "GIT_ATTR_NOSYSTEM",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_NOSYSTEM",
        "GIT_CONFIG_SYSTEM",
        "GIT_TERMINAL_PROMPT",
        "HOME",
        "LC_ALL",
        "XDG_CONFIG_HOME",
    ];
    if std::env::var_os("PATH").is_some() {
        expected.push("PATH");
    }
    expected.sort_unstable();
    assert_eq!(names, expected, "the environment allowlist changed");

    // The one thing a deny-list would most likely get wrong.
    let home = command
        .get_envs()
        .find(|(key, _)| *key == "HOME")
        .and_then(|(_, value)| value)
        .expect("HOME is set");
    assert_ne!(
        Some(home),
        std::env::var_os("HOME").as_deref(),
        "the child must not inherit the host user's home directory"
    );
}
