use gpui::{Action, Menu, MenuItem, SystemMenuType, actions};
use wtf_core::{Attention, Branch, Scope};

use crate::graph_view::{WorktreeNames, WorktreeOrder};
use crate::workspace::{
    ClearRecent, CloseTab, CloseWindow, NewTab, NextTab, OpenPath, PreviousTab, ReopenClosedTab,
};

use crate::repo_view::{
    CommandPalette, Fetch, GoTo, Minimize, OpenRepository, OpenWorktreeWith, Prune, ShowAllFiles,
    ShowOneFile, StashChanges, StructuralDiff, TextDiff, ToggleCombineCommits, ToggleConsole,
    ToggleRefreshCommands, ToggleSidePanel, Zoom,
};

actions!(
    wtf,
    [
        Quit,
        About,
        OpenSettings,
        Hide,
        HideOthers,
        ShowAll,
        NewWindow,
        AllAuthors,
        ResetBranches,
        ToggleRemotes,
        ToggleTags,
        ToggleAttentionPanel
    ]
);

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = wtf, no_json)]
pub struct SetWorktreeNames {
    pub names: WorktreeNames,
}

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = wtf, no_json)]
pub struct SetWorktreeOrder {
    pub order: WorktreeOrder,
}

/// What the menu bar shows: it is rebuilt when any of this changes.
#[derive(Clone, PartialEq)]
pub struct MenuState {
    pub attention: Attention,
    pub branches: Vec<Branch>,
    pub combine: bool,
    pub names: WorktreeNames,
    pub order: WorktreeOrder,
    pub panel: bool,
    /// Recently opened repositories, newest first.
    pub recent: Vec<String>,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            attention: Attention::default(),
            branches: Vec::new(),
            combine: true,
            names: WorktreeNames::FolderAndBranch,
            order: WorktreeOrder::LastCommit,
            panel: false,
            recent: Vec::new(),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = wtf, no_json)]
pub struct SetScope {
    pub scope: Scope,
}

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = wtf, no_json)]
pub struct SetTimeWindow {
    pub days: Option<u32>,
}

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = wtf, no_json)]
pub struct ToggleAuthor {
    pub name: String,
}

/// Pins a branch attention leaves out, or hides one it shows.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = wtf, no_json)]
pub struct ToggleBranch {
    pub name: String,
}

const TIME_WINDOWS: [(Option<u32>, &str); 5] = [
    (Some(1), "Last Day"),
    (Some(7), "Last 7 Days"),
    (Some(14), "Last 14 Days"),
    (Some(30), "Last 30 Days"),
    (None, "All Time"),
];

/// GPUI menus have no checkmarks, so the labels carry them.
fn check(on: bool, label: &str) -> String {
    if on {
        format!("✓ {label}")
    } else {
        format!("    {label}")
    }
}

