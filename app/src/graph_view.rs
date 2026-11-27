use gpui::{
    AnyElement, ClickEvent, Context, Hsla, MouseButton, MouseDownEvent, PathBuilder, Pixels, Point,
    ScrollHandle, SharedString, canvas, div, point, prelude::*, px,
};
use wtf_core::{GraphNode, NodeKind, Revision, WorkflowGraph};

use crate::popups::Target;
use crate::repo_view::RepoView;
use crate::theme::Theme;

/// How the worktree list names worktrees.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum WorktreeNames {
    FolderAndBranch,
    Branch,
    Folder,
}

/// A worktree's name in the list: its folder, its branch, or both when they
/// differ; detached worktrees only have a folder.
pub fn worktree_label(node: &GraphNode, names: WorktreeNames) -> String {
    let folder = match &node.tip {
        Revision::Worktree(path) => crate::workspace::folder_name(path),
        Revision::Commit(_) => String::new(),
    };
    match (names, &node.branch) {
        (WorktreeNames::Branch, Some(branch)) => branch.clone(),
        (WorktreeNames::Folder, _) | (WorktreeNames::Branch, None) => folder,
        (WorktreeNames::FolderAndBranch, Some(branch)) if *branch == folder => folder,
        (WorktreeNames::FolderAndBranch, Some(branch)) => format!("{folder} · {branch}"),
        (WorktreeNames::FolderAndBranch, None) => format!("{folder} · detached"),
    }
}

/// How worktree rows are ordered down from the main row.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum WorktreeOrder {
    /// The latest to branch off nearest the main row, so no row's way down
    /// crosses a row above it; at one fork, newest first.
    ForkPoint,
    /// Newest first, by the last commit.
    LastCommit,
    /// Newest first, by the first commit.
    FirstCommit,
}

/// What a right-click on a worktree acts on.
fn worktree_target(graph: &WorkflowGraph, index: usize) -> Target {
    let node = &graph.nodes[index];
    let path = match &node.tip {
        Revision::Worktree(path) => path.clone(),
        Revision::Commit(_) => String::new(),
    };
    Target::Worktree {
        path,
        branch: node.branch.clone(),
        commit: node.base.clone().unwrap_or_default(),
        leaf: leaf(graph, index),
        stale: stale_worktree(graph, index),
    }
}

/// A worktree done with: no changes, on a branch a channel has merged or
/// whose remote branch is gone.
pub fn stale_worktree(graph: &WorkflowGraph, index: usize) -> bool {
    let node = &graph.nodes[index];
    let clean = (node.added, node.removed) == (0, 0);
    let done = node.branch.as_ref().is_some_and(|name| {
        graph
            .branches
            .iter()
            .find(|branch| branch.name == *name)
            .is_some_and(|branch| !branch.stale.is_empty())
    });
    node.kind == NodeKind::Uncommitted && clean && done
}

/// Whether a worktree's branch can go with it, and whether the main branch
/// has merged it: only the main branch's history, if anything, follows its
/// HEAD; no other branch or worktree grows from it.
fn leaf(graph: &WorkflowGraph, worktree: usize) -> Option<bool> {
    let head = *graph.nodes[worktree].parents.first()?;
    // A detached worktree has no branch to delete.
    graph.nodes[worktree].branch.as_ref()?;
    grows_alone(graph, head, worktree)
}

/// What a branch's menu acts on, the same wherever the branch is shown:
/// its worktree when one has it checked out, a remote-tracking branch, or
/// a local one.
pub fn label_target(graph: &WorkflowGraph, name: &str, commit: String) -> Target {
    let worktree = graph.nodes.iter().position(|node| {
        node.kind == NodeKind::Uncommitted && node.refs.first().map(String::as_str) == Some(name)
    });
    if let Some(worktree) = worktree {
        return worktree_target(graph, worktree);
    }
    let remote = graph
        .branches
        .iter()
        .any(|branch| branch.name == name && branch.remote);
    match name.split_once('/').filter(|_| remote) {
        Some((remote, branch)) => Target::Remote {
            remote: remote.to_string(),
            branch: branch.to_string(),
            commit,
        },
        None => branch_target(graph, name, commit),
    }
}

/// A branch label's target, with whether the branch can go: nothing but the
/// main branch's history grows from its node, and whether that has merged it.
fn branch_target(graph: &WorkflowGraph, name: &str, commit: String) -> Target {
    let node = graph
        .nodes
        .iter()
        .position(|node| node.kind != NodeKind::Uncommitted && node.refs.iter().any(|r| r == name));
    Target::Branch {
        name: name.to_string(),
        commit,
        leaf: node.and_then(|node| grows_alone(graph, node, node)),
    }
}

/// Whether only the main branch's history follows node `head`, leaving out
/// node `except`: `Some(merged)` then, and `None` when another branch or
/// worktree grows from it or it is the main branch's tip.
fn grows_alone(graph: &WorkflowGraph, head: usize, except: usize) -> Option<bool> {
    if graph.main_node == Some(head) {
        return None;
    }
    let on_main = |i: usize| graph.on_main.get(i).copied().unwrap_or(false);
    let others_grow = graph
        .nodes
        .iter()
        .enumerate()
        .any(|(i, node)| i != except && node.parents.contains(&head) && !on_main(i));
    (!others_grow).then(|| on_main(head))
}

/// The graph's look, from Settings.
fn style() -> crate::preferences::GraphStyle {
    crate::preferences::graph()
}

/// A worktree's rectangle around its dot, and the step between rectangles.
fn symbol() -> f32 {
    style().dot + 2. * FRAME_PAD
}

fn symbol_step() -> f32 {
    symbol() + 4.
}

/// Beads are narrow ellipses, a little over half a dot wide.
fn bead_width() -> f32 {
    style().dot * 0.6
}

/// Room left and right of the graph inside its margin.
const PAD_X: f32 = 12.;
/// What the mark says while it is being pulled.
const HISTORY_HINT: &str = "Keep scrolling";

/// Room on the left for the mark of older commits left out.
const OLDER_WIDTH: f32 = 110.;

/// Where the graph's first column starts.
fn left_edge(graph: &WorkflowGraph) -> f32 {
    match graph.older {
        0 => PAD_X,
        _ => PAD_X + OLDER_WIDTH,
    }
}
const PAD_Y: f32 = 4.;
/// Label width estimates for the extra-small font: per character, and for a
/// tag's padding, edge and hide button.
const LABEL_CHAR: f32 = 7.;
const LABEL_EXTRA: f32 = 28.;
const LABEL_SPACING: f32 = 6.;
/// Room between a label's name and its stale note.
const NOTE_GAP: f32 = 5.;
const ROW_GAP: f32 = 24.;
const HIT: f32 = 20.;
const TAG_HEIGHT: f32 = 16.;
const TAG_GAP: f32 = 8.;
/// How far a worktree off the middle of a lane sits from its commit.
const DETACHED_OFFSET: f32 = 36.;
/// Room between a tip and its worktrees when more of them fork below.
const FORK_GAP: f32 = 22.;
/// Room between the graph and the worktree list.
const COLUMN_GAP: f32 = 24.;
const SYMBOL_GAP: f32 = 6.;
/// Room between a worktree's rectangle and what it frames, and how much more
/// each further clean worktree on the same commit takes.
const FRAME_PAD: f32 = 4.;
const FRAME_STEP: f32 = 3.;

pub struct Layout {
    /// Horizontal extent and vertical center of each node.
    pub nodes: Vec<(f32, f32, f32)>,
    /// Worktrees off a commit in the middle of a lane, on rows of their own.
    pub detached: Vec<usize>,
    /// The clean ones of them: frames around their commits, with no rail to
    /// their rows.
    pub framed: Vec<usize>,
    /// Where the worktree list starts.
    pub column: f32,
    /// Dashed leads to the list, one per row of worktrees: where each starts,
    /// its row, and its worktrees.
    pub leads: Vec<(f32, f32, Vec<usize>)>,
    /// Where each worktree's tag sits in the list; worktrees sharing a row
    /// sit side by side.
    pub list_x: Vec<Option<f32>>,
    /// Stashes, each on a row of its own off the commit it was made on.
    pub stashes: Vec<StashPlace>,
    pub width: f32,
    pub height: f32,
}

/// A stash's or tag's place: like a short branch, forking from its commit.
pub struct StashPlace {
    /// The node of the commit it was made on, or is on.
    pub node: usize,
    pub name: String,
    /// A tag, not a stash.
    pub tag: bool,
    /// The center of a stash's mark; where a tag's label starts.
    pub x: f32,
    pub y: f32,
}

