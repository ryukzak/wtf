use std::collections::HashMap;
use std::path::Path;

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
        Revision::Worktree(path) => Path::new(path)
            .file_name()
            .map_or(path.clone(), |name| name.to_string_lossy().into_owned()),
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

/// Which commit orders worktree rows, newest first.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum WorktreeOrder {
    LastCommit,
    FirstCommit,
}

/// What a right-click on a worktree acts on.
pub fn worktree_target(graph: &WorkflowGraph, index: usize) -> Target {
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
    }
}

/// Whether a worktree's branch can go with it, and whether the main branch
/// has merged it: only the main branch's history, if anything, follows its
/// HEAD; no other branch or worktree grows from it.
pub fn leaf(graph: &WorkflowGraph, worktree: usize) -> Option<bool> {
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
pub fn branch_target(graph: &WorkflowGraph, name: &str, commit: String) -> Target {
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
    // The main branch's history, by walking back from its tip.
    let mut main = std::collections::HashSet::new();
    let mut stack: Vec<usize> = graph.main_node.into_iter().collect();
    while let Some(node) = stack.pop() {
        if main.insert(node) {
            stack.extend(&graph.nodes[node].parents);
        }
    }
    let others_grow = graph
        .nodes
        .iter()
        .enumerate()
        .any(|(i, node)| i != except && node.parents.contains(&head) && !main.contains(&i));
    (!others_grow).then(|| main.contains(&head))
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

#[derive(Clone, Copy, PartialEq)]
pub enum Axis {
    Topological,
    CompressedTime,
    RealTime,
}

const PAD_X: f32 = 24.;
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
const MIN_TIME_WIDTH: f32 = 900.;
const TIME_WIDTH_PER_NODE: f32 = 36.;

pub struct Layout {
    /// Horizontal extent and vertical center of each node.
    pub nodes: Vec<(f32, f32, f32)>,
    /// Worktrees whose squares follow each node in its row, in order.
    pub worktrees: Vec<Vec<usize>>,
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
    pub width: f32,
    pub height: f32,
}

pub fn layout(graph: &WorkflowGraph, axis: Axis, order: WorktreeOrder) -> Layout {
    let mut xs = match axis {
        Axis::Topological => topological(graph),
        Axis::CompressedTime => timed(graph, style().idle_gap()),
        Axis::RealTime => timed(graph, i64::MAX),
    };

    // Worktrees sit as squares right after the node of their HEAD commit when
    // nothing follows it; otherwise later commits or a merge rail are in the
    // way, so they branch off to rows of their own.
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
    // A clean one there is the frame around its commit, and its lead runs on
    // the commit's own row, from the commit's labels straight to the list.
    let mut worktrees = vec![Vec::new(); graph.nodes.len()];
    let mut detached = Vec::new();
    let mut framed = Vec::new();
    for (i, node) in graph.nodes.iter().enumerate() {
        if node.kind != NodeKind::Uncommitted {
            continue;
        }
        let parent = node.parents[0];
        // At a tip, the first worktree follows the commit on its row; any
        // more get rows of their own, so no two sit in one row.
        if !followed[parent] && worktrees[parent].is_empty() {
            worktrees[parent].push(i);
        } else if followed[parent] && (node.added, node.removed) == (0, 0) {
            xs[i] = xs[parent];
            framed.push(i);
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
    for (parent, group) in worktrees.iter().enumerate() {
        let gap = if forks(parent) { FORK_GAP } else { SYMBOL_GAP };
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
    let lane_count = graph.lane_count + detached.len();
    let own_lane = |worktree: usize| {
        graph.lane_count + detached.iter().position(|&w| w == worktree).unwrap_or(0)
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
    for (i, node) in graph.nodes.iter().enumerate() {
        if node.kind == NodeKind::Uncommitted {
            continue;
        }
        let start = node
            .parents
            .iter()
            .map(|&p| xs[p].1)
            .fold(xs[i].0, f32::min);
        let leads_on =
            !worktrees[i].is_empty() || framed.iter().any(|&w| graph.nodes[w].parents[0] == i);
        let end = match leads_on {
            false => tags_start(i, &xs, &worktrees) + labels_width(graph, node),
            true => f32::INFINITY,
        };
        widen(&mut spans, node.lane, start, end);
        // A merge rail runs in the merged lane's row up to the merge.
        for &p in node.parents.iter().skip(1) {
            if let Some(span) = &mut spans[graph.nodes[p].lane] {
                span.1 = span.1.max(xs[i].0);
            }
        }
    }
    // Lanes forking from or merging into the main lane sit closest to it, merged
    // branches first, then branches with names; other lanes come after.
    let mut touches_main = vec![false; lane_count];
    let mut named = vec![false; lane_count];
    for (i, node) in graph.nodes.iter().enumerate() {
        if node.kind == NodeKind::Uncommitted {
            match detached.contains(&i) {
                true => {
                    named[own_lane(i)] = true;
                    if graph.nodes[node.parents[0]].lane == 0 {
                        touches_main[own_lane(i)] = true;
                    }
                }
                false => named[graph.nodes[node.parents[0]].lane] = true,
            }
            continue;
        }
        named[node.lane] |= !node.refs.is_empty();
        for &p in &node.parents {
            match (node.lane, graph.nodes[p].lane) {
                (0, 0) => {}
                (0, lane) => touches_main[lane] = true,
                (lane, 0) => touches_main[lane] = true,
                _ => {}
            }
        }
    }
    let mut lanes: Vec<(usize, (f32, f32))> = (1..lane_count)
        .filter_map(|lane| Some((lane, spans[lane]?)))
        .collect();
    // Worktree rows come right after merged branches, newest first by the chosen
    // commit; other lanes follow by where they start.
    let mut worktree_lane = vec![false; lane_count];
    let mut times: Vec<Option<(i64, i64)>> = vec![None; lane_count];
    for node in graph
        .nodes
        .iter()
        .filter(|n| n.kind != NodeKind::Uncommitted)
    {
        let time = times[node.lane].get_or_insert((node.first_time, node.last_time));
        *time = (time.0.min(node.first_time), time.1.max(node.last_time));
    }
    for (i, node) in graph.nodes.iter().enumerate() {
        if node.kind != NodeKind::Uncommitted {
            continue;
        }
        let parent = &graph.nodes[node.parents[0]];
        let lane = match detached.contains(&i) {
            true => own_lane(i),
            false => parent.lane,
        };
        worktree_lane[lane] = true;
        times[lane].get_or_insert((parent.last_time, parent.last_time));
    }
    let time = |lane: usize| {
        let (first, last) = times[lane].unwrap_or_default();
        match order {
            WorktreeOrder::LastCommit => last,
            WorktreeOrder::FirstCommit => first,
        }
    };
    let group = |lane: usize| match (worktree_lane[lane], touches_main[lane], named[lane]) {
        (false, true, false) => 0,
        (true, _, _) => 1,
        (false, true, true) => 2,
        (false, false, _) => 3,
    };
    lanes.sort_by(|&(a, a_span), &(b, b_span)| {
        group(a).cmp(&group(b)).then_with(|| match group(a) {
            1 => time(b).cmp(&time(a)),
            _ => a_span.0.total_cmp(&b_span.0),
        })
    });

    // Each lane takes the first row with a free stretch for it.
    let mut row_of = vec![0; lane_count];
    let mut rows: Vec<Vec<(f32, f32)>> = vec![Vec::new()];
    for (lane, (start, end)) in lanes {
        let free = |row: &Vec<(f32, f32)>| {
            row.iter()
                .all(|&(s, e)| end + ROW_GAP < s || e + ROW_GAP < start)
        };
        let row = match (1..rows.len()).find(|&row| free(&rows[row])) {
            Some(row) => row,
            None => {
                rows.push(Vec::new());
                rows.len() - 1
            }
        };
        rows[row].push((start, end));
        row_of[lane] = row;
    }

    let row_y =
        |lane: usize| PAD_Y + row_of[lane] as f32 * style().row_height + style().row_height / 2.;
    let nodes: Vec<(f32, f32, f32)> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let lane = match node.kind {
                NodeKind::Uncommitted if detached.contains(&i) => own_lane(i),
                NodeKind::Uncommitted => graph.nodes[node.parents[0]].lane,
                _ => node.lane,
            };
            (xs[i].0, xs[i].1, row_y(lane))
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
        let from = xs[parent].1 + style().dot / 2.;
        leads.push((from, nodes[parent].2, vec![worktree]));
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
        .map(|parent| {
            tags_start(parent, &xs, &worktrees) + labels_width(graph, &graph.nodes[parent])
        });
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
                + node
                    .branch
                    .as_deref()
                    .map_or(0., |name| extras_width(graph, name));
        }
        list = f32::max(list, x);
    }
    Layout {
        nodes,
        worktrees,
        detached,
        framed,
        column,
        leads,
        list_x,
        width: column + list + PAD_X,
        height: PAD_Y + rows.len() as f32 * style().row_height,
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

/// Where the tags of a node start: after its worktree symbols, if any.
fn tags_start(node: usize, xs: &[(f32, f32)], worktrees: &[Vec<usize>]) -> f32 {
    match worktrees[node].last() {
        Some(&last) => xs[last].1 + symbol() / 2. + TAG_GAP,
        None => xs[node].1 + style().dot / 2. + TAG_GAP,
    }
}

/// How wide a label is, roughly, with the room after it.
fn label_width(name: &str) -> f32 {
    name.chars().count() as f32 * LABEL_CHAR + LABEL_EXTRA + LABEL_SPACING
}

/// How wide a node's labels are, roughly, so nothing is drawn over them.
fn labels_width(graph: &WorkflowGraph, node: &GraphNode) -> f32 {
    let notes: f32 = node.refs.iter().map(|name| extras_width(graph, name)).sum();
    let names = node.refs.iter().chain(&node.tags).chain(&node.stashes);
    names.map(|name| label_width(name)).sum::<f32>() + notes
}

/// Why a branch looks left behind, as its labels say it: `merged · idle 30d`.
/// Idle counts from its last commit, against the days set in Settings.
pub fn stale_note(graph: &WorkflowGraph, name: &str) -> Option<String> {
    let branch = graph.branches.iter().find(|branch| branch.name == name)?;
    let mut reasons = branch.stale.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_secs() as i64);
    let days = (now - branch.time) / 86_400;
    let live = branch.busy || branch.remote || branch.name == graph.main;
    if !live && branch.time > 0 && days >= i64::from(crate::preferences::idle_days()) {
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

/// How much a branch's stale note and sync badge widen its label.
fn extras_width(graph: &WorkflowGraph, name: &str) -> f32 {
    [stale_note(graph, name), sync_badge(graph, name)]
        .iter()
        .flatten()
        .map(|text| note_width(text))
        .sum()
}

/// A branch's label: filled when a worktree has it checked out, and when it
/// looks left behind, faded, in italics, with why after its name. How far
/// it is from its upstream follows the name.
fn branch_tag(
    name: &str,
    note: Option<String>,
    badge: Option<String>,
    color: Hsla,
    filled: bool,
    theme: &Theme,
) -> gpui::Div {
    let badge = badge.map(|badge| div().ml(px(NOTE_GAP)).opacity(0.8).child(badge));
    let Some(note) = note else {
        return tag(name, color, filled, theme).children(badge);
    };
    tag(name, color.opacity(0.45), false, theme)
        .italic()
        .text_color(theme.secondary)
        .children(badge)
        .child(div().ml(px(NOTE_GAP)).child(note))
}

/// Each node one column after its parents, as long as its commits' beads, and
/// past a parent's labels when it continues that parent's branch.
fn topological(graph: &WorkflowGraph) -> Vec<(f32, f32)> {
    let mut xs: Vec<(f32, f32)> = Vec::with_capacity(graph.nodes.len());
    for node in &graph.nodes {
        let start = node
            .parents
            .iter()
            .map(|&p| {
                let parent = &graph.nodes[p];
                let labels = match parent.lane == node.lane {
                    true => {
                        style().dot / 2. + TAG_GAP + labels_width(graph, parent) + style().curve
                    }
                    false => 0.,
                };
                xs[p].1 + style().column.max(labels)
            })
            .fold(left_edge(graph), f32::max);
        let length =
            node.commits.len().saturating_sub(1).min(style().beads) as f32 * style().bead_spacing;
        xs.push((start, start + length));
    }
    xs
}

/// Positions by commit time, with idle gaps longer than `cap` shortened to it.
fn timed(graph: &WorkflowGraph, cap: i64) -> Vec<(f32, f32)> {
    let mut points: Vec<i64> = graph
        .nodes
        .iter()
        .flat_map(|node| [node.first_time, node.last_time])
        .collect();
    points.sort_unstable();
    points.dedup();

    let mut offsets = HashMap::new();
    let mut offset = 0.;
    for (i, &time) in points.iter().enumerate() {
        if i > 0 {
            offset += (time - points[i - 1]).min(cap) as f64;
        }
        offsets.insert(time, offset);
    }
    let width = (graph.nodes.len() as f32 * TIME_WIDTH_PER_NODE).max(MIN_TIME_WIDTH);
    let left = left_edge(graph);
    let x = |time: i64| left + (offsets[&time] / offset.max(1.)) as f32 * width;

    graph
        .nodes
        .iter()
        .map(|node| (x(node.first_time), x(node.last_time)))
        .collect()
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
}

/// Empty room around the graph, to drag its edges a little inward.
pub const MARGIN: f32 = 40.;

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
    } = marks;
    let color = |node: &GraphNode| match node.kind {
        NodeKind::History => theme.secondary,
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
            lane: graph.nodes[oldest].lane,
            color: theme.secondary,
        });
    }
    // Dashed lines lead from the worktrees to their names in the list.
    for (from, y, group) in &layout.leads {
        let first = group[0];
        let picked = group.iter().any(|w| selected.contains(w));
        edges.push(Edge {
            from: point(px(*from), px(*y)),
            to: point(px(layout.column - 6.), px(*y)),
            merge: false,
            dashed: true,
            // Heavier for a selected worktree, or one hovered with its
            // commit, its branch label or its tag.
            strong: group.iter().any(|&w| {
                selected.contains(&w)
                    || hovered == Some(w)
                    || hovered.is_some() && hovered == graph.nodes[w].parents.first().copied()
            }),
            lane: row_lane(graph, layout, first),
            // A selected worktree's lead takes the selection's color.
            color: match picked {
                true => theme.accent,
                false => color(&graph.nodes[first]),
            },
        });
    }
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
        let is_selected = selected.contains(&i);
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
            content = content.child(
                div()
                    .absolute()
                    .left(px(start - style().dot / 2. - pad))
                    .top(px(y - style().dot / 2. - pad))
                    .w(px(end - start + style().dot + 2. * pad))
                    .h(px(style().dot + 2. * pad))
                    .rounded(px(4.))
                    .map(|frame| match is_selected {
                        true => frame
                            .border_2()
                            .border_color(theme.accent)
                            .bg(theme.accent.opacity(0.2)),
                        false => frame.border_1().border_color(color),
                    }),
            );
            // The frame's edge picks the worktree and opens its menu; inside
            // it, the commit keeps its own clicks.
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
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_context_menu(target.clone(), event.position, cx);
                            }),
                        ),
                );
            }
            continue;
        }
        // Commits are dots, or with bars a run of beads. Selected nodes get a ring.
        let (size, hit) = match (worktree, is_selected) {
            (true, false) => (symbol(), symbol_step()),
            (true, true) => (symbol() + 4., symbol_step()),
            (false, false) => (style().dot, HIT),
            (false, true) => (style().dot + 4., HIT),
        };
        let shape = if worktree {
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(style().dot + 2. * FRAME_PAD))
                .rounded(px(4.))
                .map(|frame| match is_selected {
                    true => frame.border_2().border_color(theme.accent),
                    false => frame.border_1().border_color(color),
                })
                .child(
                    div()
                        .size(px(style().dot - 2.))
                        .rounded(px(style().dot))
                        .bg(color),
                )
        } else if end > start {
            let selection = is_selected.then_some(picked);
            beads(i, &node.commits, end - start, color, selection, theme, cx)
        } else {
            div()
                .w(px(end - start + size))
                .h(px(size))
                .rounded(px(if worktree { 2. } else { size / 2. }))
                .map(|shape| match (is_selected, worktree && !dirty) {
                    (true, hollow) => shape
                        .bg(if hollow {
                            theme.graph_background
                        } else {
                            color
                        })
                        .border_2()
                        .border_color(theme.accent),
                    (false, true) => shape
                        .bg(theme.graph_background)
                        .border_2()
                        .border_color(color),
                    (false, false) => shape.bg(color),
                })
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
        // A selected commit or bar is underlined in the accent: a border on
        // the dots would vanish on lanes of a like colour.
        if is_selected && !worktree {
            content = content.child(
                div()
                    .absolute()
                    .left(px(start - style().dot / 2.))
                    .top(px(y + style().dot / 2. + 3.))
                    .w(px(end - start + style().dot))
                    .h(px(2.5))
                    .rounded(px(1.5))
                    .bg(theme.accent),
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
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        this.click_node(i, event.modifiers().platform, cx)
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.hover_node(i, *hovered, cx)
                    }))
                    // A commit node's menu acts on its newest commit.
                    .when_some(node.commits.last().filter(|_| !worktree), |dot, commit| {
                        let target = Target::Commit {
                            commit: commit.id.clone(),
                            message: match commit.body.is_empty() {
                                true => commit.summary.clone(),
                                false => format!("{}\n\n{}", commit.summary, commit.body),
                            },
                        };
                        dot.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_context_menu(target.clone(), event.position, cx);
                            }),
                        )
                    })
                    .when(worktree, |square| {
                        let target = worktree_target(graph, i);
                        square.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_context_menu(target.clone(), event.position, cx);
                            }),
                        )
                    })
                    .child(shape),
            );
        if worktree {
            // A worktree on a row of its own carries its branch's label there.
            if let Some(name) = own_row_branch(graph, &layout.detached, i) {
                let target = label_target(graph, name, String::new());
                let (note, badge) = (stale_note(graph, name), sync_badge(graph, name));
                let tag = branch_tag(name, note, badge, color, true, theme);
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

        if node.refs.is_empty() && node.tags.is_empty() && node.stashes.is_empty() {
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
        let mut branches: Vec<AnyElement> = node
            .refs
            .iter()
            .filter(|name| !elsewhere.contains(&name.as_str()))
            .map(|name| {
                let commit = match &node.tip {
                    Revision::Commit(id) => id.clone(),
                    Revision::Worktree(_) => String::new(),
                };
                let target = label_target(graph, name, commit);
                // A branch checked out in a worktree is filled, as its tag in
                // the worktree list is.
                let checked_out = matches!(target, Target::Worktree { .. });
                let (note, badge) = (stale_note(graph, name), sync_badge(graph, name));
                let tag = branch_tag(name, note, badge, color, checked_out, theme);
                menu_label(tag, name, target, i, true, cx)
            })
            .collect();
        let commit = node
            .commits
            .last()
            .map(|commit| commit.id.clone())
            .unwrap_or_default();
        branches.extend(node.tags.iter().enumerate().map(|(k, name)| {
            let target = Target::Tag {
                name: name.clone(),
                commit: commit.clone(),
            };
            let label = div()
                .flex_none()
                .h(px(TAG_HEIGHT))
                .flex()
                .items_center()
                .px(px(5.))
                .rounded(px(3.))
                .bg(theme.git_tag_background)
                .text_color(theme.git_tag_text)
                .child(name.clone());
            with_menu(label, ("git-tag", i * 100 + k), target, cx)
        }));
        // Stashes made on the node: a dashed-looking chip, opened by a click.
        branches.extend(node.stashes.iter().enumerate().filter_map(|(k, name)| {
            let stash = graph.stashes.iter().find(|stash| stash.name == *name)?;
            let target = Target::Stash {
                name: name.clone(),
                id: stash.id.clone(),
                message: stash.message.clone(),
            };
            let show = name.clone();
            // The stash in the review area stands out in the accent color.
            let shown = shown_stash == Some(name.as_str());
            let label = div()
                .flex_none()
                .h(px(TAG_HEIGHT))
                .flex()
                .items_center()
                .px(px(5.))
                .rounded(px(3.))
                .border_1()
                .border_color(if shown { theme.accent } else { theme.secondary })
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
                .child(name.clone());
            Some(
                div()
                    .id(("stash-label", i * 100 + k))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.show_stash(&show, cx)))
                    .child(with_menu(label, ("stash", i * 100 + k), target, cx))
                    .into_any_element(),
            )
        }));
        content = content.child(
            div()
                .absolute()
                .left(px(tags_start(i, &xs, &layout.worktrees)))
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
        let tags: Vec<AnyElement> = worktree
            .refs
            .first()
            .map(|name| {
                let branch = worktree.branch.as_deref();
                let note = branch.and_then(|b| stale_note(graph, b));
                let badge = branch.and_then(|b| sync_badge(graph, b));
                let tag = branch_tag(&label, note, badge, color, true, theme);
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
                    // A selected worktree stands out from hover: a solid
                    // accent ring around its tags.
                    .when(selected.contains(&w), |tags| {
                        tags.rounded(px(5.))
                            .p(px(2.))
                            .m(px(-2.))
                            .bg(theme.accent.opacity(0.3))
                            .border_2()
                            .border_color(theme.accent)
                            .shadow_md()
                    })
                    .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                        this.click_node(w, event.modifiers().platform, cx)
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.hover_node(w, *hovered, cx)
                    }))
                    .children(tags),
            );
    }

    // Scrolled away from the worktree list, a hovered worktree, or a commit
    // with worktrees, shows its tags at the view's right edge, on their rows.
    // The view's edges, where the graph's own coordinates start past the margin.
    let view_right = f32::from(scroll.bounds().size.width - scroll.offset().x) - MARGIN;
    if let Some(hovered) = hovered.filter(|_| view_right > 0. && view_right < layout.column) {
        for (w, worktree) in graph.nodes.iter().enumerate() {
            let on_it = w == hovered || worktree.parents.first() == Some(&hovered);
            if worktree.kind != NodeKind::Uncommitted || !on_it || layout.list_x[w].is_none() {
                continue;
            }
            let (_, _, y) = layout.nodes[w];
            // The arrow points on to the worktree list, off to the right.
            let name = format!("{} →", worktree_label(worktree, WorktreeNames::Folder));
            let width = label_width(&name);
            // Drawn last, above rails and labels, so it always reads.
            content = content.child(
                gpui::deferred(
                    tag(&name, color(worktree), true, theme)
                        .absolute()
                        .left(px(view_right - width - 8.))
                        .top(px(y - TAG_HEIGHT / 2.))
                        .whitespace_nowrap()
                        .text_xs()
                        .shadow_md(),
                )
                .with_priority(1),
            );
        }
    }
    // The other way: a hovered worktree whose commit is scrolled off to the
    // left shows its branch, or its commit, at the view's left edge, on the
    // commit's row.
    let view_left = f32::from(-scroll.offset().x) - MARGIN;
    if let Some(worktree) = hovered
        .map(|w| &graph.nodes[w])
        .filter(|node| node.kind == NodeKind::Uncommitted && view_right > 0.)
    {
        let parent = worktree.parents[0];
        let (_, end, y) = layout.nodes[parent];
        if end < view_left {
            let commit = &graph.nodes[parent];
            let (name, filled) = match &worktree.branch {
                Some(branch) => (branch.clone(), true),
                None => (
                    commit
                        .commits
                        .last()
                        .map_or(String::new(), |commit| commit.short_id.clone()),
                    false,
                ),
            };
            content = content.child(
                gpui::deferred(
                    tag(&format!("← {name}"), color(worktree), filled, theme)
                        .absolute()
                        .left(px(view_left + 8.))
                        .top(px(y - TAG_HEIGHT / 2.))
                        .whitespace_nowrap()
                        .text_xs()
                        .shadow_md(),
                )
                .with_priority(1),
            );
        }
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
                let mut path = match edge.dashed {
                    true => {
                        let width = if edge.strong { 2. } else { 1. };
                        PathBuilder::stroke(px(width)).dash_array(&[px(4.), px(3.)])
                    }
                    false => PathBuilder::stroke(px(width)),
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
                    let corner = point(from.x + bend, to.y);
                    s_curve(&mut path, from, corner);
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
/// Clicking a bead selects its commit; when commits outnumber the beads,
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
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    if event.modifiers().platform {
                        return;
                    }
                    cx.stop_propagation();
                    this.click_bead(node, commit.clone(), cx)
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

/// A label that opens its right-click menu.
fn with_menu(
    label: gpui::Div,
    id: (&'static str, usize),
    target: Target,
    cx: &mut Context<RepoView>,
) -> AnyElement {
    label
        .id(id)
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.open_context_menu(target.clone(), event.position, cx);
            }),
        )
        .into_any_element()
}

