use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::git::{self, LOG_FORMAT};
use crate::{Attention, Commit, CoreError, Revision};

/// Untracked files larger than this are not counted in diff sizes.
const MAX_COUNTED_FILE: u64 = 1 << 20;
/// At most this many commits show before the oldest fork point, those in
/// the attention window.
const RECENT_COMMITS: usize = 50;
/// At least this many commits show before the oldest fork point, however
/// old, so a small repository shows whole.
const MIN_CONTEXT: usize = 20;

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
    /// Commits of a release line, a channel's, that work grows from: not
    /// the work's own.
    pub channel: bool,
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
    /// The folder of the main worktree, which git never removes; `None` in
    /// a bare repository.
    pub main_worktree: Option<String>,
    /// Which nodes the main branch's history holds, by node.
    pub on_main: Vec<bool>,
    /// The channel patterns, as git config keeps them, global and the
    /// repository's own; the main branch is a channel besides.
    pub channels: Vec<String>,
    /// Days without a commit after which a branch is idle, when git config
    /// sets them.
    pub idle_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
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
    /// A channel: the main branch, or one the channel patterns name. Work
    /// merged into one is done; a channel itself is never left behind.
    pub channel: bool,
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

    /// Which nodes the main branch's history holds: its tip's node and every
    /// node back from it.
    pub fn main_history(&self) -> Vec<bool> {
        let mut on_main = vec![false; self.nodes.len()];
        let mut stack: Vec<usize> = self.main_node.into_iter().collect();
        while let Some(node) = stack.pop() {
            if !std::mem::replace(&mut on_main[node], true) {
                stack.extend(&self.nodes[node].parents);
            }
        }
        on_main
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
    let remotes = remote_names(repo_path)?;
    let channels = crate::channels::channel_patterns(repo_path);
    let (mut tips, mut branches, main_worktree) =
        tips(repo_path, attention, now, &remotes, &channels)?;
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
    // Some of the main line before the oldest fork point shows too: its
    // recent commits, and a few more however old. When every tip sits on
    // one line, as in a repository without branches, they are all there is.
    let base = recent_base(repo_path, &base, attention, now)?;
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
    let ids = tip_ids(&tips);

    // Newest first, with the base last. Before the attention window only the
    // tips' first-parent lines count, so old merges read as plain commits and
    // old merged branches stay out.
    let range: Vec<String> = ids
        .iter()
        .map(|id| id.to_string())
        .chain([format!("^{base}")])
        .collect();
    let mut log = match attention.since(now) {
        None => commits(repo_path, &[], &range)?,
        Some(since) => {
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
    // A commit a shown stash was made on ends its run, as a tip does, so the
    // stash's mark sits right after it.
    let stashes = git::stashes(repo_path)?;
    let stash_bases: HashSet<&str> = match attention.stashes {
        true => stashes.iter().map(|stash| stash.base.as_str()).collect(),
        false => HashSet::new(),
    };
    // Every tag, newest first, and the commit each labels. A commit a shown
    // tag is on ends its run, as a stash's does, so the tag sits right
    // after it.
    let tag_list = git::run(
        repo_path,
        &[
            "for-each-ref",
            "--sort=-creatordate",
            "--format=%(refname:short)%09%(objectname)%09%(*objectname)",
            "refs/tags",
        ],
    )?;
    let mut tags = Vec::new();
    let mut tagged: Vec<(String, String)> = Vec::new();
    for line in tag_list.lines() {
        // Annotated tags point at a tag object; the commit is the peeled one.
        let fields: Vec<&str> = line.split('\t').collect();
        let [name, id, peeled] = fields[..] else {
            continue;
        };
        tags.push(name.to_string());
        if attention.tags {
            let commit = if peeled.is_empty() { id } else { peeled };
            tagged.push((name.to_string(), commit.to_string()));
        }
    }
    let tag_commits: HashSet<&str> = tagged.iter().map(|(_, commit)| commit.as_str()).collect();

    let (channel_ends, channel_commits) = release_lines(&log, &index, &tips[0].id, &branches);
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
            && !stash_bases.contains(log[p].0.id.as_str())
            && !tag_commits.contains(log[p].0.id.as_str())
            && !channel_ends.contains_key(log[p].0.id.as_str())
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
            channel: channel_commits.contains(commit.id.as_str()),
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
                let (added, removed, touched) = worktree_stats(path)?;
                // Its last work: the newest commit, or a change since.
                let time = log[i].0.time.max(touched);
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
                    channel: false,
                });
            }
        }
    }

    // Where a release line ends, its branch is named, as a tip's is.
    for (id, name) in &channel_ends {
        let node = &mut nodes[node_of[index[id.as_str()]]];
        if !node.refs.contains(name) {
            node.refs.push(name.clone());
        }
    }

    for (name, commit) in &tagged {
        if let Some(&i) = index.get(commit.as_str()) {
            nodes[node_of[i]].tags.push(name.clone());
        }
    }

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
    let mut graph = WorkflowGraph {
        nodes,
        lane_count,
        main_node,
        main: tips[0].name.clone(),
        branches,
        older,
        remotes,
        main_worktree,
        on_main: Vec::new(),
        tags,
        stashes,
        channels,
        idle_days: crate::project::idle_days(repo_path),
    };
    graph.on_main = graph.main_history();
    Ok(graph)
}