pub fn layout(graph: &WorkflowGraph, order: WorktreeOrder) -> Layout {
    let mut xs = topological(graph);
    // Branches stacked on others get lanes of their own, as stairs; the
    // layout's other lanes come after them.
    let (lane_of, mut step_from) = stair_lanes(graph);
    let base = step_from.len();
    // A branch growing off another branch, not the main line, goes below
    // it too, however its lane came about: a stack reads down.
    for (i, node) in graph.nodes.iter().enumerate() {
        let lane = lane_of[i];
        let Some(&p) = node.parents.first() else {
            continue;
        };
        let first = node.kind != NodeKind::Uncommitted
            && lane_of[p] != lane
            && lane_of[p] != 0
            && step_from[lane].is_none()
            && !graph.nodes[p].channel;
        if first {
            step_from[lane] = Some(lane_of[p]);
        }
    }

    // A clean worktree is a frame around its HEAD commit. A changed one is a
    // square right after that commit when nothing follows it; otherwise later
    // commits or a merge rail are in the way, so it branches off to a row of
    // its own.
    let mut followed = vec![false; graph.nodes.len()];
    for node in graph
        .nodes
        .iter()
        .filter(|n| n.kind != NodeKind::Uncommitted)
    {
        for &p in &node.parents {
            followed[p] = true;
        }
    }
    // A frame's lead runs on the commit's own row, from the commit's labels
    // straight to the list.
    let mut worktrees = vec![Vec::new(); graph.nodes.len()];
    let mut detached = Vec::new();
    let mut framed = Vec::new();
    for (i, node) in graph.nodes.iter().enumerate() {
        if node.kind != NodeKind::Uncommitted {
            continue;
        }
        let parent = node.parents[0];
        // A clean one is always the frame, as `render` draws it. Of changed
        // ones at a tip, the first follows the commit on its row; any more
        // get rows of their own, so no two sit in one row.
        if (node.added, node.removed) == (0, 0) {
            xs[i] = xs[parent];
            framed.push(i);
        } else if !followed[parent] && worktrees[parent].is_empty() {
            worktrees[parent].push(i);
        } else {
            let x = xs[parent].1 + DETACHED_OFFSET;
            xs[i] = (x, x);
            detached.push(i);
        }
    }
    // Worktrees forking from a tip stand one under the other, past room
    // for the fork's curve.
    let forks = |parent: usize| {
        !followed[parent]
            && detached
                .iter()
                .any(|&w| graph.nodes[w].parents.first() == Some(&parent))
    };
    // A tip reads commit, its labels, then the work on it: the worktrees'
    // squares come after the branch they have checked out.
    for (parent, group) in worktrees.iter().enumerate() {
        let labels = labels_width(graph, &graph.nodes[parent]);
        let gap = match (labels > 0., forks(parent)) {
            (true, _) => TAG_GAP + labels,
            (false, true) => FORK_GAP,
            (false, false) => SYMBOL_GAP,
        };
        for (k, &worktree) in group.iter().enumerate() {
            let x =
                xs[parent].1 + style().dot / 2. + gap + symbol() / 2. + k as f32 * symbol_step();
            xs[worktree] = (x, x);
        }
    }
    for &worktree in &detached {
        let parent = graph.nodes[worktree].parents[0];
        if let Some(&first) = worktrees[parent].first() {
            xs[worktree] = xs[first];
        }
    }

    // Lanes span from their first edge to their last tag, or to the worktree
    // list for rows with a worktree's dashed line. The main lane keeps the top
    // row; other lanes share rows where they do not overlap.
    // Worktrees off a commit with something after it get layout lanes of their
    // own, past the graph's lanes, so their rows never hold their commit.
    // Stashes fork off their commits onto rows of their own too, as short
    // branches do; so do tags, off the main line, which they would break.
    let stash_rows: Vec<(usize, String, bool)> = graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.kind != NodeKind::Uncommitted)
        .flat_map(|(i, node)| {
            let stashes = node
                .stashes
                .iter()
                .map(move |name| (i, name.clone(), false));
            stashes.chain(node.tags.iter().map(move |name| (i, name.clone(), true)))
        })
        .collect();
    let stash_x: Vec<f32> = stash_rows
        .iter()
        .map(|&(p, _, tag)| {
            let mark = if tag { 0. } else { symbol() / 2. };
            xs[p].1 + style().dot / 2. + FORK_GAP + mark
        })
        .collect();
    // Clean worktrees on a commit with history after it: their frames stay
    // on the commit, but their leads rise to rows of their own above the
    // main row, not along the commit's row past later commits and their
    // labels, which would read as if the worktree were on them.
    // Only what goes on along the commit's own row counts: branches forking
    // off it go elsewhere, so the lead along the row passes none of them.
    let mut continued = vec![false; graph.nodes.len()];
    for (i, node) in graph.nodes.iter().enumerate() {
        if node.kind == NodeKind::Uncommitted {
            continue;
        }
        for &p in &node.parents {
            if lane_of[p] == lane_of[i] {
                continued[p] = true;
            }
        }
    }
    // A detached one has no branch to start from: its lead leaves the
    // commit for a row of its own, whatever follows the commit.
    let framed_up: Vec<usize> = framed
        .iter()
        .copied()
        .filter(|&w| continued[graph.nodes[w].parents[0]] || graph.nodes[w].branch.is_none())
        .collect();
    let lane_count = base + detached.len() + stash_rows.len() + framed_up.len();
    // A worktree off the middle of the main line goes up; off another line,
    // down, a merged one's too: going up it would cross the main row. Without
    // the main line's history known, every one goes up.
    let goes_up = |worktree: usize| {
        let commit = graph.nodes[worktree].parents[0];
        followed[commit]
            && graph
                .on_main
                .get(commit)
                .is_none_or(|&on| on && lane_of[commit] == 0)
    };
    let own_lane =
        |worktree: usize| base + detached.iter().position(|&w| w == worktree).unwrap_or(0);
    let stash_lane = |k: usize| base + detached.len() + k;
    let framed_lane = |worktree: usize| {
        base + detached.len()
            + stash_rows.len()
            + framed_up.iter().position(|&w| w == worktree).unwrap_or(0)
    };
    // A worktree off the middle of a line other than the main one steps
    // down from it, as a stair does: right under it, crossing no other row.
    step_from.resize(lane_count, None);
    for (&worktree, lane) in detached
        .iter()
        .zip(base..)
        .chain(framed_up.iter().map(|w| (w, framed_lane(*w))))
    {
        let commit = graph.nodes[worktree].parents[0];
        if !goes_up(worktree) && lane_of[commit] != 0 {
            step_from[lane] = Some(lane_of[commit]);
        }
    }
    // The layout lane whose row a node sits in: a worktree's commit's, or
    // one of its own when detached.
    let lane_row = |i: usize| {
        let node = &graph.nodes[i];
        match node.kind {
            NodeKind::Uncommitted if detached.contains(&i) => own_lane(i),
            NodeKind::Uncommitted if framed_up.contains(&i) => framed_lane(i),
            NodeKind::Uncommitted => lane_of[node.parents[0]],
            _ => lane_of[i],
        }
    };
    let mut spans: Vec<Option<(f32, f32)>> = vec![None; lane_count];
    let widen = |spans: &mut Vec<Option<(f32, f32)>>, lane: usize, start: f32, end: f32| {
        let span = spans[lane].get_or_insert((start, end));
        *span = (span.0.min(start), span.1.max(end));
    };
    for &worktree in &detached {
        let start = xs[graph.nodes[worktree].parents[0]].1;
        widen(&mut spans, own_lane(worktree), start, f32::INFINITY);
    }
    for &worktree in &framed_up {
        let start = xs[graph.nodes[worktree].parents[0]].1;
        widen(&mut spans, framed_lane(worktree), start, f32::INFINITY);
    }
    for (k, (parent, name, tag)) in stash_rows.iter().enumerate() {
        let mark = if *tag { 0. } else { symbol() / 2. + TAG_GAP };
        let end = stash_x[k] + mark + label_width(name);
        widen(&mut spans, stash_lane(k), xs[*parent].1, end);
    }
    for (i, node) in graph.nodes.iter().enumerate() {
        if node.kind == NodeKind::Uncommitted {
            continue;
        }
        // A rail from another lane's commit drops at that commit and turns
        // into this row a curve later; one into another lane's leaves this
        // row a curve before. So a branch merged into a commit and one
        // forking from it can share a row.
        let curve = style().curve;
        let start = node
            .parents
            .iter()
            .map(|&p| match lane_of[p] == lane_of[i] {
                true => xs[p].1,
                false => xs[p].1 + curve,
            })
            .fold(xs[i].0, f32::min);
        let leads_on = !worktrees[i].is_empty()
            || framed
                .iter()
                .any(|&w| graph.nodes[w].parents[0] == i && !framed_up.contains(&w));
        let end = match leads_on {
            false => tags_start(i, &xs) + labels_width(graph, node),
            true => f32::INFINITY,
        };
        widen(&mut spans, lane_of[i], start, end);
        // A merge rail runs in the merged lane's row up to the merge.
        for &p in node.parents.iter().skip(1) {
            if let Some(span) = &mut spans[lane_of[p]] {
                span.1 = span.1.max(xs[i].0 - curve);
            }
        }
    }
    // Lanes forking from or merging into the main lane sit closest to it, merged
    // branches first, then branches with names; other lanes come after.
    let mut touches_main = vec![false; lane_count];
    let mut named = vec![false; lane_count];
    // Lanes that come back into the main lane, merged.
    let mut returns = vec![false; lane_count];
    for (i, node) in graph.nodes.iter().enumerate() {
        if node.kind == NodeKind::Uncommitted {
            match detached.contains(&i) {
                true => {
                    named[own_lane(i)] = true;
                    if graph.nodes[node.parents[0]].lane == 0 {
                        touches_main[own_lane(i)] = true;
                    }
                }
                false => named[lane_of[node.parents[0]]] = true,
            }
            continue;
        }
        named[lane_of[i]] |= !node.refs.is_empty();
        for &p in &node.parents {
            match (lane_of[i], lane_of[p]) {
                (0, 0) => {}
                (0, lane) => {
                    touches_main[lane] = true;
                    returns[lane] = true;
                }
                (lane, 0) => touches_main[lane] = true,
                _ => {}
            }
        }
    }
    // Rows of worktrees off the middle of the main line, and stash rows,
    // are placed apart, above the main row. Worktrees off the middle of
    // another line stay below, near it: going up they would cross every
    // row between; so do worktrees stacked under another at a tip.
    let mid_lane = |k: usize| goes_up(detached[k]);
    // A stash or tag on a line other than the main one gets a row of its own right
    // above that line's, below the rest: going up from it, it crosses no
    // other row. Stashes on the main line go up above it.
    let stash_over: Vec<Option<usize>> = stash_rows
        .iter()
        .map(|&(parent, _, _)| (lane_of[parent] != 0).then_some(lane_of[parent]))
        .collect();
    let mut lanes: Vec<(usize, (f32, f32))> = (1..stash_lane(0))
        .filter(|&lane| !(base..stash_lane(0)).contains(&lane) || { !mid_lane(lane - base) })
        .chain(
            framed_up
                .iter()
                .filter(|&&w| !goes_up(w))
                .map(|&w| framed_lane(w)),
        )
        .chain(
            (0..stash_rows.len())
                .filter(|&k| stash_over[k].is_some())
                .map(stash_lane),
        )
        .filter_map(|lane| Some((lane, spans[lane]?)))
        .collect();
    // Worktree rows come right after merged branches, newest first by the chosen
    // commit; other lanes follow by where they start.
    let mut worktree_lane = vec![false; lane_count];
    let mut times: Vec<Option<(i64, i64)>> = vec![None; lane_count];
    for (i, node) in graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind != NodeKind::Uncommitted)
    {
        let time = times[lane_of[i]].get_or_insert((node.first_time, node.last_time));
        *time = (time.0.min(node.first_time), time.1.max(node.last_time));
    }
    for (i, node) in graph.nodes.iter().enumerate() {
        if node.kind != NodeKind::Uncommitted {
            continue;
        }
        let parent = &graph.nodes[node.parents[0]];
        let lane = lane_row(i);
        worktree_lane[lane] = true;
        times[lane].get_or_insert((parent.last_time, parent.last_time));
    }

    let time = |lane: usize| {
        let (first, last) = times[lane].unwrap_or_default();
        match order {
            WorktreeOrder::ForkPoint | WorktreeOrder::LastCommit => last,
            WorktreeOrder::FirstCommit => first,
        }
    };
    // Merged branches come first, named or not: their way back to the main
    // row then crosses no other row.
    let group = |lane: usize| match (worktree_lane[lane], touches_main[lane], named[lane]) {
        (false, true, _) if returns[lane] => 0,
        (false, true, false) => 0,
        (true, _, _) => 1,
        (false, true, true) => 2,
        (false, false, _) => 3,
    };
    // A lane's name, by the refs at its newest node, to keep lanes the rest
    // can't tell apart in a set order.
    let mut names: Vec<Option<(i64, &str)>> = vec![None; lane_count];
    for (i, node) in graph.nodes.iter().enumerate() {
        if node.kind == NodeKind::Uncommitted {
            continue;
        }
        if let Some(name) = node.refs.first() {
            let lane = lane_of[i];
            if names[lane].is_none_or(|(time, _)| node.last_time > time) {
                names[lane] = Some((node.last_time, name.as_str()));
            }
        }
    }
    // Lanes alike in all else: the latest work first, then by name.
    let tie = |a: usize, b: usize| {
        let last = |lane: usize| times[lane].map_or(0, |(_, last)| last);
        last(b)
            .cmp(&last(a))
            .then_with(|| names[a].map(|(_, n)| n).cmp(&names[b].map(|(_, n)| n)))
    };
    lanes.sort_by(|&(a, a_span), &(b, b_span)| {
        group(a)
            .cmp(&group(b))
            .then_with(|| match (group(a), order) {
                // A row that starts further left goes further down: rows above
                // it start later, so its way down passes none of them.
                (1, WorktreeOrder::ForkPoint) => b_span
                    .0
                    .total_cmp(&a_span.0)
                    .then_with(|| time(b).cmp(&time(a))),
                (1, _) => time(b).cmp(&time(a)),
                // Off the main row and back: the one that ends first nearest
                // it, so a lane inside another's run crosses it neither going
                // out nor coming back.
                (0 | 2, _) if returns[a] || returns[b] => a_span
                    .1
                    .total_cmp(&b_span.1)
                    .then_with(|| a_span.0.total_cmp(&b_span.0)),
                // Off it for good: where it ends crosses nothing. One that
                // starts further left goes further down, its way down
                // passing no row above.
                (0 | 2, _) => b_span.0.total_cmp(&a_span.0),
                _ => a_span.0.total_cmp(&b_span.0),
            })
            .then_with(|| tie(a, b))
    });

    // Each lane takes the first row with a free stretch for it; a stair,
    // one below the lane it steps down from.
    let mut row_of = vec![0; lane_count];
    let mut rows: Vec<Vec<(f32, f32)>> = vec![Vec::new()];
    // A stack goes in whole: each lane right after the one it steps down
    // from, whatever its own place in the order, so it knows that one's row
    // and no other lane comes between.
    let in_order: Vec<usize> = lanes.iter().map(|&(lane, _)| lane).collect();
    let parent_of = |lane: usize| {
        step_from
            .get(lane)
            .copied()
            .flatten()
            .filter(|from| in_order.contains(from))
    };
    let mut ordered: Vec<(usize, (f32, f32))> = Vec::with_capacity(lanes.len());
    let mut stack: Vec<usize> = Vec::new();
    for &(root, _) in lanes.iter().filter(|&&(lane, _)| parent_of(lane).is_none()) {
        stack.push(root);
        while let Some(lane) = stack.pop() {
            if let Some(&found) = lanes.iter().find(|&&(l, _)| l == lane) {
                ordered.push(found);
            }
            // Children in the order's own order, the first on top.
            let children: Vec<usize> = in_order
                .iter()
                .copied()
                .filter(|&child| parent_of(child) == Some(lane))
                .collect();
            stack.extend(children.into_iter().rev());
        }
    }
    // Any left, as a cycle would leave them, keep their place.
    for &(lane, span) in &lanes {
        if !ordered.iter().any(|&(l, _)| l == lane) {
            ordered.push((lane, span));
        }
    }
    // Each such stash goes right before its line, so it takes the row above
    // the line's and the line the one under it.
    for (k, over) in stash_over.iter().enumerate() {
        let Some(line) = *over else {
            continue;
        };
        let lane = stash_lane(k);
        let Some(at) = ordered.iter().position(|&(l, _)| l == lane) else {
            continue;
        };
        let stash = ordered.remove(at);
        let to = ordered
            .iter()
            .position(|&(l, _)| l == line)
            .unwrap_or(ordered.len());
        ordered.insert(to, stash);
    }
    let stash_of_lane = |lane: usize| {
        (0..stash_rows.len())
            .find(|&k| stash_lane(k) == lane)
            .and_then(|k| stash_over[k])
    };
    for (lane, (start, end)) in ordered {
        let free = |row: &Vec<(f32, f32)>| {
            row.iter()
                .all(|&(s, e)| end + ROW_GAP < s || e + ROW_GAP < start)
        };
        // A stash's row is as low as its line's would be; the line goes
        // under its stashes.
        let stepped = |lane: usize| {
            step_from
                .get(lane)
                .copied()
                .flatten()
                .map_or(1, |from| row_of[from] + 1)
        };
        let lowest = match stash_of_lane(lane) {
            Some(line) => stepped(line),
            None => (0..stash_rows.len())
                .filter(|&k| stash_over[k] == Some(lane))
                .map(|k| row_of[stash_lane(k)] + 1)
                .fold(stepped(lane), usize::max),
        };
        let row = match (lowest.max(1)..rows.len()).find(|&row| free(&rows[row])) {
            Some(row) => row,
            None => {
                rows.push(Vec::new());
                rows.len() - 1
            }
        };
        rows[row].push((start, end));
        row_of[lane] = row;
    }

    // Branches go down from the main row; stashes, set-aside work rather
    // than history, go up: each takes the first row above with room for it,
    // the nearest first.
    // Worktrees on a commit with something after it go up too, as stashes
    // do: they are where work stands, off the history, not a branch of it.
    // The row that ends first comes nearest the main row, as below it: a
    // stash, ending at its label, sits under a worktree whose lead runs on
    // to the list, so the stash's way up crosses no worktree's lead.
    let mut above: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut above_row: Vec<Option<usize>> = vec![None; lane_count];
    let mut up: Vec<(usize, (f32, f32))> = (0..detached.len())
        .filter(|&k| mid_lane(k))
        .map(|k| base + k)
        .chain(
            framed_up
                .iter()
                .filter(|&&w| goes_up(w))
                .map(|&w| framed_lane(w)),
        )
        .chain(
            (0..stash_rows.len())
                .filter(|&k| stash_over[k].is_none())
                .map(stash_lane),
        )
        .filter_map(|lane| Some((lane, spans[lane]?)))
        .collect();
    up.sort_by(|a, b| a.1.1.total_cmp(&b.1.1).then(a.1.0.total_cmp(&b.1.0)));
    for (lane, (start, end)) in up {
        let free = |row: &Vec<(f32, f32)>| {
            row.iter()
                .all(|&(s, e)| end + ROW_GAP < s || e + ROW_GAP < start)
        };
        let row = match above.iter().position(free) {
            Some(row) => row,
            None => {
                above.push(Vec::new());
                above.len() - 1
            }
        };
        above[row].push((start, end));
        above_row[lane] = Some(row);
    }
    let shift = above.len() as f32 * style().row_height;
    let above_y = |row: usize| {
        PAD_Y + shift - (row + 1) as f32 * style().row_height + style().row_height / 2.
    };
    let row_y = |lane: usize| match above_row[lane] {
        Some(row) => above_y(row),
        None => PAD_Y + shift + row_of[lane] as f32 * style().row_height + style().row_height / 2.,
    };
    let nodes: Vec<(f32, f32, f32)> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, _)| (xs[i].0, xs[i].1, row_y(lane_row(i))))
        .collect();
    let stashes: Vec<StashPlace> = stash_rows
        .into_iter()
        .enumerate()
        .map(|(k, (node, name, tag))| StashPlace {
            node,
            name,
            tag,
            x: stash_x[k],
            y: row_y(stash_lane(k)),
        })
        .collect();
    // A row of squares after a commit gets one lead, from the commit; a
    // worktree on a row of its own starts at its square. The list starts past
    // the labels of the commits those leads leave.
    let mut leads: Vec<(f32, f32, Vec<usize>)> = Vec::new();
    for (parent, group) in worktrees.iter().enumerate() {
        // From the commit, under its squares and labels, which hide it, so it
        // shows right where they end.
        if !group.is_empty() {
            let from = xs[parent].1 + style().dot / 2.;
            leads.push((from, nodes[parent].2, group.clone()));
        }
    }
    for &worktree in &detached {
        leads.push((
            xs[worktree].1 + symbol() / 2. + 3.,
            nodes[worktree].2,
            vec![worktree],
        ));
    }
    // A framed worktree's lead leaves its commit and runs under the commit's
    // labels, which hide it, so it shows right where they end, however wide
    // they turn out.
    for &worktree in &framed {
        let parent = graph.nodes[worktree].parents[0];
        match framed_up.contains(&worktree) {
            // On a row of its own: from past the way off the frame.
            true => {
                let from = xs[parent].1 + style().dot / 2. + FORK_GAP;
                leads.push((from, nodes[worktree].2, vec![worktree]));
            }
            false => {
                let from = xs[parent].1 + style().dot / 2.;
                leads.push((from, nodes[parent].2, vec![worktree]));
            }
        }
    }
    // Leads on one row merge into one, from its leftmost start, and their
    // worktrees share the row of the list, in the order their leads start.
    leads.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
    let mut merged: Vec<(f32, f32, Vec<usize>)> = Vec::new();
    for (from, y, group) in leads {
        match merged.last_mut() {
            Some(last) if last.1 == y => last.2.extend(group),
            _ => merged.push((from, y, group)),
        }
    }
    let leads = merged;
    let labels_end = worktrees
        .iter()
        .enumerate()
        .filter(|(_, group)| !group.is_empty())
        .map(|(parent, _)| parent)
        .chain(framed.iter().map(|&w| graph.nodes[w].parents[0]))
        .map(|parent| tags_start(parent, &xs) + labels_width(graph, &graph.nodes[parent]));
    let own_labels_end = detached.iter().filter_map(|&w| {
        let name = own_row_branch(graph, &detached, w)?;
        Some(xs[w].1 + symbol() / 2. + TAG_GAP + label_width(name) + extras_width(graph, name))
    });
    let right = spans
        .iter()
        .flatten()
        .map(|span| span.1)
        .chain(own_labels_end)
        .filter(|end| end.is_finite())
        .chain(xs.iter().map(|x| x.1 + symbol()))
        .chain(leads.iter().map(|lead| lead.0))
        .chain(labels_end)
        .fold(0., f32::max);
    let column = right + COLUMN_GAP;
    let mut list_x = vec![None; graph.nodes.len()];
    let mut list = 0.;
    for (_, _, group) in &leads {
        let mut x = 0.;
        for &worktree in group {
            list_x[worktree] = Some(column + x);
            let node = &graph.nodes[worktree];
            x += label_width(&worktree_label(node, WorktreeNames::Folder))
                + AGE_WIDTH
                + node
                    .branch
                    .as_deref()
                    .map_or(0., |name| extras_width(graph, name));
        }
        list = f32::max(list, x);
    }
    Layout {
        nodes,
        detached,
        framed,
        column,
        leads,
        list_x,
        stashes,
        width: column + list + PAD_X,
        height: PAD_Y + shift + rows.len() as f32 * style().row_height,
    }
}

