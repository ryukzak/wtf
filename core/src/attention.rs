use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::CoreError;

/// The groups of branches attention starts from.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum Scope {
    /// The main branch and branches checked out in worktrees.
    Worktrees,
    /// Also local branches that pass the time window and author filters.
    #[default]
    Branches,
}

/// A filter over branches for the workflow graph. The main branch is always
/// shown; any other branch can be hidden or pinned by name.
// A field added later reads as its default, so the rest of a saved
// attention survives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Attention {
    pub scope: Scope,
    /// Branches with commits in this many days; `None` is all time.
    pub window_days: Option<u32>,
    /// Branches whose latest commit is by one of these; empty is everyone.
    pub authors: Vec<String>,
    /// Shown whatever the filters say.
    pub pinned: Vec<String>,
    /// Hidden whatever the filters say.
    pub hidden: Vec<String>,
    /// Remote branches pass the same filters as local ones.
    pub remotes: bool,
    /// Tags label the commits they point to.
    pub tags: bool,
    /// Stashes mark the commits they were made on.
    pub stashes: bool,
}

impl Default for Attention {
    fn default() -> Self {
        Self {
            scope: Scope::Branches,
            window_days: Some(14),
            authors: Vec::new(),
            pinned: Vec::new(),
            hidden: Vec::new(),
            remotes: false,
            tags: false,
            stashes: false,
        }
    }
}

impl Attention {
    /// When the time window starts, in Unix seconds; `None` for all time.
    pub fn since(&self, now: i64) -> Option<i64> {
        self.window_days.map(|days| now - i64::from(days) * 86_400)
    }

    /// Whether a branch with its latest commit at `time` by `author` is shown.
    pub fn shows(&self, name: &str, author: &str, time: i64, now: i64, worktree: bool) -> bool {
        if self.hidden.iter().any(|hidden| hidden == name) {
            return false;
        }
        if worktree || self.pinned.iter().any(|pinned| pinned == name) {
            return true;
        }
        let recent = self.since(now).is_none_or(|since| time >= since);
        let by_author = self.authors.is_empty() || self.authors.iter().any(|a| a == author);
        self.scope == Scope::Branches && recent && by_author
    }

    /// Whether the graph shows a branch: a remote one only while remote
    /// branches are on, unless pinned; else as [`Self::shows`] says.
    pub fn shows_branch(&self, branch: &crate::Branch, now: i64) -> bool {
        if branch.remote && !self.remotes && !self.pinned.contains(&branch.name) {
            return false;
        }
        self.shows(
            &branch.name,
            &branch.author,
            branch.time,
            now,
            branch.worktree,
        )
    }

    /// The saved attention of a repository, or the default one.
    pub fn load(repo_path: &str) -> Self {
        read_all().remove(repo_path).unwrap_or_default()
    }

    pub fn save(&self, repo_path: &str) -> Result<(), CoreError> {
        let mut all = read_all();
        all.insert(repo_path.to_string(), self.clone());
        let path = file();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(CoreError::new)?;
        }
        let json = serde_json::to_string_pretty(&all).map_err(CoreError::new)?;
        fs::write(path, json).map_err(CoreError::new)
    }
}

fn file() -> PathBuf {
    let home = env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/wtf/attention.json")
}

fn read_all() -> HashMap<String, Attention> {
    fs::read_to_string(file())
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn filters_by_window_and_author_with_pins_and_hides() {
        let now = 100 * DAY;
        let mut attention = Attention::default();
        assert!(attention.shows("new", "Ann", now - DAY, now, false));
        assert!(!attention.shows("old", "Ann", now - 30 * DAY, now, false));

        attention.authors = vec!["Bot".to_string()];
        assert!(!attention.shows("new", "Ann", now - DAY, now, false));
        assert!(attention.shows("agent", "Bot", now - DAY, now, false));

        attention.pinned = vec!["old".to_string()];
        attention.hidden = vec!["agent".to_string()];
        assert!(attention.shows("old", "Ann", now - 30 * DAY, now, false));
        assert!(!attention.shows("agent", "Bot", now - DAY, now, false));

        attention.window_days = None;
        attention.authors.clear();
        assert!(attention.shows("ancient", "Ann", 0, now, false));
    }

    #[test]
    fn worktrees_only_scope_keeps_worktrees_and_pins() {
        let now = 100 * DAY;
        let mut attention = Attention {
            scope: Scope::Worktrees,
            pinned: vec!["pinned".to_string()],
            ..Attention::default()
        };
        assert!(attention.shows("agent", "Bot", 0, now, true));
        assert!(attention.shows("pinned", "Ann", 0, now, false));
        assert!(!attention.shows("fresh", "Ann", now, now, false));

        attention.hidden = vec!["agent".to_string()];
        assert!(
            !attention.shows("agent", "Bot", 0, now, true),
            "worktrees can be hidden"
        );
    }
}
