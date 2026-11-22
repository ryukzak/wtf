use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::git::{self, LOG_FORMAT};
use crate::{Attention, Commit, CoreError, Revision};

/// Untracked files larger than this are not counted in diff sizes.
const MAX_COUNTED_FILE: u64 = 1 << 20;
/// At most this many commits show for a branch alone on its line.
const RECENT_COMMITS: usize = 50;

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    /// The oldest fork point, standing for all history before it.
    History,
    /// A linear run of commits.
    Commits,
    /// A worktree's uncommitted changes.
    Uncommitted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    /// Stable while the node grows: its first commit, or its worktree path.
    pub key: String,
    pub kind: NodeKind,
    pub lane: usize,
    /// Oldest first; empty for uncommitted changes.
    pub commits: Vec<Commit>,
    pub parents: Vec<usize>,
    pub first_time: i64,
    pub last_time: i64,
    /// Branch and worktree names ending at this node.
    pub refs: Vec<String>,
    /// For a worktree, its checked-out branch; `None` when detached.
    pub branch: Option<String>,
    /// Tags on the node's commits, when attention shows tags.
    pub tags: Vec<String>,
    /// Stashes made on the node's commits, when attention shows them.
    pub stashes: Vec<String>,
    /// Lines added and removed by uncommitted changes; zero for commits.
    pub added: u32,
    pub removed: u32,
    /// The node's changes are `base..tip`; a `None` base is the empty tree.
    pub base: Option<String>,
    pub tip: Revision,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkflowGraph {
    /// Parents come before their children.
    pub nodes: Vec<GraphNode>,
    pub lane_count: usize,
    /// The commit node at the tip of the main branch.
    pub main_node: Option<usize>,
    /// The name of the main branch, which attention always shows.
    pub main: String,
    /// Branches and worktrees: the main branch first, then the newest.
    pub branches: Vec<Branch>,
    /// Commits older than the oldest node, left out of the graph.
    pub older: usize,
    /// Every tag, newest first, drawn in the graph or not.
    pub tags: Vec<String>,
    /// Every stash, newest first, marked in the graph or not.
    pub stashes: Vec<crate::Stash>,
    /// The repository's remotes, such as `origin`.
    pub remotes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Branch {
    pub name: String,
    /// The commit at its tip.
    pub commit: String,
    /// The author of the latest commit.
    pub author: String,
    pub time: i64,
    /// Whether attention includes it.
    pub shown: bool,
    /// Checked out in a worktree.
    pub worktree: bool,
    /// A remote-tracking branch, such as `origin/fix`.
    pub remote: bool,
    /// Why it looks left behind, such as `merged`; empty for live work and
    /// the main branch.
    pub stale: Vec<String>,
    /// Its worktree has uncommitted changes: work in progress.
    pub busy: bool,
    /// Shares no history with the main branch, as a `gh-pages` branch, so the
    /// graph cannot draw it.
    pub unrelated: bool,
    /// The remote branch it tracks, such as `origin/fix`.
    pub upstream: Option<String>,
    /// Commits it has that its upstream lacks, and the other way round.
    pub ahead: u32,
    pub behind: u32,
}

impl WorkflowGraph {
    /// The diff covering `selected` nodes when they lie on one line of history:
    /// from the base of the earliest to the tip of the latest.
    pub fn range(&self, selected: &[usize]) -> Option<(Option<String>, Revision)> {
        let comparable = |a: usize, b: usize| self.is_ancestor(a, b) || self.is_ancestor(b, a);
        if !selected
            .iter()
            .all(|&a| selected.iter().all(|&b| comparable(a, b)))
        {
            return None;
        }
        let earliest = selected
            .iter()
            .find(|&&a| selected.iter().all(|&b| self.is_ancestor(a, b)))?;
        let latest = selected
            .iter()
            .find(|&&b| selected.iter().all(|&a| self.is_ancestor(a, b)))?;
        Some((
            self.nodes[*earliest].base.clone(),
            self.nodes[*latest].tip.clone(),
        ))
    }

    /// Whether `a` is `b` or one of its ancestors.
    fn is_ancestor(&self, a: usize, b: usize) -> bool {
        let mut stack = vec![b];
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            if node == a {
                return true;
            }
            if seen.insert(node) {
                stack.extend(&self.nodes[node].parents);
            }
        }
        false
    }
}

struct Tip {
    name: String,
    id: String,
    worktree: Option<String>,
    /// A worktree with a detached HEAD, named after its folder.
    detached: bool,
}

type Log = Vec<(Commit, Vec<String>)>;

