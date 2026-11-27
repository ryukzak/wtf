//! Cherry-picks worked out in git's object store, without touching any
//! files: whether commits apply somewhere, and applying them onto a branch
//! no worktree has checked out.

use crate::{CoreError, git};
use std::path::Path;

/// What applying commits, oldest first, onto a commit would do.
#[derive(Debug, Clone, PartialEq)]
pub enum Forecast {
    /// Every commit applies; `empty` of them change nothing there, as when
    /// they were applied before, and are left out.
    Clean { empty: usize },
    /// The commit at `step`, from 0, conflicts with what is there in `files`.
    Conflicts { step: usize, files: Vec<String> },
}

impl Forecast {
    /// A few words for a menu: "clean", "1 conflict", "already there".
    pub fn hint(&self, picked: usize) -> String {
        match self {
            Forecast::Clean { empty } if *empty == picked => "already there".into(),
            Forecast::Clean { .. } => "clean".into(),
            Forecast::Conflicts { files, .. } => match files.len() {
                1 => "1 conflict".into(),
                n => format!("{n} conflicts"),
            },
        }
    }
}

/// Applies `commits`, oldest first, onto `onto` as cherry-pick would, a merge
/// against its first parent: the new tip, or where it stops. Only objects
/// are written; no ref, index or file changes.
fn apply(repo: &str, onto: &str, commits: &[String]) -> Result<(String, Forecast), CoreError> {
    let mut tip = git::run(
        repo,
        &["rev-parse", "--verify", &format!("{onto}^{{commit}}")],
    )?
    .trim()
    .to_string();
    let mut empty = 0;
    for (step, commit) in commits.iter().enumerate() {
        let base = format!("--merge-base={commit}^1");
        let (clean, out) = git::run_answer(
            repo,
            &[
                "merge-tree",
                "--write-tree",
                "--name-only",
                "--no-messages",
                &base,
                &tip,
                commit,
            ],
            None,
            &[],
        )?;
        let mut lines = out.lines();
        let tree = lines.next().unwrap_or_default().trim().to_string();
        if !clean {
            let mut files: Vec<String> = lines
                .take_while(|line| !line.is_empty())
                .map(str::to_string)
                .collect();
            files.dedup();
            return Ok((tip, Forecast::Conflicts { step, files }));
        }
        let before = git::run(repo, &["rev-parse", &format!("{tip}^{{tree}}")])?;
        if before.trim() == tree {
            empty += 1;
            continue;
        }
        tip = git::copy_commit_onto(repo, commit, &tree, &[&tip])?;
    }
    Ok((tip, Forecast::Clean { empty }))
}

/// Whether `commits`, oldest first, would apply onto `onto`, a branch or a
/// commit.
pub fn pick_forecast(repo: &str, onto: &str, commits: &[String]) -> Result<Forecast, CoreError> {
    apply(repo, onto, commits).map(|(_, forecast)| forecast)
}

/// Cherry-picks `commits`, oldest first, onto `branch` without checking it
/// out: only when all apply cleanly, else nothing changes. Returns what it
/// found either way.
pub fn pick_onto_branch(
    repo: &str,
    branch: &str,
    commits: &[String],
) -> Result<Forecast, CoreError> {
    let reference = format!("refs/heads/{branch}");
    let old = git::run(repo, &["rev-parse", "--verify", &reference])?
        .trim()
        .to_string();
    let (tip, forecast) = apply(repo, &old, commits)?;
    if matches!(forecast, Forecast::Clean { .. }) && tip != old {
        git::run(
            repo,
            &[
                "update-ref",
                "-m",
                "wtf: cherry-pick",
                &reference,
                &tip,
                &old,
            ],
        )?;
    }
    Ok(forecast)
}

/// Whether `rev`'s history holds `commit`.
pub fn has_commit(repo: &str, rev: &str, commit: &str) -> bool {
    git::run_answer(
        repo,
        &["merge-base", "--is-ancestor", commit, rev],
        None,
        &[],
    )
    .is_ok_and(|(yes, _)| yes)
}

