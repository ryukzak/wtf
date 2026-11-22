use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use gix::ObjectId;

use crate::{ChangedFile, Commit, CoreError, Revision, WorktreeStatus, console, patch};

/// Fields split by 0x1f, records ended by 0x1e; read by [`parse_log`].
pub const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%ct%x1f%an%x1f%s%x1f%b%x1e";

const NULL_ID: &str = "0000000000000000000000000000000000000000";

/// Caps commit lists that reach back to the root commit.
const MAX_COMMITS: &str = "--max-count=1000";

fn open(repo_path: &str) -> Result<gix::Repository, CoreError> {
    gix::open(repo_path).map_err(CoreError::new)
}

/// Runs git in `dir` and records it in the console log.
pub fn run<S: AsRef<OsStr>>(dir: &str, args: &[S]) -> Result<String, CoreError> {
    let started = Instant::now();
    let line = console::shell_line("git", args);
    let output = Command::new("git").args(args).current_dir(dir).output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            console::record(dir, line, false, started, error.to_string());
            return Err(CoreError::new(error));
        }
    };
    if !output.status.success() {
        let mut error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if error.is_empty() {
            error = format!("{line} failed with {}", output.status);
        }
        console::record(dir, line, false, started, error.clone());
        return Err(CoreError::new(error));
    }
    console::record(dir, line, true, started, String::new());
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Fetches every remote, dropping remote branches deleted there when
/// `prune`. Git asks no password: with no terminal to answer it, a fetch
/// that needs one fails rather than waits.
pub fn fetch(repo: &str, prune: bool) -> Result<(), CoreError> {
    let mut args = vec!["fetch", "--all", "--quiet"];
    if prune {
        args.push("--prune");
    }
    run_with_env(repo, &args, "", &[("GIT_TERMINAL_PROMPT", "0")]).map(|_| ())
}

/// Runs a git command that talks to a remote, such as `pull --rebase`, in
/// `path`. As with [`fetch`], git asks no password.
pub fn remote(path: &str, args: &[&str]) -> Result<(), CoreError> {
    run_with_env(path, args, "", &[("GIT_TERMINAL_PROMPT", "0")]).map(|_| ())
}

/// Pushes the branch checked out in `path` with `options`, to `to` or else
/// where it tracks. A branch never pushed goes to `to`, else the remote git
/// config names for it, else `origin`, and tracks it from then on.
pub fn push(path: &str, to: Option<&str>, options: &[&str]) -> Result<(), CoreError> {
    let mut args = vec!["push"];
    args.extend(options);
    let upstream = run(path, &["rev-parse", "--abbrev-ref", "@{upstream}"]).ok();
    let tracked = upstream
        .as_deref()
        .and_then(|upstream| upstream.trim().split_once('/'))
        .map(|(remote, _)| remote.to_string());
    match (to, &tracked) {
        (None, Some(_)) => return remote(path, &args),
        (Some(to), Some(tracked)) if to == tracked => return remote(path, &args),
        // A tracked branch sent elsewhere goes as it is, tracking unchanged.
        (Some(to), Some(_)) => {
            args.extend([to, "HEAD"]);
            return remote(path, &args);
        }
        _ => {}
    }
    let branch = run(path, &["symbolic-ref", "--short", "HEAD"])?;
    let branch = branch.trim();
    let configured = |key: String| {
        run(path, &["config", "--get", &key])
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let to = to
        .map(str::to_string)
        .or_else(|| configured(format!("branch.{branch}.pushRemote")))
        .or_else(|| configured("remote.pushDefault".to_string()))
        .unwrap_or_else(|| "origin".to_string());
    args.extend(["-u", to.as_str(), branch]);
    remote(path, &args)
}

/// Runs git with `input` on its standard input, recorded like [`run`].
fn run_with_input(dir: &str, args: &[&str], input: &str) -> Result<String, CoreError> {
    run_with_env(dir, args, input, &[])
}

/// Runs git with `input` on stdin and extra environment variables.
fn run_with_env(
    dir: &str,
    args: &[&str],
    input: &str,
    env: &[(&str, &str)],
) -> Result<String, CoreError> {
    use std::io::Write;
    use std::process::Stdio;

    let started = Instant::now();
    let line = console::shell_line("git", args);
    let spawned = Command::new("git")
        .args(args)
        .envs(env.iter().copied())
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let output = spawned.and_then(|mut child| {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input.as_bytes())?;
        }
        child.wait_with_output()
    });
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            console::record(dir, line, false, started, error.to_string());
            return Err(CoreError::new(error));
        }
    };
    if !output.status.success() {
        let mut error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if error.is_empty() {
            error = format!("{line} failed with {}", output.status);
        }
        console::record(dir, line, false, started, error.clone());
        return Err(CoreError::new(error));
    }
    console::record(dir, line, true, started, String::new());
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A worktree's changes: staged ones go from HEAD to the index, unstaged
/// ones from the index to the files on disk, untracked files included.
pub fn worktree_status(path: &str) -> Result<WorktreeStatus, CoreError> {
    let staged = run(
        path,
        &["diff", "--cached", "--raw", "-z", "--no-abbrev", "-M"],
    )?;
    let unstaged = run(path, &["diff", "--raw", "-z", "--no-abbrev"])?;
    let mut status = WorktreeStatus {
        staged: parse_raw(&staged, None),
        unstaged: parse_raw(&unstaged, Some(path)),
    };
    status
        .unstaged
        .extend(untracked(path)?.into_iter().map(|file| ChangedFile {
            status: "A".to_string(),
            old_path: None,
            new_path: Some(file),
            old_id: NULL_ID.to_string(),
            new_id: NULL_ID.to_string(),
            worktree: Some(path.to_string()),
        }));
    Ok(status)
}

