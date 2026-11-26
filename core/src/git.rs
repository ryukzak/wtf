use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use gix::ObjectId;

use crate::{ChangedFile, Commit, CoreError, Revision, WorktreeStatus, console, patch};

/// Fields split by 0x1f, records ended by 0x1e; read by [`parse_log`].
pub const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%ct%x1f%an%x1f%ae%x1f%s%x1f%b%x1e";

const NULL_ID: &str = "0000000000000000000000000000000000000000";

/// Caps commit lists that reach back to the root commit.
const MAX_COMMITS: &str = "--max-count=1000";

fn open(repo_path: &str) -> Result<gix::Repository, CoreError> {
    gix::open(repo_path).map_err(CoreError::new)
}

/// Runs git in `dir` and records it in the console log.
pub fn run<S: AsRef<OsStr>>(dir: &str, args: &[S]) -> Result<String, CoreError> {
    run_with_env(dir, args, None, &[])
}

/// Fetches every remote, dropping remote branches deleted there when
/// `prune`. Git asks no password: with no terminal to answer it, a fetch
/// that needs one fails rather than waits.
pub fn fetch(repo: &str, prune: bool) -> Result<(), CoreError> {
    let prune = if prune { "--prune" } else { "--no-prune" };
    let args = ["fetch", "--all", "--quiet", prune];
    run_with_env(repo, &args, None, &[("GIT_TERMINAL_PROMPT", "0")]).map(|_| ())
}

/// An object's raw bytes, such as `HEAD:src/main.rs`, recorded like [`run`].
pub fn show(dir: &str, object: &str) -> Result<Vec<u8>, CoreError> {
    let started = Instant::now();
    let args = ["show", object];
    let line = console::shell_line("git", &args);
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(CoreError::new)?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        console::record(dir, line, false, started, error.clone());
        return Err(CoreError::new(error));
    }
    console::record(dir, line, true, started, String::new());
    Ok(output.stdout)
}

/// Runs a git command that talks to a remote, such as `pull --rebase`, in
/// `path`. As with [`fetch`], git asks no password.
pub fn remote(path: &str, args: &[&str]) -> Result<(), CoreError> {
    run_with_env(path, args, None, &[("GIT_TERMINAL_PROMPT", "0")]).map(|_| ())
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
    run_with_env(dir, args, Some(input), &[])
}

/// Runs git with `input`, if any, on stdin and extra environment variables.
/// Whether `args` read git config: `config` with a `--get…` option.
fn reads_config<S: AsRef<OsStr>>(args: &[S]) -> bool {
    let mut words = args.iter().map(|arg| arg.as_ref().to_string_lossy());
    words.next().is_some_and(|word| word == "config") && words.any(|word| word.starts_with("--get"))
}

