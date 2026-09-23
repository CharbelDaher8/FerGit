//! The exact value of every ref, as recorded before and after an operation.

use std::collections::BTreeMap;

use gix::refs::TargetRef;

use super::{RepoError, git_error, lossy, to_oid};
use crate::types::Oid;

/// What HEAD and every ref point to, unpeeled: an annotated tag's value is the tag object, not the
/// commit, so restoring a recorded value restores the ref exactly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefValues {
    /// `ref: refs/heads/main` when HEAD names a branch (born or not), else the detached commit id.
    pub head: String,
    /// Direct refs by full name. Symbolic refs other than HEAD (`refs/remotes/origin/HEAD`) are left
    /// out: they follow other refs and are never the target of an operation.
    pub refs: BTreeMap<String, Oid>,
}

pub(super) fn read(repo: &gix::Repository) -> Result<RefValues, RepoError> {
    let context = "Can't list the repository's refs";
    let head = match repo
        .find_reference("HEAD")
        .map_err(|err| git_error("Can't read HEAD", err))?
        .target()
    {
        TargetRef::Symbolic(name) => format!("ref: {}", lossy(name.as_bstr())),
        TargetRef::Object(id) => to_oid(id).to_string(),
    };
    let mut refs = BTreeMap::new();
    let platform = repo.references().map_err(|err| git_error(context, err))?;
    for reference in platform.all().map_err(|err| git_error(context, err))? {
        let Ok(reference) = reference else {
            continue;
        };
        if let TargetRef::Object(id) = reference.target() {
            refs.insert(lossy(reference.name().as_bstr()), to_oid(id));
        }
    }
    Ok(RefValues { head, refs })
}