/// The branch a worktree on a row of its own has checked out: its label
/// sits on that row, not on its commit's.
fn own_row_branch<'a>(
    graph: &'a WorkflowGraph,
    detached: &[usize],
    worktree: usize,
) -> Option<&'a str> {
    detached
        .contains(&worktree)
        .then(|| graph.nodes[worktree].branch.as_deref())
        .flatten()
}

/// A filled diamond of `size` points, drawn as an image: GPUI turns no box.
fn diamond(color: Hsla, size: f32) -> impl IntoElement {
    let rgba = gpui::Rgba::from(color);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="0 0 16 16"><path d="M8 1 L15 8 L8 15 L1 8 Z" fill="rgb({},{},{})" fill-opacity="{}"/></svg>"#,
        (rgba.r * 255.) as u8,
        (rgba.g * 255.) as u8,
        (rgba.b * 255.) as u8,
        rgba.a,
    );
    gpui::img(std::sync::Arc::new(gpui::Image::from_bytes(
        gpui::ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .flex_none()
    .size(px(size))
}

/// Where the tags of a node start: after its worktree symbols, if any.
fn tags_start(node: usize, xs: &[(f32, f32)]) -> f32 {
    xs[node].1 + style().dot / 2. + TAG_GAP
}

/// How wide a label is, roughly, with the room after it.
fn label_width(name: &str) -> f32 {
    name.chars().count() as f32 * LABEL_CHAR + LABEL_EXTRA + LABEL_SPACING
}

/// A remote branch at the same commit as its local one: that one's dot
/// stands for it, with no label of its own.
fn beside_local(graph: &WorkflowGraph, node: &GraphNode, name: &str) -> bool {
    graph.branches.iter().any(|b| b.name == name && b.remote)
        && partner(graph, name).is_some_and(|local| node.refs.contains(&local))
}

/// How wide a node's labels are, roughly, so nothing is drawn over them.
fn labels_width(graph: &WorkflowGraph, node: &GraphNode) -> f32 {
    let shown: Vec<&String> = node
        .refs
        .iter()
        .filter(|name| !beside_local(graph, node, name))
        .collect();
    let notes: f32 = shown.iter().map(|name| extras_width(graph, name)).sum();
    shown.into_iter().map(|name| label_width(name)).sum::<f32>() + notes
}

/// Why a branch looks left behind, as its labels say it: `merged · idle 30d`.
/// Idle counts from its last commit, against the days set in Settings.
pub fn stale_note(graph: &WorkflowGraph, name: &str) -> Option<String> {
    let branch = graph.branches.iter().find(|branch| branch.name == name)?;
    let mut reasons = branch.stale.clone();
    let days = (crate::repo_view::now_seconds() - branch.time) / 86_400;
    let live = branch.busy || branch.remote || branch.name == graph.main;
    if !live
        && branch.time > 0
        && days
            >= i64::from(
                graph
                    .idle_days
                    .unwrap_or(crate::preferences::DEFAULT_IDLE_DAYS),
            )
    {
        reasons.push(format!("idle {days}d"));
    }
    (!reasons.is_empty()).then(|| reasons.join(" · "))
}

/// How far a local branch is from its upstream, as its labels say it:
/// `↑2 ↓1`; nothing when in step, or when Settings turn badges off.
pub fn sync_badge(graph: &WorkflowGraph, name: &str) -> Option<String> {
    if !crate::preferences::sync_badges() {
        return None;
    }
    let branch = graph
        .branches
        .iter()
        .find(|branch| branch.name == name && !branch.remote)?;
    let counts = [("↑", branch.ahead), ("↓", branch.behind)];
    let parts: Vec<String> = counts
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(arrow, count)| format!("{arrow}{count}"))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// How much a stale note widens its label, roughly.
fn note_width(note: &str) -> f32 {
    note.chars().count() as f32 * LABEL_CHAR + NOTE_GAP
}

/// How much a branch's remote dot, its counts and its idle age widen its
/// label.
fn extras_width(graph: &WorkflowGraph, name: &str) -> f32 {
    let dot = match remote_of(graph, name) {
        Some(_) => REMOTE_DOT + NOTE_GAP,
        None => 0.,
    };
    let counts = match remote_of(graph, name) {
        Some(Remote::Apart(counts)) => note_width(&counts),
        _ => 0.,
    };
    dot + counts + idle_age(graph, name).map_or(0., |age| note_width(&age))
}

/// The remote dot's size.
const REMOTE_DOT: f32 = 7.;

/// Where a local branch stands against the remote, as the dot after its
/// label shows it.
#[derive(Clone, PartialEq, Debug)]
enum Remote {
    /// On the remote, at the same commit: a filled dot.
    InStep,
    /// Ahead or behind it, by the counts: a half-filled dot and `↑2 ↓1`.
    Apart(String),
    /// Never pushed: a hollow dot.
    Local,
    /// Its remote branch is gone, as after a merged pull request: a red ring.
    Gone,
}

/// A local branch's remote standing; none for a remote branch.
fn remote_of(graph: &WorkflowGraph, name: &str) -> Option<Remote> {
    let branch = graph
        .branches
        .iter()
        .find(|branch| branch.name == name && !branch.remote)?;
    if branch.stale.iter().any(|reason| reason == "remote gone") {
        return Some(Remote::Gone);
    }
    let pushed = branch.upstream.is_some()
        || graph
            .branches
            .iter()
            .any(|other| other.remote && other.name.ends_with(&format!("/{name}")));
    if !pushed {
        return Some(Remote::Local);
    }
    Some(match sync_badge(graph, name) {
        Some(counts) => Remote::Apart(counts),
        None => Remote::InStep,
    })
}

/// How long a branch has sat idle, short, as `40d`: from its last commit,
/// against the days set in Settings; none for live work.
fn idle_age(graph: &WorkflowGraph, name: &str) -> Option<String> {
    let branch = graph.branches.iter().find(|branch| branch.name == name)?;
    let days = (crate::repo_view::now_seconds() - branch.time) / 86_400;
    let live = branch.busy || branch.remote || branch.name == graph.main;
    (!live && branch.time > 0 && days >= i64::from(graph.idle_days.unwrap_or(crate::preferences::DEFAULT_IDLE_DAYS)))
        .then(|| format!("{days}d"))
}

/// The dot after a local branch's label.
fn remote_dot(remote: &Remote, color: Hsla, theme: &Theme) -> gpui::Div {
    let dot = div()
        .flex_none()
        .size(px(REMOTE_DOT))
        .ml(px(NOTE_GAP))
        .rounded_full();
    match remote {
        Remote::InStep => dot.bg(color),
        Remote::Apart(_) => dot.relative().border_1().border_color(color).child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(REMOTE_DOT / 2.))
                .rounded_l_full()
                .bg(color),
        ),
        Remote::Local => dot.border_1().border_color(theme.secondary),
        Remote::Gone => dot.border_1().border_color(theme.error),
    }
}

/// A branch's label: filled when a worktree has it checked out. After its
/// name, a dot for where it stands against the remote, and how far apart
/// they are. Left behind, merged or its remote gone, it fades, in italics,
/// struck through when the remote is gone; idle long, its age follows. A
/// branch only on the remote is outlined, dashed, with no fill.
fn branch_tag(
    graph: &WorkflowGraph,
    name: &str,
    shown: &str,
    color: Hsla,
    filled: bool,
    theme: &Theme,
) -> gpui::Div {
    let remote_only = graph
        .branches
        .iter()
        .any(|branch| branch.name == name && branch.remote);
    if remote_only {
        return pill()
            .pl(px(6.))
            .border_1()
            .border_dashed()
            .border_color(color)
            .text_color(theme.secondary)
            .italic()
            .child(shown.to_string());
    }
    let remote = remote_of(graph, name);
    let faded = stale_note(graph, name).is_some();
    // On a filled label, the dot takes the label's text colour, to show.
    let dot_color = match (faded, filled) {
        (true, _) => theme.secondary,
        (false, true) => theme.tag_head_text,
        (false, false) => color,
    };
    let dot = remote
        .as_ref()
        .map(|remote| remote_dot(remote, dot_color, theme));
    let counts = match &remote {
        Some(Remote::Apart(counts)) => Some(
            div()
                .ml(px(NOTE_GAP / 2.))
                .opacity(0.8)
                .child(counts.clone()),
        ),
        _ => None,
    };
    let age = idle_age(graph, name).map(|age| div().ml(px(NOTE_GAP)).opacity(0.7).child(age));
    let label = match faded {
        false => tag(shown, color, filled, theme),
        true => tag(shown, color.opacity(0.45), false, theme)
            .italic()
            .text_color(theme.secondary),
    };
    label
        .when(remote == Some(Remote::Gone), |label| label.line_through())
        .flex()
        .items_center()
        .children(dot)
        .children(counts)
        .children(age)
}

