//! Carrying out [`Operation`]s with the git CLI.
//!
//! Git itself performs every mutation, so hooks, signing, credential helpers and ssh behave exactly
//! as they do in a terminal. This module turns an operation into git command lines and git's
//! failure into an [`OpError`] the user can act on. Names from the UI are validated as ref names
//! and always follow `--end-of-options`, so a branch named `--upload-pack=…` can never become an
//! option.

mod git;
mod scrub;

use std::path::Path;

use gix::bstr::{BStr, ByteSlice};

use self::git::{Git, Output};
use super::upstream::current_config;
use crate::askpass::Askpass;
use crate::types::{CheckoutTarget, ForceMode, OpError, OpErrorKind, Operation};

/// Why an operation failed, and git's exit code if it got as far as running git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpFailure {
    pub error: OpError,
    pub exit_code: Option<i32>,
}

impl OpFailure {
    fn before_git(kind: OpErrorKind, message: impl Into<String>) -> OpFailure {
        OpFailure { error: OpError { kind, message: message.into(), output: String::new() }, exit_code: None }
    }
}

pub(super) fn run(
    repo: &gix::Repository,
    root: &Path,
    op: &Operation,
    askpass: Option<&Askpass>,
    progress: &mut dyn FnMut(&str),
) -> Result<(), OpFailure> {
    let writer = Writer { repo, git: Git { root, askpass }, progress };
    writer.run(op)
}

struct Writer<'a> {
    repo: &'a gix::Repository,
    git: Git<'a>,
    progress: &'a mut dyn FnMut(&str),
}

