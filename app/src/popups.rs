use crate::workspace::folder_name;

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
        /// Done with: clean, its branch merged or gone from the remote.
        stale: bool,
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
    },
    /// A group of the attention list: local branches, or remote ones.
    Group {
        remote: bool,
    },
    /// A changed file: its path in the repository, and where it is on
    /// disk in the worktree, if it is there.
    File {
        name: String,
        path: String,
        exists: bool,
    },
    /// The facts beside a commit's message, each by its name, to copy.
    Facts {
        facts: Vec<(String, String)>,
    },
    /// Several commits of the commit list, or a section of it, oldest first,
    /// each with its message.
    Commits {
        commits: Vec<(String, String)>,
        /// What the menu's first line says of them, such as "4 commits · fix".
        title: String,
        /// A section: its newest commit can be checked out and branched from.
        section: bool,
    },
    /// The branches no worktree has, to cherry-pick `commits`, with their
    /// messages, onto.
    PickBranches {
        commits: Vec<(String, String)>,
    },
}

impl Target {
    /// The commits a cherry-pick from this menu takes, oldest first.
    pub fn picks(&self) -> Vec<String> {
        self.picked().into_iter().map(|(id, _)| id).collect()
    }

    /// Those commits with their messages.
    pub fn picked(&self) -> Vec<(String, String)> {
        match self {
            Target::Commit { commit, message } => vec![(commit.clone(), message.clone())],
            Target::Commits { commits, .. } | Target::PickBranches { commits } => commits.clone(),
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    CopyName,
    CopyHash,
    CopyShortHash,
    CopyMessage,
    CopyPath,
    /// A file's path in the repository, against `CopyPath`'s full one.
    CopyRelativePath,
    /// Opens a file in its default app.
    Open,
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
    /// Selects the worktree that has the branch already.
    SelectWorktree(usize),
    /// Applies a stash in one of the worktrees, keeping it, or popping it.
    StashApply(usize),
    StashPop(usize),
    /// A new worktree on the stash's commit, with the stash applied.
    StashWorktree,
    DropStash,
    /// Stashes a worktree's changes, untracked files too.
    StashChanges,
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
    /// Adds the branch's name as a channel, or takes it away.
    ToggleChannel,
    /// One of the facts of a `Target::Facts`.
    CopyFact(usize),
    /// Cherry-picks the menu's commits in one of the worktrees.
    CherryPick(usize),
    /// Offers the branches no worktree has, to cherry-pick onto.
    CherryPickInto,
    /// Cherry-picks onto one of those branches, without checking it out.
    CherryPickOnto(usize),
    /// The menu's commits' full hashes, or messages, one a line.
    CopyHashes,
    CopyMessages,
    /// The menu's title line, which does nothing.
    Title,
    /// Changes to the history the menu's commits are in.
    Reword,
    Edit,
    Squash,
    Fixup,
    MoveUp,
    MoveDown,
    DropCommits,
    /// Moves the current worktree's branch to start from this commit or
    /// branch.
    RebaseHere,
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
        Target::Worktree { path, .. } => folder_name(path),
        Target::Commit { commit, .. } => commit.clone(),
        Target::Remote { remote, branch, .. } => format!("{remote}/{branch}"),
        Target::Tag { name, .. } | Target::Stash { name, .. } => name.clone(),
        Target::Group { .. }
        | Target::File { .. }
        | Target::Facts { .. }
        | Target::Commits { .. }
        | Target::PickBranches { .. } => String::new(),
    }
}

/// How many Open With commands the menu remembers.
pub const RECENT_COMMANDS: usize = 3;

/// How much of a commit id names it short, as git's `%h` does.
const SHORT_HASH: usize = 7;

pub fn short_hash(id: &str) -> String {
    id.chars().take(SHORT_HASH).collect()
}

/// A worktree a menu sends a command to.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    /// Its folder's name.
    pub name: String,
    /// The current worktree, which commands go to first.
    pub current: bool,
    /// It has the menu's branch checked out, so git switches nothing there.
    pub has: bool,
}

impl Place {
    pub fn new(name: &str, current: bool, has: bool) -> Self {
        Self {
            name: name.to_string(),
            current,
            has,
        }
    }
}

/// A menu item's label: a short name, and the worktree it acts in, shown
/// on the right as shortcuts are.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub text: String,
    pub place: Option<Place>,
    /// It deletes something.
    pub danger: bool,
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.place {
            Some(place) => write!(f, "{} · {}", self.text, place.name),
            None => f.write_str(&self.text),
        }
    }
}

