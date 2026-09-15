//! Naming the graph's lines after branches, and finding where lines split off and join.
//!
//! Git doesn't record which branch a commit was made on, so a line's name is inferred as it is
//! followed down the rows, from refs and merge messages. The rules, for commit rows:
//!
//! 1. **The node's line.** Lines coming down into a node end there; the leftmost of them that isn't
//!    a stash or uncommitted-changes line is the trunk, and the node is on the trunk's branch.
//!    - A named trunk gives the node its name: a line keeps its name all the way down its
//!      first-parent chain, even past commits that carry other branch refs.
//!    - A node without a trunk (a branch tip) or with an unnamed trunk takes the best branch ref at
//!      its commit (HEAD's local branch, then local branches, then remote branches; never tags; see
//!      [`branch_name`]).
//!    - Failing that, a merge commit takes the target its message names (`… into dev`).
//!    - Otherwise the node is unnamed.
//! 2. **Continuing down.** The first parent continues in the node's lane with the node's name.
//! 3. **Merges.** Each other parent leaves on its own lane. A lane that is new at this row takes the
//!    merge message's source for that parent (the n-th source names the (n+1)-th parent); a lane
//!    that already existed (another child waits for the same commit) keeps its name, or takes that
//!    source if it had none. A [`Relation::Merges`] names the lane's branch and the node's.
//! 4. **Branch points.** Every other line ending at the node, apart from the node's own column, the
//!    trunk and stash or uncommitted-changes lines, gets a [`Relation::BranchedFrom`], unless it has
//!    the same name as the node (the same branch drawn twice, as after criss-cross merges).
//!
//! Stash and uncommitted-changes rows produce no relations, and their lines name nothing.

use std::collections::HashMap;

use fergit_graph::{GraphRow, Half};

use crate::repo::MergeNames;
use crate::types::{RefKind, RefLabel, Relation, RowKind};

/// What the naming needs to know about one row.
pub(super) struct RowInput<'a> {
    pub kind: RowKind,
    pub graph: &'a GraphRow,
    /// `parent_lanes[i]` is the lane the row's `i`-th parent continues in, from
    /// [`fergit_graph::Layout::push_with_parent_lanes`].
    pub parent_lanes: &'a [u16],
    /// The best branch ref at this row's commit; see [`branch_name`].
    pub branch: Option<&'a str>,
    /// What this row's commit's message names, for merge commits.
    pub merge: Option<&'a MergeNames>,
}

/// The name to give a line starting at a commit with these labels: the first local or remote
/// branch in display order (HEAD's local branch first, then local branches, then remote branches).
/// Tags and the detached `HEAD` label never name a line.
pub(super) fn branch_name(labels: &[RefLabel]) -> Option<&str> {
    let mut branches: Vec<&RefLabel> = labels
        .iter()
        .filter(|label| matches!(label.kind, RefKind::LocalBranch | RefKind::RemoteBranch))
        .collect();
    branches.sort_by(|a, b| (!a.is_head, a.kind, &a.name).cmp(&(!b.is_head, b.kind, &b.name)));
    branches.first().map(|label| label.name.as_str())
}

type NameId = u32;

#[derive(Debug, Clone, Copy)]
struct Line {
    name: Option<NameId>,
    /// Started by a stash or the uncommitted changes.
    transparent: bool,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    merges: bool,
    lane: u16,
    branch: Option<NameId>,
    /// `from` for a branch point, `into` for a merge.
    other: Option<NameId>,
}

/// Computes relations row by row, fed in display order alongside the layout.
#[derive(Debug, Default)]
pub(super) struct RelationsBuilder {
    lanes: Vec<Option<Line>>,
    names: Vec<String>,
    name_ids: HashMap<String, NameId>,
    next_row: u32,
    relations: Relations,
    converging: Vec<(u16, Option<Line>)>,
}

