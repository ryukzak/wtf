mod attention;
mod channels;
mod console;
mod difft;
mod git;
mod patch;
mod submodule;
mod summary;
mod text;
mod tools;

pub use text::carry_line;
mod workflow;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub use attention::{Attention, Scope};
pub use channels::{add_channel, glob, is_channel, remove_channel};
pub use console::{LogEntry, log_since};
pub use git::{
    changed_files, commit, discard, discard_lines, head_message, push, range_commits, reword,
    stage, stage_lines, stash_files, unstage, worktree_status,
};
pub use tools::{ToolCheck, check_tools};
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
    /// A submodule's pointer, not a file.
    pub submodule: bool,
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

/// A commit message for what a worktree has staged, written by the model
/// from the staged files and a short diff of them.
pub fn write_commit_message(
    worktree: &str,
    prompt: &str,
    varied: bool,
) -> Result<String, CoreError> {
    let stat = git::run(worktree, &["diff", "--cached", "--stat=100"])?;
    let diff = git::run(worktree, &["diff", "--cached", "-U1", "--no-color"])?;
    // The latest subjects show the repository's style; a repository with no
    // commits yet has none.
    let subjects = git::run(
        worktree,
        &["log", "-n", "5", "--no-merges", "--format=%s", "HEAD"],
    )
    .unwrap_or_default();
    let examples = summary::style_examples(&subjects);
    summary::commit_message(
        &format!("Files changed:\n{stat}\nDiff:\n{diff}"),
        &format!("{examples}{prompt}"),
        varied,
    )
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
    /// Submodules with changes of their own but where the index has them:
    /// nothing for this repository to commit.
    pub inside: Vec<String>,
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

/// The largest file a snapshot shows.
const MAX_SNAPSHOT_FILE: usize = 2 * 1024 * 1024;

/// The submodules of the repository as it is at `revision`, by path.
pub fn snapshot_submodules(repo: &str, revision: &Revision) -> Vec<String> {
    let text = match revision {
        Revision::Commit(id) => {
            console::as_refresh(|| git::show(repo, &format!("{id}:.gitmodules"))).ok()
        }
        Revision::Worktree(path) => std::fs::read(Path::new(path).join(".gitmodules")).ok(),
    };
    text.map_or_else(Vec::new, |text| {
        submodule::paths(&String::from_utf8_lossy(&text))
    })
}

/// Every file of the repository as it is at `revision`: in a commit's
/// tree, or in a worktree, tracked or new and not ignored. Read quietly,
/// with the refresh commands.
pub fn snapshot_files(repo: &str, revision: &Revision) -> Result<Vec<String>, CoreError> {
    console::as_refresh(|| {
        let listing = match revision {
            Revision::Commit(id) => git::run(repo, &["ls-tree", "-r", "--name-only", "-z", id])?,
            Revision::Worktree(path) => git::run(
                path,
                &[
                    "ls-files",
                    "-z",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                ],
            )?,
        };
        let mut files: Vec<String> = listing
            .split('\0')
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        // A worktree lists tracked files it has deleted too.
        if let Revision::Worktree(path) = revision {
            files.retain(|name| Path::new(path).join(name).exists());
        }
        files.sort();
        files.dedup();
        Ok(files)
    })
}

/// A line of a file that a search found.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub path: String,
    /// From 1.
    pub line: u32,
    pub text: String,
}

/// How many lines a search lists at most, and from one file.
const MAX_FOUND: usize = 300;
const MAX_FOUND_IN_FILE: &str = "--max-count=20";

/// The lines holding `query`, ignoring case, in the repository as it is at
/// `revision`: a commit's tree, or a worktree's files, new ones too but not
/// ignored ones. Binary files are left out. Read quietly, with the refresh
/// commands.
pub fn snapshot_grep(repo: &str, revision: &Revision, query: &str) -> Vec<Found> {
    let base = ["grep", "-n", "-I", "-i", "-F", "-z", MAX_FOUND_IN_FILE];
    let output = console::as_refresh(|| match revision {
        Revision::Commit(id) => {
            let mut args = base.to_vec();
            args.extend(["-e", query, id.as_str()]);
            git::run(repo, &args)
        }
        Revision::Worktree(path) => {
            let mut args = base.to_vec();
            args.extend(["--untracked", "-e", query]);
            git::run(path, &args)
        }
    });
    // git grep fails when it finds nothing.
    let Ok(output) = output else {
        return Vec::new();
    };
    let prefix = match revision {
        Revision::Commit(id) => format!("{id}:"),
        Revision::Worktree(_) => String::new(),
    };
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\0');
            let path = parts.next()?;
            let number = parts.next()?.parse().ok()?;
            let text = parts.next()?.trim().to_string();
            Some(Found {
                path: path.strip_prefix(&prefix).unwrap_or(path).to_string(),
                line: number,
                text,
            })
        })
        .take(MAX_FOUND)
        .collect()
}

/// A file's text as it is at `revision`; an error for binary files and very
/// large ones.
pub fn snapshot_file(repo: &str, revision: &Revision, file: &str) -> Result<String, CoreError> {
    if let Some(text) = git::snapshot_submodule(repo, revision, file) {
        return Ok(text);
    }
    let bytes = match revision {
        Revision::Commit(id) => console::as_refresh(|| git::show(repo, &format!("{id}:{file}")))?,
        Revision::Worktree(path) => {
            std::fs::read(Path::new(path).join(file)).map_err(CoreError::new)?
        }
    };
    if bytes.len() > MAX_SNAPSHOT_FILE {
        return Err(CoreError::new("Too large to show"));
    }
    if bytes.contains(&0) {
        return Err(CoreError::new("Binary file"));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The commits that changed `file` on the line of history ending at `tip`,
/// newest first, following renames, and where among them is the newest
/// change at or before `at`. Read in `dir`, quietly.
pub fn file_history(
    dir: &str,
    tip: &str,
    at: &str,
    file: &str,
) -> Result<(Vec<String>, Option<usize>), CoreError> {
    console::as_refresh(|| {
        let log = |from: &str, limit: Option<&str>| -> Result<Vec<String>, CoreError> {
            let mut args = vec!["log", "--format=%H", "--follow"];
            args.extend(limit);
            args.extend([from, "--", file]);
            Ok(git::run(dir, &args)?.lines().map(str::to_string).collect())
        };
        let history = log(tip, None)?;
        let last = log(at, Some("-1"))?;
        let position = last
            .first()
            .and_then(|change| history.iter().position(|id| id == change));
        Ok((history, position))
    })
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
