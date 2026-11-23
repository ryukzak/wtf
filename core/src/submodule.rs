//! Submodules: a change to one is its pointer moving from one commit to
//! another. Shown as text a diff can show: the commits it gained, those it
//! lost, and changes left inside.

use std::path::Path;

use crate::git;

/// How many commits either way a pointer change lists.
const MAX_COMMITS: usize = 100;

/// The text a side of a submodule's change reads as: where its pointer is,
/// and the commits only that side has. `dir` is the submodule's checkout,
/// where its commits are looked up; `id` its commit, `None` when not there;
/// `other` the other side's commit. With `dirty`, the checkout is the
/// worktree's and its own changes are said too.
pub fn side_text(
    dir: &Path,
    path: &str,
    id: Option<&str>,
    other: Option<&str>,
    dirty: bool,
) -> String {
    let Some(id) = id else {
        return String::new();
    };
    let dir = dir.to_string_lossy();
    let quiet = |args: &[&str]| crate::console::as_refresh(|| git::run(&dir, args)).ok();
    let name = quiet(&["describe", "--tags", "--always", id])
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty() && !id.starts_with(name.as_str()));
    let short: String = id.chars().take(7).collect();
    let mut text = format!("Submodule {path}\n");
    text.push_str(&match name {
        Some(name) => format!("at {short} ({name})\n"),
        None => format!("at {short}\n"),
    });
    if let Some(other) = other.filter(|other| *other != id) {
        let range = format!("{other}..{id}");
        let count = format!("--max-count={MAX_COMMITS}");
        match quiet(&["log", "--format=%h %s", &count, &range]) {
            Some(log) => {
                for line in log.lines() {
                    text.push_str(&format!("  {line}\n"));
                }
            }
            None => text.push_str(
                "  (its commits are not in the submodule's repository: fetch there to see them)\n",
            ),
        }
    }
    if dirty {
        text.push_str("\n⊙ uncommitted changes inside\n");
    }
    text
}

/// The commit a worktree's submodule has checked out, and whether it has
/// changes of its own; `None` when it is not checked out.
pub fn checkout(dir: &Path) -> Option<(String, bool)> {
    let dir = dir.to_string_lossy();
    let head = crate::console::as_refresh(|| git::run(&dir, &["rev-parse", "HEAD"])).ok()?;
    let status = crate::console::as_refresh(|| {
        git::run(
            &dir,
            &["status", "--porcelain", "--ignore-submodules=dirty"],
        )
    })
    .unwrap_or_default();
    Some((head.trim().to_string(), !status.trim().is_empty()))
}

/// The submodules' paths in `.gitmodules` text.
pub fn paths(gitmodules: &str) -> Vec<String> {
    gitmodules
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "path").then(|| value.trim().to_string())
        })
        .collect()
}
