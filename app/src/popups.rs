/// What a right-click menu acts on.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Branch {
        name: String,
        commit: String,
        /// When nothing but the main branch grows from it: whether that has
        /// merged it.
        leaf: Option<bool>,
    },
    Worktree {
        path: String,
        /// `None` when the worktree's HEAD is detached.
        branch: Option<String>,
        commit: String,
        /// When nothing but the main branch grows from its branch, both can
        /// go together: whether the main branch has merged it.
        leaf: Option<bool>,
    },
    /// A commit in the commit list.
    Commit {
        commit: String,
        message: String,
    },
    /// A remote-tracking branch, such as `origin/fix`.
    Remote {
        remote: String,
        branch: String,
        commit: String,
    },
    Tag {
        name: String,
        commit: String,
    },
    /// A stash, by its name such as `stash@{0}` and its commit.
    Stash {
        name: String,
        id: String,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    CopyName,
    CopyHash,
    CopyShortHash,
    CopyMessage,
    CopyPath,
    RevealInFinder,
    /// Opens with one of the recent commands.
    OpenWithRecent(usize),
    OpenWith,
    Rename,
    Delete,
    DeleteWithCommits,
    /// A worktree for a branch, or for a new branch at a commit.
    NewWorktree,
    /// A new branch at a commit.
    NewBranch,
    /// Checks a branch out in one of the open worktrees.
    CheckoutIn(usize),
    /// Applies a stash in one of the worktrees, keeping it, or popping it.
    StashApply(usize),
    StashPop(usize),
    /// A new worktree on the stash's commit, with the stash applied.
    StashWorktree,
    DropStash,
    RemoveWorktree,
    RemoveWorktreeAndBranch,
    DeleteRemote,
    DeleteTag,
    /// Hides the branch from attention.
    Hide,
    /// Pins a branch attention leaves out.
    Pin,
    /// Drops a pin or hide, back to the filters.
    FollowFilters,
}

/// The name attention knows a target by: its branch, or a detached
/// worktree's folder.
pub fn attention_name(target: &Target) -> String {
    match target {
        Target::Branch { name, .. } => name.clone(),
        Target::Worktree {
            branch: Some(branch),
            ..
        } => branch.clone(),
        Target::Worktree { path, .. } => std::path::Path::new(path)
            .file_name()
            .map_or(path.clone(), |name| name.to_string_lossy().into_owned()),
        Target::Commit { commit, .. } => commit.clone(),
        Target::Remote { remote, branch, .. } => format!("{remote}/{branch}"),
        Target::Tag { name, .. } | Target::Stash { name, .. } => name.clone(),
    }
}

/// How many Open With commands the menu remembers.
pub const RECENT_COMMANDS: usize = 3;

const SHORT_HASH: usize = 7;

