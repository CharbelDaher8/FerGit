//! Which ref each local branch follows: its upstream, as `git status` and `@{upstream}` see it.
//!
//! `branch.<name>.remote` and `branch.<name>.merge` name a branch on a remote; the remote's fetch
//! refspecs say which local ref tracks it (`+refs/heads/*:refs/remotes/origin/*` maps
//! `refs/heads/main` to `refs/remotes/origin/main`, but a remote can map anywhere). With
//! `remote = .` the upstream is the local branch `merge` names.
//!
//! # Fresh configuration
//!
//! The repository's configuration is loaded once, when it's opened, but `git branch
//! --set-upstream-to` changes only `.git/config`. So every read parses the repository's own
//! configuration file again (and `config.worktree` where enabled) and lays it over the system and
//! user configuration loaded at open. Parsing is all this does: nothing is executed, and the parsed
//! file is used for these lookups only, never handed to the repository, so the filter and diff
//! drivers it may define stay out of reach. `include.path` directives in the repository's file
//! aren't followed for these lookups.

use gix::bstr::{BStr, BString, ByteSlice};

use super::{BranchUpstream, RepoError, git_error, lossy, to_oid};
use crate::types::{Oid, RefKind, RefLabel};

/// The upstream of every local branch in `refs` that has one configured, sorted by branch.
pub(super) fn read(repo: &gix::Repository, refs: &[(Oid, RefLabel)]) -> Result<Vec<BranchUpstream>, RepoError> {
    let config = current_config(repo)?;
    let mut upstreams = Vec::new();
    for (_, branch) in refs.iter().filter(|(_, label)| label.kind == RefKind::LocalBranch) {
        let Some(full_name) = upstream_ref(&config, branch.name.as_bytes().as_bstr()) else {
            continue;
        };
        let Ok(full_name) = gix::refs::FullName::try_from(full_name) else {
            continue;
        };
        let full_name_str = lossy(full_name.as_bstr());
        let id = match refs.iter().find(|(_, label)| label.full_name == full_name_str) {
            Some((id, _)) => Some(*id),
            // An upstream outside the listed refs (a refspec can map anywhere) is looked up directly.
            None => repo
                .try_find_reference(full_name.as_ref())
                .map_err(|err| git_error(format!("Can't read {full_name_str}"), err))?
                .and_then(|mut reference| reference.peel_to_id().ok())
                .map(|id| to_oid(&id)),
        };
        upstreams.push(BranchUpstream {
            branch: branch.full_name.clone(),
            name: lossy(full_name.as_ref().shorten()),
            full_name: full_name_str,
            id,
        });
    }
    upstreams.sort_by(|a, b| a.branch.cmp(&b.branch));
    Ok(upstreams)
}

/// The full name of the ref tracking `branch`'s upstream, whether or not it exists.
fn upstream_ref(config: &gix::config::File, branch: &BStr) -> Option<BString> {
    let merge = config.string_by("branch", Some(branch), "merge")?;
    let remote = config.string_by("branch", Some(branch), "remote")?;
    let merge: BString = if merge.starts_with(b"refs/") {
        merge
    } else {
        format!("refs/heads/{merge}").into()
    };
    if remote == "." {
        return Some(merge);
    }
    // A remote given as a URL has no fetch refspecs, hence no tracking ref.
    let specs = config.strings_by("remote", Some(remote.as_bstr()), "fetch")?;
    let specs = specs
        .iter()
        .filter_map(|spec| gix::refspec::parse(spec.as_bstr(), gix::refspec::parse::Operation::Fetch).ok())
        .filter(|spec| spec.source().is_some() && spec.destination().is_some());
    let null = gix::ObjectId::null(gix::hash::Kind::Sha1);
    let item = gix::refspec::match_group::Item { full_ref_name: merge.as_bstr(), target: &null, object: None };
    // Like git, the first matching refspec decides.
    gix::refspec::MatchGroup::from_fetch_specs(specs)
        .match_lhs(std::iter::once(item))
        .mappings
        .into_iter()
        .find_map(|mapping| mapping.rhs.map(|name| name.into_owned()))
}

/// The configuration loaded at open, with the repository's own files re-read from disk. Falls back
/// to the configuration as loaded at open if the files can't be read or parsed right now (git may
/// be halfway through rewriting them).
fn current_config(repo: &gix::Repository) -> Result<gix::config::File, RepoError> {
    use gix::config::Source;

    let opened = repo.config_snapshot().plumbing().clone();
    let local_path = repo.common_dir().join("config");
    let Ok(local) = gix::config::File::from_path_no_includes(local_path, Source::Local) else {
        return Ok(opened);
    };
    let worktree = if local.boolean("extensions.worktreeConfig").ok().flatten().unwrap_or(false) {
        let path = repo.git_dir().join("config.worktree");
        match path.exists() {
            true => match gix::config::File::from_path_no_includes(path, Source::Worktree) {
                Ok(file) => Some(file),
                Err(_) => return Ok(opened),
            },
            false => None,
        }
    } else {
        None
    };

    let mut config = opened;
    let stale: Vec<_> = config
        .sections_and_ids()
        .filter(|(section, _)| matches!(section.meta().source, Source::Local | Source::Worktree))
        .map(|(_, id)| id)
        .collect();
    for id in stale {
        config.remove_section_by_id(id);
    }
    for file in std::iter::once(local).chain(worktree) {
        config
            .append(file)
            .map_err(|err| git_error("Can't combine the repository's configuration", err))?;
    }
    Ok(config)
}