/// Whether any of `commits` is a merge, which cherry-pick takes with `-m 1`.
pub fn has_merges(repo: &str, commits: &[String]) -> bool {
    let mut args = vec!["rev-list", "--no-walk", "--min-parents=2"];
    args.extend(commits.iter().map(String::as_str));
    git::run(repo, &args).is_ok_and(|out| !out.trim().is_empty())
}

/// Whether a cherry-pick in the worktree at `path` stopped and waits: on a
/// conflict, or for a commit it couldn't make.
pub fn picking(path: &str) -> bool {
    git::run(path, &["rev-parse", "--git-path", "CHERRY_PICK_HEAD"])
        .is_ok_and(|found| Path::new(path).join(found.trim()).exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Repo(std::path::PathBuf);

    impl Repo {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("wtf-pick-{name}-{}", std::process::id()));
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
            self.git(&["commit", "--quiet", "-m", &format!("{file}: {text}")]);
            self.git(&["rev-parse", "HEAD"])
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn picks_onto_a_branch_without_checking_it_out() {
        let repo = Repo::new("onto");
        repo.commit("a.txt", "one\n");
        repo.git(&["branch", "release"]);
        let b = repo.commit("b.txt", "two\n");
        let c = repo.commit("c.txt", "three\n");
        let picks = vec![b.clone(), c.clone()];
        assert_eq!(
            pick_forecast(repo.path(), "release", &picks).unwrap(),
            Forecast::Clean { empty: 0 }
        );
        let head = repo.git(&["rev-parse", "HEAD"]);
        assert_eq!(
            pick_onto_branch(repo.path(), "release", &picks).unwrap(),
            Forecast::Clean { empty: 0 }
        );
        // The worktree and its branch stay as they were.
        assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
        assert_eq!(repo.git(&["status", "--porcelain"]), "");
        let log = repo.git(&["log", "--format=%s", "release"]);
        assert_eq!(log, "c.txt: three\nb.txt: two\na.txt: one");
        assert_eq!(repo.git(&["log", "-1", "--format=%an", "release"]), "T");
        // Picked again, they change nothing: already there.
        let again = pick_forecast(repo.path(), "release", &picks).unwrap();
        assert_eq!(again.hint(2), "already there");
    }

    #[test]
    fn a_conflict_changes_nothing_and_names_the_files() {
        let repo = Repo::new("conflict");
        repo.commit("a.txt", "one\n");
        repo.git(&["checkout", "--quiet", "-b", "release"]);
        let release = repo.commit("a.txt", "release\n");
        repo.git(&["checkout", "--quiet", "main"]);
        let b = repo.commit("b.txt", "two\n");
        let fix = repo.commit("a.txt", "fix\n");
        let forecast = pick_onto_branch(repo.path(), "release", &[b, fix]).unwrap();
        assert_eq!(
            forecast,
            Forecast::Conflicts {
                step: 1,
                files: vec!["a.txt".into()]
            }
        );
        assert_eq!(forecast.hint(2), "1 conflict");
        assert_eq!(repo.git(&["rev-parse", "release"]), release);
    }

    #[test]
    fn knows_a_stopped_pick_and_where_commits_are() {
        let repo = Repo::new("stopped");
        let base = repo.commit("a.txt", "one\n");
        repo.git(&["checkout", "--quiet", "-b", "side"]);
        let side = repo.commit("a.txt", "side\n");
        repo.git(&["checkout", "--quiet", "main"]);
        repo.commit("a.txt", "main\n");
        assert!(has_commit(repo.path(), "main", &base));
        assert!(!has_commit(repo.path(), "main", &side));
        assert!(!picking(repo.path()));
        assert!(git::run(repo.path(), &["cherry-pick", &side]).is_err());
        assert!(picking(repo.path()));
        assert!(!has_merges(repo.path(), &[side]));
    }
}