impl RelationsBuilder {
    pub fn push(&mut self, row: RowInput<'_>) {
        let row_number = self.next_row;
        self.next_row += 1;
        let column = row.graph.column;

        self.converging.clear();
        for edge in row.graph.edges.iter().filter(|e| e.half == Half::Upper && e.to == column) {
            let line = self.lanes.get_mut(usize::from(edge.from)).and_then(Option::take);
            self.converging.push((edge.from, line));
        }

        if row.kind != RowKind::Commit {
            for &lane in row.parent_lanes {
                if self.line(lane).is_none() {
                    self.set_line(lane, Line { name: None, transparent: true });
                }
            }
            return;
        }

        let trunk = self
            .converging
            .iter()
            .find(|(_, line)| line.is_some_and(|line| !line.transparent))
            .map(|&(lane, line)| (lane, line.and_then(|line| line.name)));
        let mut name = trunk.and_then(|(_, name)| name);
        if name.is_none() {
            name = row.branch.map(|branch| self.intern(branch));
        }
        if name.is_none() {
            name = row.merge.and_then(|merge| merge.target.as_deref()).map(|target| self.intern(target));
        }

        let first_entry = self.relations.entries.len();
        for &(lane, line) in &self.converging {
            let Some(line) = line else { continue };
            let is_trunk = trunk.is_some_and(|(trunk, _)| trunk == lane);
            if lane == column || is_trunk || line.transparent || (line.name.is_some() && line.name == name) {
                continue;
            }
            let entry = Entry { merges: false, lane, branch: line.name, other: name };
            self.relations.rows.push(row_number);
            self.relations.entries.push(entry);
        }

        for (i, &lane) in row.parent_lanes.iter().enumerate() {
            if row.parent_lanes[..i].contains(&lane) {
                continue;
            }
            if i == 0 {
                self.set_line(lane, Line { name, transparent: false });
                continue;
            }
            let source = row.merge.and_then(|merge| merge.sources.get(i - 1)).map(|source| self.intern(source));
            let branch = self.line(lane).and_then(|line| line.name).or(source);
            self.set_line(lane, Line { name: branch, transparent: false });
            self.relations.rows.push(row_number);
            self.relations.entries.push(Entry { merges: true, lane, branch, other: name });
        }
        self.relations.entries[first_entry..].sort_by_key(|entry| (entry.lane, entry.merges));
    }

    pub fn finish(mut self) -> Relations {
        self.relations.names = self.names;
        self.relations
    }

    fn line(&self, lane: u16) -> Option<Line> {
        self.lanes.get(usize::from(lane)).copied().flatten()
    }

    fn set_line(&mut self, lane: u16, line: Line) {
        let lane = usize::from(lane);
        if self.lanes.len() <= lane {
            self.lanes.resize(lane + 1, None);
        }
        self.lanes[lane] = Some(line);
    }

    fn intern(&mut self, name: &str) -> NameId {
        if let Some(&id) = self.name_ids.get(name) {
            return id;
        }
        let id = NameId::try_from(self.names.len()).expect("fewer than 4 billion branch names");
        self.names.push(name.to_owned());
        self.name_ids.insert(name.to_owned(), id);
        id
    }
}

/// The relations of every row, compactly: most rows have none.
#[derive(Debug, Default)]
pub(super) struct Relations {
    names: Vec<String>,
    /// `rows[k]` is the row of `entries[k]`; ascending.
    rows: Vec<u32>,
    entries: Vec<Entry>,
}