/// With `combine`, linear runs of commits share a node; without it every
/// commit is a node of its own.
pub fn build(
    repo_path: &str,
    attention: &Attention,
    combine: bool,
    history: usize,
    now: i64,
) -> Result<WorkflowGraph, CoreError> {
    let (mut tips, mut branches) = tips(repo_path, attention, now)?;
    let base = match octopus_base(repo_path, &tips) {
        Ok(base) => base,
        Err(_) => {
            // Some tip shares no history with the main branch, such as a
            // `gh-pages` branch; it is left out.
            let main = tips[0].id.clone();
            let related = |tip: &Tip| git::run(repo_path, &["merge-base", &main, &tip.id]).is_ok();
            let (kept, unrelated): (Vec<Tip>, Vec<Tip>) = tips.into_iter().partition(related);
            for tip in unrelated {
                if let Some(branch) = branches.iter_mut().find(|b| b.name == tip.name) {
                    branch.shown = false;
                    branch.unrelated = true;
                }
            }
            tips = kept;
            octopus_base(repo_path, &tips)?
        }
    };
    // When every tip sits on one line, as in a repository without branches,
    // the fork point is a tip; the line's recent commits still make a node.
    let base = match tips.iter().any(|tip| tip.id == base) {
        true => recent_base(repo_path, &base, attention, now)?,
        false => base,
    };
    // Scrolling back asks for `history` more commits on the main line.
    let base = match history {
        0 => base,
        _ => {
            let count = format!("--max-count={}", history + 1);
            let line = git::run(repo_path, &["rev-list", "--first-parent", &count, &base])?;
            line.lines().last().unwrap_or(&base).to_string()
        }
    };
    let older = git::run(repo_path, &["rev-list", "--count", &base])?
        .trim()
        .parse::<usize>()
        .unwrap_or(1)
        .saturating_sub(1);
    let mut ids: Vec<&str> = tips.iter().map(|tip| tip.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();

    // Newest first, with the base last. Before the attention window only the
    // tips' first-parent lines count, so old merges read as plain commits and
    // old merged branches stay out.
    let range: Vec<String> = ids
        .iter()
        .map(|id| id.to_string())
        .chain([format!("^{base}")])
        .collect();
    let mut log = match attention.window_days {
        None => commits(repo_path, &[], &range)?,
        Some(days) => {
            let since = now - i64::from(days) * 86_400;
            let mut log = commits(repo_path, &["--first-parent".to_string()], &range)?;
            let seen: HashSet<String> = log.iter().map(|(commit, _)| commit.id.clone()).collect();
            let recent = commits(repo_path, &[format!("--since=@{since}")], &range)?;
            log.extend(
                recent
                    .into_iter()
                    .filter(|(commit, _)| !seen.contains(&commit.id)),
            );
            topo_sort(log)
        }
    };
    log.extend(git::parse_log(&git::run(
        repo_path,
        &["log", "-1", LOG_FORMAT, &base],
    )?));
    let index: HashMap<&str, usize> = log
        .iter()
        .enumerate()
        .map(|(i, (commit, _))| (commit.id.as_str(), i))
        .collect();
    let base_ix = log.len() - 1;

    // Each tip claims its first-parent chain as a lane; leftovers, such as merged
    // branches, get lanes of their own.
    let mut lane_of = vec![usize::MAX; log.len()];
    let mut lane_count = 0;
    let mut tip_lanes = Vec::new();
    for tip in &tips {
        let claimed = index
            .get(tip.id.as_str())
            .is_some_and(|&start| claim(start, lane_count, &log, &index, &mut lane_of));
        if claimed || tip.worktree.is_some() {
            tip_lanes.push(Some(lane_count));
            lane_count += 1;
        } else {
            tip_lanes.push(None);
        }
    }
    for start in 0..log.len() {
        if claim(start, lane_count, &log, &index, &mut lane_of) {
            lane_count += 1;
        }
    }

    let tip_ids: HashSet<&str> = tips.iter().map(|tip| tip.id.as_str()).collect();
    let mut children = vec![0; log.len()];
    for parent in log.iter().flat_map(|(_, parents)| parents) {
        if let Some(&p) = index.get(parent.as_str()) {
            children[p] += 1;
        }
    }

    // Oldest first, so parents get their nodes before children.
    let mut node_of = vec![usize::MAX; log.len()];
    let mut nodes: Vec<GraphNode> = Vec::new();
    for i in (0..log.len()).rev() {
        let (commit, parents) = &log[i];
        // Parents outside the graph, such as old merged branches, are left out.
        let present: Vec<usize> = parents
            .iter()
            .filter_map(|parent| index.get(parent.as_str()).copied())
            .collect();
        let first_present = parents
            .first()
            .is_some_and(|parent| index.contains_key(parent.as_str()));
        if let [p] = present[..]
            && combine
            && first_present
            && i != base_ix
            && p != base_ix
            && children[p] == 1
            && !tip_ids.contains(log[p].0.id.as_str())
            && lane_of[p] == lane_of[i]
        {
            let node = &mut nodes[node_of[p]];
            node.commits.push(commit.clone());
            node.last_time = commit.time;
            node.tip = Revision::Commit(commit.id.clone());
            node_of[i] = node_of[p];
            continue;
        }

        let mut node_parents = Vec::new();
        if i != base_ix {
            for p in present {
                let node = node_of[p];
                if !node_parents.contains(&node) {
                    node_parents.push(node);
                }
            }
        }
        nodes.push(GraphNode {
            key: commit.id.clone(),
            kind: if i == base_ix {
                NodeKind::History
            } else {
                NodeKind::Commits
            },
            lane: lane_of[i],
            commits: vec![commit.clone()],
            parents: node_parents,
            first_time: commit.time,
            last_time: commit.time,
            refs: Vec::new(),
            branch: None,
            tags: Vec::new(),
            stashes: Vec::new(),
            added: 0,
            removed: 0,
            base: commit.parent.clone(),
            tip: Revision::Commit(commit.id.clone()),
        });
        node_of[i] = nodes.len() - 1;
    }

    for (tip, lane) in tips.iter().zip(&tip_lanes) {
        let Some(&i) = index.get(tip.id.as_str()) else {
            continue;
        };
        match &tip.worktree {
            None => nodes[node_of[i]].refs.push(tip.name.clone()),
            Some(path) => {
                // A checked-out branch is labelled at its commit like any
                // other; the worktree's own name goes to the worktree list.
                if !tip.detached {
                    nodes[node_of[i]].refs.push(tip.name.clone());
                }
                let (added, removed) = worktree_stats(path)?;
                let time = log[i].0.time;
                nodes.push(GraphNode {
                    key: format!("worktree:{path}"),
                    kind: NodeKind::Uncommitted,
                    lane: lane.unwrap_or(lane_of[i]),
                    commits: Vec::new(),
                    parents: vec![node_of[i]],
                    first_time: time,
                    last_time: time,
                    refs: vec![tip.name.clone()],
                    branch: (!tip.detached).then(|| tip.name.clone()),
                    tags: Vec::new(),
                    stashes: Vec::new(),
                    added,
                    removed,
                    base: Some(tip.id.clone()),
                    tip: Revision::Worktree(path.clone()),
                });
            }
        }
    }

    if attention.tags {
        let format = "--format=%(refname:short)%09%(objectname)%09%(*objectname)";
        for line in git::run(repo_path, &["for-each-ref", format, "refs/tags"])?.lines() {
            // Annotated tags point at a tag object; the commit is the peeled one.
            let fields: Vec<&str> = line.split('\t').collect();
            let [name, id, peeled] = fields[..] else {
                continue;
            };
            let commit = if peeled.is_empty() { id } else { peeled };
            if let Some(&i) = index.get(commit) {
                nodes[node_of[i]].tags.push(name.to_string());
            }
        }
    }

    let stashes = git::stashes(repo_path)?;
    if attention.stashes {
        for stash in &stashes {
            if let Some(&i) = index.get(stash.base.as_str()) {
                nodes[node_of[i]].stashes.push(stash.name.clone());
            }
        }
    }

    // A worktree with uncommitted changes is work in progress, however its
    // branch looks.
    for node in nodes
        .iter()
        .filter(|node| (node.added, node.removed) != (0, 0))
    {
        if let Some(branch) = branches
            .iter_mut()
            .find(|b| node.branch.as_ref() == Some(&b.name))
        {
            branch.stale.clear();
            branch.busy = true;
        }
    }

    let main_node = index.get(tips[0].id.as_str()).map(|&i| node_of[i]);
    Ok(WorkflowGraph {
        nodes,
        lane_count,
        main_node,
        main: tips[0].name.clone(),
        branches,
        older,
        remotes: remote_names(repo_path)?,
        tags: git::run(
            repo_path,
            &[
                "for-each-ref",
                "--sort=-creatordate",
                "--format=%(refname:short)",
                "refs/tags",
            ],
        )?
        .lines()
        .map(str::to_string)
        .collect(),
        stashes,
    })
}

/// Commits on `tip`'s first-parent line to show before it: those in the
/// attention window, up to `RECENT_COMMITS`. Returns the commit they start
/// after, or the root when they reach it.
fn recent_base(
    repo_path: &str,
    tip: &str,
    attention: &Attention,
    now: i64,
) -> Result<String, CoreError> {
    let mut args = vec![
        "rev-list".to_string(),
        "--first-parent".to_string(),
        format!("--max-count={RECENT_COMMITS}"),
    ];
    if let Some(days) = attention.window_days {
        args.push(format!("--since=@{}", now - i64::from(days) * 86_400));
    }
    args.push(tip.to_string());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let listed = git::run(repo_path, &args)?;
    let oldest = listed.lines().last().unwrap_or(tip);
    // `^@` names the parents, none for the root, without failing.
    let parents = git::run(repo_path, &["rev-parse", &format!("{oldest}^@")])?;
    Ok(parents.lines().next().unwrap_or(oldest).to_string())
}

/// The newest commit every tip descends from.
fn octopus_base(repo_path: &str, tips: &[Tip]) -> Result<String, CoreError> {
    let mut ids: Vec<&str> = tips.iter().map(|tip| tip.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() == 1 {
        return Ok(ids[0].to_string());
    }
    let mut args = vec!["merge-base", "--octopus"];
    args.extend(&ids);
    Ok(git::run(repo_path, &args)?.trim().to_string())
}

fn commits(repo_path: &str, options: &[String], range: &[String]) -> Result<Log, CoreError> {
    let mut args = vec!["log".to_string(), "--topo-order".into(), LOG_FORMAT.into()];
    args.extend_from_slice(options);
    args.extend_from_slice(range);
    Ok(git::parse_log(&git::run(repo_path, &args)?))
}

/// Orders commits newest first, every commit before its parents.
fn topo_sort(log: Log) -> Log {
    let index: HashMap<String, usize> = log
        .iter()
        .enumerate()
        .map(|(i, (commit, _))| (commit.id.clone(), i))
        .collect();
    // A post-order walk over parents puts ancestors first.
    let mut visited = vec![false; log.len()];
    let mut order = Vec::with_capacity(log.len());
    for start in 0..log.len() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut stack = vec![(start, 0)];
        while let Some((node, next)) = stack.last_mut() {
            let node = *node;
            match log[node].1.get(*next) {
                Some(parent) => {
                    *next += 1;
                    if let Some(&p) = index.get(parent)
                        && !visited[p]
                    {
                        visited[p] = true;
                        stack.push((p, 0));
                    }
                }
                None => {
                    order.push(node);
                    stack.pop();
                }
            }
        }
    }
    let mut entries: Vec<Option<(Commit, Vec<String>)>> = log.into_iter().map(Some).collect();
    order
        .into_iter()
        .rev()
        .filter_map(|i| entries[i].take())
        .collect()
}

/// Marks the unclaimed first-parent chain from `start` with `lane`.
fn claim(
    start: usize,
    lane: usize,
    log: &Log,
    index: &HashMap<&str, usize>,
    lane_of: &mut [usize],
) -> bool {
    let mut at = Some(start);
    let mut claimed = false;
    while let Some(i) = at.filter(|&i| lane_of[i] == usize::MAX) {
        lane_of[i] = lane;
        claimed = true;
        at = log[i]
            .1
            .first()
            .and_then(|parent| index.get(parent.as_str()).copied());
    }
    claimed
}

/// The main branch first, then worktrees, then the branches attention shows; and
/// all branches outside worktrees with whether they are shown.
fn tips(
    repo_path: &str,
    attention: &Attention,
    now: i64,
) -> Result<(Vec<Tip>, Vec<Branch>), CoreError> {
    let main = main_branch(repo_path);
    let main_id = git::run(
        repo_path,
        &["rev-parse", "--verify", &format!("{main}^{{commit}}")],
    )?;
    let mut tips = vec![Tip {
        name: main.clone(),
        id: main_id.trim().to_string(),
        worktree: None,
        detached: false,
    }];

    let mut branches = Vec::new();
    for (path, head, branch) in worktrees(repo_path)? {
        if branch.as_deref() == Some(main.as_str()) {
            tips[0].worktree = Some(path);
            continue;
        }
        let detached = branch.is_none();
        let name = branch.unwrap_or_else(|| {
            let dir = Path::new(&path).file_name().unwrap_or_default();
            dir.to_string_lossy().into_owned()
        });
        let shown = attention.shows(&name, "", 0, now, true);
        branches.push(Branch {
            name: name.clone(),
            commit: head.clone(),
            author: String::new(),
            time: 0,
            shown,
            worktree: true,
            remote: false,
            stale: Vec::new(),
            busy: false,
            unrelated: false,
            upstream: None,
            ahead: 0,
            behind: 0,
        });
        if shown {
            tips.push(Tip {
                name,
                id: head,
                worktree: Some(path),
                detached,
            });
        }
    }

    // The main branch is listed too, always shown, as a worktree when one
    // has it checked out.
    branches.push(Branch {
        name: main.clone(),
        commit: tips[0].id.clone(),
        author: String::new(),
        time: 0,
        shown: true,
        worktree: tips[0].worktree.is_some(),
        remote: false,
        stale: Vec::new(),
        busy: false,
        unrelated: false,
        upstream: None,
        ahead: 0,
        behind: 0,
    });

    let format =
        "--format=%(refname:short)%09%(objectname)%09%(committerdate:unix)%09%(authorname)";
    // Remote branches are listed always, for attention to offer; the graph
    // draws them only while attention asks.
    let args = [
        "for-each-ref",
        "--sort=-committerdate",
        format,
        "refs/heads",
        "refs/remotes",
    ];
    let refs = git::run(repo_path, &args)?;
    let remote_names = remote_names(repo_path)?;
    for line in refs.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [name, id, time, author] = fields[..] else {
            continue;
        };
        let time = time.parse().unwrap_or_default();
        // `origin/HEAD` names another branch, and a bare `origin` is its short form.
        let remote = remote_names
            .iter()
            .any(|remote| name.starts_with(&format!("{remote}/")));
        if name.ends_with("/HEAD") || remote_names.iter().any(|r| r == name) {
            continue;
        }
        if let Some(branch) = branches.iter_mut().find(|branch| branch.name == name) {
            (branch.author, branch.time) = (author.to_string(), time);
            continue;
        }
        let mut branch = Branch {
            name: name.to_string(),
            commit: id.to_string(),
            author: author.to_string(),
            time,
            shown: false,
            worktree: false,
            remote,
            stale: Vec::new(),
            busy: false,
            unrelated: false,
            upstream: None,
            ahead: 0,
            behind: 0,
        };
        branch.shown = attention.shows_branch(&branch, now);
        if branch.shown {
            tips.push(Tip {
                name: name.to_string(),
                id: id.to_string(),
                worktree: None,
                detached: false,
            });
        }
        branches.push(branch);
    }
    // The main branch first, then the newest.
    branches.sort_by_key(|branch| (branch.name != main, std::cmp::Reverse(branch.time)));
    mark_stale(repo_path, &main, &mut branches)?;
    Ok((tips, branches))
}