impl Writer<'_> {
    fn run(mut self, op: &Operation) -> Result<(), OpFailure> {
        match op {
            Operation::Checkout { target: CheckoutTarget::Branch { name } } => {
                branch_ref(name)?;
                self.git(&["switch", "--no-guess", "--end-of-options", name])
                    .map_err(|out| checkout_failure(out, &format!("Can't check out {name}")))
            }
            Operation::Checkout { target: CheckoutTarget::Commit { id } } => {
                let id = id.to_string();
                self.git(&["switch", "--detach", "--end-of-options", &id])
                    .map_err(|out| checkout_failure(out, &format!("Can't check out commit {}", &id[..7])))
            }
            Operation::CreateBranch { name, at, checkout, upstream } => {
                branch_ref(name)?;
                if let Some(upstream) = upstream {
                    remote_tracking_ref(upstream)?;
                }
                let at = at.to_string();
                let created = if *checkout {
                    let create = format!("--create={name}");
                    self.git(&["switch", "--no-guess", &create, "--end-of-options", &at])
                        .map_err(|out| checkout_failure(out, &format!("Can't create branch {name}")))
                } else {
                    self.git(&["branch", "--end-of-options", name, &at])
                        .map_err(|out| create_failure(out, &format!("Can't create branch {name}")))
                };
                created?;
                match upstream {
                    Some(upstream) => {
                        let set = format!("--set-upstream-to={upstream}");
                        self.git(&["branch", &set, "--end-of-options", name]).map_err(|out| {
                            failure(out, OpErrorKind::Git, format!("Created {name}, but couldn't make it follow {upstream}"))
                        })
                    }
                    None => Ok(()),
                }
            }
            Operation::DeleteBranch { name, force } => {
                let full_name = branch_ref(name)?;
                if !self.exists(&full_name)? {
                    return Ok(());
                }
                let mut args = vec!["branch", "--delete"];
                if *force {
                    args.push("--force");
                }
                args.extend(["--end-of-options", name]);
                match self.git(&args) {
                    Ok(()) => Ok(()),
                    // Deleted by someone else since the check: the goal is met all the same.
                    Err(_) if !self.exists(&full_name)? => Ok(()),
                    Err(out) if out.stderr().contains("not fully merged") => Err(failure(
                        out,
                        OpErrorKind::NotFullyMerged,
                        format!("{name} has commits that aren't merged anywhere. Delete it anyway to discard them."),
                    )),
                    Err(out) => Err(git_failure(out, &format!("Can't delete branch {name}"))),
                }
            }
            Operation::CreateTag { name, at, message } => {
                tag_ref(name)?;
                let at = at.to_string();
                let message = message.as_deref().filter(|m| !m.trim().is_empty()).map(|m| format!("--message={m}"));
                let mut args = vec!["tag"];
                if let Some(message) = &message {
                    args.extend(["--annotate", message]);
                }
                args.extend(["--end-of-options", name, &at]);
                self.git(&args).map_err(|out| create_failure(out, &format!("Can't create tag {name}")))
            }
            Operation::DeleteTag { name } => {
                let full_name = tag_ref(name)?;
                if !self.exists(&full_name)? {
                    return Ok(());
                }
                match self.git(&["tag", "--delete", "--end-of-options", name]) {
                    Ok(()) => Ok(()),
                    Err(_) if !self.exists(&full_name)? => Ok(()),
                    Err(out) => Err(git_failure(out, &format!("Can't delete tag {name}"))),
                }
            }
            Operation::Fetch { remote, prune } => {
                let mut args = vec!["fetch", "--progress"];
                if *prune {
                    args.push("--prune");
                }
                match remote {
                    Some(remote) => {
                        self.configured_remote(remote)?;
                        args.extend(["--end-of-options", remote]);
                    }
                    None => args.push("--all"),
                }
                let from = remote.as_deref().unwrap_or("the remotes");
                self.git(&args).map_err(|out| network_failure(out, &format!("Can't fetch from {from}")))
            }
            Operation::Pull => self.git(&["pull", "--no-rebase", "--ff-only", "--progress"]).map_err(pull_failure),
            Operation::Push { branch, remote, force, set_upstream } => {
                let full_name = branch_ref(branch)?;
                let remote = match remote {
                    Some(remote) => {
                        self.configured_remote(remote)?;
                        remote.clone()
                    }
                    None => self.push_remote(branch)?,
                };
                let mut args = vec!["push".to_owned(), "--porcelain".to_owned(), "--progress".to_owned()];
                if *set_upstream {
                    args.push("--set-upstream".to_owned());
                }
                if let ForceMode::WithLease { expected } = force {
                    let expected = expected.map(|id| id.to_string()).unwrap_or_default();
                    args.push(format!("--force-with-lease={full_name}:{expected}"));
                }
                args.extend(["--end-of-options".to_owned(), remote.clone(), format!("{full_name}:{full_name}")]);
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                self.git(&args).map_err(|out| push_failure(out, branch, &remote))
            }
        }
    }

    /// Runs git; `Err` carries the output of a command that failed.
    fn git(&mut self, args: &[&str]) -> Result<(), GitFailed> {
        match self.git.run(args, self.progress) {
            Ok(output) if output.success() => Ok(()),
            Ok(output) => Err(GitFailed::Ran(output)),
            Err(err) => Err(GitFailed::NotStarted(err)),
        }
    }

    fn exists(&self, full_name: &str) -> Result<bool, OpFailure> {
        self.repo
            .try_find_reference(full_name)
            .map(|found| found.is_some())
            .map_err(|err| OpFailure::before_git(OpErrorKind::Git, format!("Can't read {full_name}: {err}")))
    }

    /// Checks that `remote` names a configured remote, as the configuration is now.
    fn configured_remote(&self, remote: &str) -> Result<(), OpFailure> {
        if self.remotes()?.iter().any(|name| name == remote) {
            Ok(())
        } else {
            Err(OpFailure::before_git(OpErrorKind::InvalidInput, format!("There is no remote named {remote:?}.")))
        }
    }

    /// Where `git push` would push `branch`: its push remote, the default push remote, the remote
    /// it follows, or else the only remote or one named `origin`.
    fn push_remote(&self, branch: &str) -> Result<String, OpFailure> {
        let config = current_config(self.repo).map_err(|err| OpFailure::before_git(OpErrorKind::Git, err.to_string()))?;
        let branch_key = |key: &str| config.string_by("branch", Some(branch.as_bytes().as_bstr()), key);
        let configured = branch_key("pushRemote")
            .or_else(|| config.string_by("remote", None, "pushDefault"))
            .or_else(|| branch_key("remote").filter(|remote| remote.as_slice() != b"."));
        if let Some(remote) = configured {
            return Ok(remote.to_str_lossy().into_owned());
        }
        let remotes = self.remotes()?;
        match remotes.as_slice() {
            [only] => Ok(only.clone()),
            _ if remotes.iter().any(|name| name == "origin") => Ok("origin".to_owned()),
            [] => Err(OpFailure::before_git(OpErrorKind::InvalidInput, "This repository has no remote to push to.")),
            _ => Err(OpFailure::before_git(
                OpErrorKind::InvalidInput,
                format!("{branch} has no upstream, and there are several remotes; choose one to push to."),
            )),
        }
    }

    fn remotes(&self) -> Result<Vec<String>, OpFailure> {
        let config = current_config(self.repo).map_err(|err| OpFailure::before_git(OpErrorKind::Git, err.to_string()))?;
        let mut names: Vec<String> = config
            .sections_by_name("remote")
            .into_iter()
            .flatten()
            .filter_map(|section| section.header().subsection_name().map(|name| name.to_str_lossy().into_owned()))
            .collect();
        names.dedup();
        Ok(names)
    }
}