/// Room for a worktree's age after its name, as "12m".
const AGE_WIDTH: f32 = 34.;
/// How long a worktree goes without work before it shows quiet.
const QUIET_AFTER: i64 = 86_400;

/// How many beads past the first a release line's commits take at most.
const RELEASE_BEADS: usize = 2;

/// Whether node `i` steps down from its parent `p`, a branch stacked on
/// another: it carries `p`'s lane on, and `p` ends a named branch. The
/// main line and release lines never step.
fn steps_down(graph: &WorkflowGraph, p: usize, i: usize) -> bool {
    let (parent, node) = (&graph.nodes[p], &graph.nodes[i]);
    node.kind != NodeKind::Uncommitted
        && parent.lane == node.lane
        && node.lane != 0
        && !parent.channel
        && !node.channel
        && parent
            .refs
            .iter()
            .any(|name| graph.branches.iter().any(|b| b.name == *name))
}

/// The lanes the layout gives rows to: the graph's, with each branch
/// stacked on another moved to a lane of its own; and for each lane made
/// so, the lane it steps down from. Lanes made start at `graph.lane_count`.
fn stair_lanes(graph: &WorkflowGraph) -> (Vec<usize>, Vec<Option<usize>>) {
    let mut lane = vec![0; graph.nodes.len()];
    let mut from: Vec<Option<usize>> = vec![None; graph.lane_count];
    for (i, node) in graph.nodes.iter().enumerate() {
        lane[i] = match node.parents.first() {
            Some(&p) if node.kind != NodeKind::Uncommitted && steps_down(graph, p, i) => {
                from.push(Some(lane[p]));
                from.len() - 1
            }
            Some(&p) if graph.nodes[p].lane == node.lane => lane[p],
            Some(&p) if node.kind == NodeKind::Uncommitted => lane[p],
            _ => node.lane,
        };
    }
    (lane, from)
}

/// Each node one column after its parents, as long as its commits' beads, and
/// past a parent's labels and tags when it continues that parent's branch.
fn topological(graph: &WorkflowGraph) -> Vec<(f32, f32)> {
    let mut xs: Vec<(f32, f32)> = Vec::with_capacity(graph.nodes.len());
    for (index, node) in graph.nodes.iter().enumerate() {
        let start = node
            .parents
            .iter()
            .map(|&p| {
                let parent = &graph.nodes[p];
                // A branch stepping down from it clears its labels on the
                // row below, so it starts as a fork would.
                let continues = parent.lane == node.lane && !steps_down(graph, p, index);
                // Its tags, on a row above, end before the next ones along
                // the line start, so a line's tags share one row.
                let tags = parent
                    .tags
                    .iter()
                    .map(|name| style().dot / 2. + FORK_GAP + label_width(name) + ROW_GAP + TAG_GAP)
                    .fold(0., f32::max);
                let labels = match continues {
                    true => {
                        (style().dot / 2. + TAG_GAP + labels_width(graph, parent) + style().curve)
                            .max(tags)
                    }
                    false => 0.,
                };
                xs[p].1 + style().column.max(labels)
            })
            .fold(left_edge(graph), f32::max);
        // A release line takes little room: its commits are not the work.
        let beads = match node.channel {
            true => RELEASE_BEADS,
            false => style().beads,
        };
        let length = node.commits.len().saturating_sub(1).min(beads) as f32 * style().bead_spacing;
        xs.push((start, start + length));
    }
    xs
}

/// What the graph shows of the user's state.
pub struct Marks<'a> {
    pub selected: &'a [usize],
    /// Commits the commit list narrows the selection to; empty for all.
    pub picked: &'a [String],
    /// How far a pull for older commits has gone, from 0 to 1.
    pub pull: Option<f32>,
    pub hovered: Option<usize>,
    /// The stash shown in the review area, if any.
    pub stash: Option<&'a str>,
    /// The current worktree, where Pull, Push and the commands go.
    pub current: Option<usize>,
}

/// Empty room around the graph, to drag its edges a little inward.
pub const MARGIN: f32 = 16.;

