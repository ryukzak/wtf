use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use gpui::{App, WindowId};

/// A window's active tab and its repositories.
type Tabs = (usize, Vec<String>);

/// Each open window's tabs, as last drawn, and what was last written, so the
/// file changes only when they do.
static OPEN: LazyLock<Mutex<(HashMap<WindowId, Tabs>, String)>> = LazyLock::new(Mutex::default);

fn file() -> PathBuf {
    let home = env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/wtf/session")
}

/// The windows open when wtf last quit: each with its active tab and its
/// repositories.
pub fn load() -> Vec<Tabs> {
    parse(&fs::read_to_string(file()).unwrap_or_default())
}

/// Notes a window's tabs and saves every open window's when they changed.
/// Windows closed since drop out; quitting draws nothing, so what was open
/// then stays saved. A session that cannot be saved is only a missing
/// convenience, so errors are ignored.
pub fn remember(window: WindowId, active: usize, tabs: Vec<String>, cx: &App) {
    let mut open = OPEN.lock().unwrap_or_else(|error| error.into_inner());
    let (windows, written) = &mut *open;
    windows.insert(window, (active, tabs));
    let alive: Vec<WindowId> = cx
        .windows()
        .iter()
        .map(|window| window.window_id())
        .collect();
    windows.retain(|window, _| alive.contains(window));
    let mut ids: Vec<&WindowId> = windows.keys().collect();
    ids.sort_by_key(|id| id.as_u64());
    let sessions: Vec<Tabs> = ids.into_iter().map(|id| windows[id].clone()).collect();
    let text = format(&sessions);
    if text != *written {
        let file = file();
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).ok();
        }
        fs::write(file, &text).ok();
        *written = text;
    }
}

/// A line a window: its active tab, then its repositories, tab-separated.
fn format(windows: &[Tabs]) -> String {
    windows
        .iter()
        .filter(|(_, tabs)| !tabs.is_empty())
        .map(|(active, tabs)| format!("{active}\t{}\n", tabs.join("\t")))
        .collect()
}

fn parse(text: &str) -> Vec<Tabs> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let active = fields.next()?.parse().ok()?;
            let tabs: Vec<String> = fields
                .filter(|tab| !tab.is_empty())
                .map(str::to_string)
                .collect();
            (!tabs.is_empty()).then_some((active, tabs))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_survive_saving_and_loading() {
        let windows = vec![
            (1, vec!["/a".to_string(), "/b c".to_string()]),
            (0, Vec::new()),
            (0, vec!["/d".to_string()]),
        ];
        assert_eq!(
            parse(&format(&windows)),
            [windows[0].clone(), windows[2].clone()]
        );
        assert!(parse("x\t/a\n").is_empty());
    }
}