/// Notes why local branches look left behind: the main branch has merged
/// them, or their remote branch is gone. How long one sat idle is up to the
/// reader, from its time. Notes too how far each is from its upstream.
fn mark_stale(repo_path: &str, main: &str, branches: &mut [Branch]) -> Result<(), CoreError> {
    let format = "--format=%(refname:short)";
    let merged = git::run(repo_path, &["branch", format, "--merged", main])?;
    let merged: HashSet<&str> = merged.lines().collect();
    let tracks = git::run(
        repo_path,
        &[
            "for-each-ref",
            "--format=%(refname:short)%09%(upstream:short)%09%(upstream:track,nobracket)",
            "refs/heads",
        ],
    )?;
    let tracks: HashMap<&str, (&str, &str)> = tracks
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some((
                fields.next()?,
                (fields.next()?, fields.next().unwrap_or("")),
            ))
        })
        .collect();
    for branch in branches.iter_mut().filter(|branch| !branch.remote) {
        if let Some(&(upstream, track)) = tracks.get(branch.name.as_str()) {
            let (ahead, behind) = parse_track(track);
            branch.upstream = (!upstream.is_empty()).then(|| upstream.to_string());
            (branch.ahead, branch.behind) = (ahead, behind);
            if track == "gone" && branch.name != main {
                branch.stale.push("remote gone".to_string());
            }
        }
        if branch.name != main && merged.contains(branch.name.as_str()) {
            branch.stale.insert(0, "merged".to_string());
        }
    }
    Ok(())
}