pub fn render(
    graph: &WorkflowGraph,
    layout: &Layout,
    marks: Marks,
    scroll: &ScrollHandle,
    theme: &Theme,
    cx: &mut Context<RepoView>,
) -> AnyElement {
    let Marks {
        selected,
        picked,
        pull,
        hovered,
        stash: shown_stash,
        current,
    } = marks;
    // History, and a release line work grows from, are not the work's own:
    // muted.
    let color = |node: &GraphNode| match node.kind {
        NodeKind::History => theme.secondary,
        _ if node.channel => theme.secondary,
        _ => theme.lanes[node.lane % theme.lanes.len()],
    };

    // Like Sublime Merge's rails: a branch leaves its fork point at once and runs
    // in its own row; a merge runs in the merged branch's row until the end.
    let mut edges: Vec<Edge> = graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(i, node)| {
            node.kind != NodeKind::Uncommitted
                || (layout.detached.contains(i) && !layout.framed.contains(i))
        })
        .flat_map(|(i, node)| {
            let (start, end, y) = layout.nodes[i];
            let start = start - bar_gap(start, end);
            node.parents.iter().enumerate().map(move |(k, &p)| {
                let (parent_start, parent_end, parent_y) = layout.nodes[p];
                let end = parent_end + bar_gap(parent_start, parent_end);
                let merge = k > 0;
                // A merge rail belongs to the merged branch, a fork to the new one.
                let owner = if merge { &graph.nodes[p] } else { node };
                Edge {
                    from: point(px(end), px(parent_y)),
                    to: point(px(start), px(y)),
                    merge,
                    dashed: false,
                    strong: false,
                    current: false,
                    lane: owner.lane,
                    color: color(owner),
                }
            })
        })
        .collect();
    // Older commits left out: a dashed rail into the oldest node from a mark
    // that brings more of them, as scrolling to the left edge does.
    let oldest = graph
        .nodes
        .iter()
        .position(|node| node.kind == NodeKind::History);
    if let Some(oldest) = oldest.filter(|_| graph.older > 0) {
        let (start, _, y) = layout.nodes[oldest];
        edges.push(Edge {
            from: point(px(PAD_X + OLDER_WIDTH - 12.), px(y)),
            to: point(px(start), px(y)),
            merge: false,
            dashed: true,
            strong: false,
            current: false,
            lane: graph.nodes[oldest].lane,
            color: theme.secondary,
        });
    }
    // A clean worktree off the middle of a lane leaves its frame for a row
    // of its own, where its lead runs on to its name.
    for &w in &layout.framed {
        let parent = graph.nodes[w].parents[0];
        let (_, parent_end, parent_y) = layout.nodes[parent];
        let y = layout.nodes[w].2;
        if y == parent_y {
            continue;
        }
        let frames = layout
            .framed
            .iter()
            .filter(|&&other| graph.nodes[other].parents.first() == Some(&parent))
            .count();
        let pad = FRAME_PAD + (frames - 1) as f32 * FRAME_STEP;
        let picked = current == Some(w);
        // Up from the frame's top edge, or down from its bottom one.
        let edge = match y < parent_y {
            true => parent_y - style().dot / 2. - pad,
            false => parent_y + style().dot / 2. + pad,
        };
        edges.push(Edge {
            from: point(px(parent_end), px(edge)),
            to: point(px(parent_end + style().dot / 2. + FORK_GAP), px(y)),
            merge: false,
            dashed: true,
            strong: picked || selected.contains(&w),
            current: picked,
            lane: row_lane(graph, layout, w),
            // The worktree's color, as its lead's: one line from frame to name.
            color: color(&graph.nodes[w]),
        });
    }
    // Dashed lines lead from the worktrees to their names in the list.
    for (from, y, group) in &layout.leads {
        let first = group[0];
        // The current worktree's lead has longer dashes, from its branch or
        // commit to its name: where work goes, whatever the review shows.
        let picked = group.iter().any(|&w| current == Some(w));
        // Heavier for the current worktree, a selected one, or one hovered
        // with its commit, its branch label or its tag.
        let strong = picked
            || group.iter().any(|&w| {
                selected.contains(&w)
                    || hovered == Some(w)
                    || hovered.is_some() && hovered == graph.nodes[w].parents.first().copied()
            });
        edges.push(Edge {
            from: point(px(*from), px(*y)),
            to: point(px(layout.column - 6.), px(*y)),
            merge: false,
            dashed: true,
            current: picked,
            strong,
            lane: row_lane(graph, layout, first),
            // Full color for the current worktree and one picked or hovered;
            // lighter for the rest, pointers rather than history; fainter
            // still for worktrees done with.
            color: match (strong, group.iter().all(|&w| stale_worktree(graph, w))) {
                (true, _) => color(&graph.nodes[first]),
                (false, false) => color(&graph.nodes[first]).opacity(0.6),
                (false, true) => color(&graph.nodes[first]).opacity(0.25),
            },
        });
    }
    // A stash forks up from its commit, dashed: set aside, not history; a
    // tag does too, off the main line.
    for place in &layout.stashes {
        let (parent_start, parent_end, parent_y) = layout.nodes[place.node];
        // A commit framed by clean worktrees: the line leaves the outermost
        // frame's top edge, not the commit inside it, so it crosses none.
        let frames = layout
            .framed
            .iter()
            .filter(|&&w| graph.nodes[w].parents.first() == Some(&place.node))
            .count();
        let from = match frames {
            0 => point(
                px(parent_end + bar_gap(parent_start, parent_end)),
                px(parent_y),
            ),
            n => {
                let pad = FRAME_PAD + (n - 1) as f32 * FRAME_STEP;
                point(px(parent_end), px(parent_y - style().dot / 2. - pad))
            }
        };
        // A tag's line is history's, solid, in its colour, up to its label.
        edges.push(match place.tag {
            true => Edge {
                from,
                to: point(px(place.x), px(place.y)),
                merge: false,
                dashed: false,
                strong: false,
                current: false,
                lane: usize::MAX,
                color: theme.git_tag_background,
            },
            false => Edge {
                from,
                to: point(px(place.x - symbol() / 2.), px(place.y)),
                merge: false,
                dashed: true,
                strong: shown_stash == Some(place.name.as_str()),
                current: false,
                lane: usize::MAX,
                color: theme.secondary,
            },
        });
    }
    // The current worktree's branch, whose label has its bar as the
    // worktree's name in the list does.
    let picked_branches: Vec<&str> = current
        .and_then(|w| graph.nodes[w].branch.as_deref())
        .into_iter()
        .collect();
    // Hovering a line pairs a local branch with its remote one: both
    // labels are ringed, wherever they are.
    let paired: Vec<String> = hovered
        .map(|i| {
            let node = &graph.nodes[i];
            let names = node.refs.iter().chain(node.branch.as_ref());
            names
                .filter_map(|name| Some([name.clone(), partner(graph, name)?]))
                .flatten()
                .collect()
        })
        .unwrap_or_default();
    // The hovered node's branch is drawn bold on top of everything else.
    let hovered_lane = hovered.map(|i| row_lane(graph, layout, i));
    let (bold, plain): (Vec<Edge>, Vec<Edge>) = edges
        .into_iter()
        // Leads stay under the labels they run behind, bold or not.
        .partition(|edge| !edge.dashed && Some(edge.lane) == hovered_lane);
    let mut content = div()
        .relative()
        .w(px(layout.width))
        .h(px(layout.height))
        .child(rails(plain, style().line));
    let xs: Vec<(f32, f32)> = layout.nodes.iter().map(|node| (node.0, node.1)).collect();
    for (i, node) in graph.nodes.iter().enumerate() {
        let (start, end, y) = layout.nodes[i];
        // A commit picked in the list can sit off the selected nodes, on a
        // branch a selected merge brought in: its node shows it as picked.
        let is_selected = selected.contains(&i)
            || (!picked.is_empty() && node.commits.iter().any(|c| picked.contains(&c.id)));
        let color = color(node);
        let worktree = node.kind == NodeKind::Uncommitted;
        let dirty = (node.added, node.removed) != (0, 0);

        // A worktree is a rectangle: around its HEAD commit when clean, around
        // a dot for its unsaved changes otherwise. Clean ones on one commit nest.
        if worktree && !dirty {
            let parent = node.parents[0];
            let (start, end, y) = layout.nodes[parent];
            let nested = graph.nodes[..i]
                .iter()
                .filter(|other| {
                    other.kind == NodeKind::Uncommitted
                        && other.parents.first() == Some(&parent)
                        && (other.added, other.removed) == (0, 0)
                })
                .count();
            let pad = FRAME_PAD + nested as f32 * FRAME_STEP;
            // Drawn only while the worktree is selected, in the selection's
            // color: otherwise its dashed lead shows where it stands, and a
            // frame would crowd the commit.
            if is_selected {
                content = content.child(
                    div()
                        .absolute()
                        .left(px(start - style().dot / 2. - pad))
                        .top(px(y - style().dot / 2. - pad))
                        .w(px(end - start + style().dot + 2. * pad))
                        .h(px(style().dot + 2. * pad))
                        .rounded(px(4.))
                        .border_2()
                        .border_color(theme.foreground)
                        .bg(theme.accent.opacity(0.2)),
                );
            }
            // The frame's edge, drawn or not, picks the worktree and opens its
            // menu; inside it, the commit keeps its own clicks.
            let target = worktree_target(graph, i);
            let (left, top) = (start - style().dot / 2. - pad, y - style().dot / 2. - pad);
            let (width, height) = (end - start + style().dot + 2. * pad, style().dot + 2. * pad);
            for (k, (x, y, w, h)) in [
                (left, top - 2., width, 4.),
                (left, top + height - 2., width, 4.),
                (left - 2., top, 4., height),
                (left + width - 2., top, 4., height),
            ]
            .into_iter()
            .enumerate()
            {
                let target = target.clone();
                content = content.child(
                    div()
                        .id(("frame-edge", i * 4 + k))
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(w))
                        .h(px(h))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                            this.click_node(i, event.modifiers().platform, cx)
                        }))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            this.hover_node(i, *hovered, cx)
                        }))
                        .on_mouse_down(MouseButton::Right, open_menu(target.clone(), cx)),
                );
            }
            continue;
        }
        // Commits are dots, or with bars a run of beads. Selected nodes get a ring.
        // Only a changed worktree is left here; a clean one was its frame.
        let hit = if worktree { symbol_step() } else { HIT };
        // A selected dot keeps its size: the line under it says it is selected.
        let size = style().dot;
        let shape = if worktree {
            // Work not yet committed: a diamond, no shape a commit has, alone
            // in the room a clean worktree's frame takes. Selected, it is
            // underlined as a commit is.
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(symbol()))
                .child(diamond(color, symbol() - 2.))
        } else if end > start {
            let selection = is_selected.then_some(picked);
            beads(i, &node.commits, end - start, color, selection, theme, cx)
        } else {
            // A commit is an upright ellipse, as each bead of a bar is.
            let width = (end - start + size).min(bead_width() * size / style().dot);
            div()
                .w(px(width))
                .h(px(size))
                .rounded(px(size / 2.))
                .bg(color)
                // A selected commit the commit list leaves out fades.
                .when(
                    is_selected
                        && !picked.is_empty()
                        && !node.commits.is_empty()
                        && !node
                            .commits
                            .iter()
                            .any(|commit| picked.contains(&commit.id)),
                    |shape| shape.opacity(0.3),
                )
        };
        // A selected commit, bar or worktree's changes is underlined in the
        // text's color, near black or, when dark, near white: a border on the dots,
        // or the accent, would vanish on lanes of a like colour.
        // Framed by a selected clean worktree, whose frame is drawn, the line
        // goes under the outermost frame, clear of it, as wide as the frame.
        if is_selected {
            let frames = layout
                .framed
                .iter()
                .filter(|&&w| graph.nodes[w].parents.first() == Some(&i))
                .filter(|w| selected.contains(w))
                .count();
            let half = match (worktree, frames) {
                (true, _) => symbol() / 2.,
                (false, 0) => style().dot / 2.,
                (false, n) => style().dot / 2. + FRAME_PAD + (n - 1) as f32 * FRAME_STEP,
            };
            content = content.child(
                div()
                    .absolute()
                    .left(px(start - half))
                    .top(px(y + half + 3.))
                    .w(px(end - start + 2. * half))
                    .h(px(2.5))
                    .rounded(px(1.5))
                    .bg(theme.foreground),
            );
        }
        content =
            content.child(
                div()
                    .id(("node", i))
                    .absolute()
                    .left(px(start - hit / 2.))
                    .top(px(y - HIT / 2.))
                    .w(px(end - start + hit))
                    .h(px(HIT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.hover_node(i, *hovered, cx)
                    }))
                    // A commit node's menu, and a double click, act on its
                    // newest commit.
                    .when_some(node.commits.last().filter(|_| !worktree), |dot, commit| {
                        let target = Target::Commit {
                            commit: commit.id.clone(),
                            message: match commit.body.is_empty() {
                                true => commit.summary.clone(),
                                false => format!("{}\n\n{}", commit.summary, commit.body),
                            },
                        };
                        dot.on_mouse_down(MouseButton::Right, open_menu(target.clone(), cx))
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                match event.click_count() {
                                    2 => this.check_out_here(&target, window, cx),
                                    _ => this.click_node(i, event.modifiers().platform, cx),
                                }
                            }))
                    })
                    .when(worktree, |square| {
                        square.on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                            this.click_node(i, event.modifiers().platform, cx)
                        }))
                    })
                    .when(worktree, |square| {
                        let target = worktree_target(graph, i);
                        square.on_mouse_down(MouseButton::Right, open_menu(target.clone(), cx))
                    })
                    .child(shape),
            );
        if worktree {
            // A worktree on a row of its own carries its branch's label there.
            if let Some(name) = own_row_branch(graph, &layout.detached, i) {
                let target = label_target(graph, name, String::new());
                let tag = branch_tag(graph, name, name, color, true, theme);
                let tag = with_current_lines(tag, picked_branches.contains(&name), theme);
                let tag = with_pair(tag, graph, name, &paired, theme);
                content = content.child(
                    div()
                        .absolute()
                        .left(px(end + symbol() / 2. + TAG_GAP))
                        .top(px(y - TAG_HEIGHT / 2.))
                        .flex()
                        .whitespace_nowrap()
                        .text_xs()
                        .child(menu_label(tag, name, target, i, true, cx)),
                );
            }
            continue;
        }

        if node.refs.is_empty() {
            continue;
        }
        // Branches get outlined tags.
        // Branches whose worktree has a row of its own are labelled there.
        let elsewhere: Vec<&str> = graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, w)| w.kind == NodeKind::Uncommitted && w.parents.first() == Some(&i))
            .filter_map(|(w, _)| own_row_branch(graph, &layout.detached, w))
            .collect();
        let branches: Vec<AnyElement> = node
            .refs
            .iter()
            .filter(|name| !elsewhere.contains(&name.as_str()))
            .filter(|name| !beside_local(graph, node, name))
            .map(|name| {
                let commit = match &node.tip {
                    Revision::Commit(id) => id.clone(),
                    Revision::Worktree(_) => String::new(),
                };
                let target = label_target(graph, name, commit);
                // A branch checked out in a worktree is filled, as its tag in
                // the worktree list is.
                let checked_out = matches!(target, Target::Worktree { .. });
                let tag = branch_tag(graph, name, name, color, checked_out, theme);
                let tag = with_current_lines(tag, picked_branches.contains(&name.as_str()), theme);
                let tag = with_pair(tag, graph, name, &paired, theme);
                menu_label(tag, name, target, i, true, cx)
            })
            .collect();
        content = content.child(
            div()
                .absolute()
                .left(px(tags_start(i, &xs)))
                .top(px(y - TAG_HEIGHT / 2.))
                .flex()
                .items_center()
                .gap(px(6.))
                .whitespace_nowrap()
                .text_xs()
                .children(branches),
        );
    }

    // The worktree list: checked-out branches get filled tags, as Sublime Merge
    // marks HEAD.
    for (w, worktree) in graph.nodes.iter().enumerate() {
        if worktree.kind != NodeKind::Uncommitted {
            continue;
        }
        let (_, _, y) = layout.nodes[w];
        let Some(x) = layout.list_x[w] else {
            continue;
        };
        let color = color(worktree);
        // Attention knows a worktree by its first ref, whatever the label says.
        // Branches have labels of their own at their commits; the list names
        // the worktrees by folder.
        let label = worktree_label(worktree, WorktreeNames::Folder);
        // How long since its last work, a commit or a change; one quiet for
        // long fades, unless it is the current one.
        let now = crate::repo_view::now_seconds();
        let quiet = now - worktree.last_time > QUIET_AFTER && current != Some(w);
        let age = div()
            .ml(px(4.))
            .text_color(theme.secondary)
            .child(crate::repo_view::short_age(worktree.last_time, now));
        let tags: Vec<AnyElement> = worktree
            .refs
            .first()
            .map(|name| {
                let branch = worktree.branch.as_deref();
                // The folder's name, with its branch's standing.
                let tag = branch_tag(
                    graph,
                    branch.unwrap_or_default(),
                    &label,
                    color,
                    true,
                    theme,
                );
                menu_label(tag, name, worktree_target(graph, w), w, false, cx)
            })
            .into_iter()
            .collect();
        content =
            content.child(
                div()
                    .id(("worktree-tags", w))
                    .absolute()
                    .left(px(x))
                    .top(px(y - TAG_HEIGHT / 2.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .whitespace_nowrap()
                    .text_xs()
                    .cursor_pointer()
                    // The current worktree, where work goes, has a bar
                    // before its name, whatever the review shows; what the
                    // review shows is marked on the nodes themselves.
                    .children(
                        (current == Some(w))
                            .then(|| current_lines(theme))
                            .into_iter()
                            .flatten(),
                    )
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        this.click_node(w, event.modifiers().platform, cx)
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.hover_node(w, *hovered, cx)
                    }))
                    .when(quiet, |row| row.opacity(0.5))
                    .children(tags)
                    .child(age),
            );
    }

    // Each stash: a muted diamond, set-aside changes as a worktree's are,
    // and its label; a click shows it, a right click offers its menu.
    for (k, place) in layout.stashes.iter().enumerate() {
        // A tag: its label where its line ends, with the tag's menu.
        if place.tag {
            let target = Target::Tag {
                name: place.name.clone(),
                commit: graph.nodes[place.node]
                    .commits
                    .last()
                    .map(|commit| commit.id.clone())
                    .unwrap_or_default(),
            };
            let label = pill()
                .bg(theme.git_tag_background)
                .text_color(theme.git_tag_text)
                .child(place.name.clone());
            content = content.child(
                div()
                    .absolute()
                    .left(px(place.x))
                    .top(px(place.y - TAG_HEIGHT / 2.))
                    .whitespace_nowrap()
                    .text_xs()
                    .child(with_menu(label, ("git-tag", k), target, cx)),
            );
            continue;
        }
        let Some(stash) = graph.stashes.iter().find(|stash| stash.name == place.name) else {
            continue;
        };
        let shown = shown_stash == Some(place.name.as_str());
        let size = style().dot + 1.;
        let show = place.name.clone();
        content = content.child(
            div()
                .id(("stash-mark", k))
                .absolute()
                .left(px(place.x - size / 2.))
                .top(px(place.y - size / 2.))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.show_stash(&show, cx)))
                .child(diamond(
                    if shown {
                        theme.foreground
                    } else {
                        theme.secondary
                    },
                    size,
                )),
        );
        let target = Target::Stash {
            name: place.name.clone(),
            id: stash.id.clone(),
        };
        let label = pill()
            .border_1()
            .border_color(if shown {
                theme.foreground
            } else {
                theme.secondary
            })
            .bg(if shown {
                theme.selection
            } else {
                theme.tag_background
            })
            .text_color(if shown {
                theme.foreground
            } else {
                theme.secondary
            })
            .italic()
            .child(place.name.clone());
        let show = place.name.clone();
        content = content.child(
            div()
                .id(("stash-label", k))
                .absolute()
                .left(px(place.x + symbol() / 2. + TAG_GAP))
                .top(px(place.y - TAG_HEIGHT / 2.))
                .whitespace_nowrap()
                .text_xs()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.show_stash(&show, cx)))
                .child(with_menu(label, ("stash", k), target, cx)),
        );
    }

    if let Some(oldest) = oldest.filter(|_| graph.older > 0) {
        let (_, _, y) = layout.nodes[oldest];
        content = content.child(
            div()
                .id("older-commits")
                .absolute()
                .left(px(PAD_X - 8.))
                .top(px(y - TAG_HEIGHT / 2.))
                .h(px(TAG_HEIGHT))
                .flex()
                .items_center()
                .px(px(6.))
                .rounded(px(4.))
                .whitespace_nowrap()
                .text_xs()
                .text_color(theme.secondary)
                .cursor_pointer()
                .hover(|mark| mark.bg(theme.selection))
                .on_click(cx.listener(|this, _, _, cx| this.load_older(cx)))
                .map(|mark| match pull {
                    // Pulling: the mark fills as it nears loading.
                    Some(done) if done >= 1. => mark.text_color(theme.accent).child("Loading…"),
                    Some(done) => mark
                        .text_color(theme.foreground)
                        .bg(theme.selection.opacity(0.3 + 0.7 * done))
                        .child(format!("↤ {HISTORY_HINT}")),
                    None => mark.child(format!("⋯ {} older", graph.older)),
                }),
        );
    }

    div()
        .id("graph")
        .size_full()
        .overflow_scroll()
        .track_scroll(scroll)
        // A graph narrower than the view ends at its right edge, as wider ones
        // open scrolled there. The scroll area measures this row, so it is as
        // wide as the graph, or the view when that is wider; the auto margin
        // then pushes the graph right.
        .child(
            div()
                .flex()
                .w(px(layout.width + 2. * MARGIN))
                .min_w_full()
                .p(px(MARGIN))
                .child(
                    content
                        .flex_none()
                        .ml_auto()
                        .child(rails(bold, style().hover_line)),
                ),
        )
        .into_any_element()
}

/// The lane whose row a node is drawn in: worktree squares after a lane's
/// last commit sit in that lane's row.
fn row_lane(graph: &WorkflowGraph, layout: &Layout, i: usize) -> usize {
    let node = &graph.nodes[i];
    match node.kind {
        NodeKind::Uncommitted if !layout.detached.contains(&i) => graph.nodes[node.parents[0]].lane,
        _ => node.lane,
    }
}

