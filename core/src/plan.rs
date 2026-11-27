//! A plan of changes to history: commits picked onto a branch, dropped,
//! squashed, reordered, reworded or edited. It is worked out in git's object
//! store before anything changes: each step's outcome, conflicts resolved
//! by choosing a side or typing the lines, files edited; then the branch
//! moves to the result in one step, with a way back.
//!
//! Steps after one still in conflict are worked out on a guess, the
//! commit's side winning, and say so; resolving a step works them out again
//! on what was chosen.

use crate::{CoreError, git};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The branch a plan rewrites.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Onto {
    /// The branch, or detached HEAD, checked out in the worktree at a path.
    Worktree(String),
    /// A local branch no worktree has.
    Branch(String),
}

/// Where the steps are rebuilt from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Base {
    /// The branch's tip, wherever it is when the plan is worked out: commits
    /// go on top, as a cherry-pick's do.
    Tip,
    /// A commit in or under its history: its commits after this one are the
    /// plan's first steps, and the branch must still be at `tip`.
    Commit { base: String, tip: String },
}

/// What a step does with its commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Pick,
    /// Left out.
    Drop,
    /// Into the step before, the messages joined.
    Squash,
    /// Into the step before, its message dropped.
    Fixup,
    /// With the message in `Step::message`.
    Reword,
    /// With the files in `Step::edits`.
    Edit,
}

impl Action {
    pub const ALL: [Action; 6] = [
        Action::Pick,
        Action::Squash,
        Action::Fixup,
        Action::Reword,
        Action::Edit,
        Action::Drop,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Action::Pick => "pick",
            Action::Drop => "drop",
            Action::Squash => "squash",
            Action::Fixup => "fixup",
            Action::Reword => "reword",
            Action::Edit => "edit",
        }
    }
}

/// Changes to history, oldest step first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub onto: Onto,
    pub base: Base,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub commit: String,
    pub action: Action,
    /// The message a reword gives it, or a squash gives the joined commit.
    pub message: Option<String>,
    /// By file: the choices made in its conflicts.
    pub resolutions: HashMap<String, Resolution>,
    /// By file: its content typed in this step; none to delete it.
    pub edits: HashMap<String, Option<String>>,
}

impl Step {
    pub fn new(commit: &str) -> Self {
        Step {
            commit: commit.to_string(),
            action: Action::Pick,
            message: None,
            resolutions: HashMap::new(),
            edits: HashMap::new(),
        }
    }
}

/// The choices for one file's conflicts, kept for the versions they were
/// made on: when the file's sides change, they no longer hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub sides: Sides,
    /// One per conflicting part, in order; none while not chosen yet.
    pub choices: Vec<Option<Choice>>,
}

/// Which lines a conflicting part takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Choice {
    /// Those already on the branch.
    Ours,
    /// The commit's.
    Theirs,
    /// Both: ours, then the commit's.
    OursThenTheirs,
    /// Both: the commit's, then ours.
    TheirsThenOurs,
    /// Lines as typed.
    Text(String),
}

/// A file's three versions in a conflict, by blob id; none where the file
/// is not there.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Sides {
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
}

/// A file in conflict in a step.
#[derive(Debug, Clone, PartialEq)]
pub struct FileConflict {
    pub path: String,
    pub mode: String,
    pub sides: Sides,
    /// Choices hold for every conflicting part.
    pub resolved: bool,
}

impl FileConflict {
    /// Both sides have the file, as text: its conflicts are resolved part by
    /// part. Otherwise, a file one side lacks or a submodule, it takes one
    /// side whole: `Choice::Ours` or `Choice::Theirs`.
    pub fn by_lines(&self) -> bool {
        self.sides.ours.is_some() && self.sides.theirs.is_some() && self.mode != "160000"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Applies as it is.
    Clean,
    /// Changes nothing there, as when applied before: left out.
    Empty,
    /// Dropped by the plan.
    Dropped,
    /// Conflicts wait for choices.
    Conflict,
    /// Conflicts all have their choices.
    Resolved,
}

/// What a step comes to.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub state: State,
    /// The commit it makes, not yet on any branch; none for one left out or
    /// still in conflict. A squash's is the joined commit.
    pub commit: Option<String>,
    pub files: Vec<FileConflict>,
    /// Worked out on a guess for an earlier step still in conflict.
    pub guessed: bool,
    /// For a step still in conflict, the commit it is guessed as, the
    /// commit's side winning: what the steps after go on from, and what a
    /// view of the history to come shows in its place.
    pub guess: Option<String>,
}

impl Outcome {
    /// The commit that stands for the step in the history to come: the one
    /// it makes, or its guess.
    pub fn shown(&self) -> Option<&str> {
        self.commit.as_deref().or(self.guess.as_deref())
    }
}

/// What working out a plan found.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkedOut {
    /// What the steps were rebuilt from.
    pub base: String,
    /// The branch's tip then: what Apply replaces.
    pub tip_before: String,
    pub outcomes: Vec<Outcome>,
}

impl WorkedOut {
    /// Every step can be applied.
    pub fn ready(&self) -> bool {
        self.outcomes.iter().all(|o| {
            matches!(
                o.state,
                State::Clean | State::Empty | State::Dropped | State::Resolved
            )
        })
    }

    /// The tip once applied.
    pub fn tip(&self) -> &str {
        self.outcomes
            .iter()
            .rev()
            .find_map(|o| o.commit.as_deref())
            .unwrap_or(&self.base)
    }

    /// The tip of the history to come, guesses standing for steps still in
    /// conflict.
    pub fn shown_tip(&self) -> &str {
        self.outcomes
            .iter()
            .rev()
            .find_map(Outcome::shown)
            .unwrap_or(&self.base)
    }