/// Stages files; all of them when `files` is empty.
pub fn stage(path: &str, files: &[String]) -> Result<(), CoreError> {
    let mut args = vec!["add".to_string(), "-A".into(), "--".into()];
    args.extend(files.iter().cloned());
    run(path, &args).map(drop)
}

/// Unstages files; all of them when `files` is empty.
pub fn unstage(path: &str, files: &[String]) -> Result<(), CoreError> {
    let mut args = vec!["reset".to_string(), "-q".into(), "--".into()];
    args.extend(files.iter().cloned());
    run(path, &args).map(drop)
}

/// Stages, or with `unstage` unstages, only some lines of a file: removed
/// lines by their old numbers and added ones by their new numbers.
pub fn stage_lines(
    repo_path: &str,
    worktree: &str,
    file: &ChangedFile,
    removed: &HashSet<u32>,
    added: &HashSet<u32>,
    unstage: bool,
) -> Result<(), CoreError> {
    let (old, new) = read_blobs(repo_path, file)?;
    let (old, new) = (String::from_utf8_lossy(&old), String::from_utf8_lossy(&new));
    let path = file
        .new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .unwrap_or_default();
    // A new file needs an entry in the index before lines can go into it.
    let untracked = file.worktree.is_some() && file.status == "A" && file.old_id == NULL_ID;
    if untracked && !unstage {
        run(worktree, &["add", "-N", "--", path])?;
    }
    // Unstaging takes the index back toward HEAD, so the patch runs reversed.
    let patch = match unstage {
        false => patch::partial(path, &old, &new, removed, added),
        true => patch::partial(path, &new, &old, added, removed),
    };
    match patch {
        Some(patch) => run_with_input(worktree, &["apply", "--cached", "-"], &patch).map(drop),
        None => Ok(()),
    }
}

/// Throws away a worktree's unstaged changes to files: tracked ones go back
/// to what is staged, untracked ones are deleted.
pub fn discard(path: &str, files: &[ChangedFile]) -> Result<(), CoreError> {
    for file in files {
        let name = file
            .new_path
            .as_deref()
            .or(file.old_path.as_deref())
            .unwrap_or_default();
        let untracked = file.status == "A" && file.old_id == NULL_ID;
        match untracked {
            true => run(path, &["clean", "-f", "-q", "--", name])?,
            false => run(path, &["restore", "--worktree", "--", name])?,
        };
    }
    Ok(())
}

/// Throws away some unstaged lines of a file: removed lines come back and
/// added ones go, by their numbers, as staging them would take them.
pub fn discard_lines(
    repo_path: &str,
    worktree: &str,
    file: &ChangedFile,
    removed: &HashSet<u32>,
    added: &HashSet<u32>,
) -> Result<(), CoreError> {
    let (old, new) = read_blobs(repo_path, file)?;
    let (old, new) = (String::from_utf8_lossy(&old), String::from_utf8_lossy(&new));
    let path = file
        .new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .unwrap_or_default();
    match patch::partial(path, &old, &new, removed, added) {
        Some(patch) => run_with_input(worktree, &["apply", "-R", "-"], &patch).map(drop),
        None => Ok(()),
    }
}

