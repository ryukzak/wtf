use wtf_core::{NodeKind, WorkflowGraph};

use crate::graph_view::{self, WorktreeNames};

/// What the Go To picker jumps to, in the order it lists them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Worktree,
    /// A command run on a worktree; its entry's `run` is the command.
    Shell,
    Branch,
    Commit,
    /// A palette command; its entry's `node` is its place in the list.
    Command,
}

/// A place in the graph to jump to: its node, and for a commit its id.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub kind: Kind,
    pub label: String,
    /// Shown dimmed after the label: a path, a short hash and author.
    pub detail: String,
    pub node: usize,
    pub commit: Option<String>,
    pub time: i64,
    /// A shell command and the folder it runs in.
    pub run: Option<(String, String)>,
}

/// Worktrees, then branches, then commits newest first.
pub fn entries(graph: &WorkflowGraph, names: WorktreeNames) -> Vec<Entry> {
    let mut worktrees = Vec::new();
    let mut branches = Vec::new();
    let mut commits = Vec::new();
    for (i, node) in graph.nodes.iter().enumerate() {
        match node.kind {
            NodeKind::Uncommitted => worktrees.push(Entry {
                kind: Kind::Worktree,
                label: graph_view::worktree_label(node, names),
                detail: match &node.tip {
                    wtf_core::Revision::Worktree(path) => path.clone(),
                    wtf_core::Revision::Commit(_) => String::new(),
                },
                node: i,
                commit: None,
                time: node.last_time,
                run: None,
            }),
            NodeKind::Commits | NodeKind::History => {
                for name in &node.refs {
                    branches.push(Entry {
                        kind: Kind::Branch,
                        label: name.clone(),
                        detail: String::new(),
                        node: i,
                        commit: None,
                        time: node.last_time,
                        run: None,
                    });
                }
                for commit in &node.commits {
                    commits.push(Entry {
                        kind: Kind::Commit,
                        label: commit.summary.clone(),
                        detail: format!("{} · {}", commit.short_id, commit.author),
                        node: i,
                        commit: Some(commit.id.clone()),
                        time: commit.time,
                        run: None,
                    });
                }
            }
        }
    }
    commits.sort_by_key(|entry| std::cmp::Reverse(entry.time));
    worktrees
        .into_iter()
        .chain(branches)
        .chain(commits)
        .collect()
}

/// `command` run in the worktree at `path`, as `$ command in ~/path`.
pub fn shell_entry(command: &str, path: &str) -> Entry {
    let command = command.trim();
    Entry {
        kind: Kind::Shell,
        label: command.to_string(),
        detail: format!("in {}", crate::popups::tilde(path)),
        node: 0,
        commit: None,
        time: 0,
        run: Some((command.to_string(), path.to_string())),
    }
}

/// How well `query` matches an entry, lower is better: a run of its words
/// in the label beats one in the detail, which beats the letters in order;
/// `None` when it does not match. Case is ignored.
fn score(entry: &Entry, query: &str) -> Option<usize> {
    let label = entry.label.to_lowercase();
    let detail = entry.detail.to_lowercase();
    if let Some(at) = label.find(query) {
        return Some(at);
    }
    if detail.contains(query) {
        return Some(1_000);
    }
    let mut letters = label.chars();
    query
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|c| letters.any(|l| l == c))
        .then_some(2_000)
}

/// The entries matching `query`: worktrees, then branches, then commits,
/// each kind best match first, keeping the given order among equals.
pub fn filter<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return entries.iter().collect();
    }
    let mut found: Vec<(usize, &Entry)> = entries
        .iter()
        .filter_map(|entry| score(entry, &query).map(|score| (score, entry)))
        .collect();
    found.sort_by_key(|(score, entry)| (entry.kind as usize, *score));
    found.into_iter().map(|(_, entry)| entry).collect()
}