/// The menu for a target, `None` marking separators. Copy items show what
/// they copy; `recent` are the last Open With commands. The main branch and
/// the main worktree cannot be renamed, deleted or removed.
pub fn commands(
    target: &Target,
    main: &str,
    recent: &[String],
    shown: bool,
    overridden: bool,
    worktrees: &[String],
) -> Vec<Option<(Command, String)>> {
    let item = |command, label: String| Some((command, label));
    // Labels other than local branches and worktrees have menus of their own.
    let short = |commit: &str| commit.chars().take(SHORT_HASH).collect::<String>();
    match target {
        Target::Remote {
            remote,
            branch,
            commit,
        } => {
            return vec![
                item(
                    Command::CopyName,
                    format!("Copy Branch Name — {remote}/{branch}"),
                ),
                item(Command::CopyHash, format!("Copy SHA — {}…", short(commit))),
                None,
                attention_item(shown, overridden),
                None,
                item(
                    Command::DeleteRemote,
                    format!("Delete {branch} on {remote}…"),
                ),
            ];
        }
        Target::Tag { name, .. } => {
            return vec![
                item(Command::CopyName, format!("Copy Tag Name — {name}")),
                None,
                item(Command::DeleteTag, "Delete Tag…".to_string()),
            ];
        }
        // A deleted branch can come back where it ended.
        // A stash goes into a worktree, or one of its own, or away.
        Target::Stash { name, id, .. } => {
            let mut items = vec![
                item(Command::CopyName, format!("Copy Stash Name — {name}")),
                item(Command::CopyHash, format!("Copy SHA — {}…", short(id))),
                None,
            ];
            items.extend(
                worktrees
                    .iter()
                    .enumerate()
                    .map(|(k, label)| item(Command::StashApply(k), format!("Apply to {label}"))),
            );
            items.extend(
                worktrees
                    .iter()
                    .enumerate()
                    .map(|(k, label)| item(Command::StashPop(k), format!("Pop into {label}"))),
            );
            items.extend([
                item(
                    Command::StashWorktree,
                    "New Worktree from Stash…".to_string(),
                ),
                None,
                item(Command::DropStash, "Drop Stash…".to_string()),
            ]);
            return items;
        }
        _ => {}
    }
    let (name, commit) = match target {
        Target::Branch { name, commit, .. } => (Some(name), commit),
        Target::Worktree { branch, commit, .. } => (branch.as_ref(), commit),
        Target::Commit { commit, .. } => (None, commit),
        _ => return Vec::new(),
    };
    let mut items = Vec::new();
    if let Some(name) = name {
        items.push(item(
            Command::CopyName,
            format!("Copy Branch Name — {name}"),
        ));
    }
    if !commit.is_empty() {
        let short: String = commit.chars().take(SHORT_HASH).collect();
        let long: String = commit.chars().take(12).collect();
        items.push(item(Command::CopyHash, format!("Copy SHA — {long}…")));
        items.push(item(
            Command::CopyShortHash,
            format!("Copy Short SHA — {short}"),
        ));
    }
    if let Target::Commit { .. } = target {
        items.extend([
            item(Command::CopyMessage, "Copy Message".to_string()),
            None,
            item(Command::NewBranch, "New Branch Here…".to_string()),
            item(Command::NewWorktree, "New Worktree Here…".to_string()),
        ]);
        return items;
    }
    // A branch opens in a worktree of its own, or in one already open; the
    // main branch stays in the main worktree.
    if let Target::Branch { name, .. } = target
        && name != main
    {
        items.push(None);
        items.push(item(
            Command::NewWorktree,
            "New Worktree for Branch…".to_string(),
        ));
        items.extend(
            worktrees
                .iter()
                .enumerate()
                .map(|(k, label)| item(Command::CheckoutIn(k), format!("Check Out in {label}"))),
        );
    }
    if attention_name(target) != main {
        items.push(None);
        items.push(attention_item(shown, overridden));
    }
    match target {
        Target::Branch { name, leaf, .. } => {
            if name != main {
                // An unmerged branch nothing else grows from goes with its
                // commits; others only as git allows.
                let delete = match leaf {
                    Some(false) => item(
                        Command::DeleteWithCommits,
                        "Delete Branch and Its Commits…".to_string(),
                    ),
                    _ => item(Command::Delete, "Delete Branch…".to_string()),
                };
                items.extend([
                    None,
                    item(Command::Rename, "Rename Branch…".to_string()),
                    delete,
                ]);
            }
        }
        Target::Commit { .. }
        | Target::Remote { .. }
        | Target::Tag { .. }
        | Target::Stash { .. } => {}
        Target::Worktree {
            path, branch, leaf, ..
        } => {
            items.push(item(
                Command::CopyPath,
                format!("Copy Path — {}", tilde(path)),
            ));
            items.push(None);
            items.push(item(
                Command::RevealInFinder,
                "Reveal in Finder".to_string(),
            ));
            for (k, command) in recent.iter().take(RECENT_COMMANDS).enumerate() {
                items.push(item(
                    Command::OpenWithRecent(k),
                    format!("Open with {command}"),
                ));
            }
            items.push(item(Command::OpenWith, "Open With…".to_string()));
            if branch.as_deref() != Some(main) {
                items.push(None);
                if branch.is_some() {
                    items.push(item(Command::Rename, "Rename Branch…".to_string()));
                }
                // Removing names the worktree it removes.
                let folder = std::path::Path::new(path)
                    .file_name()
                    .map_or(path.clone(), |name| name.to_string_lossy().into_owned());
                items.push(item(
                    Command::RemoveWorktree,
                    format!("Remove Worktree — {folder}…"),
                ));
                if let (Some(_), Some(branch)) = (leaf, branch) {
                    items.push(item(
                        Command::RemoveWorktreeAndBranch,
                        format!("Remove Worktree {folder} and Branch {branch}…"),
                    ));
                }
            }
        }
    }
    items
}

