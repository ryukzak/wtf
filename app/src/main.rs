mod about;
mod cli;
mod console_view;
mod diff_view;
mod goto;
mod graph_view;
mod line_edit;
mod menus;
mod palette;
mod popups;
mod preferences;
mod recent;
mod repo_view;
mod session;
mod sync;
mod syntax;
mod terminal_view;
mod theme;
mod tools;
mod workspace;

use gpui::{App, Application, KeyBinding};

use crate::menus::{
    About, CheckTools, Hide, HideOthers, InstallCommand, NewWindow, OpenSettings, Quit, ShowAll,
};
use crate::workspace::{
    ClearRecent, CloseTab, CloseWindow, NewTab, NextTab, OpenPath, PreviousTab, ReopenClosedTab,
    SelectTab, open_window,
};

use crate::repo_view::{
    CommandPalette, Fetch, GoTo, Minimize, OpenRepository, StructuralDiff, TextDiff, ToggleConsole,
    ToggleSidePanel, ToggleSnapshot,
};

fn main() {
    let paths: Vec<String> = std::env::args()
        .skip(1)
        .map(|path| workspace::repository_path(&path))
        .collect();
    // Folders dropped on the Dock icon come as URLs, outside any app context;
    // a channel hands them to the app.
    let (opened, mut to_open) = futures::channel::mpsc::unbounded::<Vec<String>>();
    let application = Application::new();
    application.on_open_urls(move |urls| {
        let paths: Vec<String> = urls
            .iter()
            .filter_map(|url| workspace::path_from_url(url))
            .collect();
        opened.unbounded_send(paths).ok();
    });
    application.run(move |cx: &mut App| {
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(paths) = to_open.next().await {
                cx.update(|cx| workspace::open_paths(paths, cx)).ok();
            }
        })
        .detach();
        preferences::load();
        // Missing or broken external programs are reported, and the first
        // run finds out whether this Mac can summarize commits.
        tools::check_at_launch(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &OpenSettings, cx| preferences::open(cx));
        cx.on_action(|_: &About, cx| about::open(cx));
        cx.on_action(|_: &CheckTools, cx| tools::open(cx));
        cx.on_action(|_: &InstallCommand, cx| cli::install(cx));
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        // Menus show these shortcuts next to their items.
        // ⌘1…⌘9 pick tabs, as in browsers.
        cx.bind_keys((0..9).map(|index| {
            KeyBinding::new(&format!("cmd-{}", index + 1), SelectTab { index }, None)
        }));
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("alt-cmd-h", HideOthers, None),
            KeyBinding::new("cmd-o", OpenRepository, None),
            KeyBinding::new("cmd-shift-n", NewWindow, None),
            KeyBinding::new("cmd-j", ToggleConsole, None),
            KeyBinding::new("cmd-r", ToggleSidePanel, None),
            KeyBinding::new("cmd-p", GoTo, None),
            KeyBinding::new("cmd-shift-p", CommandPalette, None),
            KeyBinding::new("alt-cmd-f", Fetch, None),
            KeyBinding::new("ctrl-`", ToggleConsole, None),
            KeyBinding::new("alt-cmd-1", StructuralDiff, None),
            KeyBinding::new("alt-cmd-2", TextDiff, None),
            KeyBinding::new("alt-cmd-3", ToggleSnapshot, None),
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("cmd-t", NewTab, None),
            KeyBinding::new("cmd-w", CloseTab, None),
            KeyBinding::new("ctrl-shift-t", ReopenClosedTab, None),
            KeyBinding::new("cmd-shift-w", CloseWindow, None),
            KeyBinding::new("ctrl-tab", NextTab, None),
            KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
            KeyBinding::new("cmd-shift-]", NextTab, None),
            KeyBinding::new("cmd-shift-[", PreviousTab, None),
        ]);
        cx.set_menus(menus::menus(&menus::MenuState::default()));

        cx.on_action(|_: &NewWindow, cx| open_window(Vec::new(), cx));
        // With every window closed, opening still works: a window comes for
        // what is opened. While one is open, its own handlers come first.
        cx.on_action(|action: &OpenPath, cx| workspace::open_paths(vec![action.path.clone()], cx));
        cx.on_action(|_: &NewTab, cx| open_window(Vec::new(), cx));
        cx.on_action(|_: &OpenRepository, cx| {
            let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: None,
            });
            cx.spawn(async move |cx| {
                if let Ok(Ok(Some(paths))) = paths.await {
                    let paths = paths
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect();
                    cx.update(|cx| workspace::open_paths(paths, cx)).ok();
                }
            })
            .detach();
        });
        cx.on_action(|_: &ClearRecent, cx| {
            recent::clear();
            cx.set_menus(menus::menus(&menus::MenuState::default()));
        });
        // The windows from last time always return; repositories named on
        // the command line open as tabs in the front one, as folders handed
        // to a running app do. Opening only those would save them over the
        // session, losing its tabs.
        let restored = session::load();
        if restored.is_empty() {
            open_window(paths, cx);
        } else {
            for (active, tabs) in restored {
                workspace::open_window_at(tabs, active, cx);
            }
            if !paths.is_empty() {
                workspace::open_paths(paths, cx);
            }
        }
        cx.activate(true);
    });
}