/// Reads git's `ahead 2, behind 1` into the two counts.
fn parse_track(track: &str) -> (u32, u32) {
    let count = |word: &str| {
        track
            .split(", ")
            .find_map(|part| part.strip_prefix(word)?.trim().parse().ok())
            .unwrap_or(0)
    };
    (count("ahead "), count("behind "))
}

/// The names of the remotes.
fn remote_names(repo_path: &str) -> Result<Vec<String>, CoreError> {
    Ok(git::run(repo_path, &["remote"])?
        .lines()
        .map(str::to_string)
        .collect())
}

/// The local branch behind `origin/HEAD`, else `main` or `master`, else `HEAD`.
fn main_branch(repo_path: &str) -> String {
    let exists = |name: &str| {
        git::run(
            repo_path,
            &["rev-parse", "--verify", "-q", &format!("refs/heads/{name}")],
        )
        .is_ok()
    };
    let origin = git::run(
        repo_path,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    );
    let origin = origin
        .ok()
        .and_then(|name| Some(name.trim().strip_prefix("origin/")?.to_string()));
    origin
        .into_iter()
        .chain(["main".to_string(), "master".to_string()])
        .find(|name| exists(name))
        .unwrap_or_else(|| "HEAD".to_string())
}

/// Path, HEAD and branch of each usable worktree, the main one first.
fn worktrees(repo_path: &str) -> Result<Vec<(String, String, Option<String>)>, CoreError> {
    let raw = git::run(repo_path, &["worktree", "list", "--porcelain"])?;
    let mut list = Vec::new();
    for block in raw.split("\n\n") {
        let (mut path, mut head, mut branch, mut usable) = (None, None, None, true);
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("worktree ") {
                path = Some(value.to_string());
            } else if let Some(value) = line.strip_prefix("HEAD ") {
                usable &= value.bytes().any(|byte| byte != b'0');
                head = Some(value.to_string());
            } else if let Some(value) = line.strip_prefix("branch ") {
                branch = Some(value.trim_start_matches("refs/heads/").to_string());
            } else if line == "bare" || line.starts_with("prunable") {
                usable = false;
            }
        }
        if let (true, Some(path), Some(head)) = (usable, path, head) {
            list.push((path, head, branch));
        }
    }
    Ok(list)
}

/// Lines added and removed in the worktree, untracked files included.
fn worktree_stats(path: &str) -> Result<(u32, u32), CoreError> {
    let (mut added, removed) = numstat(&git::run(path, &["diff", "--numstat", "HEAD"])?);
    for file in git::untracked(path)? {
        let file = Path::new(path).join(file);
        if fs::metadata(&file).is_ok_and(|meta| meta.len() <= MAX_COUNTED_FILE)
            && let Ok(bytes) = fs::read(&file)
            && !bytes.contains(&0)
        {
            added += bytes.iter().filter(|&&byte| byte == b'\n').count() as u32;
        }
    }
    Ok((added, removed))
}

