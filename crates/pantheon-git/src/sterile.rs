//! The sterile, non-interactive execution profile every Git process runs
//! under.
//!
//! `docs/architecture/artifacts-and-workspaces/workspace-and-git-integration.md`
//! is explicit that this profile is **defense in depth, not the security
//! boundary** — Pantheon must stay safe if Git grows another
//! repository-configurable execution mechanism. The boundary is structural:
//! the Workspace is an independent repository with no remote, no alternate
//! object store and no shared common directory. This module removes the
//! ambient authority a Git process would otherwise inherit anyway.
//!
//! # An allowlist, not a deny-list
//!
//! The child environment is *cleared* and then rebuilt from the few variables
//! Git needs. Removing known-dangerous names — `SSH_AUTH_SOCK`, `GIT_ASKPASS`,
//! `GIT_CONFIG_COUNT`, `GIT_ALTERNATE_OBJECT_DIRECTORIES` and the rest — would
//! be a list that is correct until Git or a credential helper adds one more
//! name. Clearing is correct for every name that exists and every name that
//! will.
//!
//! `PATH` is the single inherited variable, because locating the `git`
//! executable requires it. It comes from the daemon's own process
//! environment, which is operator-controlled and never Agent-writable.
//!
//! # What each setting removes
//!
//! | Setting | Removes |
//! |---|---|
//! | cleared environment | every ambient credential agent, config override and Git directory override |
//! | `HOME`/`XDG_CONFIG_HOME` → controller-owned empty directory | the host user's `~/.gitconfig`, `~/.ssh` and credential stores |
//! | `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` → controller-owned empty file | global and system configuration, including `init.templateDir` |
//! | `GIT_CONFIG_NOSYSTEM` | system configuration on Git builds that predate the variables above |
//! | `GIT_ATTR_NOSYSTEM` | the system `gitattributes` file |
//! | `GIT_TERMINAL_PROMPT=0`, null stdin | interactive credential prompting |
//! | `core.hooksPath` → controller-owned empty directory | every hook, whatever the repository or a template says |
//! | `--template=` → controller-owned empty directory | template-seeded hooks in a newly created repository |
//!
//! `HOME` and `GIT_CONFIG_GLOBAL` are both set on purpose rather than either
//! one alone: the config variables are the precise mechanism, and an empty
//! `HOME` also removes `~/.ssh` and platform credential stores that config
//! variables say nothing about.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Controller-owned scratch state the sterile profile points Git at.
///
/// Created once, owned by Pantheon, and never derived from repository content
/// or from anything an Agent can write.
#[derive(Debug, Clone)]
pub(crate) struct SterileProfile {
    /// An empty directory used as `HOME`.
    home: PathBuf,
    /// An empty directory used as the hooks path and as the `git init`
    /// template.
    empty: PathBuf,
    /// An empty file used as both global and system configuration.
    empty_config: PathBuf,
}

impl SterileProfile {
    /// Creates the controller-owned scratch state beneath `control_root`.
    ///
    /// # Errors
    ///
    /// Any I/O failure creating the directories or the empty configuration
    /// file.
    pub(crate) fn create(control_root: &Path) -> io::Result<Self> {
        let base = control_root.join("sterile");
        let home = base.join("home");
        let empty = base.join("empty");
        let empty_config = base.join("empty.gitconfig");

        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(&empty)?;
        // Truncating rather than create-new keeps this idempotent across
        // daemon restarts, and an empty file is what makes it a *defined*
        // empty configuration rather than a path Git might interpret.
        std::fs::write(&empty_config, b"")?;

        Ok(Self {
            home,
            empty,
            empty_config,
        })
    }

    /// The empty directory used to seed a new repository, so a template
    /// configured anywhere else cannot install hooks into it.
    pub(crate) fn template(&self) -> &Path {
        &self.empty
    }

    /// A `git` invocation with no ambient authority.
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new("git");
        command.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        command
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &self.empty_config)
            .env("GIT_CONFIG_SYSTEM", &self.empty_config)
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            // No terminal to prompt on, whatever a helper decides to try.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.args([
            "-c",
            &format!("core.hooksPath={}", self.empty.display()),
            // Command-line configuration is "protected" configuration to Git,
            // so these override anything a repository sets.
            "-c",
            "advice.detachedHead=false",
            "-c",
            "init.defaultBranch=main",
        ]);
        command
    }

    /// Runs `git` with `args`, returning its output whether or not it
    /// succeeded.
    ///
    /// # Errors
    ///
    /// Only when the process could not be started or waited on.
    pub(crate) fn run<I, S>(&self, args: I) -> io::Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        self.command().args(args).output()
    }
}