/// The stashes, newest first.
pub fn stashes(repo_path: &str) -> Result<Vec<crate::Stash>, CoreError> {
    let format = "--format=%gd%x1f%H%x1f%P%x1f%ct%x1f%gs";
    let list = run(repo_path, &["stash", "list", format])?;
    Ok(list
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\x1f').collect();
            let [name, id, parents, time, subject] = fields[..] else {
                return None;
            };
            let parents: Vec<&str> = parents.split(' ').collect();
            // "WIP on fix: 1a2b3c4 subject" or "On fix: message".
            let (branch, message) = subject
                .strip_prefix("WIP on ")
                .or_else(|| subject.strip_prefix("On "))
                .and_then(|rest| rest.split_once(": "))
                .map_or((None, subject), |(branch, message)| {
                    let branch = (branch != "(no branch)").then(|| branch.to_string());
                    (branch, message)
                });
            Some(crate::Stash {
                name: name.to_string(),
                id: id.to_string(),
                base: parents.first()?.to_string(),
                index: parents.get(1)?.to_string(),
                untracked: parents.get(2).map(|id| id.to_string()),
                message: message.to_string(),
                branch,
                time: time.parse().unwrap_or_default(),
            })
        })
        .collect())
}

/// What a stash holds: unstaged changes, then untracked files, then the
/// staged ones, which start at the returned index.
pub fn stash_files(
    repo_path: &str,
    stash: &crate::Stash,
) -> Result<(Vec<ChangedFile>, usize), CoreError> {
    let mut files = changed_files(
        repo_path,
        Some(&stash.index),
        &Revision::Commit(stash.id.clone()),
    )?;
    if let Some(untracked) = &stash.untracked {
        files.extend(changed_files(
            repo_path,
            None,
            &Revision::Commit(untracked.clone()),
        )?);
    }
    let split = files.len();
    files.extend(changed_files(
        repo_path,
        Some(&stash.base),
        &Revision::Commit(stash.index.clone()),
    )?);
    Ok((files, split))
}

/// Commits what is staged, with the message as typed; amending rewrites the
/// latest commit with them instead.
pub fn commit(path: &str, message: &str, amend: bool) -> Result<(), CoreError> {
    let args: &[&str] = match amend {
        true => &["commit", "--amend", "--allow-empty", "-F", "-"],
        false => &["commit", "-F", "-"],
    };
    run_with_input(path, args, message).map(drop)
}

/// The full message of a worktree's latest commit.
pub fn head_message(path: &str) -> Result<String, CoreError> {
    Ok(run(path, &["log", "-1", "--format=%B"])?
        .trim_end()
        .to_string())
}

/// Rewrites the message of a worktree's latest commit, leaving staged
/// changes out of it.
pub fn reword(path: &str, commit: &str, message: &str) -> Result<(), CoreError> {
    let head = run(path, &["rev-parse", "HEAD"])?.trim().to_string();
    let base = run(path, &["merge-base", commit, &head])?;
    if base.trim() != commit {
        return Err(CoreError::new(
            "The commit is not in this worktree's history",
        ));
    }
    let range = format!("{commit}..{head}");
    if !run(path, &["rev-list", "--merges", &range])?
        .trim()
        .is_empty()
    {
        return Err(CoreError::new(
            "Merges follow this commit; reword it with an interactive rebase",
        ));
    }
    // Only messages change, so each commit keeps its tree and the index and
    // files stay as they are; later commits are copied onto the new one.
    let parents = run(path, &["rev-parse", &format!("{commit}^@")])?;
    let parents: Vec<&str> = parents.lines().collect();
    let mut rewritten = copy_commit(path, commit, &parents, message)?;
    for later in run(path, &["rev-list", "--reverse", &range])?.lines() {
        let message = run(path, &["show", "-s", "--format=%B", later])?;
        rewritten = copy_commit(path, later, &[&rewritten], message.trim_end())?;
    }
    // A branch moves; a detached HEAD moves itself.
    let name = run(path, &["rev-parse", "--symbolic-full-name", "HEAD"])?;
    match name.trim() {
        "HEAD" => run(
            path,
            &["update-ref", "--no-deref", "HEAD", &rewritten, &head],
        )?,
        branch => run(
            path,
            &["update-ref", "-m", "wtf: reword", branch, &rewritten, &head],
        )?,
    };
    Ok(())
}