/// Sums `git diff --numstat`; binary files count as zero.
fn numstat(raw: &str) -> (u32, u32) {
    raw.lines().fold((0, 0), |(added, removed), line| {
        let mut fields = line.split('\t');
        let mut next = || {
            fields
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0)
        };
        (added + next(), removed + next())
    })
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::DiffMode;

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let fixture = Self {
                dir: tempfile::tempdir().unwrap(),
            };
            fixture.git(".", &["init", "-q", "-b", "master"]);
            fixture
        }

        fn path(&self) -> String {
            self.dir.path().to_string_lossy().into_owned()
        }

        /// Runs git in `dir`, relative to the fixture, without user config.
        fn git(&self, dir: &str, args: &[&str]) -> String {
            let output = Command::new("git")
                .args(args)
                .current_dir(self.dir.path().join(dir))
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }

        fn write(&self, dir: &str, file: &str, content: &str) {
            fs::write(self.dir.path().join(dir).join(file), content).unwrap();
        }

        fn commit(&self, dir: &str, file: &str, content: &str, message: &str) -> String {
            self.write(dir, file, content);
            self.git(dir, &["add", "-A"]);
            self.git(dir, &["commit", "-q", "-m", message]);
            self.git(dir, &["rev-parse", "HEAD"])
        }

        /// Commits everything with a fixed date, e.g. long before the window.
        fn commit_at(&self, date: &str, message: &str) {
            self.git(".", &["add", "-A"]);
            let date = format!("--date={date}");
            let output = Command::new("git")
                .args(["commit", "-q", "--allow-empty", &date, "-m", message])
                .current_dir(self.dir.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_DATE", date.trim_start_matches("--date="))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn graph(&self) -> WorkflowGraph {
            self.graph_with(&Attention::default())
        }

        fn graph_with(&self, attention: &Attention) -> WorkflowGraph {
            crate::workflow_graph(&self.path(), attention, true, 0).unwrap()
        }
    }

    /// The node a branch ends in: its worktree when one has it checked out,
    /// which comes after the commit labelled with it.
    fn node<'a>(graph: &'a WorkflowGraph, reference: &str) -> (usize, &'a GraphNode) {
        let found = graph
            .nodes
            .iter()
            .rposition(|node| node.refs.iter().any(|name| name == reference));
        let index = found.unwrap_or_else(|| panic!("no node for {reference}"));
        (index, &graph.nodes[index])
    }

    fn summaries(node: &GraphNode) -> Vec<&str> {
        node.commits
            .iter()
            .map(|commit| commit.summary.as_str())
            .collect()
    }

    /// master: c1 - c2 - c3; feature forks at c2 with f1 - f2.
    fn forked() -> Fixture {
        let fixture = Fixture::new();
        fixture.commit(".", "a.txt", "1\n", "c1");
        fixture.commit(".", "a.txt", "1\n2\n", "c2");
        fixture.git(".", &["branch", "feature"]);
        fixture.commit(".", "a.txt", "1\n2\n3\n", "c3");
        fixture.git(".", &["checkout", "-q", "feature"]);
        fixture.commit(".", "b.txt", "x\n", "f1");
        fixture.commit(".", "b.txt", "x\ny\n", "f2");
        fixture.git(".", &["checkout", "-q", "master"]);
        fixture
    }

    #[test]
    fn collapses_linear_runs_and_splits_at_the_fork() {
        let fixture = forked();
        let graph = fixture.graph();

        let history = &graph.nodes[0];
        assert_eq!(history.kind, NodeKind::History);
        assert_eq!(summaries(history), vec!["c2"]);

        let (feature_ix, feature) = node(&graph, "feature");
        assert_eq!(feature.kind, NodeKind::Commits);
        assert_eq!(summaries(feature), vec!["f1", "f2"]);
        assert_eq!(feature.parents, vec![0]);

        // The main worktree is on master, so master ends in its uncommitted node.
        let (_, worktree) = node(&graph, "master");
        assert_eq!(worktree.kind, NodeKind::Uncommitted);
        let main = &graph.nodes[graph.main_node.unwrap()];
        assert_eq!(summaries(main), vec!["c3"]);
        assert_eq!(main.lane, 0);
        assert_eq!(history.lane, 0);
        assert_ne!(feature.lane, 0);
        assert!(feature_ix > 0);
    }

    #[test]
    fn worktree_node_counts_uncommitted_and_untracked_lines() {
        let fixture = Fixture::new();
        fixture.commit(".", "a.txt", "1\n", "c1");
        fixture.git(".", &["worktree", "add", "-q", "-b", "agent", "wt"]);
        fixture.commit("wt", "a.txt", "1\n2\n", "a1");
        fixture.write("wt", "a.txt", "1\n2\n3\n4\n");
        fixture.write("wt", "new.txt", "n\n");
        let graph = fixture.graph();

        let (agent_ix, agent) = node(&graph, "agent");
        assert_eq!(agent.kind, NodeKind::Uncommitted);
        assert_eq!((agent.added, agent.removed), (3, 0));
        let parent = &graph.nodes[agent.parents[0]];
        assert_eq!(summaries(parent), vec!["a1"]);
        assert_eq!(parent.lane, agent.lane);

        let (base, tip) = graph.range(&[agent_ix]).unwrap();
        let files = crate::changed_files(&fixture.path(), base.as_deref(), &tip).unwrap();
        let mut paths: Vec<&str> = files.iter().filter_map(|f| f.new_path.as_deref()).collect();
        paths.sort();
        assert_eq!(paths, vec!["a.txt", "new.txt"]);

        let a = files
            .iter()
            .find(|f| f.new_path.as_deref() == Some("a.txt"))
            .unwrap();
        let diff = crate::file_diff(fixture.path(), a.clone(), DiffMode::Text).unwrap();
        let added: Vec<&str> = diff
            .rows
            .iter()
            .filter(|row| row.left.is_none())
            .filter_map(|row| row.right.as_ref().map(|side| side.text.as_str()))
            .collect();
        assert_eq!(added, vec!["3", "4"]);
    }

    #[test]
    fn range_needs_one_line_of_history() {
        let fixture = forked();
        let graph = fixture.graph();
        let (feature_ix, feature) = node(&graph, "feature");
        let main_ix = graph.main_node.unwrap();

        let (base, tip) = graph.range(&[feature_ix, 0]).unwrap();
        assert_eq!(base, graph.nodes[0].base);
        assert_eq!(tip, feature.tip);
        let commits = crate::range_commits(&fixture.path(), base.as_deref(), &tip).unwrap();
        let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, vec!["f2", "f1", "c2"]);

        assert_eq!(graph.range(&[feature_ix, main_ix]), None);
    }

    #[test]
    fn merged_branch_gets_its_own_lane() {
        let fixture = forked();
        fixture.git(".", &["merge", "-q", "--no-ff", "--no-edit", "feature"]);
        fixture.git(".", &["branch", "-q", "-D", "feature"]);
        // Keeps the graph reaching back past the merged branch.
        fixture.git(".", &["branch", "old", "master~2"]);
        let graph = fixture.graph();

        let main = &graph.nodes[graph.main_node.unwrap()];
        assert_eq!(summaries(main), vec!["Merge branch 'feature'"]);
        assert_eq!(main.parents.len(), 2);
        let side = &graph.nodes[main.parents[1]];
        assert_eq!(summaries(side), vec!["f1", "f2"]);
        assert_ne!(side.lane, main.lane);
    }

    #[test]
    fn attention_decides_which_branches_are_drawn() {
        let fixture = forked();
        fixture.git(".", &["checkout", "-q", "-b", "bot", "master~1"]);
        fixture.git(
            ".",
            &[
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "b1",
                "--author",
                "Bot <bot@example.invalid>",
            ],
        );
        fixture.git(".", &["checkout", "-q", "master"]);
        let drawn = |graph: &WorkflowGraph, name: &str| {
            graph
                .nodes
                .iter()
                .any(|node| node.refs.iter().any(|r| r == name))
        };

        let graph = fixture.graph();
        let names: Vec<&str> = graph.branches.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"bot") && names.contains(&"feature"));
        assert_eq!(names[0], "master", "the main branch is listed first");
        assert!(graph.branches[0].shown && graph.branches[0].worktree);
        assert!(drawn(&graph, "bot") && drawn(&graph, "feature"));

        let mut attention = Attention {
            authors: vec!["Bot".to_string()],
            ..Attention::default()
        };
        let graph = fixture.graph_with(&attention);
        assert!(drawn(&graph, "bot"));
        assert!(!drawn(&graph, "feature"));
        let feature = graph.branches.iter().find(|b| b.name == "feature").unwrap();
        assert!(!feature.shown);

        attention.pinned = vec!["feature".to_string()];
        attention.hidden = vec!["bot".to_string()];
        let graph = fixture.graph_with(&attention);
        assert!(drawn(&graph, "feature"));
        assert!(!drawn(&graph, "bot"));
    }

    #[test]
    fn without_combining_every_commit_is_a_node() {
        let fixture = forked();
        let graph =
            crate::workflow_graph(&fixture.path(), &Attention::default(), false, 0).unwrap();
        let (_, feature) = node(&graph, "feature");
        assert_eq!(summaries(feature), ["f2"]);
        let parent = &graph.nodes[feature.parents[0]];
        assert_eq!(summaries(parent), ["f1"]);
    }

    #[test]
    fn scrolling_back_brings_older_commits() {
        let fixture = Fixture::new();
        for n in 1..=5 {
            fixture.commit(".", "a.txt", &format!("{n}\n"), &format!("c{n}"));
        }
        fixture.git(".", &["checkout", "-q", "-b", "feature", "HEAD~1"]);
        fixture.commit(".", "b.txt", "x\n", "f1");
        fixture.git(".", &["checkout", "-q", "master"]);
        let graph = fixture.graph();
        assert_eq!(graph.older, 3, "c1..c3 lie before the fork at c4");
        let more = crate::workflow_graph(&fixture.path(), &Attention::default(), true, 2).unwrap();
        assert_eq!(more.older, 1);
        let all = crate::workflow_graph(&fixture.path(), &Attention::default(), true, 10).unwrap();
        assert_eq!(all.older, 0);
    }

    #[test]
    fn stale_branches_say_why() {
        let fixture = forked();
        fixture.git(".", &["branch", "done", "master~1"]);
        let graph = fixture.graph();
        let stale = |name: &str| {
            graph
                .branches
                .iter()
                .find(|b| b.name == name)
                .map(|b| b.stale.clone())
                .unwrap()
        };
        assert_eq!(stale("done"), ["merged"]);
        assert!(stale("feature").is_empty());
        assert!(stale("master").is_empty());
    }

    #[test]
    fn branches_count_commits_against_their_upstream() {
        let fixture = forked();
        let path = fixture.path();
        fixture.git(".", &["remote", "add", "origin", &path]);
        fixture.git(".", &["fetch", "-q", "origin"]);
        fixture.git(".", &["branch", "-q", "-u", "origin/master", "feature"]);
        let graph = fixture.graph();
        let feature = graph.branches.iter().find(|b| b.name == "feature").unwrap();
        assert_eq!(feature.upstream.as_deref(), Some("origin/master"));
        assert_eq!((feature.ahead, feature.behind), (2, 1));
        let master = graph.branches.iter().find(|b| b.name == "master").unwrap();
        assert_eq!(master.upstream, None);
        // Remote branches are listed for attention, but not drawn by default.
        let remote = graph
            .branches
            .iter()
            .find(|b| b.name == "origin/master")
            .unwrap();
        assert!(remote.remote && !remote.shown);
        assert_eq!(parse_track("ahead 3"), (3, 0));
        assert_eq!(parse_track("gone"), (0, 0));
    }

    #[test]
    fn first_push_sets_the_upstream() {
        let fixture = forked();
        let path = fixture.path();
        fixture.git(".", &["init", "-q", "--bare", "remote.git"]);
        fixture.git(
            ".",
            &["remote", "add", "origin", &format!("{path}/remote.git")],
        );
        crate::git::push(&path, None, &[]).unwrap();
        let upstream = fixture.git(".", &["rev-parse", "--abbrev-ref", "@{upstream}"]);
        assert_eq!(upstream.trim(), "origin/master");
        // Once tracked, a push goes where it tracks.
        fixture.commit(".", "a.txt", "1\n2\n3\n4\n", "c4");
        crate::git::push(&path, None, &[]).unwrap();
        let graph = fixture.graph();
        let master = graph.branches.iter().find(|b| b.name == "master").unwrap();
        assert_eq!((master.ahead, master.behind), (0, 0));
        // Pushed to another remote, it keeps tracking the first.
        fixture.git(".", &["init", "-q", "--bare", "fork.git"]);
        fixture.git(".", &["remote", "add", "fork", &format!("{path}/fork.git")]);
        crate::git::push(&path, Some("fork"), &[]).unwrap();
        let forked = fixture.git("fork.git", &["rev-parse", "master"]);
        assert_eq!(forked, fixture.git(".", &["rev-parse", "master"]));
        let upstream = fixture.git(".", &["rev-parse", "--abbrev-ref", "@{upstream}"]);
        assert_eq!(upstream.trim(), "origin/master");
        assert_eq!(fixture.graph().remotes, ["fork", "origin"]);
    }

    #[test]
    fn a_lone_branch_shows_its_commits() {
        let fixture = Fixture::new();
        fixture.commit(".", "a.txt", "1\n", "c1");
        fixture.commit(".", "a.txt", "2\n", "c2");
        fixture.commit(".", "a.txt", "3\n", "c3");
        let graph = fixture.graph();
        let summaries: Vec<Vec<&str>> = graph
            .nodes
            .iter()
            .filter(|node| node.kind != NodeKind::Uncommitted)
            .map(|node| node.commits.iter().map(|c| c.summary.as_str()).collect())
            .collect();
        assert_eq!(summaries, [vec!["c1"], vec!["c2", "c3"]]);
    }

    #[test]
    fn worktrees_are_listed_and_can_be_hidden() {
        let fixture = Fixture::new();
        fixture.commit(".", "a.txt", "1\n", "c1");
        fixture.git(".", &["worktree", "add", "-q", "-b", "agent", "wt"]);
        fixture.commit("wt", "a.txt", "2\n", "a1");
        fixture.git(".", &["branch", "idle"]);
        let worktree_node = |graph: &WorkflowGraph| {
            graph
                .nodes
                .iter()
                .any(|node| node.kind == NodeKind::Uncommitted && node.refs == ["agent"])
        };

        let only_worktrees = Attention {
            scope: crate::Scope::Worktrees,
            ..Attention::default()
        };
        let graph = fixture.graph_with(&only_worktrees);
        let agent = graph.branches.iter().find(|b| b.name == "agent").unwrap();
        assert!(agent.worktree && agent.shown);
        assert!(worktree_node(&graph));
        let idle = graph.branches.iter().find(|b| b.name == "idle").unwrap();
        assert!(!idle.worktree && !idle.shown);

        let hidden = Attention {
            hidden: vec!["agent".to_string()],
            ..only_worktrees
        };
        let graph = fixture.graph_with(&hidden);
        assert!(!worktree_node(&graph));
        assert!(
            !graph
                .branches
                .iter()
                .find(|b| b.name == "agent")
                .unwrap()
                .shown
        );
    }

    #[test]
    fn remote_branches_and_tags_follow_attention() {
        let fixture = forked();
        fixture.git(
            ".",
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/repo.git",
            ],
        );
        fixture.git(
            ".",
            &["update-ref", "refs/remotes/origin/feature", "feature"],
        );
        fixture.git(".", &["update-ref", "refs/remotes/origin/far", "master~1"]);
        fixture.git(".", &["tag", "v1", "master"]);
        fixture.git(".", &["tag", "-a", "-m", "release", "v0", "feature~1"]);
        let labels = |graph: &WorkflowGraph| -> (Vec<String>, Vec<String>) {
            let refs = graph.nodes.iter().flat_map(|n| n.refs.clone()).collect();
            let tags = graph.nodes.iter().flat_map(|n| n.tags.clone()).collect();
            (refs, tags)
        };

        let (refs, tags) = labels(&fixture.graph());
        assert!(!refs.iter().any(|r| r.starts_with("origin/")));
        assert!(tags.is_empty());

        let attention = Attention {
            remotes: true,
            tags: true,
            ..Attention::default()
        };
        let graph = fixture.graph_with(&attention);
        let (refs, mut tags) = labels(&graph);
        assert!(refs.contains(&"origin/feature".to_string()));
        assert!(refs.contains(&"origin/far".to_string()));
        tags.sort();
        assert_eq!(tags, ["v0", "v1"]);
        let remote = graph
            .branches
            .iter()
            .find(|b| b.name == "origin/feature")
            .unwrap();
        assert!(remote.remote && remote.shown);
    }

    #[test]
    fn branch_without_shared_history_is_left_out() {
        let fixture = forked();
        fixture.git(".", &["checkout", "-q", "--orphan", "pages"]);
        fixture.git(".", &["rm", "-rq", "--cached", "."]);
        fixture.commit(".", "index.html", "<p>\n", "pages");
        fixture.git(".", &["checkout", "-q", "-f", "master"]);

        let graph = fixture.graph();
        let pages = graph.branches.iter().find(|b| b.name == "pages").unwrap();
        assert!(!pages.shown);
        assert!(pages.unrelated);
        assert!(graph.nodes.iter().any(|node| node.refs == ["feature"]));
    }

    #[test]
    fn stages_unstages_and_commits() {
        let fixture = Fixture::new();
        fixture.commit(".", "a.txt", "1\n", "c1");
        fixture.write(".", "a.txt", "1\n2\n");
        fixture.write(".", "new.txt", "n\n");
        let path = fixture.path();
        let names = |files: &[crate::ChangedFile]| -> Vec<String> {
            let mut names: Vec<String> = files
                .iter()
                .filter_map(|file| file.new_path.clone())
                .collect();
            names.sort();
            names
        };

        let status = crate::worktree_status(&path).unwrap();
        assert!(status.staged.is_empty());
        assert_eq!(names(&status.unstaged), ["a.txt", "new.txt"]);

        crate::stage(&path, &["a.txt".to_string()]).unwrap();
        let status = crate::worktree_status(&path).unwrap();
        assert_eq!(names(&status.staged), ["a.txt"]);
        assert_eq!(names(&status.unstaged), ["new.txt"]);
        // A staged file reads from the index, an untracked one from disk.
        let staged = crate::file_diff(
            path.clone(),
            status.staged[0].clone(),
            crate::DiffMode::Text,
        )
        .unwrap();
        assert!(
            staged
                .rows
                .iter()
                .any(|row| row.right.as_ref().is_some_and(|side| side.text == "2"))
        );

        crate::unstage(&path, &[]).unwrap();
        assert!(crate::worktree_status(&path).unwrap().staged.is_empty());

        crate::stage(&path, &[]).unwrap();
        // Commits run with the user's identity; the test repository has its own.
        fixture.git(".", &["config", "user.name", "Test"]);
        fixture.git(".", &["config", "user.email", "test@example.invalid"]);
        crate::commit(&path, "Add the second line\n\nAnd a new file.", false).unwrap();
        let status = crate::worktree_status(&path).unwrap();
        assert!(status.staged.is_empty() && status.unstaged.is_empty());
        let message = fixture.git(".", &["log", "-1", "--format=%B"]);
        assert_eq!(message, "Add the second line\n\nAnd a new file.");

        // Rewording keeps the commit's changes and leaves staged ones staged.
        fixture.write(".", "a.txt", "1\n2\n3\n");
        crate::stage(&path, &[]).unwrap();
        let head = fixture.git(".", &["rev-parse", "HEAD"]);
        crate::reword(&path, &head, "Second line, reworded").unwrap();
        let message = fixture.git(".", &["log", "-1", "--format=%B"]);
        assert_eq!(message, "Second line, reworded");
        let files = fixture.git(".", &["show", "--format=", "--name-only", "HEAD"]);
        assert_eq!(files, "a.txt\nnew.txt");
        assert_eq!(
            names(&crate::worktree_status(&path).unwrap().staged),
            ["a.txt"]
        );

        // Amending takes the staged change into the latest commit.
        assert_eq!(crate::head_message(&path).unwrap(), "Second line, reworded");
        crate::commit(&path, "Three lines", true).unwrap();
        assert!(crate::worktree_status(&path).unwrap().staged.is_empty());
        assert_eq!(fixture.git(".", &["rev-list", "--count", "HEAD"]), "2");
        assert_eq!(crate::head_message(&path).unwrap(), "Three lines");

        // An older commit is reworded and the later one copied onto it.
        let first = fixture.git(".", &["rev-parse", "HEAD~1"]);
        crate::reword(&path, &first, "The first commit").unwrap();
        let messages = fixture.git(".", &["log", "--format=%s"]);
        assert_eq!(messages, "Three lines\nThe first commit");
        assert_eq!(fixture.git(".", &["rev-list", "--count", "HEAD"]), "2");
    }

    #[test]
    fn stashes_are_listed_with_what_they_hold() {
        let fixture = Fixture::new();
        fixture.commit(".", "a.txt", "1\n", "c1");
        fixture.commit(".", "b.txt", "1\n", "c2");
        fixture.write(".", "a.txt", "staged\n");
        fixture.git(".", &["add", "a.txt"]);
        fixture.write(".", "b.txt", "unstaged\n");
        fixture.write(".", "new.txt", "untracked\n");
        fixture.git(".", &["stash", "push", "-u", "-m", "try"]);
        let attention = Attention {
            stashes: true,
            ..Attention::default()
        };
        let graph = fixture.graph_with(&attention);
        let [stash] = &graph.stashes[..] else {
            panic!("one stash")
        };
        assert_eq!(stash.name, "stash@{0}");
        assert_eq!(stash.message, "try");
        assert_eq!(stash.branch.as_deref(), Some("master"));
        assert_eq!(stash.base, fixture.git(".", &["rev-parse", "HEAD"]));
        assert!(graph.nodes.iter().any(|node| node.stashes == ["stash@{0}"]));

        let (files, split) = crate::stash_files(&fixture.path(), stash).unwrap();
        let names: Vec<&str> = files
            .iter()
            .map(|file| file.new_path.as_deref().unwrap_or_default())
            .collect();
        assert_eq!((names, split), (vec!["b.txt", "new.txt", "a.txt"], 2));
    }

    #[test]
    fn discards_lines_and_files() {
        let fixture = Fixture::new();
        let lines: Vec<String> = (1..=10).map(|n| n.to_string()).collect();
        fixture.commit(".", "a.txt", &format!("{}\n", lines.join("\n")), "c1");
        fixture.commit(".", "b.txt", "b\n", "c2");
        let mut changed = lines.clone();
        changed[1] = "TWO".to_string();
        changed[8] = "NINE".to_string();
        fixture.write(".", "a.txt", &format!("{}\n", changed.join("\n")));
        fixture.write(".", "b.txt", "changed\n");
        fixture.write(".", "new.txt", "x\n");
        let path = fixture.path();
        let file = |name: &str| {
            crate::worktree_status(&path)
                .unwrap()
                .unstaged
                .into_iter()
                .find(|f| f.new_path.as_deref() == Some(name))
                .unwrap()
        };
        let set = |n: &[u32]| {
            n.iter()
                .copied()
                .collect::<std::collections::HashSet<u32>>()
        };
        let read = |name: &str| std::fs::read_to_string(format!("{path}/{name}")).unwrap();

        // One changed line goes back; the other stays.
        crate::discard_lines(&path, &path, &file("a.txt"), &set(&[2]), &set(&[2])).unwrap();
        assert!(read("a.txt").contains("\n2\n") && read("a.txt").contains("NINE"));

        crate::discard(&path, &[file("b.txt"), file("new.txt")]).unwrap();
        assert_eq!(read("b.txt"), "b\n");
        assert!(!std::path::Path::new(&format!("{path}/new.txt")).exists());
    }

    #[test]
    fn stages_and_unstages_single_lines() {
        let fixture = Fixture::new();
        let lines: Vec<String> = (1..=10).map(|n| n.to_string()).collect();
        fixture.commit(".", "a.txt", &format!("{}\n", lines.join("\n")), "c1");
        let mut changed = lines.clone();
        changed[1] = "TWO".to_string();
        changed[8] = "NINE".to_string();
        fixture.write(".", "a.txt", &format!("{}\n", changed.join("\n")));
        fixture.write(".", "new.txt", "x\ny\n");
        let path = fixture.path();
        let set = |n: &[u32]| {
            n.iter()
                .copied()
                .collect::<std::collections::HashSet<u32>>()
        };
        let staged = || fixture.git(".", &["diff", "--cached"]);

        let status = crate::worktree_status(&path).unwrap();
        let file = |name: &str| {
            status
                .unstaged
                .iter()
                .find(|f| f.new_path.as_deref() == Some(name))
                .unwrap()
                .clone()
        };
        crate::stage_lines(&path, &path, &file("a.txt"), &set(&[2]), &set(&[2]), false).unwrap();
        assert!(staged().contains("+TWO"));
        assert!(
            !staged().contains("NINE"),
            "only the selected line is staged"
        );

        // Part of an untracked file.
        crate::stage_lines(&path, &path, &file("new.txt"), &set(&[]), &set(&[2]), false).unwrap();
        assert_eq!(fixture.git(".", &["show", ":new.txt"]), "y");

        let status = crate::worktree_status(&path).unwrap();
        let staged_a = status
            .staged
            .iter()
            .find(|f| f.new_path.as_deref() == Some("a.txt"))
            .unwrap()
            .clone();
        crate::stage_lines(&path, &path, &staged_a, &set(&[2]), &set(&[2]), true).unwrap();
        assert!(!staged().contains("TWO"), "and unstaged again");
    }

    #[test]
    fn old_merges_read_as_plain_commits() {
        let old = "2020-01-01T00:00:00";
        let fixture = Fixture::new();
        fixture.commit_at(old, "c1");
        fixture.git(".", &["branch", "release"]);
        fixture.git(".", &["checkout", "-q", "-b", "side"]);
        fixture.commit_at(old, "s1");
        fixture.git(".", &["checkout", "-q", "master"]);
        fixture.commit_at(old, "c2");
        fixture.git(".", &["merge", "-q", "--no-ff", "--no-commit", "side"]);
        fixture.commit_at(old, "old merge");
        fixture.git(".", &["branch", "-q", "-D", "side"]);
        fixture.commit(".", "a.txt", "new\n", "c3");
        // A recent backport on a release line that forked before the merge.
        fixture.git(".", &["checkout", "-q", "release"]);
        fixture.commit(".", "b.txt", "fix\n", "r1");
        fixture.git(".", &["checkout", "-q", "master"]);

        let graph = fixture.graph();
        let all: Vec<&str> = graph.nodes.iter().flat_map(summaries).collect();
        assert!(
            !all.contains(&"s1"),
            "the old merged branch is left out: {all:?}"
        );
        let main = &graph.nodes[graph.main_node.unwrap()];
        assert_eq!(summaries(main), ["c2", "old merge", "c3"]);
        assert_eq!(main.parents, [0]);

        // With all time in attention, the old branch is back.
        let attention = Attention {
            window_days: None,
            ..Attention::default()
        };
        let graph = fixture.graph_with(&attention);
        let all: Vec<&str> = graph.nodes.iter().flat_map(summaries).collect();
        assert!(all.contains(&"s1"));
    }

    #[test]
    fn sums_numstat_and_skips_binary() {
        assert_eq!(numstat("3\t1\ta.rs\n-\t-\tlogo.png\n2\t0\tb.rs\n"), (5, 1));
    }
}
