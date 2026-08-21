//! Materializing one Task Workspace as an independent Git repository.
//!
//! # The strategy, and why it is this one
//!
//! ```text
//! git init  <destination>        an empty, controller-created repository
//! git fetch <source> <base OID>  exactly the history behind one commit
//! git update-ref refs/pantheon/base <base OID>
//! git checkout --detach <base OID>
//! ```
//!
//! Four properties follow from those four commands, and each is one the
//! canonical Workspace contract asks for:
//!
//! 1. **No shared Git authority.** `git init` creates a repository whose
//!    object database, ref store and configuration are its own. Nothing links
//!    it to the source: there is no `objects/info/alternates`, no `commondir`
//!    indirection and no gitfile. A linked worktree would share all three,
//!    which is why the contract refuses one as a boundary for an untrusted
//!    shell — and why the `pantheon-git` test suite proves the contrast
//!    against a real worktree rather than asserting it.
//!
//! 2. **No remote, ever.** A remote is not configured and then removed; it is
//!    never created. `git clone` would create `origin` pointing at the source,
//!    and pushing to a non-current branch of a local non-bare repository
//!    succeeds — so a worker could move the source repository's refs. Fetching
//!    into a repository that has no remote leaves nothing to push to and
//!    nothing to carry a credential.
//!
//! 3. **The base cannot silently move.** The fetch names the immutable object
//!    identity, not the ref it came from. If the source's `main` advances
//!    between resolution and materialization, this fetch is unaffected: it
//!    asks for a commit, and a commit does not move. Fetching the *ref* and
//!    checking afterwards would leave a window where the two disagreed.
//!
//! 4. **Nothing from the source's control state is inherited.** The
//!    repository's configuration is created by `git init` under the sterile
//!    profile, so the source's credential helpers, `insteadOf` rewrites,
//!    hooks, template directory and `gitattributes` filters are simply not
//!    present in it. They are not filtered out; they were never copied.
//!
//! The cost is a full object copy rather than the hardlinks `git clone
//! --local` uses. That is the intended trade: hardlinked object files are one
//! inode shared between a trusted repository and a Workspace an Agent
//! controls, and this mission is where that link is refused.
//!
//! # What is still trusted
//!
//! Resolving the requested base runs Git *in the source repository*, so the
//! source repository's own configuration applies to that one read-only
//! command. That is the correct boundary for this mission: the source is
//! operator-supplied input at materialization time, not Agent-writable state.
//! The property #27 establishes is the other direction — that none of the
//! source's control state becomes worker authority. Repository state that an
//! Agent *has* written is untrusted input under the canonical
//! hostile-repository rule, and controller operations against it belong to
//! the missions that perform them.

use std::io;
use std::path::Path;
use std::process::Output;

use pantheon_core::workspace::{Materialization, RequestedBase, ResolvedBase};
use pantheon_engine::workspace::{
    MaterializationTarget, MaterializerError, RepositoryMaterializer,
};

use crate::sterile::SterileProfile;

/// The Pantheon-owned ref that pins the immutable base inside a Workspace.
///
/// The canonical contract permits controller-owned refs under a Pantheon
/// namespace to pin objects. This one keeps the base commit reachable so an
/// ordinary `git gc` inside the Workspace cannot prune the history the
/// Workspace is bound to. It is retention, not identity: a worker may delete
/// it, and doing so changes nothing Pantheon believes.
pub const PANTHEON_BASE_REF: &str = "refs/pantheon/base";

/// The identity a Workspace repository records for worker commits.
///
/// The sterile profile removes the host user's Git identity along with
/// everything else in `HOME`, and a repository where `git commit` refuses to
/// run is not "writable repository state suitable for arbitrary coding work".
/// So the Workspace gets its own, repository-local. `.invalid` is the
/// reserved never-resolvable TLD from RFC 2606, so the address cannot become
/// a real one.
const WORKSPACE_IDENTITY_NAME: &str = "Pantheon Task Workspace";
const WORKSPACE_IDENTITY_EMAIL: &str = "workspace@pantheon.invalid";

/// Fail-closed codes this materializer reports.
mod code {
    /// The requested base does not resolve to a commit in the source.
    pub(super) const UNRESOLVABLE: &str = "workspace.base-unresolvable";
    /// Materialization could not be completed.
    pub(super) const FAILED: &str = "workspace.materialization-failed";
    /// The canonical code for a repository boundary that cannot be
    /// established. Used when a materialized Workspace turns out to share Git
    /// authority with something else, which must never be handed to a worker.
    pub(super) const HOSTILE: &str = "workspace.hostile-repository-state";
}