/// A copy of `commit` with its tree, author and date on other parents and
/// with another message.
fn copy_commit(
    path: &str,
    commit: &str,
    parents: &[&str],
    message: &str,
) -> Result<String, CoreError> {
    let author = run(
        path,
        &["show", "-s", "--format=%an%n%ae%n%ad", "--date=raw", commit],
    )?;
    let author: Vec<&str> = author.lines().collect();
    let [name, email, date] = author[..] else {
        return Err(CoreError::new(format!(
            "Cannot read the author of {commit}"
        )));
    };
    let tree = format!("{commit}^{{tree}}");
    let mut args = vec!["commit-tree", tree.as_str()];
    for parent in parents {
        args.extend(["-p", parent]);
    }
    args.extend(["-F", "-"]);
    let env = [
        ("GIT_AUTHOR_NAME", name),
        ("GIT_AUTHOR_EMAIL", email),
        ("GIT_AUTHOR_DATE", date),
    ];
    Ok(run_with_env(path, &args, message, &env)?.trim().to_string())
}

pub fn empty_tree(repo_path: &str) -> Result<String, CoreError> {
    Ok(open(repo_path)?.empty_tree().id.to_string())
}

/// The directory to run git in and the revision to name there.
fn locate<'a>(repo_path: &'a str, tip: &'a Revision) -> (&'a str, &'a str) {
    match tip {
        Revision::Commit(id) => (repo_path, id),
        Revision::Worktree(path) => (path, "HEAD"),
    }
}

/// Returns each commit with all its parents.
pub fn parse_log(raw: &str) -> Vec<(Commit, Vec<String>)> {
    raw.split('\x1e')
        .filter_map(|record| {
            let fields: Vec<&str> = record.trim_start_matches('\n').split('\x1f').collect();
            let [id, parents, time, author, summary, body] = fields[..] else {
                return None;
            };
            let parents: Vec<String> = parents.split_whitespace().map(str::to_string).collect();
            let commit = Commit {
                id: id.to_string(),
                short_id: id.chars().take(7).collect(),
                summary: summary.to_string(),
                body: body.trim().to_string(),
                author: author.to_string(),
                time: time.parse().unwrap_or_default(),
                parent: parents.first().cloned(),
            };
            Some((commit, parents))
        })
        .collect()
}

pub fn range_commits(
    repo_path: &str,
    base: Option<&str>,
    tip: &Revision,
) -> Result<Vec<Commit>, CoreError> {
    let (dir, rev) = locate(repo_path, tip);
    let mut args = vec![
        "log".to_string(),
        "--topo-order".into(),
        MAX_COMMITS.into(),
        LOG_FORMAT.into(),
        rev.into(),
    ];
    args.extend(base.map(|base| format!("^{base}")));
    Ok(parse_log(&run(dir, &args)?)
        .into_iter()
        .map(|(commit, _)| commit)
        .collect())
}

/// Files changed from `base` (`None` is the empty tree) to `tip`. A worktree tip
/// includes its uncommitted and untracked files.
/// The files a range changes, with how much, as `git diff --stat` puts it.
pub fn diff_stat(repo_path: &str, base: Option<&str>, tip: &Revision) -> Result<String, CoreError> {
    let base = match base {
        Some(base) => base.to_string(),
        None => empty_tree(repo_path)?,
    };
    let (dir, rev) = locate(repo_path, tip);
    run(dir, &["diff", "--stat=100", &base, rev])
}