/// Draws rails over the whole graph area.
fn rails(edges: Vec<Edge>, width: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            for edge in &edges {
                let (from, to) = (bounds.origin + edge.from, bounds.origin + edge.to);
                let mut path = match (edge.dashed, edge.current) {
                    (true, true) => PathBuilder::stroke(px(2.)).dash_array(&[px(8.), px(4.)]),
                    (true, false) => {
                        let width = if edge.strong { 2. } else { 1. };
                        PathBuilder::stroke(px(width)).dash_array(&[px(4.), px(3.)])
                    }
                    (false, _) => PathBuilder::stroke(px(width)),
                };
                path.move_to(from);
                let bend = (to.x - from.x).clamp(px(0.), px(style().curve));
                if from.y == to.y {
                    path.line_to(to);
                } else if edge.merge {
                    let corner = point(to.x - bend, from.y);
                    path.line_to(corner);
                    s_curve(&mut path, corner, to);
                } else {
                    // A fork drops from its commit first, then turns into its
                    // row: the commit's labels sit right after it on its row,
                    // and forks from one commit share the drop, the further
                    // turning later, so neither crosses a label nor another.
                    let bend = bend.min((to.y - from.y).abs());
                    let toward = if to.y > from.y { -bend } else { bend };
                    path.line_to(point(from.x, to.y + toward));
                    path.curve_to(point(from.x + bend, to.y), point(from.x, to.y));
                    path.line_to(to);
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, edge.color);
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A node's commits as narrow ellipses close together over `length` pixels,
/// one per commit up to what fits.
/// Clicking a bead selects its commit, Cmd-click adds or removes it and
/// Shift-click picks the commits from the last one; when commits outnumber the beads,
/// each stands for the commit at its place along the node.
/// With `selection`, the node is selected: beads for the commits picked in
/// the commit list stand out and the others fade.
fn beads(
    node: usize,
    commits: &[wtf_core::Commit],
    length: f32,
    color: Hsla,
    selection: Option<&[String]>,
    theme: &Theme,
    cx: &mut Context<RepoView>,
) -> gpui::Div {
    let count = bead_count(commits.len(), length);
    // Newest last, as the node runs left to right.
    let ats: Vec<usize> = (0..count)
        .map(|k| match count {
            1 => 0,
            _ => ((k * (commits.len() - 1) + (count - 1) / 2) / (count - 1)).min(commits.len() - 1),
        })
        .collect();
    // A bead stands for the commits nearer to it than to its neighbours.
    let covers = |k: usize| {
        let low = if k == 0 {
            0
        } else {
            (ats[k - 1] + ats[k]) / 2 + 1
        };
        let high = if k + 1 == count {
            commits.len() - 1
        } else {
            (ats[k] + ats[k + 1]) / 2
        };
        &commits[low.min(high)..=high]
    };
    let step = if count > 1 {
        length / (count - 1) as f32
    } else {
        0.
    };
    let mut row = div()
        .relative()
        .w(px(length + bead_width()))
        .h(px(style().dot));
    for k in 0..count {
        let commit = commits[ats[k]].id.clone();
        // A right click on a bead offers the menu of its own commit, not
        // the bar's newest.
        let bead_commit = &commits[ats[k]];
        let menu = Target::Commit {
            commit: bead_commit.id.clone(),
            message: match bead_commit.body.is_empty() {
                true => bead_commit.summary.clone(),
                false => format!("{}\n\n{}", bead_commit.summary, bead_commit.body),
            },
        };
        let picked = selection
            .filter(|picked| !picked.is_empty())
            .map(|picked| covers(k).iter().any(|commit| picked.contains(&commit.id)));
        // Each bead's click area reaches halfway to its neighbours.
        let reach = step.max(bead_width());
        row = row.child(
            div()
                .id(("bead", node * 1000 + k))
                .absolute()
                .left(px(k as f32 * step + bead_width() / 2. - reach / 2.))
                .w(px(reach))
                .h(px(HIT))
                .top(px((style().dot - HIT) / 2.))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Right, open_menu(menu.clone(), cx))
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    let modifiers = event.modifiers();
                    // Cmd or Shift on a selected node's bead picks commits as
                    // in the commit list; elsewhere Cmd adds the node.
                    let picked = (modifiers.platform || modifiers.shift)
                        && this.pick_bead(node, &commit, modifiers, cx);
                    if modifiers.platform && !picked {
                        return;
                    }
                    cx.stop_propagation();
                    match event.click_count() {
                        2 => this.check_out_here(&menu, window, cx),
                        _ if picked => {}
                        _ => this.click_bead(node, commit.clone(), cx),
                    }
                })),
        );
        row = row.child(
            div()
                .absolute()
                .left(px(k as f32 * step))
                .w(px(bead_width()))
                .h(px(style().dot))
                .rounded(px(bead_width() / 2.))
                .bg(color)
                .map(|bead| match (selection, picked) {
                    // A picked commit stands taller in its own colour.
                    (_, Some(true)) => bead.top(px(-2.)).h(px(style().dot + 4.)),
                    (_, Some(false)) => bead.opacity(0.3),
                    (Some(_), None) | (None, _) => bead,
                }),
        );
    }
    // More commits than beads: the middle of the bar says how many it holds,
    // over a gap, so the run does not read as one commit per bead.
    let hidden = commits.len().saturating_sub(count);
    if hidden > 0 && count >= 2 {
        let label = format!("{}", commits.len());
        let width = label.chars().count() as f32 * 6. + 6.;
        row = row.child(
            div()
                .absolute()
                .left(px(length / 2. + bead_width() / 2. - width / 2.))
                .top(px(style().dot / 2. - 7.))
                .w(px(width))
                .h(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .bg(theme.graph_background)
                .border_1()
                .border_color(color)
                .text_color(color)
                .text_size(px(9.))
                .child(label),
        );
    }
    row
}

/// How far rails stop short of a bar's centre line, so they end at its
/// first and last beads instead of running over them; dots cover their ends.
fn bar_gap(start: f32, end: f32) -> f32 {
    if end > start {
        bead_width() / 2. + 1.
    } else {
        0.
    }
}

/// Beads never overlap: at most one per `style().bead_spacing` pixels.
fn bead_count(commits: usize, length: f32) -> usize {
    let fits = (length / style().bead_spacing).floor() as usize + 1;
    commits.min(fits).max(2)
}

