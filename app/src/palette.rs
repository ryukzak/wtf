use gpui::{Action, Window};
use wtf_core::Scope;

use crate::goto::{Entry, Kind};
use crate::menus::{
    About, AllAuthors, NewWindow, OpenSettings, ResetBranches, SetScope, ToggleAttentionPanel,
    ToggleRemotes, ToggleTags,
};
use crate::repo_view::{
    Fetch, GoTo, OpenRepository, OpenWorktreeWith, Prune, ShowAllFiles, ShowOneFile, StashChanges,
    StructuralDiff, TextDiff, ToggleCombineCommits, ToggleConsole, ToggleRefreshCommands,
    ToggleSidePanel,
};
use crate::workspace::{CloseTab, NewTab, NextTab, PreviousTab, ReopenClosedTab};

/// Every command the palette offers, by name.
pub fn commands() -> Vec<(&'static str, Box<dyn Action>)> {
    vec![
        ("Go To Worktree, Branch or Commit…", Box::new(GoTo)),
        ("Prune Stale Branches and Worktrees…", Box::new(Prune)),
        ("Fetch All Remotes", Box::new(Fetch)),
        ("Stash Changes", Box::new(StashChanges)),
        ("Open Worktree With…", Box::new(OpenWorktreeWith)),
        ("Text Diff", Box::new(TextDiff)),
        ("Structural Diff", Box::new(StructuralDiff)),
        ("All Files", Box::new(ShowAllFiles)),
        ("File by File", Box::new(ShowOneFile)),
        ("Toggle Combine Commits", Box::new(ToggleCombineCommits)),
        ("Toggle Side Panel", Box::new(ToggleSidePanel)),
        ("Toggle Attention Panel", Box::new(ToggleAttentionPanel)),
        ("Toggle Console", Box::new(ToggleConsole)),
        ("Toggle Refresh Commands", Box::new(ToggleRefreshCommands)),
        (
            "Attention: Worktrees Only",
            Box::new(SetScope {
                scope: Scope::Worktrees,
            }),
        ),
        (
            "Attention: Worktrees and Recent Branches",
            Box::new(SetScope {
                scope: Scope::Branches,
            }),
        ),
        ("Attention: All Authors", Box::new(AllAuthors)),
        (
            "Attention: Reset Pinned and Hidden",
            Box::new(ResetBranches),
        ),
        ("Attention: Toggle Remote Branches", Box::new(ToggleRemotes)),
        ("Attention: Toggle Tags", Box::new(ToggleTags)),
        ("Open Repository…", Box::new(OpenRepository)),
        ("New Tab", Box::new(NewTab)),
        ("New Window", Box::new(NewWindow)),
        ("Close Tab", Box::new(CloseTab)),
        ("Reopen Closed Tab", Box::new(ReopenClosedTab)),
        ("Next Tab", Box::new(NextTab)),
        ("Previous Tab", Box::new(PreviousTab)),
        ("Settings…", Box::new(OpenSettings)),
        ("About wtf", Box::new(About)),
    ]
}

/// The commands as picker entries, each with its shortcut as its detail.
pub fn entries(window: &Window) -> Vec<Entry> {
    commands()
        .into_iter()
        .enumerate()
        .map(|(k, (name, action))| Entry {
            kind: Kind::Command,
            label: name.to_string(),
            detail: window
                .bindings_for_action(action.as_ref())
                .last()
                .map(|binding| {
                    let keys: Vec<String> = binding
                        .keystrokes()
                        .iter()
                        .map(|keystroke| symbols(&keystroke.unparse()))
                        .collect();
                    keys.join(" ")
                })
                .unwrap_or_default(),
            node: k,
            commit: None,
            time: 0,
            run: None,
        })
        .collect()
}

/// A shortcut as macOS menus write it: `cmd-shift-p` as ⇧⌘P.
fn symbols(keys: &str) -> String {
    // `cmd--` binds the minus key itself.
    let (modifiers, key) = match keys.strip_suffix("--") {
        Some(modifiers) => (modifiers, "-"),
        None => keys.rsplit_once('-').unwrap_or(("", keys)),
    };
    let parts: Vec<&str> = modifiers.split('-').collect();
    let mut text = String::new();
    for (name, symbol) in [("ctrl", "⌃"), ("alt", "⌥"), ("shift", "⇧"), ("cmd", "⌘")] {
        if parts.contains(&name) {
            text.push_str(symbol);
        }
    }
    let key = match key {
        "tab" => "⇥".to_string(),
        "enter" => "↩".to_string(),
        "escape" => "⎋".to_string(),
        "backspace" => "⌫".to_string(),
        "space" => "Space".to_string(),
        "up" => "↑".to_string(),
        "down" => "↓".to_string(),
        "left" => "←".to_string(),
        "right" => "→".to_string(),
        key => key.to_uppercase(),
    };
    text + &key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_read_as_in_menus() {
        assert_eq!(symbols("cmd-shift-p"), "⇧⌘P");
        assert_eq!(symbols("ctrl-tab"), "⌃⇥");
        assert_eq!(symbols("alt-cmd-1"), "⌥⌘1");
        assert_eq!(symbols("cmd--"), "⌘-");
        assert_eq!(symbols("shift-up"), "⇧↑");
        assert_eq!(symbols("f"), "F");
    }
}