/// Completes a shell command's last word to a file name, as a shell's Tab
/// does: `list` gives the names in a folder, relative to the command's own,
/// each with whether it is a folder. One match completes it, with `/` after a
/// folder and a space after a file; several complete what they share and
/// come back to be shown.
pub fn complete(
    command: &str,
    list: impl Fn(&str) -> Vec<(String, bool)>,
) -> (String, Vec<String>) {
    let word = command.rsplit(' ').next().unwrap_or_default();
    let (folder, prefix) = match word.rsplit_once('/') {
        Some((folder, prefix)) => (&word[..folder.len() + 1], prefix),
        None => ("", word),
    };
    let mut matches: Vec<String> = list(folder)
        .into_iter()
        .filter(|(name, _)| name.starts_with(prefix))
        // Hidden files only when asked for.
        .filter(|(name, _)| prefix.starts_with('.') || !name.starts_with('.'))
        .map(|(name, folder)| if folder { format!("{name}/") } else { name })
        .collect();
    matches.sort();
    let stem = &command[..command.len() - prefix.len()];
    match &matches[..] {
        [] => (command.to_string(), Vec::new()),
        [only] if only.ends_with('/') => (format!("{stem}{only}"), Vec::new()),
        [only] => (format!("{stem}{only} "), Vec::new()),
        [first, rest @ ..] => {
            let shared = rest.iter().fold(first.as_str(), |shared, name| {
                let same = shared
                    .char_indices()
                    .zip(name.chars())
                    .find(|((_, a), b)| a != b)
                    .map_or(shared.len().min(name.len()), |((at, _), _)| at);
                &shared[..same]
            });
            (format!("{stem}{shared}"), matches.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: Kind, label: &str, detail: &str) -> Entry {
        Entry {
            kind,
            label: label.to_string(),
            detail: detail.to_string(),
            node: 0,
            commit: None,
            time: 0,
            run: None,
        }
    }

    #[test]
    fn label_matches_beat_details_and_scattered_letters() {
        let entries = [
            entry(Kind::Commit, "Fix the parser", "abc1234 · Ann"),
            entry(Kind::Branch, "feature-parse", ""),
            entry(Kind::Commit, "Add tests", "parse99 · Bob"),
            entry(Kind::Worktree, "p-a-r-s-e", ""),
        ];
        let labels: Vec<&str> = filter(&entries, "Parse")
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        assert_eq!(
            labels,
            ["p-a-r-s-e", "feature-parse", "Fix the parser", "Add tests"]
        );
        assert!(filter(&entries, "zzz").is_empty());
        assert_eq!(filter(&entries, "  ").len(), 4);
    }

    #[test]
    fn commands_name_their_worktree() {
        let shell = shell_entry(" zed . ", "/src/limits/");
        assert_eq!(shell.label, "zed .");
        assert_eq!(shell.detail, "in /src/limits/");
        assert_eq!(
            shell.run,
            Some(("zed .".to_string(), "/src/limits/".to_string()))
        );
    }

    #[test]
    fn completes_file_names_like_a_shell() {
        let list = |folder: &str| match folder {
            "" => vec![
                ("README.md".to_string(), false),
                ("scripts".to_string(), true),
                ("src".to_string(), true),
                (".github".to_string(), true),
            ],
            "src/" => vec![("main.rs".to_string(), false)],
            _ => Vec::new(),
        };
        assert_eq!(complete("zed R", list).0, "zed README.md ");
        assert_eq!(complete("zed sc", list).0, "zed scripts/");
        assert_eq!(complete("zed src/m", list).0, "zed src/main.rs ");
        let (command, matches) = complete("zed s", list);
        assert_eq!(command, "zed s");
        assert_eq!(matches, ["scripts/", "src/"]);
        assert_eq!(complete("zed ", list).1, ["README.md", "scripts/", "src/"]);
        assert_eq!(complete("zed .g", list).0, "zed .github/");
        assert_eq!(complete("zed x", list), ("zed x".to_string(), Vec::new()));
    }
}