/// Materializes Task Workspaces as independent local Git repositories.
#[derive(Debug, Clone)]
pub struct GitMaterializer {
    sterile: SterileProfile,
}

impl GitMaterializer {
    /// Creates the controller-owned state the sterile profile needs beneath
    /// `control_root`.
    ///
    /// # Errors
    ///
    /// Any I/O failure creating that state.
    pub fn new(control_root: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self {
            sterile: SterileProfile::create(control_root.as_ref())?,
        })
    }

    /// Runs one Git command, turning a non-zero exit into a typed failure.
    fn git<I, S>(&self, code: &str, what: &str, args: I) -> Result<String, MaterializerError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = self.sterile.run(args).map_err(|err| MaterializerError {
            code: code.to_string(),
            detail: format!("could not run git while {what}: {err}"),
        })?;
        if output.status.success() {
            Ok(stdout(&output))
        } else {
            Err(MaterializerError {
                code: code.to_string(),
                detail: format!(
                    "git failed while {what}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            })
        }
    }

    /// Whether a Git query succeeds, without treating failure as an error.
    fn probe<I, S>(&self, args: I) -> Option<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = self.sterile.run(args).ok()?;
        output
            .status
            .success()
            .then(|| stdout(&output).trim().to_string())
    }

    /// Refuses a materialized repository that shares Git authority with
    /// anything else.
    ///
    /// This runs against the repository *Pantheon just created*, before any
    /// worker has touched it, so it is a check that the strategy did what it
    /// claims — not an inspection of hostile state. Each condition is a
    /// distinct way authority could leak, and any of them fails closed.
    fn verify_isolation(&self, destination: &Path) -> Result<(), MaterializerError> {
        let refuse = |detail: String| {
            Err(MaterializerError {
                code: code::HOSTILE.to_string(),
                detail,
            })
        };
        let dest = destination.display().to_string();

        // Where Git itself says this repository's control state lives. A
        // linked worktree answers with its own git dir and the *source's*
        // common directory, which is exactly the shared ref store and object
        // store an untrusted shell may not have. Asking Git rather than
        // looking for a `.git` file also covers indirections this code did
        // not anticipate: whatever route produced them, the two answers
        // disagree.
        let common = self
            .probe([
                "-C",
                &dest,
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
            ])
            .unwrap_or_default();
        let git_dir = self
            .probe([
                "-C",
                &dest,
                "rev-parse",
                "--path-format=absolute",
                "--git-dir",
            ])
            .unwrap_or_default();
        if common.is_empty() || common != git_dir {
            return refuse(format!(
                "{dest} has git dir {git_dir:?} and common dir {common:?}, which are not the same repository"
            ));
        }
        // A borrowed object database: the source's objects would be reachable
        // through this Workspace, and pruning either side would affect both.
        if destination
            .join(".git/objects/info/alternates")
            .symlink_metadata()
            .is_ok()
        {
            return refuse(format!("{dest} borrows an alternate object database"));
        }
        // Nothing to fetch from, nothing to push to, nothing to hold a
        // credential.
        let remotes = self
            .probe(["-C", &dest, "remote"])
            .ok_or_else(|| MaterializerError {
                code: code::HOSTILE.to_string(),
                detail: format!("could not list remotes of {dest}"),
            })?;
        if !remotes.trim().is_empty() {
            return refuse(format!("{dest} has configured remotes: {remotes:?}"));
        }
        Ok(())
    }
}

impl RepositoryMaterializer for GitMaterializer {
    fn resolve_base(
        &self,
        source: &Path,
        requested: &RequestedBase,
    ) -> Result<ResolvedBase, MaterializerError> {
        let source = source.display().to_string();
        // `^{commit}` makes an annotated tag resolve to the commit it points
        // at rather than to the tag object, so what is recorded is always the
        // commit identity the Workspace will be checked out at.
        //
        // `--end-of-options` is belt to `RequestedBase`'s braces: the value
        // has already been refused if it starts with `-`, and this makes even
        // a future parsing change unable to read it as an option.
        let revision = format!("{}^{{commit}}", requested.as_str());
        let resolved = self.git(
            code::UNRESOLVABLE,
            "resolving the requested base",
            [
                "-C",
                &source,
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &revision,
            ],
        )?;

        ResolvedBase::parse(resolved.trim()).map_err(|err| MaterializerError {
            code: code::UNRESOLVABLE.to_string(),
            detail: format!("{} did not resolve to a usable commit: {err}", requested),
        })
    }

