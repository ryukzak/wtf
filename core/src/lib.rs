mod attention;
mod console;
mod difft;
mod git;
mod patch;
mod summary;
mod text;
mod workflow;

use std::time::{SystemTime, UNIX_EPOCH};

pub use attention::{Attention, Scope};
pub use console::{LogEntry, log_since};
pub use workflow::{Branch, GraphNode, NodeKind, WorkflowGraph};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("{message}")]
    Failed { message: String },
}

impl CoreError {
    pub(crate) fn new(message: impl std::fmt::Display) -> Self {
        Self::Failed {
            message: message.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub id: String,
    pub short_id: String,
    pub summary: String,
    /// The message after the summary line.
    pub body: String,
    pub author: String,
    pub time: i64,
    /// First parent; `None` for a root commit.
    pub parent: Option<String>,
}

/// One end of a diff.
#[derive(Debug, Clone, PartialEq)]
pub enum Revision {
    Commit(String),
    /// The files on disk in this worktree.
    Worktree(String),
}

#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub status: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub old_id: String,
    pub new_id: String,
    /// Set when the new side is read from this worktree's files.
    pub worktree: Option<String>,
}

/// Half-open byte range on char boundaries, ready for GPUI text highlights.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffSide {
    /// 1-based.
    pub line_number: u32,
    pub text: String,
    pub changes: Vec<TextRange>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffRow {
    pub left: Option<DiffSide>,
    pub right: Option<DiffSide>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileDiff {
    pub language: String,
    pub rows: Vec<DiffRow>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffMode {
    Structural,
    Text,
}

/// Builds the workflow graph: worktrees, the main branch and the local branches
/// `attention` shows. With `combine`, linear runs of commits share a node.
pub fn workflow_graph(
    repo_path: &str,
    attention: &Attention,
    combine: bool,
    history: usize,
) -> Result<WorkflowGraph, CoreError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs() as i64);
    console::as_refresh(|| workflow::build(repo_path, attention, combine, history, now))
}

/// Commits in `base..tip`, newest first; `None` reaches back to the root.
pub fn range_commits(
    repo_path: &str,
    base: Option<&str>,
    tip: &Revision,
) -> Result<Vec<Commit>, CoreError> {
    git::range_commits(repo_path, base, tip)
}

pub use summary::{MESSAGE_PROMPT, SUMMARY_PROMPT, Summary};

/// Whether Apple's on-device model can summarize commits on this Mac.
pub fn summaries_available() -> bool {
    summary::available()
}

/// A commit message for what a worktree has staged, written by the model
/// from the staged files and a short diff of them.
pub fn write_commit_message(worktree: &str, prompt: &str) -> Result<String, CoreError> {
    let stat = git::run(worktree, &["diff", "--cached", "--stat=100"])?;
    let diff = git::run(worktree, &["diff", "--cached", "-U1", "--no-color"])?;
    summary::commit_message(&format!("Files changed:\n{stat}\nDiff:\n{diff}"), prompt)
}

/// A summary of commits, newest first, and the changes from `base` to
/// `tip` they make.
pub fn summarize_commits(
    repo_path: &str,
    commits: &[Commit],
    base: Option<&str>,
    tip: &Revision,
    prompt: &str,
) -> Result<Summary, CoreError> {
    let files = git::diff_stat(repo_path, base, tip)?;
    summary::summarize(commits, &files, prompt)
}

/// A stash: its name as git lists it, its commit, the commit it was made
/// on, its index and untracked commits, and what it says.
#[derive(Debug, Clone, PartialEq)]
pub struct Stash {
    pub name: String,
    pub id: String,
    pub base: String,
    pub index: String,
    pub untracked: Option<String>,
    pub message: String,
    /// The branch it was made on; `None` from a detached HEAD.
    pub branch: Option<String>,
    pub time: i64,
}

/// The files a stash changes: unstaged and untracked ones first, then the
/// staged ones, which start at the returned index.
pub fn stash_files(repo_path: &str, stash: &Stash) -> Result<(Vec<ChangedFile>, usize), CoreError> {
    git::stash_files(repo_path, stash)
}

pub fn changed_files(
    repo_path: &str,
    base: Option<&str>,
    tip: &Revision,
) -> Result<Vec<ChangedFile>, CoreError> {
    git::changed_files(repo_path, base, tip)
}

/// The top of the repository or worktree holding `path`: a folder or file
/// anywhere inside it. `None` outside any repository.
pub fn repository_root(path: &str) -> Option<String> {
    let path = std::path::Path::new(path);
    let dir = if path.is_dir() { path } else { path.parent()? };
    let root = git::run(&dir.to_string_lossy(), &["rev-parse", "--show-toplevel"]).ok()?;
    Some(root.trim().to_string())
}

/// A worktree's staged and unstaged changes, as in Sublime Merge.
#[derive(Debug, Clone, Default)]
pub struct WorktreeStatus {
    pub staged: Vec<ChangedFile>,
    pub unstaged: Vec<ChangedFile>,
}

pub fn worktree_status(path: &str) -> Result<WorktreeStatus, CoreError> {
    git::worktree_status(path)
}

/// Stages files in a worktree; all changes when `files` is empty.
pub fn stage(path: &str, files: &[String]) -> Result<(), CoreError> {
    git::stage(path, files)
}

/// Unstages files in a worktree; everything when `files` is empty.
pub fn unstage(path: &str, files: &[String]) -> Result<(), CoreError> {
    git::unstage(path, files)
}

/// Stages, or with `unstage` unstages, some lines of a worktree's file:
/// removed lines by their old numbers, added ones by their new numbers.
pub fn stage_lines(
    repo_path: &str,
    worktree: &str,
    file: &ChangedFile,
    removed: &std::collections::HashSet<u32>,
    added: &std::collections::HashSet<u32>,
    unstage: bool,
) -> Result<(), CoreError> {
    git::stage_lines(repo_path, worktree, file, removed, added, unstage)
}

/// Throws away a worktree's unstaged changes to files.
pub fn discard(path: &str, files: &[ChangedFile]) -> Result<(), CoreError> {
    git::discard(path, files)
}

/// Throws away some unstaged lines of a worktree's file.
pub fn discard_lines(
    repo_path: &str,
    worktree: &str,
    file: &ChangedFile,
    removed: &std::collections::HashSet<u32>,
    added: &std::collections::HashSet<u32>,
) -> Result<(), CoreError> {
    git::discard_lines(repo_path, worktree, file, removed, added)
}

/// Fetches every remote of a repository. A background fetch is logged as
/// a refresh command, hidden with them.
pub fn fetch(repo: &str, prune: bool, background: bool) -> Result<(), CoreError> {
    match background {
        true => console::as_refresh(|| git::fetch(repo, prune)),
        false => git::fetch(repo, prune),
    }
}

/// Runs a pull or fetch, such as `["pull", "--rebase"]`, in a worktree.
pub fn pull(path: &str, args: &[&str]) -> Result<(), CoreError> {
    git::remote(path, args)
}

/// Pushes a worktree's branch, to `to` or where it tracks, setting where
/// it goes on its first push.
pub fn push(path: &str, to: Option<&str>, options: &[&str]) -> Result<(), CoreError> {
    git::push(path, to, options)
}

/// Commits a worktree's staged changes, or amends its latest commit with them.
pub fn commit(path: &str, message: &str, amend: bool) -> Result<(), CoreError> {
    git::commit(path, message, amend)
}

/// The full message of a worktree's latest commit.
pub fn head_message(path: &str) -> Result<String, CoreError> {
    git::head_message(path)
}

/// Rewrites the message of a commit in a worktree's history, copying the
/// commits after it; files and the index stay as they are.
pub fn reword(path: &str, commit: &str, message: &str) -> Result<(), CoreError> {
    git::reword(path, commit, message)
}

pub fn file_diff(
    repo_path: String,
    file: ChangedFile,
    mode: DiffMode,
) -> Result<FileDiff, CoreError> {
    let (old, new) = git::read_blobs(&repo_path, &file)?;
    match mode {
        DiffMode::Structural => {
            let name = file.new_path.as_ref().or(file.old_path.as_ref());
            difft::diff(name.map_or("file", String::as_str), &old, &new)
        }
        DiffMode::Text => Ok(text::diff(&old, &new)),
    }
}