/// The release lines work grows from: channel branches, other than the
/// main one, whose commits the graph holds off the main line. Of a local
/// channel and its remote one, the newer stands, so work on a release
/// line counts from where the line is now. Returns where each line ends,
/// with its branch's name, and the lines' commits.
fn release_lines(
    log: &Log,
    index: &HashMap<&str, usize>,
    main: &str,
    branches: &[Branch],
) -> (HashMap<String, String>, HashSet<String>) {
    // What the main line holds: everything back from its tip.
    let ancestors = |from: Vec<usize>, stop: &HashSet<usize>| {
        let mut seen = HashSet::new();
        let mut stack = from;
        while let Some(i) = stack.pop() {
            if stop.contains(&i) || !seen.insert(i) {
                continue;
            }
            stack.extend(
                log[i]
                    .1
                    .iter()
                    .filter_map(|p| index.get(p.as_str()).copied()),
            );
        }
        seen
    };
    let on_main = ancestors(
        index.get(main).copied().into_iter().collect(),
        &HashSet::new(),
    );
    // By the line's own name, `v2607` for `origin/v2607` too: the newest,
    // first in the log's order.
    let mut lines: HashMap<&str, (usize, &str)> = HashMap::new();
    for branch in branches.iter().filter(|b| b.channel && b.commit != main) {
        let Some(&at) = index.get(branch.commit.as_str()) else {
            continue;
        };
        if on_main.contains(&at) {
            continue;
        }
        let short = match branch.remote {
            true => branch
                .name
                .split_once('/')
                .map_or(branch.name.as_str(), |(_, b)| b),
            false => branch.name.as_str(),
        };
        let line = lines.entry(short).or_insert((at, &branch.name));
        if at < line.0 {
            *line = (at, &branch.name);
        }
    }
    let ends: HashMap<String, String> = lines
        .values()
        .map(|&(at, name)| (log[at].0.id.clone(), name.to_string()))
        .collect();
    let commits = ancestors(lines.values().map(|&(at, _)| at).collect(), &on_main)
        .into_iter()
        .map(|i| log[i].0.id.clone())
        .collect();
    (ends, commits)
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
    let line = |since: Option<i64>, count: usize| {
        let mut args = vec![
            "rev-list".to_string(),
            "--first-parent".to_string(),
            format!("--max-count={count}"),
        ];
        args.extend(since.map(|since| format!("--since=@{since}")));
        args.push(tip.to_string());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        git::run(repo_path, &args)
    };
    // The attention window's commits, or the few always shown when it holds
    // fewer.
    let recent = line(attention.since(now), RECENT_COMMITS)?;
    let few = line(None, MIN_CONTEXT)?;
    let listed = match recent.lines().count() < few.lines().count() {
        true => few,
        false => recent,
    };
    let oldest = listed.lines().last().unwrap_or(tip);
    // `^@` names the parents, none for the root, without failing.
    let parents = git::run(repo_path, &["rev-parse", &format!("{oldest}^@")])?;
    Ok(parents.lines().next().unwrap_or(oldest).to_string())
}