pub type Item = Option<(Command, Label)>;

fn text(command: Command, text: impl Into<String>) -> (Command, Label) {
    at(command, text, None)
}

fn at(command: Command, text: impl Into<String>, place: Option<&Place>) -> (Command, Label) {
    let label = Label {
        text: text.into(),
        place: place.cloned(),
        danger: false,
    };
    (command, label)
}

fn danger(command: Command, text: impl Into<String>) -> (Command, Label) {
    let (command, mut label) = at(command, text, None);
    label.danger = true;
    (command, label)
}

/// Groups joined by separators, the empty ones left out.
fn grouped(groups: Vec<Vec<(Command, Label)>>) -> Vec<Item> {
    let mut items = Vec::new();
    for group in groups.into_iter().filter(|group| !group.is_empty()) {
        if !items.is_empty() {
            items.push(None);
        }
        items.extend(group.into_iter().map(Some));
    }
    items
}

/// The menu for a target, `None` marking separators: what it does first,
/// into each worktree in `places`, the current one first; then copies;
/// then what deletes. `recent` are the last Open With commands. The main
/// branch and the main worktree, at `main_worktree`, cannot be renamed,
/// deleted or removed. `picks` are where commits can be cherry-picked:
/// worktrees, or branches for the branch menu.
#[allow(clippy::too_many_arguments)]
pub fn commands(
    target: &Target,
    main: &str,
    main_worktree: Option<&str>,
    recent: &[String],
    shown: bool,
    overridden: bool,
    places: &[Place],
    picks: &[Place],
) -> Vec<Item> {
    let copies = |name: bool, commit: &str| {
        let mut items = Vec::new();
        if name {
            items.push(text(Command::CopyName, "Copy Name"));
        }
        if !commit.is_empty() {
            items.push(text(Command::CopyHash, "Copy SHA"));
            items.push(text(Command::CopyShortHash, "Copy Short SHA"));
        }
        items
    };
    let attention = |name: &str| match name == main {
        true => Vec::new(),
        false => vec![attention_item(shown, overridden)],
    };
    // Into each worktree; one with the branch already, where git would
    // refuse, is gone to instead.
    let switches = |branch: bool| {
        places
            .iter()
            .enumerate()
            .map(|(k, place)| match place.has && branch {
                true => at(Command::SelectWorktree(k), "Go To", Some(place)),
                false => at(Command::CheckoutIn(k), "Checkout", Some(place)),
            })
            .collect::<Vec<_>>()
    };
    // The current worktree's branch moved to start here; not where it is
    // already.
    let rebase_here = || {
        places
            .iter()
            .find(|place| place.current && !place.has)
            .map(|place| at(Command::RebaseHere, "Rebase onto This", Some(place)))
    };
    // Into each worktree, then onto a branch none has.
    let cherry_picks = |count: usize| {
        let verb = match count {
            1 => "Cherry-pick".to_string(),
            n => format!("Cherry-pick {n}"),
        };
        let mut items: Vec<_> = picks
            .iter()
            .enumerate()
            .map(|(k, place)| at(Command::CherryPick(k), verb.clone(), Some(place)))
            .collect();
        items.push(text(
            Command::CherryPickInto,
            format!("{verb} into Branch…"),
        ));
        items
    };
    match target {
        Target::Commit { commit, .. } => {
            let mut actions = switches(false);
            actions.push(text(Command::NewBranch, "New Branch…"));
            actions.push(text(Command::NewWorktree, "New Worktree…"));
            let mut copied = copies(false, commit);
            copied.push(text(Command::CopyMessage, "Copy Message"));
            let mut history = vec![
                text(Command::Reword, "Reword…"),
                text(Command::Edit, "Edit…"),
                text(Command::Squash, "Squash into Previous"),
                text(Command::Fixup, "Fixup into Previous"),
                text(Command::MoveUp, "Move Up"),
                text(Command::MoveDown, "Move Down"),
                danger(Command::DropCommits, "Drop"),
            ];
            history.extend(rebase_here());
            grouped(vec![actions, cherry_picks(1), history, copied])
        }
        Target::Commits {
            commits,
            title,
            section,
        } => {
            let mut actions = Vec::new();
            if *section {
                actions = switches(false);
                actions.push(text(Command::NewBranch, "New Branch…"));
                actions.push(text(Command::NewWorktree, "New Worktree…"));
            }
            let n = commits.len();
            grouped(vec![
                vec![text(Command::Title, title.clone())],
                actions,
                cherry_picks(n),
                vec![
                    text(Command::Squash, format!("Squash {n} Together")),
                    danger(Command::DropCommits, format!("Drop {n}")),
                ],
                vec![
                    text(Command::CopyHashes, "Copy SHAs"),
                    text(Command::CopyMessages, "Copy Messages"),
                ],
            ])
        }
        // Every branch, by its name; the hint says how it would go.
        Target::PickBranches { .. } => {
            let mut items = vec![Some(text(Command::Title, "Cherry-pick onto"))];
            items.extend(
                picks
                    .iter()
                    .enumerate()
                    .map(|(k, place)| Some(text(Command::CherryPickOnto(k), place.name.clone()))),
            );
            items
        }
        Target::Branch { name, commit, leaf } => {
            let mut actions = switches(true);
            actions.extend(rebase_here());
            let mut removal = Vec::new();
            if name != main {
                actions.push(text(Command::NewWorktree, "New Worktree…"));
                removal.push(text(Command::Rename, "Rename…"));
                // An unmerged branch nothing else grows from goes with its
                // commits; others only as git allows.
                removal.push(match leaf {
                    Some(false) => danger(Command::DeleteWithCommits, "Delete with Commits…"),
                    _ => danger(Command::Delete, "Delete…"),
                });
            }
            grouped(vec![
                actions,
                copies(true, commit),
                attention(name),
                removal,
            ])
        }
        // Checked out, a remote branch becomes a local one tracking it.
        Target::Remote { remote, commit, .. } => {
            let mut actions = switches(false);
            actions.push(text(Command::NewWorktree, "New Worktree…"));
            grouped(vec![
                actions,
                copies(true, commit),
                vec![attention_item(shown, overridden)],
                vec![danger(
                    Command::DeleteRemote,
                    format!("Delete on {remote}…"),
                )],
            ])
        }
        Target::Tag { .. } => grouped(vec![
            vec![text(Command::CopyName, "Copy Name")],
            vec![danger(Command::DeleteTag, "Delete…")],
        ]),
        // A stash goes into a worktree, or one of its own, or away.
        Target::Stash { id, .. } => {
            let mut actions: Vec<_> = places
                .iter()
                .enumerate()
                .map(|(k, place)| at(Command::StashApply(k), "Apply", Some(place)))
                .collect();
            actions.extend(
                places
                    .iter()
                    .enumerate()
                    .map(|(k, place)| at(Command::StashPop(k), "Pop", Some(place))),
            );
            actions.push(text(Command::StashWorktree, "New Worktree…"));
            let mut copied = vec![text(Command::CopyName, "Copy Name")];
            copied.extend(copies(false, id).into_iter().take(1));
            grouped(vec![
                actions,
                copied,
                vec![danger(Command::DropStash, "Drop…")],
            ])
        }
        // A file opens, shows in Finder, or is copied by its name or paths.
        Target::File { exists, .. } => {
            let open = match exists {
                true => vec![
                    text(Command::Open, "Open"),
                    text(Command::RevealInFinder, "Reveal in Finder"),
                ],
                false => Vec::new(),
            };
            grouped(vec![
                open,
                vec![
                    text(Command::CopyName, "Copy Name"),
                    text(Command::CopyRelativePath, "Copy Path"),
                    text(Command::CopyPath, "Copy Full Path"),
                ],
            ])
        }
        // Each fact copies alone.
        Target::Facts { facts } => grouped(vec![
            facts
                .iter()
                .enumerate()
                .map(|(k, (name, _))| text(Command::CopyFact(k), format!("Copy {name}")))
                .collect(),
        ]),
        // A group's branches all show, all hide, or follow the filters.
        Target::Group { .. } => grouped(vec![
            vec![
                text(Command::Pin, "Show All in Attention"),
                text(Command::Hide, "Hide All from Attention"),
            ],
            vec![text(Command::FollowFilters, "All Follow Attention Filters")],
        ]),
        Target::Worktree {
            path,
            branch,
            commit,
            leaf,
            stale,
        } => {
            let rename = branch.as_deref().filter(|branch| *branch != main);
            let mut actions = vec![text(Command::StashChanges, "Stash…")];
            if rename.is_some() {
                actions.push(text(Command::Rename, "Rename Branch…"));
            }
            let mut copied = copies(branch.is_some(), commit);
            copied.push(text(Command::CopyPath, "Copy Path"));
            let mut open = vec![text(Command::RevealInFinder, "Reveal in Finder")];
            for (k, command) in recent.iter().take(RECENT_COMMANDS).enumerate() {
                open.push(text(
                    Command::OpenWithRecent(k),
                    format!("Open with {command}"),
                ));
            }
            open.push(text(Command::OpenWith, "Open With…"));
            let mut removal = Vec::new();
            if main_worktree != Some(path.as_str()) {
                removal.push(danger(Command::RemoveWorktree, "Remove…"));
                // The main branch stays, wherever it is checked out.
                if rename.is_some() && (leaf.is_some() || *stale) {
                    removal.push(danger(
                        Command::RemoveWorktreeAndBranch,
                        "Remove with Branch…",
                    ));
                }
            }
            let attention = attention(&attention_name(target));
            match stale {
                // Done with, it is offered to go first, with its branch.
                true => {
                    removal.reverse();
                    grouped(vec![removal, actions, copied, open, attention])
                }
                false => grouped(vec![actions, copied, open, attention, removal]),
            }
        }
    }
}