    fn materialize(
        &self,
        target: &MaterializationTarget<'_>,
    ) -> Result<ResolvedBase, MaterializerError> {
        let destination = target.destination.display().to_string();
        let source = target.source.display().to_string();
        let base = target.base.as_str();

        if let Some(parent) = target.destination.parent() {
            std::fs::create_dir_all(parent).map_err(|err| MaterializerError {
                code: code::FAILED.to_string(),
                detail: format!("could not create the workspace directory: {err}"),
            })?;
        }

        // An empty repository, seeded from a controller-owned empty template
        // so no `init.templateDir` anywhere can install hooks into it.
        self.git(
            code::FAILED,
            "creating the workspace repository",
            [
                "init",
                "--quiet",
                "--template",
                &self.sterile.template().display().to_string(),
                &destination,
            ],
        )?;

        // Exactly the history behind one immutable commit. No remote is
        // created, and no ref name is involved, so the source's refs moving
        // afterwards cannot change what this Workspace contains.
        self.git(
            code::FAILED,
            "fetching the immutable base",
            [
                "-C",
                &destination,
                "fetch",
                "--quiet",
                "--no-tags",
                "--end-of-options",
                &source,
                base,
            ],
        )?;

        // Pin it so an ordinary `git gc` inside the Workspace cannot prune
        // the base the Workspace is bound to.
        self.git(
            code::FAILED,
            "pinning the base",
            ["-C", &destination, "update-ref", PANTHEON_BASE_REF, base],
        )?;

        // Detached, deliberately: the Workspace starts at an object identity,
        // and no branch name is created that anything could later mistake for
        // Candidate identity.
        self.git(
            code::FAILED,
            "checking out the base",
            ["-C", &destination, "checkout", "--quiet", "--detach", base],
        )?;

        for (key, value) in [
            ("user.name", WORKSPACE_IDENTITY_NAME),
            ("user.email", WORKSPACE_IDENTITY_EMAIL),
        ] {
            self.git(
                code::FAILED,
                "setting the workspace commit identity",
                ["-C", &destination, "config", key, value],
            )?;
        }

        self.verify_isolation(target.destination)?;

        // What is actually at HEAD, read back rather than assumed. The
        // controller compares this against the durable binding before the
        // Workspace can become Ready.
        let head = self.git(
            code::FAILED,
            "verifying the checked-out base",
            ["-C", &destination, "rev-parse", "--verify", "HEAD"],
        )?;
        ResolvedBase::parse(head.trim()).map_err(|err| MaterializerError {
            code: code::FAILED.to_string(),
            detail: format!("the workspace HEAD is not a usable object name: {err}"),
        })
    }

    fn observe(
        &self,
        target: &MaterializationTarget<'_>,
    ) -> Result<Materialization, MaterializerError> {
        // `symlink_metadata` does not follow: a dangling symlink at the
        // Workspace path is *something*, and reporting it Absent would let a
        // later step conclude the path is free.
        match target.destination.symlink_metadata() {
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Materialization::Absent);
            }
            // The path could not be inspected. That is not evidence of
            // absence, and saying so is the whole point of this dimension.
            Err(_) => return Ok(Materialization::Unknown),
        }

        let destination = target.destination.display().to_string();
        let head = self.probe(["-C", &destination, "rev-parse", "--verify", "HEAD"]);
        let has_base = self
            .probe(["-C", &destination, "cat-file", "-e", target.base.as_str()])
            .is_some();
        let isolated = self.verify_isolation(target.destination).is_ok();

        Ok(if head.is_some() && has_base && isolated {
            Materialization::Present
        } else {
            // Something exists at the path but is not the Workspace this
            // record describes. Never `Absent`.
            Materialization::Unknown
        })
    }

    fn discard(&self, target: &MaterializationTarget<'_>) -> Result<(), MaterializerError> {
        match std::fs::remove_dir_all(target.destination) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(MaterializerError {
                code: code::FAILED.to_string(),
                detail: format!("could not discard {}: {err}", target.destination.display()),
            }),
        }
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}