/// What the menu offers about attention: a branch shown or left out by hand
/// goes back to the filters; otherwise it can be hidden or shown.
fn attention_item(shown: bool, overridden: bool) -> Option<(Command, String)> {
    let (command, label) = match (overridden, shown) {
        (true, _) => (Command::FollowFilters, "Follow Attention Filters"),
        (false, true) => (Command::Hide, "Hide from Attention"),
        (false, false) => (Command::Pin, "Show in Attention"),
    };
    Some((command, label.to_string()))
}

/// Remembers a command first among the recent ones, without repeats.
pub fn remember(recent: &mut Vec<String>, command: &str) {
    recent.retain(|known| known != command);
    recent.insert(0, command.to_string());
    recent.truncate(RECENT_COMMANDS);
}

fn recent_file(name: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home)
        .join("Library/Application Support/wtf")
        .join(name)
}

/// Recent commands saved in the file `name`, one per line.
pub fn load_recent(name: &str) -> Vec<String> {
    let text = std::fs::read_to_string(recent_file(name)).unwrap_or_default();
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(RECENT_COMMANDS)
        .map(str::to_string)
        .collect()
}

/// Saves the recent commands; a failure only forgets them.
pub fn save_recent(name: &str, recent: &[String]) {
    let file = recent_file(name);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, recent.join("\n"));
}