pub fn changed_files(
    repo_path: &str,
    base: Option<&str>,
    tip: &Revision,
) -> Result<Vec<ChangedFile>, CoreError> {
    let base = match base {
        Some(base) => base.to_string(),
        None => empty_tree(repo_path)?,
    };
    let (dir, rev) = locate(repo_path, tip);
    let mut args = vec!["diff", "--raw", "-z", "--no-abbrev", "-M", &base];
    let worktree = match tip {
        Revision::Worktree(path) => Some(path.as_str()),
        Revision::Commit(_) => {
            args.push(rev);
            None
        }
    };
    let mut files = parse_raw(&run(dir, &args)?, worktree);
    if let Some(root) = worktree {
        files.extend(untracked(root)?.into_iter().map(|path| ChangedFile {
            status: "A".to_string(),
            old_path: None,
            new_path: Some(path),
            old_id: NULL_ID.to_string(),
            new_id: NULL_ID.to_string(),
            worktree: Some(root.to_string()),
        }));
    }
    Ok(files)
}

pub fn untracked(worktree: &str) -> Result<Vec<String>, CoreError> {
    let raw = run(
        worktree,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    Ok(raw
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

/// Parses `git diff --raw -z`: `:mode mode id id status\0path\0[path\0]`.
fn parse_raw(raw: &str, worktree: Option<&str>) -> Vec<ChangedFile> {
    let mut fields = raw.split('\0');
    let mut files = Vec::new();
    while let Some(meta) = fields.next().filter(|meta| !meta.is_empty()) {
        let meta: Vec<&str> = meta.split(' ').collect();
        let [_, _, old_id, new_id, status] = meta[..] else {
            break;
        };
        let first = fields.next().unwrap_or_default().to_string();
        let kind = &status[..1];
        let (old_path, new_path) = match kind {
            "A" => (None, Some(first)),
            "D" => (Some(first), None),
            "R" | "C" => (Some(first), fields.next().map(str::to_string)),
            _ => (Some(first.clone()), Some(first)),
        };
        files.push(ChangedFile {
            status: kind.to_string(),
            old_path,
            new_path,
            old_id: old_id.to_string(),
            new_id: new_id.to_string(),
            worktree: worktree.map(str::to_string),
        });
    }
    files
}

pub fn read_blobs(repo_path: &str, file: &ChangedFile) -> Result<(Vec<u8>, Vec<u8>), CoreError> {
    let repo = open(repo_path)?;
    let old = read_blob(&repo, &file.old_id)?;
    // Git reports worktree files with a null id; their content is on disk.
    let new = match (&file.worktree, &file.new_path) {
        (Some(root), Some(path)) => fs::read(Path::new(root).join(path)).map_err(CoreError::new)?,
        _ => read_blob(&repo, &file.new_id)?,
    };
    Ok((old, new))
}

/// Null ids (added or deleted side) and submodule commits read as empty.
fn read_blob(repo: &gix::Repository, id: &str) -> Result<Vec<u8>, CoreError> {
    let id = ObjectId::from_hex(id.as_bytes()).map_err(CoreError::new)?;
    if id.is_null() {
        return Ok(Vec::new());
    }
    let object = repo.try_find_object(id).map_err(CoreError::new)?;
    Ok(object.map(|object| object.data.clone()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modified_added_and_renamed() {
        let raw = concat!(
            ":100644 100644 aaa bbb M\0src/a.rs\0",
            ":000000 100644 000 ccc A\0new.rs\0",
            ":100644 100644 ddd eee R087\0old name.rs\0new name.rs\0",
        );
        let files = parse_raw(raw, None);
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].status, "M");
        assert_eq!(files[0].old_path.as_deref(), Some("src/a.rs"));
        assert_eq!(files[1].old_path, None);
        assert_eq!(files[1].new_path.as_deref(), Some("new.rs"));
        assert_eq!(files[2].status, "R");
        assert_eq!(files[2].old_path.as_deref(), Some("old name.rs"));
        assert_eq!(files[2].new_path.as_deref(), Some("new name.rs"));
    }

    #[test]
    fn parses_log_records() {
        let raw = "aaaaaaaaa\x1fbbb ccc\x1f100\x1fAnn\x1fMerge it\x1fWhy:\nbecause\n\x1e\nddd\x1f\x1f50\x1fBob\x1fRoot\x1f\x1e\n";
        let log = parse_log(raw);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].0.short_id, "aaaaaaa");
        assert_eq!(log[0].0.parent.as_deref(), Some("bbb"));
        assert_eq!(log[0].1, vec!["bbb", "ccc"]);
        assert_eq!(log[0].0.body, "Why:\nbecause");
        assert_eq!(log[1].0.parent, None);
        assert_eq!(log[1].0.body, "");
        assert_eq!(log[1].0.time, 50);
    }
}