impl Relations {
    /// The relations of row `row`, in lane order.
    pub fn row(&self, row: u32) -> Vec<Relation> {
        let start = self.rows.partition_point(|&r| r < row);
        let end = start + self.rows[start..].partition_point(|&r| r == row);
        let name = |id: Option<NameId>| id.map(|id| self.names[id as usize].clone());
        self.entries[start..end]
            .iter()
            .map(|entry| match entry.merges {
                true => Relation::Merges { lane: entry.lane, branch: name(entry.branch), into: name(entry.other) },
                false => Relation::BranchedFrom { lane: entry.lane, branch: name(entry.branch), from: name(entry.other) },
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use fergit_graph::Layout;

    use super::*;

    /// One row: id, parents, kind, best branch ref, merge message names.
    struct Commit {
        id: &'static str,
        parents: &'static [&'static str],
        kind: RowKind,
        branch: Option<&'static str>,
        merge: Option<MergeNames>,
    }

    fn commit(id: &'static str, parents: &'static [&'static str]) -> Commit {
        Commit { id, parents, kind: RowKind::Commit, branch: None, merge: None }
    }

    impl Commit {
        fn on(mut self, branch: &'static str) -> Commit {
            self.branch = Some(branch);
            self
        }

        fn message(mut self, sources: &[&str], target: Option<&str>) -> Commit {
            self.merge = Some(MergeNames {
                sources: sources.iter().map(|s| s.to_string()).collect(),
                target: target.map(str::to_owned),
            });
            self
        }

        fn kind(mut self, kind: RowKind) -> Commit {
            self.kind = kind;
            self
        }
    }

    /// Lays out `commits` in the given order and returns each row's relations by id.
    fn relations(commits: &[Commit]) -> Vec<(&'static str, Vec<Relation>)> {
        let mut layout = Layout::new();
        let mut builder = RelationsBuilder::default();
        let mut parent_lanes = Vec::new();
        for commit in commits {
            let graph = layout.push_with_parent_lanes(commit.id, commit.parents, &mut parent_lanes);
            builder.push(RowInput {
                kind: commit.kind,
                graph: &graph,
                parent_lanes: &parent_lanes,
                branch: commit.branch,
                merge: commit.merge.as_ref(),
            });
        }
        let relations = builder.finish();
        commits.iter().enumerate().map(|(row, commit)| (commit.id, relations.row(row as u32))).collect()
    }

    /// Only the rows that have relations.
    fn nonempty(relations: Vec<(&'static str, Vec<Relation>)>) -> Vec<(&'static str, Vec<Relation>)> {
        relations.into_iter().filter(|(_, relations)| !relations.is_empty()).collect()
    }

    fn name(name: &str) -> Option<String> {
        Some(name.to_owned())
    }

    fn branched(lane: u16, branch: Option<&str>, from: Option<&str>) -> Relation {
        Relation::BranchedFrom { lane, branch: branch.map(str::to_owned), from: from.map(str::to_owned) }
    }

    fn merges(lane: u16, branch: Option<&str>, into: Option<&str>) -> Relation {
        Relation::Merges { lane, branch: branch.map(str::to_owned), into: into.map(str::to_owned) }
    }

    #[test]
    fn branches_forking_from_one_commit_each_branch_from_the_trunk() {
        let rows = relations(&[
            commit("main", &["base"]).on("main"),
            commit("feature", &["base"]).on("feature"),
            commit("topic", &["base"]).on("origin/topic"),
            commit("base", &[]),
        ]);
        assert_eq!(
            nonempty(rows),
            [(
                "base",
                vec![branched(1, Some("feature"), Some("main")), branched(2, Some("origin/topic"), Some("main"))]
            )]
        );
    }

    #[test]
    fn a_merge_of_a_deleted_branch_is_named_by_its_message() {
        let rows = relations(&[
            commit("merge", &["m1", "f1"]).on("main").message(&["feature/x"], None),
            commit("m1", &["base"]),
            commit("f1", &["base"]),
            commit("base", &[]),
        ]);
        assert_eq!(
            nonempty(rows),
            [
                ("merge", vec![merges(1, Some("feature/x"), Some("main"))]),
                ("base", vec![branched(1, Some("feature/x"), Some("main"))]),
            ]
        );
    }

    #[test]
    fn an_octopus_merge_names_each_parent_lane_from_its_message() {
        let rows = relations(&[
            commit("live", &["c1"]).on("c-live"),
            commit("octopus", &["m1", "a1", "c1"]).on("main").message(&["a", "c"], None),
            commit("m1", &["base"]),
            commit("a1", &["base"]),
            commit("c1", &["base"]),
            commit("base", &[]),
        ]);
        // `live` holds lane 0; the octopus starts lane 1. `a1` gets a new lane (2), named by the
        // message; `c1` joins lane 0, which `c-live` already names.
        assert_eq!(rows[1], ("octopus", vec![merges(0, Some("c-live"), Some("main")), merges(2, Some("a"), Some("main"))]));
        // At the base, the leftmost line is the trunk, even though it's the side branch.
        assert_eq!(
            rows[5],
            ("base", vec![branched(1, Some("main"), Some("c-live")), branched(2, Some("a"), Some("c-live"))])
        );
    }

    #[test]
    fn criss_cross_merges_name_both_branches_and_skip_a_branch_meeting_itself() {
        let rows = relations(&[
            commit("a2", &["a1", "b1"]).on("A").message(&["B"], None),
            commit("b2", &["b1", "a1"]).on("B").message(&["A"], None),
            commit("a1", &["base"]),
            commit("b1", &["base"]),
            commit("base", &[]),
        ]);
        assert_eq!(
            nonempty(rows),
            [
                ("a2", vec![merges(1, Some("B"), Some("A"))]),
                ("b2", vec![merges(0, Some("A"), Some("B"))]),
                // Both lines into `b1` are named B: no "B branched from B".
                ("base", vec![branched(1, Some("B"), Some("A"))]),
            ]
        );
    }

    #[test]
    fn unnamed_lines_adopt_refs_and_merge_targets_but_named_lines_keep_their_name() {
        let rows = relations(&[
            // An unnamed tip, whose line adopts `old` further down and then keeps it past `release`.
            commit("tip", &["y"]),
            commit("y", &["z"]).on("old"),
            commit("z", &["w"]).on("release"),
            commit("side", &["w"]),
            commit("w", &["merge"]),
            // An unnamed merge named by its message's target.
            commit("merge", &["p", "q"]).message(&["x"], Some("dev")),
            commit("unnamed", &["p"]),
            commit("q", &["p"]),
            commit("p", &[]),
        ]);
        assert_eq!(
            nonempty(rows),
            [
                ("w", vec![branched(1, None, Some("old"))]),
                // `merge` continues `old`'s line (named), so the target doesn't rename it.
                ("merge", vec![merges(1, Some("x"), Some("old"))]),
                ("p", vec![branched(1, Some("x"), Some("old")), branched(2, None, Some("old"))]),
            ]
        );

        let rows = relations(&[
            commit("merge", &["p", "q"]).message(&["x"], Some("dev")),
            commit("q", &["p"]),
            commit("p", &[]),
        ]);
        assert_eq!(nonempty(rows), [("merge", vec![merges(1, name("x").as_deref(), Some("dev"))]), ("p", vec![branched(1, Some("x"), Some("dev"))])]);
    }

    #[test]
    fn stash_and_uncommitted_rows_produce_nothing_and_name_nothing() {
        let rows = relations(&[
            commit("changes", &["head"]).kind(RowKind::WorkingTree),
            commit("head", &["base"]).on("main"),
            commit("x", &["x1"]).on("x"),
            commit("feature", &["base2"]).on("feature"),
            commit("x1", &[]),
            // Lane 1 is free again: the stash takes it, left of `feature`'s lane.
            commit("stash", &["base2"]).kind(RowKind::Stash),
            commit("base2", &["base"]),
            commit("topic", &["base"]).on("topic"),
            commit("base", &[]),
        ]);
        assert_eq!(
            nonempty(rows),
            [
                // `base2` is on `feature`: the stash line to its left isn't the trunk and names nothing.
                ("base", vec![branched(1, Some("feature"), Some("main")), branched(2, Some("topic"), Some("main"))]),
            ]
        );
    }

    #[test]
    fn branch_names_prefer_head_then_local_then_remote_and_never_tags() {
        let label = |kind, name: &str, is_head| RefLabel {
            kind,
            name: name.to_owned(),
            full_name: format!("refs/{name}"),
            is_head,
            upstream: None,
        };
        use RefKind::*;
        assert_eq!(branch_name(&[label(RemoteBranch, "origin/x", false)]), Some("origin/x"));
        assert_eq!(branch_name(&[label(RemoteBranch, "origin/a", false), label(LocalBranch, "z", false)]), Some("z"));
        assert_eq!(branch_name(&[label(LocalBranch, "a", false), label(LocalBranch, "main", true)]), Some("main"));
        assert_eq!(branch_name(&[label(Head, "HEAD", true), label(Tag, "v1", false)]), None);
        assert_eq!(branch_name(&[label(Tag, "v1", false), label(RemoteBranch, "origin/x", false)]), Some("origin/x"));
    }
}