/// A path with the home directory as `~`.
pub fn tilde(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match path.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// A word quoted for the shell when it needs it.
pub fn quote(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_-./=:@%+,".contains(c);
    if !word.is_empty() && word.chars().all(safe) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

pub fn rename_branch(repo: &str, old: &str, new: &str) -> String {
    format!(
        "git -C {} branch -m {} {}",
        quote(repo),
        quote(old),
        quote(new)
    )
}

pub fn delete_branch(repo: &str, name: &str) -> String {
    format!("git -C {} branch -d {}", quote(repo), quote(name))
}

/// Where a new worktree for `branch` goes: next to the repository, named
/// after both.
pub fn worktree_path(repo: &str, branch: &str) -> String {
    let repo = std::path::Path::new(repo);
    let name = repo
        .file_name()
        .map_or(String::new(), |name| name.to_string_lossy().into_owned());
    let parent = repo.parent().unwrap_or(repo);
    parent
        .join(format!("{name}-{}", branch.replace('/', "-")))
        .to_string_lossy()
        .into_owned()
}

pub fn add_worktree(repo: &str, path: &str, branch: &str) -> String {
    format!(
        "git -C {} worktree add {} {}",
        quote(repo),
        quote(path),
        quote(branch)
    )
}

/// A worktree on a new branch starting at `commit`.
pub fn add_worktree_at(repo: &str, path: &str, branch: &str, commit: &str) -> String {
    format!(
        "git -C {} worktree add -b {} {} {}",
        quote(repo),
        quote(branch),
        quote(path),
        quote(commit)
    )
}

pub fn new_branch(repo: &str, name: &str, commit: &str) -> String {
    format!(
        "git -C {} branch {} {}",
        quote(repo),
        quote(name),
        quote(commit)
    )
}

/// Checks a branch out in a worktree; git refuses when changes are in the way
/// or another worktree has it.
pub fn switch(worktree: &str, branch: &str) -> String {
    format!("git -C {} switch {}", quote(worktree), quote(branch))
}

pub fn stash_apply(worktree: &str, stash: &str, pop: bool) -> String {
    let verb = if pop { "pop" } else { "apply" };
    format!("git -C {} stash {verb} {}", quote(worktree), quote(stash))
}

pub fn stash_drop(repo: &str, stash: &str) -> String {
    format!("git -C {} stash drop {}", quote(repo), quote(stash))
}

/// A worktree on a new branch at the stash's commit, with the stash applied.
pub fn stash_worktree(repo: &str, path: &str, branch: &str, base: &str, stash: &str) -> String {
    format!(
        "{} && git -C {} stash apply {}",
        add_worktree_at(repo, path, branch, base),
        quote(path),
        quote(stash)
    )
}

/// Stashes a worktree's changes, untracked files too: all, or some paths.
pub fn stash_push(worktree: &str, message: &str, paths: &[String]) -> String {
    let mut line = format!("git -C {} stash push -u", quote(worktree));
    if !message.trim().is_empty() {
        line.push_str(&format!(
            " -m {}",
            quote(message.lines().next().unwrap_or_default())
        ));
    }
    if !paths.is_empty() {
        line.push_str(" --");
        for path in paths {
            line.push_str(&format!(" {}", quote(path)));
        }
    }
    line
}

pub fn delete_branch_with_commits(repo: &str, name: &str) -> String {
    format!("git -C {} branch -D {}", quote(repo), quote(name))
}

pub fn delete_tag(repo: &str, name: &str) -> String {
    format!("git -C {} tag -d {}", quote(repo), quote(name))
}

/// Deletes a branch on its remote, which changes the shared repository.
pub fn delete_remote_branch(repo: &str, remote: &str, branch: &str) -> String {
    format!(
        "git -C {} push {} --delete {}",
        quote(repo),
        quote(remote),
        quote(branch)
    )
}

/// Removes a worktree, then deletes its branch: a merged one as git checks
/// it, an unmerged one with its commits. The branch stays when the worktree
/// cannot go.
pub fn remove_worktree_and_branch(repo: &str, path: &str, branch: &str, merged: bool) -> String {
    format!(
        "{} && git -C {} branch {} {}",
        remove_worktree(repo, path),
        quote(repo),
        if merged { "-d" } else { "-D" },
        quote(branch)
    )
}

pub fn remove_worktree(repo: &str, path: &str) -> String {
    format!("git -C {} worktree remove {}", quote(repo), quote(path))
}

/// The user's command with the worktree path appended.
pub fn open_with(command: &str, path: &str) -> String {
    format!("{} {}", command.trim(), quote(path))
}

/// The user's command run in the worktree's folder.
pub fn run_in(command: &str, path: &str) -> String {
    format!("cd {} && {}", quote(path), command.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_branches_and_tags_have_their_own_menus() {
        let remote = Target::Remote {
            remote: "origin".to_string(),
            branch: "fix".to_string(),
            commit: "abcdef1234567".to_string(),
        };
        assert_eq!(
            labels(&remote).last().map(String::as_str),
            Some("Delete fix on origin…")
        );
        let tag = Target::Tag {
            name: "v1".to_string(),
            commit: String::new(),
        };
        assert_eq!(labels(&tag), ["Copy Tag Name — v1", "—", "Delete Tag…"]);
        assert_eq!(
            delete_remote_branch("/r", "origin", "fix"),
            "git -C /r push origin --delete fix"
        );
    }

    fn labels(target: &Target) -> Vec<String> {
        commands(target, "master", &[], true, false, &[])
            .into_iter()
            .map(|item| item.map_or("—".to_string(), |(_, label)| label))
            .collect()
    }

    fn has(labels: &[String], prefix: &str) -> bool {
        labels.iter().any(|label| label.starts_with(prefix))
    }

    #[test]
    fn main_branch_and_worktree_keep_only_safe_actions() {
        let main = Target::Branch {
            name: "master".to_string(),
            commit: "abc".to_string(),
            leaf: None,
        };
        assert_eq!(
            labels(&main),
            [
                "Copy Branch Name — master",
                "Copy SHA — abc…",
                "Copy Short SHA — abc"
            ]
        );
        let feature = Target::Branch {
            name: "feature".to_string(),
            commit: "abc".to_string(),
            leaf: None,
        };
        assert!(has(&labels(&feature), "Hide from Attention"));
        let unmerged = Target::Branch {
            name: "try".to_string(),
            commit: "abc".to_string(),
            leaf: Some(false),
        };
        assert!(has(&labels(&unmerged), "Delete Branch and Its Commits"));
        let left_out: Vec<String> = commands(&feature, "master", &[], false, false, &[])
            .into_iter()
            .flatten()
            .map(|(_, label)| label)
            .collect();
        assert!(left_out.contains(&"Show in Attention".to_string()));

        let main_worktree = Target::Worktree {
            path: "/repo".to_string(),
            branch: Some("master".to_string()),
            commit: "abc".to_string(),
            leaf: None,
        };
        assert!(!has(&labels(&main_worktree), "Remove Worktree"));

        let leaf = Target::Worktree {
            path: "/repo/wt".to_string(),
            branch: Some("fix".to_string()),
            commit: "abc".to_string(),
            leaf: Some(true),
        };
        let leaf_labels = labels(&leaf);
        assert!(has(&leaf_labels, "Remove Worktree — wt…"));
        assert!(has(&leaf_labels, "Remove Worktree wt and Branch fix…"));
        assert_eq!(
            remove_worktree_and_branch("/repo", "/repo/wt", "fix", true),
            "git -C /repo worktree remove /repo/wt && git -C /repo branch -d fix"
        );
    }

    #[test]
    fn detached_worktree_has_no_branch_actions() {
        let detached = Target::Worktree {
            path: "/repo/wt".to_string(),
            branch: None,
            commit: "abc".to_string(),
            leaf: Some(false),
        };
        let labels = labels(&detached);
        assert!(!has(&labels, "Rename Branch"));
        assert!(!has(&labels, "Remove Worktree wt and Branch"));
        assert!(!has(&labels, "Copy Branch Name"));
        assert!(has(&labels, "Remove Worktree"));
        assert!(has(&labels, "Copy Path — /repo/wt"));
    }

    #[test]
    fn branches_and_commits_open_in_worktrees() {
        let branch = Target::Branch {
            name: "fix".to_string(),
            commit: "abc".to_string(),
            leaf: None,
        };
        let labels: Vec<String> = commands(
            &branch,
            "master",
            &[],
            true,
            false,
            &["wtf · master".to_string()],
        )
        .into_iter()
        .flatten()
        .map(|(_, label)| label)
        .collect();
        assert!(labels.contains(&"New Worktree for Branch…".to_string()));
        assert!(labels.contains(&"Check Out in wtf · master".to_string()));
        assert_eq!(worktree_path("/src/wtf", "agent/fix"), "/src/wtf-agent-fix");
        assert_eq!(
            add_worktree_at("/src/wtf", "/src/wtf-x", "x", "abc"),
            "git -C /src/wtf worktree add -b x /src/wtf-x abc"
        );
    }

    #[test]
    fn commands_quote_what_the_shell_needs() {
        assert_eq!(
            rename_branch("/repo", "fix", "fix it"),
            "git -C /repo branch -m fix 'fix it'"
        );
        assert_eq!(open_with("zed -n ", "/my repo/wt"), "zed -n '/my repo/wt'");
        assert_eq!(
            run_in(" zed . ", "/my repo/wt"),
            "cd '/my repo/wt' && zed ."
        );
        assert_eq!(quote("/it's"), r"'/it'\''s'");
    }

    #[test]
    fn recent_commands_come_first_without_repeats() {
        let mut recent = Vec::new();
        for command in ["zed -n", "code", "cursor", "zed -n", "subl"] {
            remember(&mut recent, command);
        }
        assert_eq!(recent, ["subl", "zed -n", "cursor"]);

        let worktree = Target::Worktree {
            path: "/repo/wt".to_string(),
            branch: None,
            commit: String::new(),
            leaf: None,
        };
        let labels: Vec<String> = commands(&worktree, "master", &recent, true, false, &[])
            .into_iter()
            .flatten()
            .map(|(_, label)| label)
            .collect();
        assert!(labels.contains(&"Open with subl".to_string()));
        assert!(!labels.iter().any(|label| label.contains("Terminal")));
    }
}