enum GitFailed {
    NotStarted(std::io::Error),
    Ran(Output),
}

impl GitFailed {
    /// Git's messages; empty if it didn't start.
    fn stderr(&self) -> &str {
        match self {
            GitFailed::Ran(output) => &output.stderr,
            GitFailed::NotStarted(_) => "",
        }
    }

    fn stdout(&self) -> &str {
        match self {
            GitFailed::Ran(output) => &output.stdout,
            GitFailed::NotStarted(_) => "",
        }
    }
}

/// An [`OpFailure`] of `kind` with `message`, carrying git's output.
fn failure(failed: GitFailed, kind: OpErrorKind, message: String) -> OpFailure {
    match failed {
        GitFailed::NotStarted(err) => OpFailure::before_git(
            OpErrorKind::GitNotFound,
            format!("Can't run git: {err}. Install git and make sure it is on the PATH."),
        ),
        GitFailed::Ran(output) => {
            OpFailure { error: OpError { kind, message, output: output.text() }, exit_code: output.code }
        }
    }
}

/// A failure nothing more specific explains: `context` plus git's own summary of the problem.
fn git_failure(failed: GitFailed, context: &str) -> OpFailure {
    let message = match git_summary(failed.stderr()) {
        Some(summary) => format!("{context}: {summary}"),
        None => format!("{context}."),
    };
    failure(failed, OpErrorKind::Git, message)
}

fn checkout_failure(failed: GitFailed, context: &str) -> OpFailure {
    let stderr = failed.stderr();
    if stderr.contains("would be overwritten") || stderr.contains("commit your changes or stash them") {
        let message = format!("{context}: uncommitted changes would be overwritten. Commit or stash them first.");
        return failure(failed, OpErrorKind::LocalChanges, message);
    }
    create_failure(failed, context)
}

fn create_failure(failed: GitFailed, context: &str) -> OpFailure {
    if failed.stderr().contains("already exists") {
        return failure(failed, OpErrorKind::AlreadyExists, format!("{context}: the name is already taken."));
    }
    git_failure(failed, context)
}

fn network_failure(failed: GitFailed, context: &str) -> OpFailure {
    let text = format!("{}\n{}", failed.stderr(), failed.stdout());
    const AUTH: [&str; 7] = [
        "Authentication failed",
        "could not read Username",
        "could not read Password",
        "terminal prompts disabled",
        "Permission denied (publickey",
        "Access denied",
        "HTTP Basic: Access denied",
    ];
    if AUTH.iter().any(|pattern| text.contains(pattern)) {
        let message = format!("{context}: the remote didn't accept the credentials, or none were given.");
        return failure(failed, OpErrorKind::AuthFailed, message);
    }
    git_failure(failed, context)
}

fn pull_failure(failed: GitFailed) -> OpFailure {
    let stderr = failed.stderr();
    let context = "Can't pull";
    if stderr.contains("Not possible to fast-forward") {
        let message = "The branch and its upstream have diverged, and Pull only fast-forwards. \
                       Merge or rebase to combine them.";
        return failure(failed, OpErrorKind::Rejected, message.to_owned());
    }
    if stderr.contains("no tracking information") {
        return failure(failed, OpErrorKind::InvalidInput, "The current branch has no upstream to pull from.".to_owned());
    }
    if stderr.contains("not currently on a branch") {
        return failure(failed, OpErrorKind::InvalidInput, "HEAD is detached; check out a branch to pull.".to_owned());
    }
    if stderr.contains("would be overwritten") {
        let message = "Can't pull: uncommitted changes would be overwritten. Commit or stash them first.";
        return failure(failed, OpErrorKind::LocalChanges, message.to_owned());
    }
    network_failure(failed, context)
}