/// The menu bar; the attention submenu reflects the current attention.
pub fn menus(state: &MenuState) -> Vec<Menu> {
    let (attention, branches) = (&state.attention, &state.branches);
    let names = [
        (WorktreeNames::FolderAndBranch, "Folder and Branch"),
        (WorktreeNames::Branch, "Branch"),
        (WorktreeNames::Folder, "Folder"),
    ]
    .into_iter()
    .map(|(names, label)| {
        MenuItem::action(
            check(state.names == names, label),
            SetWorktreeNames { names },
        )
    })
    .collect();
    let orders = [
        (WorktreeOrder::LastCommit, "By Last Commit"),
        (WorktreeOrder::FirstCommit, "By First Commit"),
    ]
    .into_iter()
    .map(|(order, label)| {
        MenuItem::action(
            check(state.order == order, label),
            SetWorktreeOrder { order },
        )
    })
    .collect();
    let mut recent: Vec<MenuItem> = state
        .recent
        .iter()
        .map(|path| MenuItem::action(path.clone(), OpenPath { path: path.clone() }))
        .collect();
    if !recent.is_empty() {
        recent.push(MenuItem::separator());
    }
    recent.push(MenuItem::action("Clear Menu", ClearRecent));
    let time_windows = TIME_WINDOWS
        .iter()
        .map(|&(days, label)| {
            MenuItem::action(
                check(attention.window_days == days, label),
                SetTimeWindow { days },
            )
        })
        .collect();

    let mut authors: Vec<&str> = Vec::new();
    for branch in branches {
        if !authors.contains(&branch.author.as_str()) {
            authors.push(&branch.author);
        }
    }
    let mut author_items = vec![
        MenuItem::action(
            check(attention.authors.is_empty(), "All Authors"),
            AllAuthors,
        ),
        MenuItem::separator(),
    ];
    author_items.extend(authors.into_iter().map(|name| {
        let on = attention.authors.iter().any(|author| author == name);
        let name = name.to_string();
        MenuItem::action(check(on, &name), ToggleAuthor { name })
    }));

    let mut branch_items: Vec<MenuItem> = branches
        .iter()
        .map(|branch| {
            let mut notes = Vec::new();
            if branch.worktree {
                notes.push("worktree");
            }
            if branch.remote {
                notes.push("remote");
            }
            if attention.pinned.contains(&branch.name) {
                notes.push("pinned");
            } else if attention.hidden.contains(&branch.name) {
                notes.push("hidden");
            }
            let note = if notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", notes.join(", "))
            };
            let label = check(branch.shown, &format!("{}{note}", branch.name));
            MenuItem::action(
                label,
                ToggleBranch {
                    name: branch.name.clone(),
                },
            )
        })
        .collect();
    branch_items.push(MenuItem::separator());
    branch_items.push(MenuItem::action("Reset Pinned and Hidden", ResetBranches));

    let attention_menu = Menu {
        name: "Attention".into(),
        items: vec![
            MenuItem::action(
                check(attention.scope == Scope::Worktrees, "Worktrees Only"),
                SetScope {
                    scope: Scope::Worktrees,
                },
            ),
            MenuItem::action(
                check(
                    attention.scope == Scope::Branches,
                    "Worktrees and Recent Branches",
                ),
                SetScope {
                    scope: Scope::Branches,
                },
            ),
            MenuItem::separator(),
            MenuItem::action(check(attention.remotes, "Remote Branches"), ToggleRemotes),
            MenuItem::action(check(attention.tags, "Tags"), ToggleTags),
            MenuItem::separator(),
            MenuItem::submenu(Menu {
                name: "Time Window".into(),
                items: time_windows,
            }),
            MenuItem::submenu(Menu {
                name: "Authors".into(),
                items: author_items,
            }),
            MenuItem::submenu(Menu {
                name: "Branches".into(),
                items: branch_items,
            }),
        ],
    };

    vec![
        Menu {
            name: "wtf".into(),
            items: vec![
                MenuItem::action("About wtf", About),
                MenuItem::separator(),
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide wtf", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit wtf", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Tab", NewTab),
                MenuItem::action("New Window", NewWindow),
                MenuItem::separator(),
                MenuItem::action("Open Repository…", OpenRepository),
                MenuItem::action("Open Worktree With…", OpenWorktreeWith),
                MenuItem::submenu(Menu {
                    name: "Open Recent".into(),
                    items: recent,
                }),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::action("Reopen Closed Tab", ReopenClosedTab),
                MenuItem::action("Close Window", CloseWindow),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action(
                    check(state.combine, "Combine Commits"),
                    ToggleCombineCommits,
                ),
                MenuItem::submenu(Menu {
                    name: "Worktree Names".into(),
                    items: names,
                }),
                MenuItem::submenu(Menu {
                    name: "Sort Worktrees".into(),
                    items: orders,
                }),
                MenuItem::separator(),
                MenuItem::action("Go To…", GoTo),
                MenuItem::action("Command Palette…", CommandPalette),
                MenuItem::action("Prune Stale Branches and Worktrees…", Prune),
                MenuItem::action("Fetch All Remotes", Fetch),
                MenuItem::action("Stash Changes", StashChanges),
                MenuItem::action("Toggle Side Panel", ToggleSidePanel),
                MenuItem::action(check(state.panel, "Attention Panel"), ToggleAttentionPanel),
                MenuItem::submenu(attention_menu),
                MenuItem::separator(),
                MenuItem::action("Toggle Console", ToggleConsole),
                MenuItem::action("Toggle Refresh Commands", ToggleRefreshCommands),
            ],
        },
        Menu {
            name: "Diff".into(),
            items: vec![
                MenuItem::action("Structural", StructuralDiff),
                MenuItem::action("Text", TextDiff),
                MenuItem::separator(),
                MenuItem::action("File by File", ShowOneFile),
                MenuItem::action("All Files", ShowAllFiles),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
                MenuItem::separator(),
                MenuItem::action("Show Next Tab", NextTab),
                MenuItem::action("Show Previous Tab", PreviousTab),
            ],
        },
    ]
}