/// The tips' commits, each once.
fn tip_ids(tips: &[Tip]) -> Vec<&str> {
    let mut ids: Vec<&str> = tips.iter().map(|tip| tip.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// The newest commit every tip descends from.
fn octopus_base(repo_path: &str, tips: &[Tip]) -> Result<String, CoreError> {
    let ids = tip_ids(tips);
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

type Tips = (Vec<Tip>, Vec<Branch>, Option<String>);

/// The main branch first, then worktrees, then the branches attention shows;
/// all branches outside worktrees with whether they are shown; and the main
/// worktree's folder.
fn tips(
    repo_path: &str,
    attention: &Attention,
    now: i64,
    remotes: &[String],
    channels: &[String],
) -> Result<Tips, CoreError> {
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
    let (worktrees, main_worktree) = worktrees(repo_path)?;
    for (path, head, branch) in worktrees {
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
            shown,
            worktree: true,
            ..Branch::default()
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
        shown: true,
        worktree: tips[0].worktree.is_some(),
        ..Branch::default()
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
    for line in refs.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [name, id, time, author] = fields[..] else {
            continue;
        };
        let time = time.parse().unwrap_or_default();
        // `origin/HEAD` names another branch, and a bare `origin` is its short form.
        let remote = remotes
            .iter()
            .any(|remote| name.starts_with(&format!("{remote}/")));
        if name.ends_with("/HEAD") || remotes.iter().any(|r| r == name) {
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
            remote,
            ..Branch::default()
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
    mark_stale(repo_path, &main, channels, &mut branches)?;
    Ok((tips, branches, main_worktree))
}

/// Notes why local branches look left behind: a channel has merged them,
/// or their remote branch is gone. How long one sat idle is up to the
/// reader, from its time. Notes too how far each is from its upstream, and
/// which branches are channels, which are never left behind.
fn mark_stale(
    repo_path: &str,
    main: &str,
    channels: &[String],
    branches: &mut [Branch],
) -> Result<(), CoreError> {
    for branch in branches.iter_mut() {
        branch.channel = crate::channels::is_channel(channels, main, &branch.name, branch.remote);
    }
    // Merged into any channel at all, in one go: git lists a branch once
    // any of the `--merged` commits holds it.
    let mut args = vec![
        "for-each-ref".to_string(),
        "--format=%(refname:short)".to_string(),
        format!("--merged={main}"),
    ];
    args.extend(
        branches
            .iter()
            .filter(|branch| branch.channel && branch.remote)
            .map(|branch| format!("--merged=refs/remotes/{}", branch.name)),
    );
    args.push("refs/heads".to_string());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let merged = git::run(repo_path, &args)?;
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
            if track == "gone" && !branch.channel {
                branch.stale.push("remote gone".to_string());
            }
        }
        if !branch.channel && merged.contains(branch.name.as_str()) {
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

type Worktrees = (Vec<(String, String, Option<String>)>, Option<String>);

/// Path, HEAD and branch of each usable worktree, the main one first; and
/// the main worktree's path, unless the repository is bare.
fn worktrees(repo_path: &str) -> Result<Worktrees, CoreError> {
    let raw = git::run(repo_path, &["worktree", "list", "--porcelain"])?;
    let mut list = Vec::new();
    // Git lists the main worktree first.
    let main = raw
        .split("\n\n")
        .next()
        .filter(|block| !block.lines().any(|line| line == "bare"))
        .and_then(|block| block.lines().next()?.strip_prefix("worktree "))
        .map(str::to_string);
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
    Ok((list, main))
}

/// Lines added and removed in the worktree, untracked files included.
/// Work inside submodules is left out: git would look into each of them
/// on every refresh, and their lines are not the worktree's. A submodule
/// moved to another commit still counts.
fn worktree_stats(path: &str) -> Result<(u32, u32, i64), CoreError> {
    let raw = git::run(
        path,
        &["diff", "--numstat", "--ignore-submodules=dirty", "HEAD"],
    )?;
    let (mut added, removed) = numstat(&raw);
    // When the changes were last touched: the newest of their files.
    let mut touched = 0;
    let mut touch = |meta: &fs::Metadata| {
        let modified = meta.modified().ok();
        let since = modified.and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
        touched = touched.max(since.map_or(0, |since| since.as_secs() as i64));
    };
    for name in raw.lines().filter_map(|line| line.splitn(3, '\t').nth(2)) {
        if let Ok(meta) = fs::metadata(Path::new(path).join(name)) {
            touch(&meta);
        }
    }
    for file in git::untracked(path)? {
        let file = Path::new(path).join(file);
        let Ok(meta) = fs::metadata(&file) else {
            continue;
        };
        touch(&meta);
        if meta.len() <= MAX_COUNTED_FILE
            && let Ok(bytes) = fs::read(&file)
            && !bytes.contains(&0)
        {
            added += bytes.iter().filter(|&&byte| byte == b'\n').count() as u32;
        }
    }
    Ok((added, removed, touched))
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

        // Before the fork, the main line shows to its first commit.
        let history = &graph.nodes[0];
        assert_eq!(history.kind, NodeKind::History);
        assert_eq!(summaries(history), vec!["c1"]);

        let (feature_ix, feature) = node(&graph, "feature");
        assert_eq!(feature.kind, NodeKind::Commits);
        assert_eq!(summaries(feature), vec!["f1", "f2"]);
        // The fork point, c2, ends the run before it.
        let fork = &graph.nodes[feature.parents[0]];
        assert_eq!(summaries(fork), vec!["c2"]);
        assert_eq!(fork.parents, vec![0]);

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

        let top = fixture.git(".", &["rev-parse", "--show-toplevel"]);
        assert_eq!(graph.main_worktree, Some(top));

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
        let diff = crate::file_diff(
            fixture.path(),
            a.clone(),
            DiffMode::Text,
            &Default::default(),
        )
        .unwrap();
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
        assert_eq!(summaries, vec!["f2", "f1", "c2", "c1"]);

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
        for n in 1..=60 {
            fixture.commit(".", "a.txt", &format!("{n}\n"), &format!("c{n}"));
        }
        fixture.git(".", &["checkout", "-q", "-b", "feature", "HEAD~1"]);
        fixture.commit(".", "b.txt", "x\n", "f1");
        fixture.git(".", &["checkout", "-q", "master"]);
        let graph = fixture.graph();
        assert_eq!(
            graph.older, 8,
            "50 recent commits show from the fork at c59; c1..c8 lie before c9"
        );
        let more = crate::workflow_graph(&fixture.path(), &Attention::default(), true, 2).unwrap();
        assert_eq!(more.older, 6);
        let all = crate::workflow_graph(&fixture.path(), &Attention::default(), true, 100).unwrap();
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
    fn a_submodule_change_reads_as_the_commits_it_moves_over() {
        // lib is a repository of its own, added to app as a submodule.
        let lib = Fixture::new();
        lib.commit(".", "l.txt", "1\n", "lib one");
        let app = Fixture::new();
        app.commit(".", "a.txt", "1\n", "app one");
        let url = lib.path();
        app.git(
            ".",
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                &url,
                "lib",
            ],
        );
        app.git(".", &["commit", "-q", "-m", "add lib"]);
        // The worktree's lib moves on two commits, not yet committed in app.
        app.commit("lib", "l.txt", "1\n2\n", "lib two");
        app.commit("lib", "l.txt", "1\n2\n3\n", "lib three");

        let path = app.path();
        let worktree = crate::Revision::Worktree(path.clone());
        let files = crate::git::changed_files(&path, Some("HEAD"), &worktree).unwrap();
        let lib_change = files.iter().find(|f| f.path() == "lib").unwrap();
        assert!(lib_change.submodule);
        let (old, new) = crate::git::read_blobs(&path, lib_change).unwrap();
        let (old, new) = (
            String::from_utf8(old).unwrap(),
            String::from_utf8(new).unwrap(),
        );
        assert!(old.starts_with("Submodule lib\nat "), "{old}");
        assert!(!old.contains("lib two"));
        assert!(
            new.contains("lib three") && new.contains("lib two"),
            "{new}"
        );
        assert!(!new.contains("uncommitted"));
        // Changes inside are said too.
        app.write("lib", "l.txt", "dirty\n");
        let (_, new) = crate::git::read_blobs(&path, lib_change).unwrap();
        assert!(
            String::from_utf8(new)
                .unwrap()
                .contains("uncommitted changes inside")
        );
        // A snapshot shows where it points, not an unreadable folder.
        let text = crate::snapshot_file(&path, &worktree, "lib").unwrap();
        assert!(text.starts_with("Submodule lib\n"));
        let head = app.git(".", &["rev-parse", "HEAD"]);
        let text = crate::snapshot_file(&path, &crate::Revision::Commit(head), "lib").unwrap();
        assert!(text.starts_with("Submodule lib\n"));
        // Its move committed, it is changed only inside: nothing to commit
        // here, so it is listed apart.
        app.git(".", &["commit", "-q", "-am", "move lib"]);
        let status = crate::git::worktree_status(&path).unwrap();
        assert_eq!(status.inside, ["lib"]);
        assert!(status.unstaged.iter().all(|f| f.path() != "lib"));
    }

    #[test]
    fn work_on_a_release_line_counts_from_where_the_line_is_now() {
        // Release v1 forks from master at c2; the remote has it at f2, the
        // local branch is behind at f1; fix is work on it.
        let fixture = forked();
        let path = fixture.path();
        fixture.git(".", &["branch", "v1", "feature~1"]);
        fixture.git(".", &["remote", "add", "origin", &path]);
        fixture.git(".", &["update-ref", "refs/remotes/origin/v1", "feature"]);
        fixture.git(".", &["checkout", "-q", "-b", "fix", "feature"]);
        fixture.commit(".", "c.txt", "1\n", "x1");
        fixture.commit(".", "c.txt", "1\n2\n", "x2");
        fixture.git(".", &["checkout", "-q", "master"]);
        fixture.git(".", &["branch", "-q", "-D", "feature"]);

        // Without channels the line's newer commits pass for the work's.
        let graph = fixture.graph();
        assert_eq!(summaries(node(&graph, "fix").1), ["f2", "x1", "x2"]);

        crate::add_channel(&path, "v[0-9]").unwrap();
        let graph = fixture.graph();
        let (_, fix) = node(&graph, "fix");
        assert_eq!(summaries(fix), ["x1", "x2"], "only the work's own");
        assert!(!fix.channel);
        // The line ends at the remote's newer tip, labelled, and is muted.
        let (_, line) = node(&graph, "origin/v1");
        assert_eq!(summaries(line), ["f2"]);
        assert!(line.channel);
        assert!(node(&graph, "v1").1.channel);
        assert!(
            graph
                .nodes
                .iter()
                .filter(|n| n.channel)
                .all(|n| !n.commits.is_empty())
        );
        let (_, master) = node(&graph, "master");
        assert!(!master.channel, "the main line is never a release line");
    }

    #[test]
    fn branches_merged_into_a_channel_are_merged() {
        // A release branch on the remote has the fix; master does not.
        let fixture = forked();
        let path = fixture.path();
        fixture.git(".", &["branch", "v2410", "feature"]);
        fixture.git(".", &["branch", "fix", "feature~1"]);
        fixture.git(".", &["remote", "add", "origin", &path]);
        fixture.git(".", &["fetch", "-q", "origin"]);
        fixture.git(".", &["branch", "-q", "-D", "v2410"]);
        let stale = |graph: &WorkflowGraph, name: &str| {
            let branch = graph.branches.iter().find(|b| b.name == name).unwrap();
            (branch.channel, branch.stale.clone())
        };
        let graph = fixture.graph();
        assert_eq!(stale(&graph, "fix"), (false, vec![]));

        crate::add_channel(&path, "v[0-9][0-9][0-9][0-9]").unwrap();
        crate::add_channel(&path, "v[0-9][0-9][0-9][0-9]").unwrap();
        let graph = fixture.graph();
        assert_eq!(graph.channels, ["v[0-9][0-9][0-9][0-9]"], "added once");
        assert_eq!(stale(&graph, "fix"), (false, vec!["merged".to_string()]));
        assert_eq!(stale(&graph, "origin/v2410"), (true, vec![]));
        assert_eq!(stale(&graph, "master"), (true, vec![]));

        crate::remove_channel(&path, "v[0-9][0-9][0-9][0-9]").unwrap();
        let graph = fixture.graph();
        assert!(graph.channels.is_empty());
        assert_eq!(stale(&graph, "fix"), (false, vec![]));
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
    fn snapshots_list_and_read_files_at_a_revision() {
        let fixture = forked();
        let path = fixture.path();
        let master = fixture
            .git(".", &["rev-parse", "master"])
            .trim()
            .to_string();
        let feature = fixture
            .git(".", &["rev-parse", "feature"])
            .trim()
            .to_string();
        let at = |id: &str| crate::Revision::Commit(id.to_string());
        assert_eq!(
            crate::snapshot_files(&path, &at(&master)).unwrap(),
            ["a.txt"]
        );
        assert_eq!(
            crate::snapshot_files(&path, &at(&feature)).unwrap(),
            ["a.txt", "b.txt"]
        );
        assert_eq!(
            crate::snapshot_file(&path, &at(&feature), "b.txt").unwrap(),
            "x\ny\n"
        );
        assert!(crate::snapshot_file(&path, &at(&master), "b.txt").is_err());
        // A worktree: its files on disk, new ones included.
        std::fs::write(format!("{path}/new.txt"), "n\n").unwrap();
        let worktree = crate::Revision::Worktree(path.clone());
        assert_eq!(
            crate::snapshot_files(&path, &worktree).unwrap(),
            ["a.txt", "new.txt"]
        );
        assert_eq!(
            crate::snapshot_file(&path, &worktree, "new.txt").unwrap(),
            "n\n"
        );
        std::fs::write(format!("{path}/bin"), [0u8, 1, 2]).unwrap();
        assert!(crate::snapshot_file(&path, &worktree, "bin").is_err());
        // Search finds lines, ignoring case: in a commit's tree, and in a
        // worktree's files, new ones too.
        let found = crate::snapshot_grep(&path, &at(&feature), "Y");
        assert_eq!(
            found,
            [crate::Found {
                path: "b.txt".to_string(),
                line: 2,
                text: "y".to_string(),
            }]
        );
        let found = crate::snapshot_grep(&path, &worktree, "n");
        assert_eq!(
            found.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["new.txt"]
        );
        assert!(crate::snapshot_grep(&path, &worktree, "nothing").is_empty());
    }

    #[test]
    fn a_file_history_knows_where_a_commit_stands() {
        let fixture = Fixture::new();
        fixture.commit(".", "a.txt", "1\n", "a1");
        fixture.commit(".", "b.txt", "1\n", "b1");
        fixture.commit(".", "a.txt", "2\n", "a2");
        fixture.commit(".", "b.txt", "2\n", "b2");
        let path = fixture.path();
        let id = |rev: &str| fixture.git(".", &["rev-parse", rev]).trim().to_string();
        let (history, at) = crate::file_history(&path, "HEAD", "HEAD~1", "a.txt").unwrap();
        assert_eq!(history, [id("HEAD~1"), id("HEAD~3")]);
        assert_eq!(at, Some(0), "a2 itself");
        // A commit that left the file alone stands at the change before it.
        let (_, at) = crate::file_history(&path, "HEAD", "HEAD~2", "a.txt").unwrap();
        assert_eq!(at, Some(1), "b1 stands at a1");
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
    fn a_file_moved_on_disk_is_a_rename() {
        let fixture = Fixture::new();
        fixture.commit(".", "old.txt", "one\ntwo\nthree\n", "c1");
        let path = fixture.path();
        std::fs::rename(
            std::path::Path::new(&path).join("old.txt"),
            std::path::Path::new(&path).join("new.txt"),
        )
        .unwrap();
        fixture.write(".", "other.txt", "x\n");
        let index = fixture.git(".", &["ls-files", "--stage"]);

        let status = crate::worktree_status(&path).unwrap();
        let mut found: Vec<(String, Option<String>, Option<String>)> = status
            .unstaged
            .iter()
            .map(|file| {
                (
                    file.status.clone(),
                    file.old_path.clone(),
                    file.new_path.clone(),
                )
            })
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                ("A".into(), None, Some("other.txt".into())),
                ("R".into(), Some("old.txt".into()), Some("new.txt".into())),
            ]
        );
        assert_eq!(
            fixture.git(".", &["ls-files", "--stage"]),
            index,
            "index untouched"
        );

        // Staged by both names, it is a staged rename; discarded, it moves back.
        crate::stage(&path, &["old.txt".into(), "new.txt".into()]).unwrap();
        let status = crate::worktree_status(&path).unwrap();
        assert_eq!(status.staged.len(), 1);
        assert_eq!(status.staged[0].status, "R");
        crate::unstage(&path, &["old.txt".into(), "new.txt".into()]).unwrap();
        let renamed = crate::worktree_status(&path)
            .unwrap()
            .unstaged
            .into_iter()
            .find(|file| file.status == "R")
            .unwrap();
        crate::discard(&path, &[renamed]).unwrap();
        let root = std::path::Path::new(&path);
        assert!(root.join("old.txt").exists() && !root.join("new.txt").exists());
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
            &Default::default(),
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
    fn a_tag_ends_the_run_it_is_in() {
        let fixture = Fixture::new();
        fixture.commit(".", "a.txt", "1\n", "c1");
        fixture.commit(".", "a.txt", "2\n", "c2");
        fixture.git(".", &["tag", "v1"]);
        fixture.commit(".", "a.txt", "3\n", "c3");
        fixture.commit(".", "a.txt", "4\n", "c4");
        let tags = Attention {
            tags: true,
            ..Attention::default()
        };
        let runs = |graph: &WorkflowGraph| -> Vec<Vec<String>> {
            graph
                .nodes
                .iter()
                .filter(|node| node.kind != NodeKind::Uncommitted)
                .map(|node| node.commits.iter().map(|c| c.summary.clone()).collect())
                .collect()
        };
        // The tag, on c2, sits between c2 and c3.
        let graph = fixture.graph_with(&tags);
        assert_eq!(runs(&graph), [vec!["c1"], vec!["c2"], vec!["c3", "c4"]]);
        let tagged = graph.nodes.iter().find(|node| node.tags == ["v1"]).unwrap();
        assert_eq!(tagged.commits.last().unwrap().summary, "c2");
        // Tags left out of the graph split nothing.
        let graph = fixture.graph();
        assert_eq!(runs(&graph), [vec!["c1"], vec!["c2", "c3", "c4"]]);
    }

    #[test]
    fn a_stash_ends_the_run_it_was_made_in() {
        let fixture = Fixture::new();
        fixture.commit(".", "a.txt", "1\n", "c1");
        fixture.commit(".", "a.txt", "2\n", "c2");
        fixture.write(".", "a.txt", "wip\n");
        fixture.git(".", &["stash", "push", "-q"]);
        fixture.commit(".", "a.txt", "3\n", "c3");
        fixture.commit(".", "a.txt", "4\n", "c4");
        let stashes = Attention {
            stashes: true,
            ..Attention::default()
        };
        let runs = |graph: &WorkflowGraph| -> Vec<Vec<String>> {
            graph
                .nodes
                .iter()
                .filter(|node| node.kind != NodeKind::Uncommitted)
                .map(|node| node.commits.iter().map(|c| c.summary.clone()).collect())
                .collect()
        };
        // The stash, made on c2, sits between c2 and c3.
        let graph = fixture.graph_with(&stashes);
        assert_eq!(runs(&graph), [vec!["c1"], vec!["c2"], vec!["c3", "c4"]]);
        let marked = graph
            .nodes
            .iter()
            .find(|node| !node.stashes.is_empty())
            .unwrap();
        assert_eq!(marked.commits.last().unwrap().summary, "c2");
        // Stashes left out of the graph split nothing.
        let graph = fixture.graph();
        assert_eq!(runs(&graph), [vec!["c1"], vec!["c2", "c3", "c4"]]);
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

        // A file marked with `git add -N` goes too.
        fixture.write(".", "noted.txt", "x\n");
        fixture.git(".", &["add", "-N", "noted.txt"]);
        crate::discard(&path, &[file("noted.txt")]).unwrap();
        assert!(!std::path::Path::new(&format!("{path}/noted.txt")).exists());
        assert!(
            !fixture
                .git(".", &["status", "--porcelain"])
                .contains("noted.txt")
        );

        // A name is a path, not a pattern: discarding `n*` leaves `noted.txt`.
        fixture.commit(".", "noted.txt", "kept\n", "c3");
        fixture.write(".", "noted.txt", "edited\n");
        fixture.write(".", "n*", "x\n");
        crate::discard(&path, &[file("n*")]).unwrap();
        assert_eq!(read("noted.txt"), "edited\n");
        assert!(!std::path::Path::new(&format!("{path}/n*")).exists());

        // An ignored file marked with `git add -N` goes too.
        fixture.write(".", ".gitignore", "*.log\n");
        fixture.write(".", "debug.log", "x\n");
        fixture.git(".", &["add", "-N", "-f", "debug.log"]);
        crate::discard(&path, &[file("debug.log")]).unwrap();
        assert!(!std::path::Path::new(&format!("{path}/debug.log")).exists());
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