/// Classifies a failed push from its porcelain status line (`!\t<src>:<dst>\t[rejected] (reason)`).
fn push_failure(failed: GitFailed, branch: &str, remote: &str) -> OpFailure {
    let status = failed.stdout().lines().find(|line| line.starts_with('!')).unwrap_or_default().to_owned();
    if status.contains("(stale info)") {
        let message = format!(
            "{branch} on {remote} has moved since you last saw it, so it wasn't replaced. \
             Fetch to see the new commits, then decide."
        );
        return failure(failed, OpErrorKind::StaleLease, message);
    }
    if status.contains("[remote rejected]") {
        let reason = status.rsplit_once("[remote rejected]").map(|(_, reason)| reason.trim()).unwrap_or_default();
        let message = format!("{remote} refused the push of {branch} {reason}").trim_end().to_owned() + ".";
        return failure(failed, OpErrorKind::Rejected, message);
    }
    if status.contains("[rejected]") {
        let message = format!(
            "{remote} has commits on {branch} that you don't have. Pull or fetch and integrate them first, \
             or force-push with lease to replace them."
        );
        return failure(failed, OpErrorKind::Rejected, message);
    }
    network_failure(failed, &format!("Can't push {branch} to {remote}"))
}

/// The line of git's messages that best says what went wrong: the first `fatal:` or `error:`
/// line, without the prefix.
fn git_summary(stderr: &str) -> Option<&str> {
    stderr
        .lines()
        .find_map(|line| line.strip_prefix("fatal: ").or_else(|| line.strip_prefix("error: ")))
        .or_else(|| stderr.lines().rev().find(|line| !line.trim().is_empty()))
        .map(|line| line.trim().trim_end_matches('.'))
}

/// `refs/heads/<name>`, if `name` is a valid branch name.
fn branch_ref(name: &str) -> Result<String, OpFailure> {
    let full_name = format!("refs/heads/{name}");
    // `git branch` refuses names starting with a dash even where the ref format allows them.
    if name.starts_with('-') || gix::validate::reference::branch_name(BStr::new(&full_name)).is_err() || name == "HEAD" {
        return Err(invalid_name("branch", name));
    }
    Ok(full_name)
}

/// `refs/tags/<name>`, if `name` is a valid tag name.
fn tag_ref(name: &str) -> Result<String, OpFailure> {
    let full_name = format!("refs/tags/{name}");
    if name.starts_with('-') || gix::validate::reference::name(BStr::new(&full_name)).is_err() {
        return Err(invalid_name("tag", name));
    }
    Ok(full_name)
}

/// Checks that `full_name` is a valid remote-tracking branch name, `refs/remotes/<remote>/<branch>`.
fn remote_tracking_ref(full_name: &str) -> Result<(), OpFailure> {
    let valid = full_name.strip_prefix("refs/remotes/").is_some_and(|rest| rest.contains('/'))
        && gix::validate::reference::name(BStr::new(full_name)).is_ok();
    if valid {
        Ok(())
    } else {
        Err(OpFailure::before_git(OpErrorKind::InvalidInput, format!("{full_name:?} isn't a remote-tracking branch.")))
    }
}

fn invalid_name(what: &str, name: &str) -> OpFailure {
    OpFailure::before_git(
        OpErrorKind::InvalidInput,
        format!(
            "{name:?} isn't a valid {what} name. Names can't start with '-' or '.', or contain spaces, '..', \
             '~', '^', ':', '?', '*', '[' or '\\'."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_are_validated() {
        for valid in ["main", "feature/login", "fix-1.2", "user/ada/x"] {
            assert_eq!(branch_ref(valid).unwrap(), format!("refs/heads/{valid}"), "{valid}");
        }
        for invalid in ["", "-x", "--upload-pack=evil", "a..b", "a b", "x.lock", "HEAD", "a:b", "@{u}", "/x", "x/"] {
            let err = branch_ref(invalid).unwrap_err();
            assert_eq!(err.error.kind, OpErrorKind::InvalidInput, "{invalid:?}");
            assert_eq!(err.exit_code, None);
        }
    }

    #[test]
    fn remote_tracking_names_are_validated() {
        assert!(remote_tracking_ref("refs/remotes/origin/main").is_ok());
        assert!(remote_tracking_ref("refs/heads/main").is_err());
        assert!(remote_tracking_ref("refs/remotes/origin").is_err());
        assert!(remote_tracking_ref("refs/remotes/origin/a..b").is_err());
    }

    #[test]
    fn summary_prefers_git_s_error_line() {
        assert_eq!(git_summary("hint: x\nfatal: invalid reference: nosuch\n"), Some("invalid reference: nosuch"));
        assert_eq!(git_summary("something odd happened.\n"), Some("something odd happened"));
        assert_eq!(git_summary(""), None);
    }
}
