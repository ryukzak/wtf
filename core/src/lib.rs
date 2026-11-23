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
pub use git::{
    changed_files, commit, discard, discard_lines, head_message, push, range_commits, reword,
    stage, stage_lines, stash_files, unstage, worktree_status,
};
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

#[derive(Debug, Clone, PartialEq)]
pub struct ChangedFile {
    pub status: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub old_id: String,
    pub new_id: String,
    /// Set when the new side is read from this worktree's files.
    pub worktree: Option<String>,
}

impl ChangedFile {
    /// Its path: the new one, or the old one of a deleted file.
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or_default()
    }
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

pub fn file_diff(
    repo_path: String,
    file: ChangedFile,
    mode: DiffMode,
) -> Result<FileDiff, CoreError> {
    let (old, new) = git::read_blobs(&repo_path, &file)?;
    match mode {
        DiffMode::Structural => difft::diff(file.path(), &old, &new),
        DiffMode::Text => Ok(text::diff(&old, &new)),
    }
}
