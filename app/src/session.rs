use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use gpui::{App, Bounds, WindowBounds, WindowId, point, px, size};

/// A window as wtf last saw it: its active tab, its repositories, and where
/// it was on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Saved {
    pub active: usize,
    pub tabs: Vec<Tab>,
    pub bounds: Option<WindowBounds>,
}

/// A tab: its repository, and the panels it had open, such as `console`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tab {
    pub path: String,
    pub panels: Vec<String>,
}

/// Parts a tab's repository from its panels; no path has it.
const PANELS: char = '\u{1f}';

/// Each open window, as last drawn, and what was last written, so the file
/// changes only when they do.
static OPEN: LazyLock<Mutex<(HashMap<WindowId, Saved>, String)>> = LazyLock::new(Mutex::default);

fn file() -> PathBuf {
    crate::recent::support_file("session")
}

/// The windows open when wtf last quit.
pub fn load() -> Vec<Saved> {
    parse(&fs::read_to_string(file()).unwrap_or_default())
}

/// Notes a window's tabs and place, and saves every open window's when
/// they changed. Windows closed since drop out; quitting draws nothing, so
/// what was open then stays saved. A session that cannot be saved is only
/// a missing convenience, so errors are ignored.
pub fn remember(window: WindowId, saved: Saved, cx: &App) {
    let mut open = OPEN.lock().unwrap_or_else(|error| error.into_inner());
    let (windows, written) = &mut *open;
    windows.insert(window, saved);
    let alive: Vec<WindowId> = cx
        .windows()
        .iter()
        .map(|window| window.window_id())
        .collect();
    windows.retain(|window, _| alive.contains(window));
    let mut ids: Vec<&WindowId> = windows.keys().collect();
    ids.sort_by_key(|id| id.as_u64());
    let sessions: Vec<Saved> = ids.into_iter().map(|id| windows[id].clone()).collect();
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

/// Where a saved window goes again: where it was, when a display still
/// shows enough of it; else nowhere in particular.
pub fn placed(bounds: Option<WindowBounds>, cx: &App) -> Option<WindowBounds> {
    let bounds = bounds?;
    let inner = bounds.get_bounds();
    // Its title bar on some display, so it can be grabbed.
    let title = Bounds::new(inner.origin, size(inner.size.width, px(30.)));
    cx.displays()
        .iter()
        .any(|display| display.bounds().intersects(&title))
        .then_some(bounds)
}

/// A line a window: its active tab, where it was as `@x,y,w,h,state` with
/// state 0 windowed, 1 maximized, 2 full screen, then its repositories,
/// tab-separated, each with its open panels after a `PANELS`, comma-separated.
/// A window with no place known leaves it out.
fn format(windows: &[Saved]) -> String {
    windows
        .iter()
        .filter(|saved| !saved.tabs.is_empty())
        .map(|saved| {
            let place = saved.bounds.map(|bounds| {
                let (state, b) = match bounds {
                    WindowBounds::Windowed(b) => (0, b),
                    WindowBounds::Maximized(b) => (1, b),
                    WindowBounds::Fullscreen(b) => (2, b),
                };
                format!(
                    "\t@{},{},{},{},{state}",
                    f32::from(b.origin.x),
                    f32::from(b.origin.y),
                    f32::from(b.size.width),
                    f32::from(b.size.height)
                )
            });
            format!(
                "{}{}\t{}\n",
                saved.active,
                place.unwrap_or_default(),
                saved
                    .tabs
                    .iter()
                    .map(|tab| match tab.panels.is_empty() {
                        true => tab.path.clone(),
                        false => format!("{}{PANELS}{}", tab.path, tab.panels.join(",")),
                    })
                    .collect::<Vec<_>>()
                    .join("\t")
            )
        })
        .collect()
}

fn parse(text: &str) -> Vec<Saved> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t').peekable();
            let active = fields.next()?.parse().ok()?;
            let bounds = fields
                .next_if(|field| field.starts_with('@'))
                .and_then(|field| parse_place(&field[1..]));
            let tabs: Vec<Tab> = fields
                .filter(|tab| !tab.is_empty())
                .map(|field| {
                    let (path, panels) = field.split_once(PANELS).unwrap_or((field, ""));
                    Tab {
                        path: path.to_string(),
                        panels: panels
                            .split(',')
                            .filter(|panel| !panel.is_empty())
                            .map(str::to_string)
                            .collect(),
                    }
                })
                .collect();
            (!tabs.is_empty()).then_some(Saved {
                active,
                tabs,
                bounds,
            })
        })
        .collect()
}

fn parse_place(place: &str) -> Option<WindowBounds> {
    let numbers: Vec<f32> = place
        .split(',')
        .map(|n| n.parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, w, h, state] = numbers[..] else {
        return None;
    };
    let bounds = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
    Some(match state as u8 {
        1 => WindowBounds::Maximized(bounds),
        2 => WindowBounds::Fullscreen(bounds),
        _ => WindowBounds::Windowed(bounds),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_survive_saving_and_loading() {
        let place = Bounds::new(point(px(10.), px(40.)), size(px(1200.), px(800.)));
        let windows = vec![
            Saved {
                active: 1,
                tabs: vec![
                    Tab {
                        path: "/a".to_string(),
                        panels: vec!["attention".to_string(), "console".to_string()],
                    },
                    Tab {
                        path: "/b c".to_string(),
                        panels: Vec::new(),
                    },
                ],
                bounds: Some(WindowBounds::Maximized(place)),
            },
            Saved {
                active: 0,
                tabs: Vec::new(),
                bounds: None,
            },
            Saved {
                active: 0,
                tabs: vec![Tab {
                    path: "/d".to_string(),
                    panels: Vec::new(),
                }],
                bounds: None,
            },
        ];
        assert_eq!(
            parse(&format(&windows)),
            [windows[0].clone(), windows[2].clone()]
        );
        assert!(parse("x\t/a\n").is_empty());
        // Saved before places were.
        assert_eq!(parse("0\t/a\n")[0].tabs[0].path, "/a");
    }
}