    /// The tip of the steps before the first still in conflict: what is
    /// known for sure so far.
    pub fn settled(&self) -> &str {
        self.outcomes
            .iter()
            .take_while(|o| o.state != State::Conflict)
            .filter_map(|o| o.commit.as_deref())
            .last()
            .unwrap_or(&self.base)
    }
}

/// The branch's tip now.
pub fn tip_of(repo: &str, onto: &Onto) -> Result<String, CoreError> {
    let (dir, rev) = match onto {
        Onto::Worktree(path) => (path.as_str(), "HEAD".to_string()),
        Onto::Branch(branch) => (repo, format!("refs/heads/{branch}")),
    };
    Ok(git::run(
        dir,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )?
    .trim()
    .to_string())
}

/// The commits of `tip`'s history after `base`, oldest first, by first
/// parents: what a plan rewriting from `base` takes as its steps. Merges
/// among them cannot be rewritten yet.
pub fn commits_after(repo: &str, base: &str, tip: &str) -> Result<Vec<String>, CoreError> {
    let range = format!("{base}..{tip}");
    let merges = git::run(repo, &["rev-list", "--merges", "--first-parent", &range])?;
    if !merges.trim().is_empty() {
        return Err(CoreError::new(
            "Merges come after this commit: rewriting across a merge isn't supported yet",
        ));
    }
    Ok(
        git::run(repo, &["rev-list", "--reverse", "--first-parent", &range])?
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

/// A plan rewriting `onto`'s history after `base`, each commit picked as
/// it is: steps for drop, squash, reorder and the rest to change.
pub fn rewrite(repo: &str, onto: Onto, base: &str) -> Result<Plan, CoreError> {
    let tip = tip_of(repo, &onto)?;
    let steps = commits_after(repo, base, &tip)?
        .iter()
        .map(|c| Step::new(c))
        .collect();
    Ok(Plan {
        onto,
        base: Base::Commit {
            base: base.to_string(),
            tip,
        },
        steps,
    })
}

/// A plan moving `onto`'s own commits, those since it forked from `new_base`,
/// to start from `new_base`: a rebase.
pub fn rebase(repo: &str, onto: Onto, new_base: &str) -> Result<Plan, CoreError> {
    let tip = tip_of(repo, &onto)?;
    let fork = git::run(repo, &["merge-base", new_base, &tip])?
        .trim()
        .to_string();
    let steps = commits_after(repo, &fork, &tip)?
        .iter()
        .map(|c| Step::new(c))
        .collect();
    let base = git::run(repo, &["rev-parse", &format!("{new_base}^{{commit}}")])?
        .trim()
        .to_string();
    Ok(Plan {
        onto,
        base: Base::Commit { base, tip },
        steps,
    })
}

impl Plan {
    /// Commits to put on top of the branch, as a cherry-pick does.
    pub fn pick(onto: Onto, commits: &[String]) -> Plan {
        Plan {
            onto,
            base: Base::Tip,
            steps: commits.iter().map(|c| Step::new(c)).collect(),
        }
    }

    /// The step of `commit`, if the plan has it.
    pub fn step_of(&self, commit: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.commit == commit)
    }

    /// Makes `commit`, in the branch's history, one of the plan's steps:
    /// the plan then rewrites from its parent, the commits since picked as
    /// they are. Returns its step.
    pub fn reach(&mut self, repo: &str, commit: &str) -> Result<usize, CoreError> {
        if let Some(at) = self.step_of(commit) {
            return Ok(at);
        }
        let (base, tip) = match &self.base {
            Base::Tip => {
                let tip = tip_of(repo, &self.onto)?;
                (tip.clone(), tip)
            }
            Base::Commit { base, tip } => (base.clone(), tip.clone()),
        };
        let parent = git::run(repo, &["rev-parse", &format!("{commit}^")])?
            .trim()
            .to_string();
        let earlier = commits_after(repo, &parent, &base)?;
        if earlier.first().map(String::as_str) != Some(commit) {
            return Err(CoreError::new("The commit is not in the branch's history"));
        }
        let mut steps: Vec<Step> = earlier.iter().map(|c| Step::new(c)).collect();
        steps.append(&mut self.steps);
        self.steps = steps;
        self.base = Base::Commit { base: parent, tip };
        Ok(0)
    }
}

/// Works the plan out, step after step.
pub fn work_out(repo: &str, plan: &Plan) -> Result<WorkedOut, CoreError> {
    let tip_before = tip_of(repo, &plan.onto)?;
    let base = match &plan.base {
        Base::Tip => tip_before.clone(),
        Base::Commit { base, tip } => {
            if *tip != tip_before {
                return Err(CoreError::new(
                    "The branch moved since this plan was made: cancel it and start again",
                ));
            }
            base.clone()
        }
    };
    let mut tip = base.clone();
    // The commit the step before made, and its own parents, for a squash.
    let mut last: Option<(String, Vec<String>)> = None;
    let mut guessed = false;
    let mut outcomes = Vec::new();
    for step in &plan.steps {
        let commit = &step.commit;
        if step.action == Action::Drop {
            outcomes.push(Outcome {
                state: State::Dropped,
                commit: None,
                files: Vec::new(),
                guessed,
                guess: None,
            });
            continue;
        }
        let (clean, merged, mut files) = merge(repo, &tip, commit, None)?;
        let mut tree = merged;
        let mut state = State::Clean;
        if !clean {
            // Each file's choices, where they hold, give its content.
            let mut blobs = Vec::new();
            for file in &mut files {
                let held = step
                    .resolutions
                    .get(&file.path)
                    .filter(|r| r.sides == file.sides);
                // One side whole: its version, or no file where it has none.
                if let Some(resolution) = held.filter(|_| !file.by_lines()) {
                    let side = match resolution.choices.first() {
                        Some(Some(Choice::Ours)) => Some(&file.sides.ours),
                        Some(Some(Choice::Theirs)) => Some(&file.sides.theirs),
                        _ => None,
                    };
                    if let Some(side) = side {
                        let entry = side.clone().map(|id| (file.mode.clone(), id));
                        blobs.push((file.path.clone(), entry));
                        file.resolved = true;
                    }
                    continue;
                }
                if let Some(resolution) = held {
                    let regions = regions(repo, &file.sides)?;
                    if let Some(content) = resolve(&regions, &resolution.choices) {
                        let blob = write_blob(repo, &content)?;
                        blobs.push((file.path.clone(), Some((file.mode.clone(), blob))));
                        file.resolved = true;
                    }
                }
            }
            if !files.iter().all(|file| file.resolved) {
                // Later steps go on from a guess: the commit's side wins.
                let (_, guess, _) = merge(repo, &tip, commit, Some("theirs"))?;
                tip = git::copy_commit_onto(repo, commit, &guess, &[&tip])?;
                last = None;
                outcomes.push(Outcome {
                    state: State::Conflict,
                    commit: None,
                    files,
                    guessed,
                    guess: Some(tip.clone()),
                });
                guessed = true;
                continue;
            }
            tree = replace_files(repo, &tree, &blobs)?;
            state = State::Resolved;
        }
        if !step.edits.is_empty() {
            let mut blobs = Vec::new();
            for (path, content) in &step.edits {
                let entry = match content {
                    Some(content) => {
                        let mode = mode_in(repo, &tree, path)?.unwrap_or("100644".to_string());
                        Some((mode, write_blob(repo, content)?))
                    }
                    None => None,
                };
                blobs.push((path.clone(), entry));
            }
            tree = replace_files(repo, &tree, &blobs)?;
        }
        let joined = matches!(step.action, Action::Squash | Action::Fixup);
        let (made, parents) = match (&last, joined) {
            // Into the step before: its author and parents, the tree as now.
            (Some((before, parents)), true) => {
                let message = match (step.action, &step.message) {
                    (Action::Fixup, _) => message_of(repo, before)?,
                    (_, Some(message)) => message.clone(),
                    _ => format!(
                        "{}\n\n{}",
                        message_of(repo, before)?,
                        message_of(repo, commit)?
                    ),
                };
                let ids: Vec<&str> = parents.iter().map(String::as_str).collect();
                let made = git::copy_commit_as(repo, before, &tree, &ids, &message)?;
                (made, parents.clone())
            }
            _ => {
                // Nothing left to change: no commit. Resolved so, it stays
                // resolved, the choice made.
                if tree == tree_of(repo, &tip)? && step.edits.is_empty() {
                    outcomes.push(Outcome {
                        state: match state {
                            State::Resolved => State::Resolved,
                            _ => State::Empty,
                        },
                        commit: None,
                        files,
                        guessed,
                        guess: None,
                    });
                    continue;
                }
                let message = match &step.message {
                    Some(message) => message.clone(),
                    None => message_of(repo, commit)?,
                };
                let made = git::copy_commit_as(repo, commit, &tree, &[&tip], &message)?;
                (made, vec![tip.clone()])
            }
        };
        tip = made.clone();
        last = Some((made.clone(), parents));
        outcomes.push(Outcome {
            state,
            commit: Some(made),
            files,
            guessed,
            guess: None,
        });
    }
    Ok(WorkedOut {
        base,
        tip_before,
        outcomes,
    })
}

/// The local branches whose history holds `commit`, progress branches left
/// out.
pub fn branches_with(repo: &str, commit: &str) -> Result<Vec<String>, CoreError> {
    Ok(git::run(
        repo,
        &[
            "for-each-ref",
            "--contains",
            commit,
            "--format=%(refname:short)",
            "refs/heads",
        ],
    )?
    .lines()
    .filter(|name| *name != REWRITE_BRANCH)
    .map(str::to_string)
    .collect())
}

/// Each commit's first line, by id.
pub fn subjects(repo: &str, commits: &[String]) -> Result<HashMap<String, String>, CoreError> {
    if commits.is_empty() {
        return Ok(HashMap::new());
    }
    let mut args = vec!["log", "--no-walk=unsorted", "--format=%H%x00%s"];
    args.extend(commits.iter().map(String::as_str));
    Ok(git::run(repo, &args)?
        .lines()
        .filter_map(|line| line.split_once('\0'))
        .map(|(id, subject)| (id.to_string(), subject.to_string()))
        .collect())
}

/// A revision's commit id.
pub fn rev(repo: &str, rev: &str) -> Result<String, CoreError> {
    Ok(git::run(
        repo,
        &["rev-parse", "--verify", "-q", &format!("{rev}^{{commit}}")],
    )?
    .trim()
    .to_string())
}

/// A commit's full message.
pub fn message_of(repo: &str, commit: &str) -> Result<String, CoreError> {
    Ok(git::run(repo, &["show", "-s", "--format=%B", commit])?
        .trim_end()
        .to_string())
}

/// A file's mode in a tree, if it is there.
fn mode_in(repo: &str, tree: &str, path: &str) -> Result<Option<String>, CoreError> {
    let out = git::run(repo, &["ls-tree", tree, "--", path])?;
    Ok(out.split_whitespace().next().map(str::to_string))
}

/// Merges `commit`'s changes from its first parent onto `tip`, as
/// cherry-pick does: whether it went cleanly, the tree, and the files in
/// conflict with their versions.
fn merge(
    repo: &str,
    tip: &str,
    commit: &str,
    favour: Option<&str>,
) -> Result<(bool, String, Vec<FileConflict>), CoreError> {
    let base = format!("--merge-base={commit}^1");
    let mut args = vec!["merge-tree", "--write-tree", "-z", "--no-messages"];
    let option;
    if let Some(favour) = favour {
        option = format!("-X{favour}");
        args.push(&option);
    }
    args.extend([base.as_str(), tip, commit]);
    let (clean, out) = git::run_answer(repo, &args, None, &[])?;
    let mut parts = out.split('\0');
    let tree = parts.next().unwrap_or_default().trim().to_string();
    let mut files: Vec<FileConflict> = Vec::new();
    // `<mode> <object> <stage>\t<path>`, one per side in conflict.
    for entry in parts.take_while(|part| !part.is_empty()) {
        let Some((info, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut words = info.split(' ');
        let (Some(mode), Some(id), Some(stage)) = (words.next(), words.next(), words.next()) else {
            continue;
        };
        if files.last().is_none_or(|file| file.path != path) {
            files.push(FileConflict {
                path: path.to_string(),
                mode: mode.to_string(),
                sides: Sides::default(),
                resolved: false,
            });
        }
        let file = files.last_mut().expect("just pushed");
        let side = match stage {
            "1" => &mut file.sides.base,
            "2" => &mut file.sides.ours,
            _ => &mut file.sides.theirs,
        };
        *side = Some(id.to_string());
        if stage != "1" {
            file.mode = mode.to_string();
        }
    }
    Ok((clean, tree, files))
}

fn tree_of(repo: &str, commit: &str) -> Result<String, CoreError> {
    Ok(
        git::run(repo, &["rev-parse", &format!("{commit}^{{tree}}")])?
            .trim()
            .to_string(),
    )
}

/// A part of a file in conflict: lines all sides agree on, or a conflict.
/// Lines keep their line ends.
#[derive(Debug, Clone, PartialEq)]
pub enum Region {
    Same(Vec<String>),
    Conflict {
        ours: Vec<String>,
        base: Vec<String>,
        theirs: Vec<String>,
    },
}

/// Long markers, which no file's own lines are likely to start with.
const MARKER: usize = 23;

/// A file's parts in conflict, from its three versions.
pub fn regions(repo: &str, sides: &Sides) -> Result<Vec<Region>, CoreError> {
    let empty = write_blob(repo, "")?;
    let id = |side: &Option<String>| side.clone().unwrap_or_else(|| empty.clone());
    let (ours, base, theirs) = (id(&sides.ours), id(&sides.base), id(&sides.theirs));
    let marker = format!("--marker-size={MARKER}");
    let (_, merged) = git::run_answer(
        repo,
        &[
            "merge-file",
            "-p",
            "--diff3",
            "--object-id",
            &marker,
            &ours,
            &base,
            &theirs,
        ],
        None,
        &[],
    )?;
    Ok(parse_regions(&merged))
}

fn parse_regions(merged: &str) -> Vec<Region> {
    let open = "<".repeat(MARKER);
    let middle = "|".repeat(MARKER);
    let split = "=".repeat(MARKER);
    let close = ">".repeat(MARKER);
    let mut regions = Vec::new();
    let mut same = Vec::new();
    // Inside a conflict: which side's lines are coming.
    let mut side: Option<usize> = None;
    let mut parts: [Vec<String>; 3] = Default::default();
    for line in merged.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        let is = |marker: &str| bare == marker || bare.starts_with(&format!("{marker} "));
        match side {
            None if is(&open) => {
                if !same.is_empty() {
                    regions.push(Region::Same(std::mem::take(&mut same)));
                }
                side = Some(0);
            }
            None => same.push(line.to_string()),
            Some(_) if is(&middle) => side = Some(1),
            Some(_) if bare == split => side = Some(2),
            Some(_) if is(&close) => {
                let [ours, base, theirs] = std::mem::take(&mut parts);
                regions.push(Region::Conflict { ours, base, theirs });
                side = None;
            }
            Some(k) => parts[k].push(line.to_string()),
        }
    }
    if !same.is_empty() {
        regions.push(Region::Same(same));
    }
    regions
}

/// How many conflicting parts there are.
pub fn conflicts(regions: &[Region]) -> usize {
    regions
        .iter()
        .filter(|r| matches!(r, Region::Conflict { .. }))
        .count()
}

/// The lines a conflicting part takes with `choice`.
pub fn chosen(ours: &[String], theirs: &[String], choice: &Choice) -> String {
    match choice {
        Choice::Ours => ours.concat(),
        Choice::Theirs => theirs.concat(),
        Choice::OursThenTheirs => ours.concat() + &theirs.concat(),
        Choice::TheirsThenOurs => theirs.concat() + &ours.concat(),
        // Typed lines end their last line, as the lines after expect.
        Choice::Text(text) if !text.is_empty() && !text.ends_with('\n') => format!("{text}\n"),
        Choice::Text(text) => text.clone(),
    }
}

/// Lines around a conflict that go with it: shown with it, and typed over
/// with it. A run of the same lines between two conflicts is shared, half
/// each, so no line goes with both.
pub const CONTEXT: usize = 3;

/// For each conflict, how many lines before it and after it go with it.
pub fn windows(regions: &[Region]) -> Vec<(usize, usize)> {
    let is_conflict = |k: usize| matches!(regions.get(k), Some(Region::Conflict { .. }));
    let mut windows = Vec::new();
    for (k, region) in regions.iter().enumerate() {
        if !matches!(region, Region::Conflict { .. }) {
            continue;
        }
        let same = |k: Option<usize>| match k.and_then(|k| regions.get(k)) {
            Some(Region::Same(lines)) => lines.len(),
            _ => 0,
        };
        let before = same(k.checked_sub(1));
        let after = same(Some(k + 1));
        // Shared with the conflict on the other side: the earlier one takes
        // the first half, rounded down.
        let before = match k.checked_sub(2).is_some_and(is_conflict) {
            true => before - before / 2,
            false => before,
        };
        let after = match is_conflict(k + 2) {
            true => after / 2,
            false => after,
        };
        windows.push((before.min(CONTEXT), after.min(CONTEXT)));
    }
    windows
}

/// How each run of the same lines splits: the lines going with the
/// conflict before it, those between, those going with the conflict after.
fn same_split(regions: &[Region], windows: &[(usize, usize)], k: usize) -> (usize, usize) {
    let conflicts_before = regions[..k]
        .iter()
        .filter(|r| matches!(r, Region::Conflict { .. }))
        .count();
    let after_prev = match k.checked_sub(1).map(|k| &regions[k]) {
        Some(Region::Conflict { .. }) => windows[conflicts_before - 1].1,
        _ => 0,
    };
    let before_next = match regions.get(k + 1) {
        Some(Region::Conflict { .. }) => windows[conflicts_before].0,
        _ => 0,
    };
    (after_prev, before_next)
}

/// The lines of conflict `part`'s block, with `choice`: the lines going with
/// it around what it takes; typed lines are the whole block.
pub fn block(regions: &[Region], part: usize, choice: &Choice) -> String {
    let windows = windows(regions);
    let (before, after) = windows[part];
    let mut seen = 0;
    for (k, region) in regions.iter().enumerate() {
        let Region::Conflict { ours, theirs, .. } = region else {
            continue;
        };
        if seen < part {
            seen += 1;
            continue;
        }
        if let Choice::Text(text) = choice {
            return text.clone();
        }
        let tail = |k: Option<usize>, n: usize, end: bool| match k.and_then(|k| regions.get(k)) {
            Some(Region::Same(lines)) if end => lines[lines.len() - n..].concat(),
            Some(Region::Same(lines)) => lines[..n].concat(),
            _ => String::new(),
        };
        return tail(k.checked_sub(1), before, true)
            + &chosen(ours, theirs, choice)
            + &tail(Some(k + 1), after, false);
    }
    String::new()
}

/// The file's content with `choices`, once there is one for every conflict.
/// Typed lines stand for their conflict's whole block.
pub fn resolve(regions: &[Region], choices: &[Option<Choice>]) -> Option<String> {
    let windows = windows(regions);
    let mut content = String::new();
    let mut part: usize = 0;
    for (k, region) in regions.iter().enumerate() {
        match region {
            Region::Same(lines) => {
                let (after_prev, before_next) = same_split(regions, &windows, k);
                // Lines a typed block before or after took.
                let typed = |part: Option<usize>| {
                    part.and_then(|p| choices.get(p))
                        .is_some_and(|c| matches!(c, Some(Choice::Text(_))))
                };
                let front = match typed(part.checked_sub(1)) {
                    true => after_prev,
                    false => 0,
                };
                let back = match typed(Some(part)) && before_next > 0 {
                    true => before_next,
                    false => 0,
                };
                content.extend(lines[front..lines.len() - back].iter().map(String::as_str));
            }
            Region::Conflict { ours, theirs, .. } => {
                let choice = choices.get(part)?.as_ref()?;
                content.push_str(&chosen(ours, theirs, choice));
                part += 1;
            }
        }
    }
    Some(content)
}

/// The line, from 0, where conflict `part`'s block starts in the file as
/// resolved with `choices`, those not made yet taken as ours.
pub fn block_start(regions: &[Region], choices: &[Option<Choice>], part: usize) -> usize {
    let windows = windows(regions);
    let mut lines = 0;
    let mut seen: usize = 0;
    for (k, region) in regions.iter().enumerate() {
        match region {
            Region::Same(same) => {
                let typed_before = seen
                    .checked_sub(1)
                    .and_then(|p| choices.get(p))
                    .is_some_and(|c| matches!(c, Some(Choice::Text(_))));
                let typed_next = choices
                    .get(seen)
                    .is_some_and(|c| matches!(c, Some(Choice::Text(_))));
                let (after_prev, before_next) = same_split(regions, &windows, k);
                let front = if typed_before { after_prev } else { 0 };
                if seen == part {
                    return lines + same.len() - front - before_next;
                }
                // Lines a typed block after takes are counted with it.
                let back = if typed_next { before_next } else { 0 };
                lines += same.len() - front - back;
            }
            Region::Conflict { ours, theirs, .. } => {
                if seen == part {
                    return lines;
                }
                let choice = choices.get(seen).cloned().flatten().unwrap_or(Choice::Ours);
                lines += chosen(ours, theirs, &choice).lines().count();
                // A typed block took the lines after it already.
                seen += 1;
            }
        }
    }
    lines
}

/// A file's content in a commit, if it is there and text.
pub fn file_at(repo: &str, commit: &str, path: &str) -> Result<Option<String>, CoreError> {
    let object = format!("{commit}:{path}");
    match git::run_answer(repo, &["cat-file", "-e", &object], None, &[])? {
        (false, _) => Ok(None),
        (true, _) => {
            let bytes = git::show(repo, &object)?;
            Ok(String::from_utf8(bytes).ok())
        }
    }
}

/// The files a commit changes from its first parent.
pub fn files_of(repo: &str, commit: &str) -> Result<Vec<String>, CoreError> {
    Ok(git::run(
        repo,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-m",
            "--first-parent",
            commit,
        ],
    )?
    .lines()
    .map(str::to_string)
    .collect())
}

fn write_blob(repo: &str, content: &str) -> Result<String, CoreError> {
    Ok(
        git::run_with_input(repo, &["hash-object", "-w", "--stdin"], content)?
            .trim()
            .to_string(),
    )
}

/// `tree` with files replaced by blobs of a mode, or deleted where none,
/// through an index of its own.
fn replace_files(
    repo: &str,
    tree: &str,
    blobs: &[(String, Option<(String, String)>)],
) -> Result<String, CoreError> {
    let index = std::env::temp_dir().join(format!(
        "wtf-plan-index-{}-{}",
        std::process::id(),
        INDEX.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let index_path = index.to_string_lossy().to_string();
    let env = [("GIT_INDEX_FILE", index_path.as_str())];
    let result = (|| {
        git::run_with_env(repo, &["read-tree", tree], None, &env)?;
        for (path, entry) in blobs {
            match entry {
                Some((mode, blob)) => {
                    let info = format!("{mode},{blob},{path}");
                    git::run_with_env(
                        repo,
                        &["update-index", "--add", "--cacheinfo", &info],
                        None,
                        &env,
                    )?
                }
                None => git::run_with_env(
                    repo,
                    &["update-index", "--force-remove", "--", path],
                    None,
                    &env,
                )?,
            };
        }
        git::run_with_env(repo, &["write-tree"], None, &env)
    })();
    std::fs::remove_file(&index).ok();
    Ok(result?.trim().to_string())
}

static INDEX: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How to take an applied plan back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Undo {
    pub onto: Onto,
    pub before: String,
    pub after: String,
}

/// Applies a plan worked out as `worked`: the branch moves to the new tip
/// in one step. A worktree keeps its uncommitted changes, and git refuses
/// when they would be overwritten.
pub fn apply(repo: &str, onto: &Onto, worked: &WorkedOut) -> Result<Undo, CoreError> {
    if !worked.ready() {
        return Err(CoreError::new("Some steps still have conflicts"));
    }
    let now = tip_of(repo, onto)?;
    if now != worked.tip_before {
        return Err(CoreError::new(
            "The branch moved since the plan was worked out; look at the plan again",
        ));
    }
    let tip = worked.tip().to_string();
    move_to(repo, onto, &now, &tip, "wtf: apply plan")?;
    Ok(Undo {
        onto: onto.clone(),
        before: now,
        after: tip,
    })
}

/// Takes an applied plan back, while nothing else has moved the branch.
pub fn undo(repo: &str, undo: &Undo) -> Result<(), CoreError> {
    if tip_of(repo, &undo.onto)? != undo.after {
        return Err(CoreError::new("The branch moved since; nothing was undone"));
    }
    move_to(
        repo,
        &undo.onto,
        &undo.after,
        &undo.before,
        "wtf: undo plan",
    )
}

/// Moves the branch from `from` to `to`: forward in a worktree as a
/// fast-forward does, otherwise as `reset --keep` does; a branch alone by
/// its ref, only if it is still at `from`.
fn move_to(repo: &str, onto: &Onto, from: &str, to: &str, why: &str) -> Result<(), CoreError> {
    if from == to {
        return Ok(());
    }
    match onto {
        Onto::Branch(branch) => {
            let reference = format!("refs/heads/{branch}");
            git::run(repo, &["update-ref", "-m", why, &reference, to, from])?;
        }
        Onto::Worktree(path) => {
            let (forward, _) =
                git::run_answer(path, &["merge-base", "--is-ancestor", from, to], None, &[])?;
            match forward {
                true => git::run(path, &["merge", "--ff-only", "--quiet", to])?,
                false => git::run(path, &["reset", "--keep", "--quiet", to])?,
            };
        }
    }
    Ok(())
}

/// The one branch that shows a rewrite's progress in the graph: the steps
/// settled so far. In capitals, apart from any branch of the user's, and
/// short whatever branch is rewritten.
pub const REWRITE_BRANCH: &str = "WTF-REWRITE";

/// Points the progress branch at `commit`, or deletes it with none.
pub fn mark_wip(repo: &str, name: &str, commit: Option<&str>) -> Result<(), CoreError> {
    let reference = format!("refs/heads/{name}");
    let (there, current) = git::run_answer(
        repo,
        &["rev-parse", "--verify", "-q", &reference],
        None,
        &[],
    )?;
    match commit {
        Some(commit) if there && current.trim() == commit => {}
        Some(commit) => {
            git::run(repo, &["update-ref", "-m", "wtf: plan", &reference, commit])?;
        }
        None if there => {
            git::run(repo, &["update-ref", "-d", &reference])?;
        }
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Repo(std::path::PathBuf);

    impl Repo {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("wtf-plan-{name}-{}", std::process::id()));
            fs::remove_dir_all(&dir).ok();
            fs::create_dir_all(&dir).unwrap();
            let repo = Repo(dir);
            repo.git(&["init", "--quiet", "-b", "main"]);
            repo.git(&["config", "user.name", "T"]);
            repo.git(&["config", "user.email", "t@example.com"]);
            repo
        }
        fn path(&self) -> &str {
            self.0.to_str().unwrap()
        }
        fn git(&self, args: &[&str]) -> String {
            git::run(self.path(), args).unwrap().trim().to_string()
        }
        fn commit(&self, file: &str, text: &str) -> String {
            fs::write(self.0.join(file), text).unwrap();
            self.git(&["add", file]);
            self.git(&[
                "commit",
                "--quiet",
                "-m",
                &format!("{file} {}", text.lines().count()),
            ]);
            self.git(&["rev-parse", "HEAD"])
        }
        fn read(&self, file: &str) -> String {
            fs::read_to_string(self.0.join(file)).unwrap_or_default()
        }
        fn subjects(&self) -> Vec<String> {
            self.git(&["log", "--format=%s"])
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    /// main has a.txt changed in its first and last lines; a side branch
    /// makes three commits: the first and third clash with main.
    fn clashing(name: &str) -> (Repo, Vec<String>) {
        let repo = Repo::new(name);
        repo.commit("a.txt", "one\ntwo\nthree\nfour\nfive\n");
        repo.git(&["checkout", "--quiet", "-b", "side"]);
        let first = repo.commit("a.txt", "ONE\ntwo\nthree\nfour\nfive\n");
        let second = repo.commit("b.txt", "b\n");
        let third = repo.commit("a.txt", "ONE\ntwo\nthree\nfour\nFIVE\n");
        repo.git(&["checkout", "--quiet", "main"]);
        repo.commit("a.txt", "uno\ntwo\nthree\nfour\ncinco\n");
        (repo, vec![first, second, third])
    }

    fn states(worked: &WorkedOut) -> Vec<(State, bool)> {
        worked
            .outcomes
            .iter()
            .map(|o| (o.state, o.guessed))
            .collect()
    }

    fn choose(plan: &mut Plan, step: usize, file: &FileConflict, choice: Choice) {
        plan.steps[step].resolutions.insert(
            file.path.clone(),
            Resolution {
                sides: file.sides.clone(),
                choices: vec![Some(choice)],
            },
        );
    }

    #[test]
    fn steps_after_a_conflict_are_guessed_until_it_is_resolved() {
        let (repo, picks) = clashing("guess");
        let onto = Onto::Worktree(repo.path().to_string());
        let mut plan = Plan::pick(onto.clone(), &picks);
        let worked = work_out(repo.path(), &plan).unwrap();
        assert_eq!(
            states(&worked),
            [
                (State::Conflict, false),
                (State::Clean, true),
                (State::Conflict, true)
            ]
        );
        assert!(!worked.ready());
        assert_eq!(worked.settled(), worked.base);

        let file = worked.outcomes[0].files[0].clone();
        assert_eq!(file.path, "a.txt");
        assert_eq!(conflicts(&regions(repo.path(), &file.sides).unwrap()), 1);
        choose(&mut plan, 0, &file, Choice::Ours);
        let worked = work_out(repo.path(), &plan).unwrap();
        assert_eq!(
            states(&worked),
            [
                (State::Resolved, false),
                (State::Clean, false),
                (State::Conflict, false)
            ]
        );

        // The third gets its last line typed.
        let file = worked.outcomes[2].files[0].clone();
        // The block: three lines before the conflict, then it.
        let parts = regions(repo.path(), &file.sides).unwrap();
        assert_eq!(block(&parts, 0, &Choice::Ours), "two\nthree\nfour\ncinco\n");
        choose(
            &mut plan,
            2,
            &file,
            Choice::Text("two\nthree\nfour\nFINAL".into()),
        );
        let worked = work_out(repo.path(), &plan).unwrap();
        assert!(worked.ready());

        // Nothing changed so far; applying moves main, undoing moves it back.
        let before = repo.git(&["rev-parse", "HEAD"]);
        let undo_it = apply(repo.path(), &onto, &worked).unwrap();
        assert_eq!(repo.read("a.txt"), "uno\ntwo\nthree\nfour\nFINAL\n");
        // The first step, all main's, leaves nothing to commit.
        assert_eq!(repo.subjects().len(), 4);
        undo(repo.path(), &undo_it).unwrap();
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    }

    #[test]
    fn a_file_the_branch_lacks_takes_a_side_whole() {
        let repo = Repo::new("lacks");
        repo.commit("a.txt", "a\n");
        repo.git(&["checkout", "--quiet", "-b", "side"]);
        repo.commit("cart.js", "one\n");
        let change = repo.commit("cart.js", "two\n");
        repo.git(&["checkout", "--quiet", "main"]);
        let onto = Onto::Worktree(repo.path().to_string());
        let mut plan = Plan::pick(onto.clone(), std::slice::from_ref(&change));
        let worked = work_out(repo.path(), &plan).unwrap();
        let file = worked.outcomes[0].files[0].clone();
        assert!(!file.by_lines());
        assert_eq!(file.sides.ours, None);
        // Left out, as main has it: nothing to commit.
        choose(&mut plan, 0, &file, Choice::Ours);
        let worked = work_out(repo.path(), &plan).unwrap();
        assert!(worked.ready());
        assert_eq!(worked.tip(), worked.base);
        // Added as the commit has it.
        choose(&mut plan, 0, &file, Choice::Theirs);
        let worked = work_out(repo.path(), &plan).unwrap();
        apply(repo.path(), &onto, &worked).unwrap();
        assert_eq!(repo.read("cart.js"), "two\n");
    }

    #[test]
    fn blocks_share_the_lines_between_two_conflicts() {
        let m = |c: char| c.to_string().repeat(MARKER);
        let merged = format!(
            "a\nb\n{o}\no1\n{b}\nb1\n{s}\nt1\n{c}\nc\nd\ne\nf\ng\n{o}\no2\n{b}\n{s}\nt2\n{c}\nh\n",
            o = m('<'),
            b = m('|'),
            s = m('='),
            c = m('>')
        );
        let regions = parse_regions(&merged);
        // Five lines between: two go with the first, three with the second.
        assert_eq!(windows(&regions), [(2, 2), (3, 1)]);
        let ours = [Some(Choice::Ours), Some(Choice::Ours)];
        assert_eq!(block_start(&regions, &ours, 0), 0);
        assert_eq!(block_start(&regions, &ours, 1), 5);
        assert_eq!(block(&regions, 1, &Choice::Theirs), "e\nf\ng\nt2\nh\n");
        // The first block typed over, lines around it too.
        let typed = [
            Some(Choice::Text("A\nB\nX\nC\nD".into())),
            Some(Choice::Ours),
        ];
        assert_eq!(
            resolve(&regions, &typed).unwrap(),
            "A\nB\nX\nC\nD\ne\nf\ng\no2\nh\n"
        );
        assert_eq!(block_start(&regions, &typed, 1), 5);
    }

    #[test]
    fn choices_give_the_content() {
        let merged = "a\n<<<<<<<<<<<<<<<<<<<<<<< ours\nb\n||||||||||||||||||||||| base\nB\n=======================\nc\n>>>>>>>>>>>>>>>>>>>>>>> theirs\nd\n";
        let regions = parse_regions(merged);
        assert_eq!(regions.len(), 3);
        assert_eq!(resolve(&regions, &[None]), None);
        assert_eq!(
            resolve(&regions, &[Some(Choice::Ours)]).unwrap(),
            "a\nb\nd\n"
        );
        assert_eq!(
            resolve(&regions, &[Some(Choice::TheirsThenOurs)]).unwrap(),
            "a\nc\nb\nd\n"
        );
        assert_eq!(
            resolve(&regions, &[Some(Choice::Text("A\nx\nD".into()))]).unwrap(),
            "A\nx\nD\n"
        );
    }

    #[test]
    fn a_plan_onto_a_branch_leaves_the_worktree_alone() {
        let (repo, picks) = clashing("branch");
        repo.git(&["branch", "release", "main"]);
        let onto = Onto::Branch("release".into());
        let plan = Plan::pick(onto.clone(), &picks[1..2]);
        let worked = work_out(repo.path(), &plan).unwrap();
        assert!(worked.ready());
        let head = repo.git(&["rev-parse", "HEAD"]);
        apply(repo.path(), &onto, &worked).unwrap();
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
        assert_eq!(
            repo.git(&["log", "-1", "--format=%s", "release"]),
            "b.txt 1"
        );
    }

    /// Five commits on main, each its own file.
    fn line_of_five(name: &str) -> (Repo, Vec<String>) {
        let repo = Repo::new(name);
        let commits = (1..=5)
            .map(|k| repo.commit(&format!("f{k}.txt"), &format!("{k}\n")))
            .collect();
        (repo, commits)
    }

    #[test]
    fn drops_squashes_rewords_and_reorders_in_one_go() {
        let (repo, commits) = line_of_five("rewrite");
        let onto = Onto::Worktree(repo.path().to_string());
        let mut plan = rewrite(repo.path(), onto.clone(), &commits[0]).unwrap();
        assert_eq!(plan.steps.len(), 4);
        // f2 dropped, f4 squashed into f3, f5 reworded and moved first.
        plan.steps[0].action = Action::Drop;
        plan.steps[2].action = Action::Squash;
        plan.steps[3].action = Action::Reword;
        plan.steps[3].message = Some("Five first".into());
        let last = plan.steps.remove(3);
        plan.steps.insert(0, last);
        let worked = work_out(repo.path(), &plan).unwrap();
        assert!(worked.ready());
        apply(repo.path(), &onto, &worked).unwrap();
        assert_eq!(repo.subjects(), ["f3.txt 1", "Five first", "f1.txt 1"]);
        assert_eq!(repo.git(&["log", "-1", "--format=%b"]), "f4.txt 1");
        assert!(!repo.0.join("f2.txt").exists());
        assert_eq!(repo.read("f4.txt"), "4\n");
    }

    #[test]
    fn an_edit_changes_the_files_of_its_step() {
        let (repo, commits) = line_of_five("edit");
        repo.git(&["checkout", "--quiet", "--detach"]);
        let onto = Onto::Branch("main".into());
        let mut plan = Plan::pick(onto.clone(), &[]);
        assert_eq!(plan.reach(repo.path(), &commits[2]).unwrap(), 0);
        assert_eq!(plan.steps.len(), 3);
        plan.steps[0].action = Action::Edit;
        plan.steps[0]
            .edits
            .insert("f3.txt".into(), Some("three\n".into()));
        let worked = work_out(repo.path(), &plan).unwrap();
        apply(repo.path(), &onto, &worked).unwrap();
        let at = |file: &str| file_at(repo.path(), "main", file).unwrap().unwrap();
        assert_eq!(at("f3.txt"), "three\n");
        assert_eq!(at("f5.txt"), "5\n");
        assert_eq!(files_of(repo.path(), "main").unwrap(), ["f5.txt"]);
    }

    #[test]
    fn a_rebase_moves_the_branch_onto_another() {
        let (repo, picks) = clashing("rebase");
        let onto = Onto::Branch("side".into());
        let mut plan = rebase(repo.path(), onto.clone(), "main").unwrap();
        assert_eq!(plan.steps.len(), 3);
        // Leave out the commits that clash: what remains applies cleanly.
        plan.steps[0].action = Action::Drop;
        plan.steps[2].action = Action::Drop;
        let worked = work_out(repo.path(), &plan).unwrap();
        assert!(worked.ready());
        apply(repo.path(), &onto, &worked).unwrap();
        let log = repo.git(&["log", "--format=%s", "side"]);
        assert_eq!(
            log.lines().collect::<Vec<_>>(),
            ["b.txt 1", "a.txt 5", "a.txt 5"]
        );
        assert_eq!(picks.len(), 3);
    }
}