/// The node nearest a point at `x`, `y` the way an arrow `key` points, of
/// nodes as laid out: what lies off to the side counts three times what
/// lies ahead, so a row or lane is followed before others are reached.
pub fn nearest(nodes: &[(f32, f32, f32)], x: f32, y: f32, key: &str) -> Option<usize> {
    nodes
        .iter()
        .enumerate()
        .filter_map(|(i, &(start, end, row))| {
            let (dx, dy) = ((start + end) / 2. - x, row - y);
            let (ahead, aside) = match key {
                "left" => (-dx, dy),
                "right" => (dx, dy),
                "up" => (-dy, dx),
                "down" => (dy, dx),
                _ => return None,
            };
            (ahead > 1.).then_some((i, ahead + 3. * aside.abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// A right-click listener opening `target`'s menu at the mouse.
fn open_menu(
    target: Target,
    cx: &mut Context<RepoView>,
) -> impl Fn(&MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static {
    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
        cx.stop_propagation();
        this.open_context_menu(target.clone(), event.position, cx);
    })
}

/// A label that opens its right-click menu.
fn with_menu(
    label: gpui::Div,
    id: (&'static str, usize),
    target: Target,
    cx: &mut Context<RepoView>,
) -> AnyElement {
    label
        .id(id)
        .on_mouse_down(MouseButton::Right, open_menu(target.clone(), cx))
        .into_any_element()
}

/// A label with its menu on a right-click only. A left click, when the
/// label has nothing else to do on one (`select`), selects node `hover`, as
/// a click on the node does, and a double click checks it out in the
/// current worktree. Hovering it hovers that node too.
fn menu_label(
    tag: gpui::Div,
    name: &str,
    target: Target,
    hover: usize,
    select: bool,
    cx: &mut Context<RepoView>,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("label-{name}")))
        .flex()
        .items_center()
        .on_hover(
            cx.listener(move |this, hovered: &bool, _, cx| this.hover_node(hover, *hovered, cx)),
        )
        .on_mouse_down(MouseButton::Right, open_menu(target.clone(), cx))
        .when(select, |label| {
            label.cursor_pointer().on_click(cx.listener(
                move |this, event: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    match event.click_count() {
                        2 => this.check_out_here(&target, window, cx),
                        _ => this.click_node(hover, event.modifiers().platform, cx),
                    }
                },
            ))
        })
        .child(tag)
        .into_any_element()
}

/// A chevron pointing left or right, drawn so it sits on its box's centre
/// whatever the font.
pub fn chevron(left: bool, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let (o, w, h) = (bounds.origin, bounds.size.width, bounds.size.height);
            let (near, far) = match left {
                true => (o.x + w * 0.25, o.x + w * 0.75),
                false => (o.x + w * 0.75, o.x + w * 0.25),
            };
            let mut path = PathBuilder::stroke(px(2.));
            path.move_to(point(far, o.y + h * 0.1));
            path.line_to(point(near, o.y + h * 0.5));
            path.line_to(point(far, o.y + h * 0.9));
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .flex_none()
    .w(px(7.))
    .h(px(10.))
}

/// The other half of a branch's pair: a local branch's remote one, or the
/// local branch tracking a remote one.
fn partner(graph: &WorkflowGraph, name: &str) -> Option<String> {
    let branch = graph.branches.iter().find(|branch| branch.name == name)?;
    match branch.remote {
        false => branch.upstream.clone(),
        true => graph
            .branches
            .iter()
            .find(|local| local.upstream.as_deref() == Some(name))
            .map(|local| local.name.clone()),
    }
}

/// A label in the hovered pair: ringed in the text's colour, and saying
/// where the other half is and how the two differ, under it, over what is
/// there, so nothing moves.
fn with_pair(
    label: gpui::Div,
    graph: &WorkflowGraph,
    name: &str,
    paired: &[String],
    theme: &Theme,
) -> gpui::Div {
    if !paired.iter().any(|paired| paired == name) {
        return label;
    }
    let ring = div()
        .absolute()
        .top(px(-3.))
        .bottom(px(-3.))
        .left(px(-3.))
        .right(px(-3.))
        .rounded(px(6.))
        .border_2()
        .border_color(theme.foreground);
    let Some(other) = partner(graph, name) else {
        return label.relative().child(ring);
    };
    let local = match graph.branches.iter().any(|b| b.name == name && !b.remote) {
        true => name,
        false => other.as_str(),
    };
    let gone = !graph.branches.iter().any(|branch| branch.name == other);
    let state = graph
        .branches
        .iter()
        .find(|branch| branch.name == local && !gone)
        .map(|branch| match (branch.ahead, branch.behind) {
            (0, 0) => "same commit".to_string(),
            (ahead, 0) => format!("{ahead} ahead"),
            (0, behind) => format!("{behind} behind"),
            (ahead, behind) => format!("{ahead} ahead, {behind} behind"),
        })
        .unwrap_or_else(|| "gone from the remote".to_string());
    // Said once, under the local half, the remote one named.
    let note = (local == name).then(|| {
        div()
            .absolute()
            .top(px(TAG_HEIGHT + 4.))
            .left_0()
            .px(px(6.))
            .py(px(1.))
            .rounded(px(4.))
            .bg(theme.graph_background)
            .border_1()
            .border_color(theme.separator)
            .text_color(theme.foreground)
            .font_weight(gpui::FontWeight::NORMAL)
            .whitespace_nowrap()
            .child(format!("⇄ {other} · {state}"))
    });
    label.relative().child(ring).children(note)
}

/// The lines marking the current worktree's labels, in the text's color, above
/// and below a label and outside its room: the label keeps its place, and
/// nothing on its row, a commit's frame or a rail, runs into them.
fn current_lines(theme: &Theme) -> [gpui::Div; 2] {
    let line = || {
        div()
            .absolute()
            .left_0()
            .right_0()
            .h(px(2.))
            .rounded(px(1.))
            .bg(theme.foreground)
    };
    [line().top(px(-4.)), line().bottom(px(-4.))]
}

/// A label with the current worktree's lines above and below, when `on`.
fn with_current_lines(label: gpui::Div, on: bool, theme: &Theme) -> gpui::Div {
    match on {
        true => div()
            .relative()
            .flex()
            .items_center()
            .children(current_lines(theme))
            .child(label),
        false => label,
    }
}

/// A label's shape, as git tags, stashes and branches share it.
fn pill() -> gpui::Div {
    div()
        .flex_none()
        .h(px(TAG_HEIGHT))
        .flex()
        .items_center()
        .px(px(5.))
        .rounded(px(3.))
}

fn tag(name: &str, color: Hsla, filled: bool, theme: &Theme) -> gpui::Div {
    pill()
        .pl(px(6.))
        .border_l_4()
        .border_color(color)
        .map(|tag| match filled {
            true => tag
                .bg(color)
                .text_color(theme.tag_head_text)
                .font_weight(gpui::FontWeight::SEMIBOLD),
            false => tag.bg(theme.tag_background).text_color(theme.foreground),
        })
        .child(name.to_string())
}

struct Edge {
    from: Point<Pixels>,
    to: Point<Pixels>,
    /// From a merged branch into the merge commit.
    merge: bool,
    /// A worktree's lead to the worktree list.
    dashed: bool,
    /// The lead of a selected or hovered worktree, drawn heavier.
    strong: bool,
    /// The current worktree's lead: longer dashes, full color. Leads stay
    /// dashed, so solid always means history.
    current: bool,
    /// The branch the rail belongs to.
    lane: usize,
    color: Hsla,
}

/// A smooth step from `from` to `to`, horizontal at both ends.
fn s_curve(path: &mut PathBuilder, from: Point<Pixels>, to: Point<Pixels>) {
    let mid = point((from.x + to.x) / 2., (from.y + to.y) / 2.);
    path.curve_to(mid, point(mid.x, from.y));
    path.curve_to(to, point(mid.x, to.y));
}

#[cfg(test)]
mod tests {
    use wtf_core::{Commit, Revision};

    use super::*;

    fn node(lane: usize, parents: &[usize], commits: usize, time: i64) -> GraphNode {
        let commit = Commit {
            id: String::new(),
            short_id: String::new(),
            summary: String::new(),
            body: String::new(),
            author: String::new(),
            email: String::new(),
            time,
            parent: None,
        };
        GraphNode {
            key: String::new(),
            kind: NodeKind::Commits,
            lane,
            commits: vec![commit; commits],
            parents: parents.to_vec(),
            first_time: time,
            last_time: time + commits as i64 - 1,
            refs: Vec::new(),
            branch: None,
            tags: Vec::new(),
            stashes: Vec::new(),
            added: 0,
            removed: 0,
            base: None,
            tip: Revision::Commit(String::new()),
            channel: false,
        }
    }

    fn graph(nodes: Vec<GraphNode>) -> WorkflowGraph {
        let lane_count = nodes.iter().map(|node| node.lane + 1).max().unwrap_or(0);
        let graph = WorkflowGraph {
            nodes,
            lane_count,
            main_node: Some(0),
            branches: Vec::new(),
            main: String::new(),
            older: 0,
            tags: Vec::new(),
            stashes: Vec::new(),
            remotes: Vec::new(),
            main_worktree: None,
            on_main: Vec::new(),
            channels: Vec::new(),
            idle_days: None,
        };
        WorkflowGraph {
            on_main: graph.main_history(),
            ..graph
        }
    }

    #[test]
    fn arrows_go_to_the_nearest_node_that_way() {
        // Two on the main row, one on a row above, one far off it.
        let nodes = [
            (0., 10., 50.),
            (40., 50., 50.),
            (20., 30., 20.),
            (200., 210., 20.),
        ];
        assert_eq!(nearest(&nodes, 5., 50., "right"), Some(1), "along the row");
        assert_eq!(nearest(&nodes, 45., 50., "left"), Some(0));
        assert_eq!(
            nearest(&nodes, 5., 50., "up"),
            Some(2),
            "the nearer one above"
        );
        assert_eq!(nearest(&nodes, 25., 20., "down"), Some(0));
        assert_eq!(nearest(&nodes, 5., 50., "left"), None, "nothing that way");
    }

    #[test]
    fn topology_places_children_a_column_after_parents() {
        let graph = graph(vec![
            node(0, &[], 1, 0),
            node(0, &[0], 1, 1),
            node(1, &[0], 1, 2),
        ]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);

        assert_eq!(layout.nodes[0].0, PAD_X);
        assert_eq!(layout.nodes[1].0, PAD_X + style().column);
        assert_eq!(layout.nodes[2].0, PAD_X + style().column);
        assert!(
            layout.nodes[2].2 > layout.nodes[1].2,
            "the branch gets its own row"
        );
    }

    #[test]
    fn beads_grow_with_commit_count_up_to_a_cap() {
        let graph = graph(vec![node(0, &[], 4, 0), node(0, &[0], 2000, 1)]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);
        let length = |i: usize| layout.nodes[i].1 - layout.nodes[i].0;
        assert_eq!(length(0), 3. * style().bead_spacing);
        assert_eq!(length(1), style().beads as f32 * style().bead_spacing);
    }

    #[test]
    fn worktree_on_a_merged_branch_stays_below_the_main_row() {
        let mut worktree = node(1, &[2], 1, 4);
        worktree.kind = NodeKind::Uncommitted;
        worktree.added = 1;
        let mut merged = node(1, &[0], 1, 2);
        merged.refs = vec!["merged".to_string()];
        let mut open = node(2, &[0], 1, 5);
        open.refs = vec!["open".to_string()];
        let mut open_worktree = node(2, &[5], 1, 5);
        open_worktree.kind = NodeKind::Uncommitted;
        // main: 0 → 1 → 3, merging 2, the branch the worktree is on; 5, an
        // open branch with a clean worktree, off 0 too.
        let mut graph = graph(vec![
            node(0, &[], 1, 0),
            node(0, &[0], 1, 1),
            merged,
            node(0, &[1, 2], 1, 3),
            worktree,
            open,
            open_worktree,
        ]);
        graph.main_node = Some(3);
        graph.on_main = graph.main_history();
        let layout = layout(&graph, WorktreeOrder::LastCommit);
        let y = |i: usize| layout.nodes[i].2;
        // Nothing crosses: the merged branch nearest the main row, its
        // worktree right under it, the open branch under both.
        assert!(y(0) < y(2), "the merged branch below the main row");
        assert!(y(2) < y(4), "the worktree under its branch");
        assert!(y(4) < y(5), "the open branch under both");
    }

    #[test]
    fn changed_worktree_off_the_middle_of_a_lane_gets_its_own_row() {
        let mut worktree = node(1, &[0], 1, 0);
        worktree.kind = NodeKind::Uncommitted;
        // Uncommitted changes need a place of their own.
        worktree.added = 1;
        let mut clean = worktree.clone();
        clean.added = 0;
        let graph_of =
            |worktree: GraphNode| graph(vec![node(0, &[], 1, 0), node(0, &[0], 1, 1), worktree]);
        // A clean one is the frame around its commit; with history after
        // the commit, its lead rises to a row of its own above, so it does
        // not run past the later commits as if it were on them.
        let framed = layout(&graph_of(clean), WorktreeOrder::LastCommit);
        assert!(framed.detached.is_empty() && framed.framed == [2]);
        assert!(framed.list_x[2].is_some());
        assert!(
            framed.nodes[2].2 < framed.nodes[0].2,
            "above its commit's row"
        );
        assert_eq!(framed.leads.len(), 1);
        assert!(framed.leads[0].0 > framed.nodes[0].1);
        assert_eq!(framed.leads[0].1, framed.nodes[2].2, "on its own row");

        let graph = graph_of(worktree);
        let layout = layout(&graph, WorktreeOrder::LastCommit);

        assert_eq!(layout.detached, [2]);
        assert_ne!(layout.nodes[2].2, layout.nodes[0].2);
        assert!(layout.nodes[2].0 > layout.nodes[0].1);
        assert!(
            layout.column > layout.nodes[1].1,
            "the list is right of the graph"
        );
    }

    #[test]
    fn worktree_labels_follow_the_naming_setting() {
        let mut worktree = node(1, &[0], 1, 0);
        worktree.kind = NodeKind::Uncommitted;
        worktree.tip = Revision::Worktree("/repo/.claude/worktrees/docs-review".to_string());
        worktree.branch = Some("worktree-docs-review".to_string());
        let label = worktree_label;
        assert_eq!(
            label(&worktree, WorktreeNames::FolderAndBranch),
            "docs-review · worktree-docs-review"
        );
        assert_eq!(
            label(&worktree, WorktreeNames::Branch),
            "worktree-docs-review"
        );
        assert_eq!(label(&worktree, WorktreeNames::Folder), "docs-review");

        worktree.branch = None;
        assert_eq!(
            label(&worktree, WorktreeNames::FolderAndBranch),
            "docs-review · detached"
        );
        assert_eq!(label(&worktree, WorktreeNames::Branch), "docs-review");
    }

    #[test]
    fn worktree_rows_sort_by_the_chosen_commit() {
        // Two branches with worktrees: one started first, the other ended last.
        let mut first = node(1, &[0], 2, 5);
        first.last_time = 30;
        let second = node(2, &[0], 1, 10);
        let mut nodes = vec![node(0, &[], 1, 0), first, second];
        for parent in [1, 2] {
            let mut worktree = node(parent, &[parent], 1, 0);
            worktree.kind = NodeKind::Uncommitted;
            // On a branch: a detached one would take a row of its own.
            worktree.branch = Some(format!("b{parent}"));
            nodes.push(worktree);
        }
        let graph = graph(nodes);
        let row = |order: WorktreeOrder, i: usize| layout(&graph, order).nodes[i].2;

        assert!(row(WorktreeOrder::LastCommit, 1) < row(WorktreeOrder::LastCommit, 2));
        assert!(row(WorktreeOrder::FirstCommit, 2) < row(WorktreeOrder::FirstCommit, 1));
    }

    #[test]
    fn worktree_rows_by_fork_point_do_not_cross() {
        // One branch forks off early and runs long, with the newest commit;
        // another forks later. The later one goes nearer the main row, so
        // the early one's way down passes under it, crossing nothing.
        let main = node(0, &[], 3, 0);
        let mut early = node(1, &[0], 1, 1);
        early.last_time = 50;
        let mut nodes = vec![main.clone(), early, main, node(2, &[2], 1, 20)];
        nodes[2].parents = vec![0];
        nodes[2].lane = 0;
        for parent in [1, 3] {
            let mut worktree = node(parent, &[parent], 1, 0);
            worktree.kind = NodeKind::Uncommitted;
            // On a branch: a detached one would take a row of its own.
            worktree.branch = Some(format!("b{parent}"));
            nodes.push(worktree);
        }
        let graph = graph(nodes);
        let layout = layout(&graph, WorktreeOrder::ForkPoint);
        assert!(
            layout.nodes[1].0 < layout.nodes[3].0,
            "the early one starts left"
        );
        assert!(
            layout.nodes[3].2 < layout.nodes[1].2,
            "the later one is nearer"
        );
        let by_time = layout_rows(&graph, WorktreeOrder::LastCommit);
        assert!(by_time[1] < by_time[3], "by time the newest is nearer");
    }

    fn layout_rows(graph: &WorkflowGraph, order: WorktreeOrder) -> Vec<f32> {
        layout(graph, order)
            .nodes
            .iter()
            .map(|node| node.2)
            .collect()
    }

    #[test]
    fn stacked_branches_step_down_a_row_each() {
        // main, then a, b on a and c on b, all in one graph lane.
        let mut nodes = vec![node(0, &[], 2, 0), node(1, &[0], 1, 1), node(1, &[1], 3, 2)];
        nodes.push(node(1, &[2], 3, 3));
        for (k, name) in ["a", "b", "c"].into_iter().enumerate() {
            nodes[k + 1].refs = vec![name.to_string()];
        }
        let mut graph = graph(nodes);
        graph.branches = ["a", "b", "c"]
            .into_iter()
            .map(|name| wtf_core::Branch {
                name: name.to_string(),
                ..Default::default()
            })
            .collect();
        let rows = layout_rows(&graph, WorktreeOrder::ForkPoint);
        assert!(
            rows[1] < rows[2] && rows[2] < rows[3],
            "each a row lower: {rows:?}"
        );
        let (lanes, from) = stair_lanes(&graph);
        assert_eq!(lanes[1], 1);
        assert_eq!(from[lanes[2]], Some(1));
        assert_eq!(from[lanes[3]], Some(lanes[2]));
    }

    #[test]
    fn changed_worktree_on_a_merged_branch_gets_its_own_row() {
        // A side branch merged into main, with a worktree still on its tip.
        let mut worktree = node(2, &[1], 1, 0);
        worktree.kind = NodeKind::Uncommitted;
        worktree.added = 1;
        let graph = graph(vec![
            node(0, &[], 1, 0),
            node(1, &[0], 1, 1),
            node(0, &[0, 1], 1, 2),
            worktree,
        ]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);

        assert_eq!(layout.detached, [3]);
        assert_ne!(
            layout.nodes[3].2, layout.nodes[1].2,
            "off the merge rail's row"
        );
    }

    #[test]
    fn labels_push_the_next_node_on_their_branch() {
        let mut tagged = node(0, &[], 1, 0);
        tagged.refs = vec!["worktree-vliw-sequential-slots".to_string()];
        let graph = graph(vec![tagged, node(0, &[0], 1, 1), node(1, &[0], 1, 2)]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);

        let label_end =
            layout.nodes[0].1 + style().dot / 2. + TAG_GAP + labels_width(&graph, &graph.nodes[0]);
        assert!(
            layout.nodes[1].0 > label_end,
            "the next commit clears the label"
        );
        assert_eq!(
            layout.nodes[2].0,
            PAD_X + style().column,
            "a branch below is not pushed"
        );
    }

    /// The boxes of everything drawn but rails: nodes, squares, labels, list
    /// tags and leads, as (what, [left, top, right, bottom]).
    fn boxes(graph: &WorkflowGraph, layout: &Layout) -> Vec<(String, [f32; 4])> {
        let xs: Vec<(f32, f32)> = layout.nodes.iter().map(|n| (n.0, n.1)).collect();
        let mut boxes = Vec::new();
        let name = |node: &GraphNode| {
            node.refs
                .first()
                .cloned()
                .unwrap_or_else(|| crate::popups::short_hash(&node.key))
        };
        for (i, node) in graph.nodes.iter().enumerate() {
            let (start, end, y) = layout.nodes[i];
            if node.kind == NodeKind::Uncommitted {
                // Only worktrees in the list have a tag there.
                let Some(x) = layout.list_x[i] else {
                    continue;
                };
                // A framed worktree is drawn around its commit, not beside it.
                let s = symbol() / 2.;
                if !layout.framed.contains(&i) {
                    boxes.push((
                        format!("square {}", name(node)),
                        [start - s, y - s, end + s, y + s],
                    ));
                }
                let width = label_width(&worktree_label(node, WorktreeNames::Folder))
                    - LABEL_SPACING
                    + node
                        .branch
                        .as_deref()
                        .map_or(0., |name| extras_width(graph, name));
                let t = TAG_HEIGHT / 2.;
                boxes.push((
                    format!("list tag {}", name(node)),
                    [x, y - t, x + width, y + t],
                ));
                continue;
            }
            let d = style().dot / 2.;
            boxes.push((
                format!("node {}", name(node)),
                [start - d, y - d, end + d, y + d],
            ));
            let width = labels_width(graph, node);
            if width > 0. {
                let x = tags_start(i, &xs);
                let t = TAG_HEIGHT / 2.;
                boxes.push((
                    format!("labels {}", name(node)),
                    [x, y - t, x + width - LABEL_SPACING, y + t],
                ));
            }
        }
        // A lead on a commit's row runs along its rail under the later
        // commits; what shows of it starts past the row's last box.
        for (from, y, group) in &layout.leads {
            let what = format!("lead {}", name(&graph.nodes[group[0]]));
            let start = boxes
                .iter()
                .filter(|(_, b)| (b[1] + b[3]) / 2. == *y)
                .map(|(_, b)| b[2])
                .fold(*from, f32::max);
            boxes.push((what, [start, y - 1., layout.column - 6., y + 1.]));
        }
        boxes
    }

    fn overlaps(graph: &WorkflowGraph, layout: &Layout) -> Vec<String> {
        let boxes = boxes(graph, layout);
        let mut found = Vec::new();
        for (k, (a, [l1, t1, r1, b1])) in boxes.iter().enumerate() {
            for (b, [l2, t2, r2, b2]) in &boxes[k + 1..] {
                if l1 < r2 && l2 < r1 && t1 < b2 && t2 < b1 {
                    found.push(format!("{a} × {b}"));
                }
            }
        }
        found
    }

    #[test]
    fn worktrees_on_one_commit_get_a_row_and_a_lead_each() {
        let mut tip = node(0, &[], 1, 0);
        tip.refs = vec!["feature-with-a-long-name".to_string()];
        tip.tags = vec!["v1.0".to_string()];
        let mut nodes = vec![tip];
        for name in ["first", "second"] {
            let mut worktree = node(0, &[0], 1, 0);
            worktree.kind = NodeKind::Uncommitted;
            // Changed, so each is a square; clean ones are frames.
            worktree.added = 1;
            worktree.refs = vec![name.to_string()];
            worktree.branch = Some(name.to_string());
            worktree.tip = Revision::Worktree(format!("/repo/{name}"));
            nodes.push(worktree);
        }
        let graph = graph(nodes);
        let layout = layout(&graph, WorktreeOrder::LastCommit);

        assert_eq!(layout.leads.len(), 2, "a lead for each row");
        assert_ne!(layout.nodes[1].2, layout.nodes[2].2);
        assert_eq!(overlaps(&graph, &layout), Vec::<String>::new());
    }

    /// Checks real repositories: `WTF_PROBE=/path/a,/path/b cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn no_overlaps_in_real_repositories() {
        let repos = std::env::var("WTF_PROBE").unwrap_or_default();
        let mut failures = Vec::new();
        for repo in repos.split(',').filter(|repo| !repo.is_empty()) {
            for (remotes, tags) in [(false, false), (true, true)] {
                let attention = wtf_core::Attention {
                    remotes,
                    tags,
                    ..Default::default()
                };
                let graph = match wtf_core::workflow_graph(repo, &attention, true, 0) {
                    Ok(graph) => graph,
                    Err(error) => {
                        println!("{repo} remotes={remotes}: cannot build the graph: {error}");
                        failures.push(error.to_string());
                        continue;
                    }
                };
                let layout = layout(&graph, WorktreeOrder::LastCommit);
                let found = overlaps(&graph, &layout);
                println!(
                    "{repo} remotes={remotes} tags={tags}: {} overlaps",
                    found.len()
                );
                for overlap in found.iter().take(20) {
                    println!("  {overlap}");
                }
                failures.extend(found);
            }
        }
        assert!(failures.is_empty());
    }

    #[test]
    fn worktree_branches_go_with_them_when_nothing_else_grows_from_them() {
        let worktree = |parent: usize| GraphNode {
            kind: NodeKind::Uncommitted,
            branch: Some("wt".to_string()),
            ..node(1, &[parent], 0, 0)
        };
        // main: 0 → 1 → 3 (merge of 2); 2 is a merged branch, 4 an open one.
        let mut graph = graph(vec![
            node(0, &[], 1, 0),
            node(0, &[0], 1, 1),
            node(1, &[0], 1, 2),
            node(0, &[1, 2], 1, 3),
            node(2, &[1], 1, 4),
            worktree(2),
            worktree(4),
            worktree(3),
        ]);
        graph.main_node = Some(3);
        graph.on_main = graph.main_history();
        assert_eq!(leaf(&graph, 5), Some(true), "merged into main");
        assert_eq!(leaf(&graph, 6), Some(false), "unmerged, nothing after");
        assert_eq!(leaf(&graph, 7), None, "on the main branch's tip");
        graph.nodes.push(node(3, &[4], 1, 5));
        assert_eq!(leaf(&graph, 6), None, "another branch grows from it");
    }

    #[test]
    fn beads_never_overlap() {
        assert_eq!(bead_count(4, 3. * style().bead_spacing), 4);
        let capped = style().beads as f32 * style().bead_spacing;
        assert_eq!(bead_count(200, capped), style().beads + 1);
        assert_eq!(bead_count(50, 7.), 2, "both ends of a short span");
    }

    #[test]
    fn lanes_that_do_not_overlap_share_a_row() {
        // Main runs over five columns; one branch forks early, another late. The
        // late one comes first in lane order, as merged branches found newest
        // first do.
        let graph = graph(vec![
            node(0, &[], 1, 0),
            node(0, &[0], 1, 1),
            node(0, &[1], 1, 2),
            node(0, &[2], 1, 3),
            node(0, &[3], 1, 4),
            node(2, &[0], 1, 5),
            node(1, &[3], 1, 6),
        ]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);
        assert_eq!(layout.nodes[5].2, layout.nodes[6].2);
        assert_ne!(layout.nodes[5].2, layout.nodes[0].2);
    }

    #[test]
    fn a_branch_merged_into_a_commit_shares_a_row_with_one_forking_from_it() {
        // Main 0-2-4; one branch from 0 merged at 2, another from 2 merged
        // at 4.
        let graph = graph(vec![
            node(0, &[], 1, 0),
            node(1, &[0], 1, 1),
            node(0, &[0, 1], 1, 2),
            node(2, &[2], 1, 3),
            node(0, &[2, 3], 1, 4),
        ]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);
        assert_eq!(layout.nodes[1].2, layout.nodes[3].2);
        assert_ne!(layout.nodes[1].2, layout.nodes[0].2);
    }

    #[test]
    fn the_first_worktree_sits_after_its_commit_and_the_next_one_below() {
        let mut first = node(1, &[0], 1, 0);
        first.kind = NodeKind::Uncommitted;
        first.added = 1;
        let mut second = node(2, &[0], 1, 0);
        second.kind = NodeKind::Uncommitted;
        second.added = 1;
        let graph = graph(vec![node(0, &[], 3, 0), first, second]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);

        let (_, bar_end, y) = layout.nodes[0];
        assert_eq!(layout.nodes[1].2, y, "the first on the commit's row");
        assert!(layout.nodes[1].0 > bar_end);
        assert_eq!(layout.detached, [2]);
        assert!(layout.nodes[2].2 > y, "the next on a row of its own");
    }

    #[test]
    fn a_worktree_off_the_middle_of_a_lane_forks_up() {
        // A worktree on the first of two commits: work stands there, with
        // history after it.
        let mut worktree = node(1, &[0], 1, 0);
        worktree.kind = NodeKind::Uncommitted;
        worktree.added = 1;
        let graph = graph(vec![node(0, &[], 1, 0), node(0, &[0], 1, 1), worktree]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);
        assert_eq!(layout.detached, [2]);
        assert!(layout.nodes[2].2 < layout.nodes[0].2, "above the main row");
    }

    #[test]
    fn a_lane_inside_another_sits_nearer_the_main_row() {
        // Two branches off the same commit, merged back: the short one
        // first, the long one later. The short one goes nearer the main
        // row, so the long one's way back up crosses nothing.
        let nodes = vec![
            node(0, &[], 1, 0),
            node(1, &[0], 1, 1),    // the long branch
            node(2, &[0], 1, 2),    // the short branch
            node(0, &[0, 2], 1, 3), // the short one merged
            node(0, &[3, 1], 1, 4), // the long one merged
        ];
        let graph = graph(nodes);
        let layout = layout(&graph, WorktreeOrder::LastCommit);
        assert!(
            layout.nodes[2].2 < layout.nodes[1].2,
            "the short branch is nearer the main row"
        );
    }

    #[test]
    fn a_stash_on_a_branch_sits_one_row_above_it() {
        // main, and a branch off it whose tip has a stash: the stash takes a
        // row between the main row and the branch's, not one above main.
        let mut tip = node(1, &[0], 2, 1);
        tip.refs = vec!["rewrite".to_string()];
        tip.stashes = vec!["stash@{0}".to_string()];
        let graph = graph(vec![node(0, &[], 3, 0), tip]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);
        let [stash] = &layout.stashes[..] else {
            panic!("one stash")
        };
        let main_y = layout.nodes[0].2;
        let branch_y = layout.nodes[1].2;
        assert!(main_y < stash.y && stash.y < branch_y, "between them");
        assert_eq!(branch_y - stash.y, style().row_height, "right above");
    }

    #[test]
    fn a_stash_sits_under_a_worktree_lead_above_the_main_row() {
        // A clean worktree on the first commit, its lead rising above; a
        // stash on the last. The stash, ending at its label, goes nearer the
        // main row, so its way up does not cross the worktree's lead.
        let mut tip = node(0, &[0], 1, 1);
        tip.stashes = vec!["stash@{0}".to_string()];
        let mut clean = node(1, &[0], 1, 0);
        clean.kind = NodeKind::Uncommitted;
        let graph = graph(vec![node(0, &[], 1, 0), tip, clean]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);
        let [stash] = &layout.stashes[..] else {
            panic!("one stash")
        };
        let main_y = layout.nodes[0].2;
        let worktree_y = layout.nodes[2].2;
        assert!(worktree_y < main_y && stash.y < main_y, "both above");
        assert!(stash.y > worktree_y, "the stash nearer the main row");
    }

    #[test]
    fn a_tip_reads_commit_then_branch_then_its_work() {
        let mut tip = node(0, &[], 1, 0);
        tip.refs = vec!["master".to_string()];
        let mut worktree = node(1, &[0], 1, 0);
        worktree.kind = NodeKind::Uncommitted;
        worktree.added = 1;
        let graph = graph(vec![tip, worktree]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);
        let labels_end = tags_start(
            0,
            &layout.nodes.iter().map(|n| (n.0, n.1)).collect::<Vec<_>>(),
        ) + labels_width(&graph, &graph.nodes[0]);
        assert!(layout.nodes[1].0 - symbol() / 2. >= labels_end - LABEL_SPACING);
        assert_eq!(layout.nodes[1].2, layout.nodes[0].2, "on the commit's row");
    }

    #[test]
    fn clean_worktrees_at_a_tip_are_frames_as_drawn() {
        let clean = |lane: usize| {
            let mut worktree = node(lane, &[0], 1, 0);
            worktree.kind = NodeKind::Uncommitted;
            worktree
        };
        let graph = graph(vec![node(0, &[], 3, 0), clean(1), clean(2)]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);
        assert!(layout.detached.is_empty());
        assert_eq!(layout.framed, [1, 2]);
    }

    #[test]
    fn tags_along_one_line_share_a_row() {
        let tagged = |parents: &[usize], time: i64, tag: &str| GraphNode {
            tags: vec![tag.to_string()],
            ..node(0, parents, 1, time)
        };
        let graph = graph(vec![
            tagged(&[], 0, "v0.0.1"),
            tagged(&[0], 1, "v0.0.2"),
            node(0, &[1], 1, 2),
        ]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);
        let [first, second] = &layout.stashes[..] else {
            panic!("two tags")
        };
        assert!(first.tag && second.tag);
        assert!(first.y < layout.nodes[0].2, "above the line");
        assert_eq!(first.y, second.y, "one row");
        assert!(second.x >= first.x + label_width("v0.0.1"), "apart");
    }

    #[test]
    fn stashes_fork_up_onto_rows_of_their_own() {
        let mut stashed = node(0, &[], 1, 0);
        stashed.stashes = vec!["stash@{0}".to_string()];
        let graph = graph(vec![stashed, node(0, &[0], 1, 1)]);
        let layout = layout(&graph, WorktreeOrder::LastCommit);

        let [place] = &layout.stashes[..] else {
            panic!("one stash")
        };
        let main_y = layout.nodes[0].2;
        assert_eq!(place.node, 0);
        assert!(place.y < main_y, "above the main row");
        assert!(place.x > layout.nodes[0].1, "after its commit");
        assert_eq!(layout.nodes[1].2, main_y, "history stays in its row");
    }
}