fn run_with_env<S: AsRef<OsStr>>(
    dir: &str,
    args: &[S],
    input: Option<&str>,
    env: &[(&str, &str)],
) -> Result<String, CoreError> {
    use std::io::Write;
    use std::process::Stdio;

    let started = Instant::now();
    let line = console::shell_line("git", args);
    // File names are paths, never patterns: `a*` names that one file.
    let spawned = Command::new("git")
        .args(args)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .envs(env.iter().copied())
        .current_dir(dir)
        .stdin(match input {
            Some(_) => Stdio::piped(),
            None => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let output = spawned.and_then(|mut child| {
        if let (Some(mut stdin), Some(input)) = (child.stdin.take(), input) {
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
        // `git config --get…` exits with 1 and says nothing when the key
        // isn't set: an answer, not a failure, so the log shows it as one.
        // Callers still get an error, which is how they learn it's unset.
        let unset = error.is_empty() && output.status.code() == Some(1) && reads_config(args);
        if error.is_empty() {
            error = format!("{line} failed with {}", output.status);
        }
        console::record(
            dir,
            line,
            unset,
            started,
            match unset {
                true => String::new(),
                false => error.clone(),
            },
        );
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
    let unstaged = worktree_diff(path, &["diff", "--raw", "-z", "--no-abbrev", "-M"])?;
    let (unstaged, inside) = changed_inside(path, unstaged);
    Ok(WorktreeStatus {
        staged: submodules_first(parse_raw(&staged, None)),
        unstaged,
        inside,
    })
}

/// Takes out the submodules changed only inside, whose checkout is still
/// at the commit the other side has: their paths go apart.
fn changed_inside(worktree: &str, files: Vec<ChangedFile>) -> (Vec<ChangedFile>, Vec<String>) {
    let (inside, files): (Vec<ChangedFile>, Vec<ChangedFile>) =
        files.into_iter().partition(|file| {
            file.submodule
                && file.new_path.is_some()
                && crate::submodule::checkout(&Path::new(worktree).join(file.path()))
                    .is_some_and(|(head, _)| head == file.old_id)
        });
    let paths = inside.iter().map(|file| file.path().to_string()).collect();
    (submodules_first(files), paths)
}

/// Submodules before files, each in the order they came.
fn submodules_first(mut files: Vec<ChangedFile>) -> Vec<ChangedFile> {
    files.sort_by_key(|file| !file.submodule);
    files
}

/// Runs a `git diff --raw` of a worktree's files, its untracked ones
/// counted, so a file moved on disk is a rename rather than a deleted one
/// and a new one. Untracked files go into a copy of the index as
/// `git add -N` puts them; the index itself is left alone.
fn worktree_diff(worktree: &str, args: &[&str]) -> Result<Vec<ChangedFile>, CoreError> {
    let untracked = untracked_files(worktree)?;
    let index = run(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?;
    let index = PathBuf::from(index.trim());
    let copy = std::env::temp_dir().join(format!(
        "wtf-index-{}-{}",
        std::process::id(),
        TEMP_INDEX.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    if untracked.is_empty() || fs::copy(&index, &copy).is_err() {
        let mut files = parse_raw(&run(worktree, args)?, Some(worktree));
        files.extend(untracked);
        return Ok(files);
    }
    let diff = (|| {
        let copy = copy.to_string_lossy();
        let env = [("GIT_INDEX_FILE", copy.as_ref())];
        let paths: String = untracked
            .iter()
            .map(|file| format!("{}\0", file.path()))
            .collect();
        run_with_env(
            worktree,
            &["add", "-N", "--pathspec-from-file=-", "--pathspec-file-nul"],
            Some(&paths),
            &env,
        )?;
        run_with_env(worktree, args, None, &env)
    })();
    fs::remove_file(&copy).ok();
    // Untracked files still new come as they always do; the others are
    // renames' new names.
    let new: HashSet<&str> = untracked.iter().map(ChangedFile::path).collect();
    let mut files: Vec<ChangedFile> = parse_raw(&diff?, Some(worktree))
        .into_iter()
        .filter(|file| !(file.status == "A" && new.contains(file.path())))
        .collect();
    let renamed: HashSet<&str> = files
        .iter()
        .filter_map(|file| file.new_path.as_deref())
        .collect();
    let new: Vec<ChangedFile> = untracked
        .into_iter()
        .filter(|file| !renamed.contains(file.path()))
        .collect();
    files.extend(new);
    Ok(files)
}

/// Numbers the copies of the index [`worktree_diff`] makes at once.
static TEMP_INDEX: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Stages files; all of them when `files` is empty. A file renamed on
/// disk is staged by both its names, the old one going as the new comes.
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
    if file.submodule {
        return Err(CoreError::new("A submodule is staged whole, not by lines"));
    }
    let (old, new) = read_blobs(repo_path, file)?;
    let (old, new) = (String::from_utf8_lossy(&old), String::from_utf8_lossy(&new));
    let path = file.path();
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
/// to what is staged, new ones are deleted, as `git add -N` left them too.
pub fn discard(path: &str, files: &[ChangedFile]) -> Result<(), CoreError> {
    for file in files {
        let name = file.path();
        let untracked = file.status == "A" && file.old_id == NULL_ID;
        if file.submodule {
            // Back to the commit the index records for it.
            run(path, &["submodule", "update", "--init", "--", name])?;
        } else if file.status == "R"
            && let Some(old) = &file.old_path
        {
            // A file moved on disk goes back to its old name.
            run(path, &["restore", "--worktree", "--", old])?;
            run(path, &["clean", "-f", "-x", "-q", "--", name])?;
        } else if untracked {
            // An intent-to-add entry in the index would keep the file from
            // `git clean`, and so would an ignore rule once it is out: `-x`.
            run(
                path,
                &["rm", "--cached", "-q", "--ignore-unmatch", "--", name],
            )?;
            run(path, &["clean", "-f", "-x", "-q", "--", name])?;
        } else {
            run(path, &["restore", "--worktree", "--", name])?;
        }
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
    if file.submodule {
        return Err(CoreError::new(
            "A submodule is discarded whole, not by lines",
        ));
    }
    let (old, new) = read_blobs(repo_path, file)?;
    let (old, new) = (String::from_utf8_lossy(&old), String::from_utf8_lossy(&new));
    let path = file.path();
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

/// Rewrites the message of a commit in a worktree's history, copying the
/// commits after it; staged changes stay out of them.
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
    Ok(run_with_env(path, &args, Some(message), &env)?
        .trim()
        .to_string())
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
            let [id, parents, time, author, email, summary, body] = fields[..] else {
                return None;
            };
            let parents: Vec<String> = parents.split_whitespace().map(str::to_string).collect();
            let commit = Commit {
                id: id.to_string(),
                short_id: id.chars().take(7).collect(),
                summary: summary.to_string(),
                body: body.trim().to_string(),
                author: author.to_string(),
                email: email.to_string(),
                time: time.parse().unwrap_or_default(),
                parent: parents.first().cloned(),
            };
            Some((commit, parents))
        })
        .collect()
}

/// Commits in `base..tip`, newest first; `None` reaches back to the root.
pub fn range_commits(
    repo_path: &str,
    base: Option<&str>,
    tip: &Revision,
) -> Result<Vec<Commit>, CoreError> {
    let (dir, rev) = locate(repo_path, tip);
    // A worktree with no commits yet has none to list.
    let unborn = || run(dir, &["rev-parse", "--verify", "-q", rev]).is_err();
    if matches!(tip, Revision::Worktree(_)) && unborn() {
        return Ok(Vec::new());
    }
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

/// The files a range changes, with how much, as `git diff --stat` puts it.
pub fn diff_stat(repo_path: &str, base: Option<&str>, tip: &Revision) -> Result<String, CoreError> {
    let base = match base {
        Some(base) => base.to_string(),
        None => empty_tree(repo_path)?,
    };
    let (dir, rev) = locate(repo_path, tip);
    run(dir, &["diff", "--stat=100", &base, rev])
}

/// The changes from `base` (`None` is the empty tree) to `tip`, with a line
/// of context.
pub fn diff_text(repo_path: &str, base: Option<&str>, tip: &Revision) -> Result<String, CoreError> {
    let base = match base {
        Some(base) => base.to_string(),
        None => empty_tree(repo_path)?,
    };
    let (dir, rev) = locate(repo_path, tip);
    run(dir, &["diff", "-U1", "--no-color", &base, rev])
}

/// How many files changed from `base` (`None` is the empty tree) to `tip`,
/// and how many lines came and went; a worktree's new files are left out.
pub fn diff_totals(
    repo_path: &str,
    base: Option<&str>,
    tip: &Revision,
) -> Result<(usize, u64, u64), CoreError> {
    let base = match base {
        Some(base) => base.to_string(),
        None => empty_tree(repo_path)?,
    };
    let (dir, rev) = locate(repo_path, tip);
    let mut args = vec!["diff", "--shortstat", base.as_str()];
    if let Revision::Commit(_) = tip {
        args.push(rev);
    }
    Ok(parse_shortstat(&run(dir, &args)?))
}

/// " 3 files changed, 10 insertions(+), 2 deletions(-)", in its parts.
fn parse_shortstat(text: &str) -> (usize, u64, u64) {
    let mut totals = (0, 0, 0);
    for part in text.trim().split(", ") {
        let mut words = part.split_whitespace();
        let (Some(number), Some(word)) = (words.next(), words.next()) else {
            continue;
        };
        let Ok(number) = number.parse::<u64>() else {
            continue;
        };
        match word {
            w if w.starts_with("file") => totals.0 = number as usize,
            w if w.starts_with("insertion") => totals.1 = number,
            w if w.starts_with("deletion") => totals.2 = number,
            _ => {}
        }
    }
    totals
}

/// Files changed from `base` (`None` is the empty tree) to `tip`. A worktree tip
/// includes its uncommitted and untracked files.
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
    let args = ["diff", "--raw", "-z", "--no-abbrev", "-M", &base];
    match tip {
        Revision::Worktree(path) => Ok(changed_inside(path, worktree_diff(path, &args)?).0),
        Revision::Commit(_) => Ok(submodules_first(parse_raw(
            &run(dir, &[&args[..], &[rev]].concat())?,
            None,
        ))),
    }
}

/// A worktree's untracked files as added ones, read from disk.
fn untracked_files(worktree: &str) -> Result<Vec<ChangedFile>, CoreError> {
    Ok(untracked(worktree)?
        .into_iter()
        .map(|path| ChangedFile {
            status: "A".to_string(),
            old_path: None,
            new_path: Some(path),
            old_id: NULL_ID.to_string(),
            new_id: NULL_ID.to_string(),
            worktree: Some(worktree.to_string()),
            submodule: false,
        })
        .collect())
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
        let submodule = meta[0].trim_start_matches(':') == GITLINK || meta[1] == GITLINK;
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
            submodule,
        });
    }
    files
}

pub fn read_blobs(repo_path: &str, file: &ChangedFile) -> Result<(Vec<u8>, Vec<u8>), CoreError> {
    if file.submodule {
        return Ok(submodule_blobs(repo_path, file));
    }
    let repo = open(repo_path)?;
    let old = read_blob(&repo, &file.old_id)?;
    // Git reports worktree files with a null id; their content is on disk.
    let new = match (&file.worktree, &file.new_path) {
        (Some(root), Some(path)) => fs::read(Path::new(root).join(path)).map_err(CoreError::new)?,
        _ => read_blob(&repo, &file.new_id)?,
    };
    Ok((old, new))
}

/// A submodule's change as two texts: each side's commit, and the commits
/// only it has. Its commits are looked up in the worktree's checkout of it,
/// or the repository's; the worktree's side is what is checked out there.
fn submodule_blobs(repo_path: &str, file: &ChangedFile) -> (Vec<u8>, Vec<u8>) {
    let path = file.path();
    let root = file.worktree.as_deref().unwrap_or(repo_path);
    let dir = Path::new(root).join(path);
    let id = |id: &str| (id != NULL_ID && !id.is_empty()).then(|| id.to_string());
    let old = file.old_path.as_ref().and_then(|_| id(&file.old_id));
    let (new, dirty) = match (&file.worktree, &file.new_path) {
        (Some(_), Some(_)) => match crate::submodule::checkout(&dir) {
            Some((head, dirty)) => (Some(head), dirty),
            None => (None, false),
        },
        (None, Some(_)) => (id(&file.new_id), false),
        (_, None) => (None, false),
    };
    let text = |this: Option<&String>, other: Option<&String>, dirty: bool| {
        crate::submodule::side_text(
            &dir,
            path,
            this.map(String::as_str),
            other.map(String::as_str),
            dirty,
        )
        .into_bytes()
    };
    (
        text(old.as_ref(), new.as_ref(), false),
        text(new.as_ref(), old.as_ref(), dirty),
    )
}

/// A submodule in a snapshot, as text: where its pointer is at `revision`.
/// `None` for anything else.
pub fn snapshot_submodule(repo: &str, revision: &Revision, file: &str) -> Option<String> {
    let (dir, id, dirty) = match revision {
        Revision::Worktree(root) => {
            let dir = Path::new(root).join(file);
            if !dir.is_dir() {
                return None;
            }
            let (head, dirty) = crate::submodule::checkout(&dir)?;
            (dir, head, dirty)
        }
        Revision::Commit(commit) => {
            let entry =
                crate::console::as_refresh(|| run(repo, &["ls-tree", commit, "--", file])).ok()?;
            let mut fields = entry.split_whitespace();
            if fields.next()? != GITLINK {
                return None;
            }
            let id = fields.nth(1)?.to_string();
            (Path::new(repo).join(file), id, false)
        }
    };
    Some(crate::submodule::side_text(
        &dir,
        file,
        Some(&id),
        None,
        dirty,
    ))
}

/// The mode git gives a submodule's entry.
const GITLINK: &str = "160000";

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
mod shortstat_tests {
    #[test]
    fn shortstats_read_in_their_parts() {
        let read = super::parse_shortstat;
        assert_eq!(
            read(" 3 files changed, 10 insertions(+), 2 deletions(-)\n"),
            (3, 10, 2)
        );
        assert_eq!(read(" 1 file changed, 1 deletion(-)"), (1, 0, 1));
        assert_eq!(read(""), (0, 0, 0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_config_key_is_no_failure_in_the_log() {
        let dir = std::env::temp_dir().join(format!("wtf-unset-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_str().unwrap();
        run(path, &["init", "--quiet"]).unwrap();
        let key = "wtf.nothing-here";
        assert!(run(path, &["config", "--local", "--get-all", key]).is_err());
        // Something that does fail stays a failure.
        assert!(run(path, &["config", "--local", "--bogus-option"]).is_err());
        let log = console::log_since(0);
        let entry = |text: &str| {
            log.iter()
                .rev()
                .find(|e| e.dir == path && e.command.contains(text))
                .unwrap()
        };
        assert!(entry(key).ok);
        assert!(entry(key).output.is_empty());
        assert!(!entry("--bogus-option").ok);
        std::fs::remove_dir_all(&dir).ok();
    }

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
        let raw = "aaaaaaaaa\x1fbbb ccc\x1f100\x1fAnn\x1fann@x.org\x1fMerge it\x1fWhy:\nbecause\n\x1e\nddd\x1f\x1f50\x1fBob\x1f\x1fRoot\x1f\x1e\n";
        let log = parse_log(raw);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].0.short_id, "aaaaaaa");
        assert_eq!(log[0].0.parent.as_deref(), Some("bbb"));
        assert_eq!(log[0].1, vec!["bbb", "ccc"]);
        assert_eq!(log[0].0.body, "Why:\nbecause");
        assert_eq!(log[0].0.email, "ann@x.org");
        assert_eq!(log[1].0.parent, None);
        assert_eq!(log[1].0.body, "");
        assert_eq!(log[1].0.time, 50);
    }
}