/// What the menu offers about attention: a branch shown or left out by hand
/// goes back to the filters; otherwise it can be hidden or shown.
fn attention_item(shown: bool, overridden: bool) -> (Command, Label) {
    match (overridden, shown) {
        (true, _) => text(Command::FollowFilters, "Follow Filters"),
        (false, true) => text(Command::Hide, "Hide"),
        (false, false) => text(Command::Pin, "Show"),
    }
}

/// The recent commands saved in the file `name`, as many as are kept.
pub fn load_recent(name: &str) -> Vec<String> {
    let mut recent = crate::recent::load_list(name);
    recent.truncate(RECENT_COMMANDS);
    recent
}

/// Remembers a command first among the recent ones, without repeats.
pub fn remember(recent: &mut Vec<String>, command: &str) {
    crate::recent::put_first(recent, command, RECENT_COMMANDS);
}

/// A path with the home directory as `~`.
pub fn tilde(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    match path.strip_prefix(&home) {
        // Only at a folder boundary: `/Users/ann2` is not under `/Users/ann`.
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
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

/// Checks a commit out in a worktree, on no branch; git refuses when changes
/// are in the way.
pub fn switch_detached(worktree: &str, commit: &str) -> String {
    format!(
        "git -C {} switch --detach {}",
        quote(worktree),
        quote(commit)
    )
}

pub fn stash_apply(worktree: &str, stash: &str, pop: bool) -> String {
    let verb = if pop { "pop" } else { "apply" };
    format!("git -C {} stash {verb} {}", quote(worktree), quote(stash))
}

/// Goes on with a stopped cherry-pick: `--continue` with the message the
/// commit had, `--skip` or `--abort`.
pub fn cherry_pick_step(path: &str, step: &str) -> String {
    format!(
        "git -C {} -c core.editor=true cherry-pick {step}",
        quote(path)
    )
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

/// The flags the Stash picker offers, with what each does.
pub const STASH_FLAGS: [(&str, &str); 3] = [
    (
        "--include-untracked",
        "Include untracked files in the stash",
    ),
    (
        "--keep-index",
        "Leave staged changes in the working directory",
    ),
    ("--staged", "Stash staged changes only"),
];

/// Stashes a worktree's changes with the flags picked.
pub fn stash_with(worktree: &str, flags: &[&str]) -> String {
    let mut line = format!("git -C {} stash push", quote(worktree));
    for flag in flags {
        line.push(' ');
        line.push_str(flag);
    }
    line
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
    fn cherry_pick_steps() {
        assert_eq!(
            cherry_pick_step("/w", "--continue"),
            "git -C /w -c core.editor=true cherry-pick --continue"
        );
    }

    #[test]
    fn remote_branches_and_tags_have_their_own_menus() {
        let remote = Target::Remote {
            remote: "origin".to_string(),
            branch: "fix".to_string(),
            commit: "abcdef1234567".to_string(),
        };
        assert_eq!(
            labels(&remote).last().map(String::as_str),
            Some("Delete on origin…")
        );
        let tag = Target::Tag {
            name: "v1".to_string(),
            commit: String::new(),
        };
        assert_eq!(labels(&tag), ["Copy Name", "—", "Delete…"]);
        assert_eq!(
            delete_remote_branch("/r", "origin", "fix"),
            "git -C /r push origin --delete fix"
        );
    }

    fn labels(target: &Target) -> Vec<String> {
        commands(target, "master", Some("/repo"), &[], true, false, &[], &[])
            .into_iter()
            .map(|item| item.map_or("—".to_string(), |(_, label)| label.to_string()))
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
        assert_eq!(labels(&main), ["Copy Name", "Copy SHA", "Copy Short SHA"]);
        let feature = Target::Branch {
            name: "feature".to_string(),
            commit: "abc".to_string(),
            leaf: None,
        };
        assert!(has(&labels(&feature), "Hide"));
        let unmerged = Target::Branch {
            name: "try".to_string(),
            commit: "abc".to_string(),
            leaf: Some(false),
        };
        assert!(has(&labels(&unmerged), "Delete with Commits…"));
        let left_out: Vec<String> = commands(&feature, "master", None, &[], false, false, &[], &[])
            .into_iter()
            .flatten()
            .map(|(_, label)| label.to_string())
            .collect();
        assert!(left_out.contains(&"Show".to_string()));

        let main_worktree = Target::Worktree {
            path: "/repo".to_string(),
            branch: Some("master".to_string()),
            commit: "abc".to_string(),
            leaf: None,
            stale: false,
        };
        assert!(!has(&labels(&main_worktree), "Remove"));
        // The main worktree is known by its folder, whatever it has checked out.
        let main_on_feature = Target::Worktree {
            path: "/repo".to_string(),
            branch: Some("feature".to_string()),
            commit: "abc".to_string(),
            leaf: None,
            stale: false,
        };
        assert!(!has(&labels(&main_on_feature), "Remove"));
        assert!(has(&labels(&main_on_feature), "Rename Branch…"));
        let linked_on_master = Target::Worktree {
            path: "/repo/wt".to_string(),
            branch: Some("master".to_string()),
            commit: "abc".to_string(),
            leaf: Some(true),
            stale: false,
        };
        let linked = labels(&linked_on_master);
        assert!(has(&linked, "Remove…"));
        assert!(!has(&linked, "Rename Branch") && !has(&linked, "Remove with Branch"));

        let leaf = Target::Worktree {
            path: "/repo/wt".to_string(),
            branch: Some("fix".to_string()),
            commit: "abc".to_string(),
            leaf: Some(true),
            stale: false,
        };
        let leaf_labels = labels(&leaf);
        assert!(has(&leaf_labels, "Remove…"));
        assert!(has(&leaf_labels, "Remove with Branch…"));
        assert_eq!(
            remove_worktree_and_branch("/repo", "/repo/wt", "fix", true),
            "git -C /repo worktree remove /repo/wt && git -C /repo branch -d fix"
        );
    }

    #[test]
    fn a_worktree_done_with_offers_to_go_first() {
        let stale = Target::Worktree {
            path: "/repo/wt".to_string(),
            branch: Some("fix".to_string()),
            commit: "abc".to_string(),
            leaf: None,
            stale: true,
        };
        let labels = labels(&stale);
        assert_eq!(labels[..3], ["Remove with Branch…", "Remove…", "—"]);
    }

    #[test]
    fn detached_worktree_has_no_branch_actions() {
        let detached = Target::Worktree {
            path: "/repo/wt".to_string(),
            branch: None,
            commit: "abc".to_string(),
            leaf: Some(false),
            stale: false,
        };
        let labels = labels(&detached);
        assert!(!has(&labels, "Rename Branch"));
        assert!(!has(&labels, "Remove with Branch"));
        assert!(!has(&labels, "Copy Name"));
        assert!(has(&labels, "Remove…"));
        assert!(has(&labels, "Copy Path"));
    }

    #[test]
    fn branches_and_commits_open_in_worktrees() {
        let branch = Target::Branch {
            name: "fix".to_string(),
            commit: "abc".to_string(),
            leaf: None,
        };
        // The current worktree first; one with the branch already selects it.
        let places = [
            Place::new("wtf", true, false),
            Place::new("wtf-x", false, true),
        ];
        let items: Vec<(Command, String)> =
            commands(&branch, "master", None, &[], true, false, &places, &[])
                .into_iter()
                .flatten()
                .map(|(command, label)| (command, label.to_string()))
                .collect();
        assert_eq!(
            items[..3],
            [
                (Command::CheckoutIn(0), "Checkout · wtf".to_string()),
                (Command::SelectWorktree(1), "Go To · wtf-x".to_string()),
                // The current worktree's branch moved to start from it.
                (Command::RebaseHere, "Rebase onto This · wtf".to_string()),
            ]
        );

        // A commit goes into any worktree, off its branch.
        let commit = Target::Commit {
            commit: "abc1234567".to_string(),
            message: "Fix".to_string(),
        };
        let places = [
            Place::new("wtf", true, false),
            Place::new("wtf-x", false, false),
        ];
        let labels: Vec<String> = commands(&commit, "master", None, &[], true, false, &places, &[])
            .into_iter()
            .map(|item| item.map_or("—".to_string(), |(_, label)| label.to_string()))
            .collect();
        assert_eq!(
            labels[..5],
            [
                "Checkout · wtf",
                "Checkout · wtf-x",
                "New Branch…",
                "New Worktree…",
                "—",
            ]
        );

        // Cherry-picks go into each worktree, then onto a branch.
        let labels: Vec<String> =
            commands(&commit, "master", None, &[], true, false, &places, &places)
                .into_iter()
                .map(|item| item.map_or("—".to_string(), |(_, label)| label.to_string()))
                .collect();
        assert_eq!(
            labels[5..9],
            [
                "Cherry-pick · wtf",
                "Cherry-pick · wtf-x",
                "Cherry-pick into Branch…",
                "—",
            ]
        );

        // Several commits: what they are, the picks and copies; a section's
        // newest commit can be checked out too.
        let commits = Target::Commits {
            commits: vec![("a1".into(), "One".into()), ("b2".into(), "Two".into())],
            title: "2 commits".into(),
            section: false,
        };
        let labels: Vec<String> = commands(
            &commits,
            "master",
            None,
            &[],
            true,
            false,
            &places,
            &places[..1],
        )
        .into_iter()
        .map(|item| item.map_or("—".to_string(), |(_, label)| label.to_string()))
        .collect();
        assert_eq!(
            labels,
            [
                "2 commits",
                "—",
                "Cherry-pick 2 · wtf",
                "Cherry-pick 2 into Branch…",
                "—",
                "Squash 2 Together",
                "Drop 2",
                "—",
                "Copy SHAs",
                "Copy Messages",
            ]
        );
        let section = Target::Commits {
            commits: vec![("a1".into(), "One".into()), ("b2".into(), "Two".into())],
            title: "2 commits · fix".into(),
            section: true,
        };
        let labels: Vec<String> =
            commands(&section, "master", None, &[], true, false, &places, &[])
                .into_iter()
                .map(|item| item.map_or("—".to_string(), |(_, label)| label.to_string()))
                .collect();
        assert_eq!(labels[2], "Checkout · wtf");
        assert_eq!(
            switch_detached("/my repo", "abc123"),
            "git -C '/my repo' switch --detach abc123"
        );
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
            stash_with("/my repo/wt", &["--keep-index", "--include-untracked"]),
            "git -C '/my repo/wt' stash push --keep-index --include-untracked"
        );
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
            stale: false,
        };
        let labels: Vec<String> =
            commands(&worktree, "master", None, &recent, true, false, &[], &[])
                .into_iter()
                .flatten()
                .map(|(_, label)| label.to_string())
                .collect();
        assert!(labels.contains(&"Open with subl".to_string()));
        assert!(!labels.iter().any(|label| label.contains("Terminal")));
    }

    #[test]
    fn a_file_opens_only_when_it_is_there() {
        let labels = |exists: bool| -> Vec<String> {
            let file = Target::File {
                name: "src/main.rs".to_string(),
                path: "/repo/src/main.rs".to_string(),
                exists,
            };
            commands(&file, "main", None, &[], true, false, &[], &[])
                .into_iter()
                .map(|item| item.map_or("—".to_string(), |(_, label)| label.to_string()))
                .collect()
        };
        assert_eq!(
            labels(true),
            [
                "Open",
                "Reveal in Finder",
                "—",
                "Copy Name",
                "Copy Path",
                "Copy Full Path"
            ]
        );
        assert_eq!(labels(false), ["Copy Name", "Copy Path", "Copy Full Path"]);
    }

    #[test]
    fn each_fact_copies_alone() {
        let facts = Target::Facts {
            facts: vec![
                ("Hash".to_string(), "abc123".to_string()),
                ("Author".to_string(), "Ann".to_string()),
            ],
        };
        let labels: Vec<String> = commands(&facts, "main", None, &[], true, false, &[], &[])
            .into_iter()
            .map(|item| item.map_or("—".to_string(), |(_, label)| label.to_string()))
            .collect();
        assert_eq!(labels, ["Copy Hash", "Copy Author"]);
    }
}