/// A label with its menu on a right-click, or on any click when `click_menu`
/// as a label has nothing else to do on one. Hovering it hovers node `hover`,
/// as hovering the node itself does.
fn menu_label(
    tag: gpui::Div,
    name: &str,
    target: Target,
    hover: usize,
    click_menu: bool,
    cx: &mut Context<RepoView>,
) -> AnyElement {
    let menu = target.clone();
    div()
        .id(SharedString::from(format!("label-{name}")))
        .flex()
        .items_center()
        .on_hover(
            cx.listener(move |this, hovered: &bool, _, cx| this.hover_node(hover, *hovered, cx)),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.open_context_menu(target.clone(), event.position, cx);
            }),
        )
        .when(click_menu, |label| {
            label.cursor_pointer().on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_context_menu(menu.clone(), event.position, cx);
                }),
            )
        })
        .child(tag)
        .into_any_element()
}

fn tag(name: &str, color: Hsla, filled: bool, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .h(px(TAG_HEIGHT))
        .flex()
        .items_center()
        .pl(px(6.))
        .pr(px(5.))
        .rounded(px(3.))
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
        }
    }

    fn graph(nodes: Vec<GraphNode>) -> WorkflowGraph {
        let lane_count = nodes.iter().map(|node| node.lane + 1).max().unwrap_or(0);
        WorkflowGraph {
            nodes,
            lane_count,
            main_node: Some(0),
            branches: Vec::new(),
            main: String::new(),
            older: 0,
            tags: Vec::new(),
            stashes: Vec::new(),
            remotes: Vec::new(),
        }
    }

    #[test]
    fn topology_places_children_a_column_after_parents() {
        let graph = graph(vec![
            node(0, &[], 1, 0),
            node(0, &[0], 1, 1),
            node(1, &[0], 1, 2),
        ]);
        let layout = layout(&graph, Axis::Topological, WorktreeOrder::LastCommit);

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
        let layout = layout(&graph, Axis::Topological, WorktreeOrder::LastCommit);
        let length = |i: usize| layout.nodes[i].1 - layout.nodes[i].0;
        assert_eq!(length(0), 3. * style().bead_spacing);
        assert_eq!(length(1), style().beads as f32 * style().bead_spacing);
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
        // A clean one is the frame around its commit, its lead on the
        // commit's row straight to its list tag.
        let framed = layout(
            &graph_of(clean),
            Axis::Topological,
            WorktreeOrder::LastCommit,
        );
        assert!(framed.detached.is_empty() && framed.framed == [2]);
        assert!(framed.list_x[2].is_some());
        assert_eq!(framed.nodes[2].2, framed.nodes[0].2, "on its commit's row");
        assert_eq!(framed.leads.len(), 1);
        assert!(framed.leads[0].0 > framed.nodes[0].1);

        let graph = graph_of(worktree);
        let layout = layout(&graph, Axis::Topological, WorktreeOrder::LastCommit);

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
            nodes.push(worktree);
        }
        let graph = graph(nodes);
        let row =
            |order: WorktreeOrder, i: usize| layout(&graph, Axis::Topological, order).nodes[i].2;

        assert!(row(WorktreeOrder::LastCommit, 1) < row(WorktreeOrder::LastCommit, 2));
        assert!(row(WorktreeOrder::FirstCommit, 2) < row(WorktreeOrder::FirstCommit, 1));
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
        let layout = layout(&graph, Axis::Topological, WorktreeOrder::LastCommit);

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
        let layout = layout(&graph, Axis::Topological, WorktreeOrder::LastCommit);

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
                .unwrap_or_else(|| node.key.chars().take(7).collect())
        };
        for (i, node) in graph.nodes.iter().enumerate() {
            let (start, end, y) = layout.nodes[i];
            if node.kind == NodeKind::Uncommitted {
                // A framed worktree is drawn around its commit's own box.
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
                let x = tags_start(i, &xs, &layout.worktrees);
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
            worktree.refs = vec![name.to_string()];
            worktree.branch = Some(name.to_string());
            worktree.tip = Revision::Worktree(format!("/repo/{name}"));
            nodes.push(worktree);
        }
        let graph = graph(nodes);
        let layout = layout(&graph, Axis::Topological, WorktreeOrder::LastCommit);

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
                let layout = layout(&graph, Axis::Topological, WorktreeOrder::LastCommit);
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
        let layout = layout(&graph, Axis::Topological, WorktreeOrder::LastCommit);
        assert_eq!(layout.nodes[5].2, layout.nodes[6].2);
        assert_ne!(layout.nodes[5].2, layout.nodes[0].2);
    }

    #[test]
    fn compressed_time_shortens_idle_gaps() {
        let graph = graph(vec![
            node(0, &[], 1, 0),
            node(0, &[0], 1, 100),
            node(0, &[1], 1, 100_000),
        ]);
        let gaps = |axis| {
            let x: Vec<f32> = layout(&graph, axis, WorktreeOrder::LastCommit)
                .nodes
                .iter()
                .map(|n| n.0)
                .collect();
            (x[1] - x[0]) / (x[2] - x[1])
        };
        assert!(gaps(Axis::RealTime) < 0.01);
        assert!((gaps(Axis::CompressedTime) - 100. / style().idle_gap() as f32).abs() < 0.001);
    }

    #[test]
    fn the_first_worktree_sits_after_its_commit_and_the_next_one_below() {
        let mut first = node(1, &[0], 1, 0);
        first.kind = NodeKind::Uncommitted;
        let mut second = node(2, &[0], 1, 0);
        second.kind = NodeKind::Uncommitted;
        let graph = graph(vec![node(0, &[], 3, 0), first, second]);
        let layout = layout(&graph, Axis::Topological, WorktreeOrder::LastCommit);

        let (_, bar_end, y) = layout.nodes[0];
        assert_eq!(layout.worktrees[0], vec![1]);
        assert_eq!(layout.nodes[1].2, y, "the first on the commit's row");
        assert!(layout.nodes[1].0 > bar_end);
        assert_eq!(layout.detached, [2]);
        assert!(layout.nodes[2].2 > y, "the next on a row of its own");
    }
}
