use std::path::Path;

use gpui::{
    Action, AnyElement, App, Bounds, ClickEvent, Context, Entity, ExternalPaths, MouseButton,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, point,
    prelude::*, px, size,
};

use crate::recent;
use crate::repo_view::RepoView;
use crate::theme::Theme;

actions!(
    wtf,
    [
        NewTab,
        CloseTab,
        CloseWindow,
        NextTab,
        PreviousTab,
        ReopenClosedTab,
        ClearRecent
    ]
);

/// Brings the tab at a place in the tab bar to the front; ⌘9 always the
/// last one, as in Safari.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = wtf, no_json)]
pub struct SelectTab {
    pub index: usize,
}

#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = wtf, no_json)]
pub struct OpenPath {
    pub path: String,
}

pub const TAB_BAR_HEIGHT: f32 = 36.;
/// Room on the left of the tab bar for the window's traffic lights.
const TRAFFIC_LIGHTS: f32 = 80.;
const MAX_TAB_WIDTH: f32 = 200.;

/// A window of repository tabs.
pub struct Workspace {
    tabs: Vec<Entity<RepoView>>,
    active: usize,
    /// Repositories of closed tabs, the last closed at the end.
    closed: Vec<String>,
    /// A press landed on a tab or button, not on the bar's empty part.
    tab_pressed: bool,
    /// The options of the Pull (`false`) or Push (`true`) button, open.
    sync_menu: Option<bool>,
    /// A menu closed by the press that began a click, so the click on its
    /// chevron does not open it again.
    sync_menu_closed: Option<(bool, std::time::Instant)>,
}

/// How many closed tabs the window can reopen.
const MAX_CLOSED: usize = 20;

/// A tab being dragged to a new place in the tab bar.
struct DraggedTab {
    index: usize,
    title: String,
}

/// What follows the mouse while a tab is dragged, and a button's tooltip.
struct TabPreview(String);

impl Render for TabPreview {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        div()
            .px(px(10.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_xs()
            .shadow_md()
            .child(self.0.clone())
    }
}

/// Opens a window with a tab for each path, or one empty tab.
pub fn open_window(paths: Vec<String>, cx: &mut App) {
    open_window_at(paths, 0, cx);
}

/// Opens a window on `paths`, with the `active` one in front.
pub fn open_window_at(paths: Vec<String>, active: usize, cx: &mut App) {
    // Each new window steps down and right from the last, as macOS cascades.
    let mut bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
    let step = px(28. * cx.windows().len() as f32);
    bounds.origin = point(bounds.origin.x + step, bounds.origin.y + step);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // The tab bar is drawn in the title bar, next to the traffic lights.
        titlebar: Some(TitlebarOptions {
            title: Some("wtf".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(12.), px(12.))),
        }),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| Workspace::new(paths, active, window, cx))
    });
    if let Err(error) = opened {
        eprintln!("cannot open a window: {error}");
    }
}

impl Workspace {
    fn new(paths: Vec<String>, active: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.activate(this.active, window, cx);
            } else if let Some(tab) = this.tabs.get(this.active) {
                tab.update(cx, |tab, _| tab.set_window_inactive());
            }
        })
        .detach();
        let mut paths: Vec<Option<String>> = paths.into_iter().map(Some).collect();
        if paths.is_empty() {
            paths.push(None);
        }
        let tabs = paths
            .into_iter()
            .map(|path| Self::new_tab(path, window, cx))
            .collect();
        let mut workspace = Self {
            tabs,
            active: 0,
            closed: Vec::new(),
            tab_pressed: false,
            sync_menu: None,
            sync_menu_closed: None,
        };
        workspace.activate(active, window, cx);
        workspace
    }

    fn new_tab(
        path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<RepoView> {
        cx.new(|cx| {
            let mut view = RepoView::new(window, cx);
            if let Some(path) = path {
                view.open(path, cx);
            }
            view
        })
    }

    /// Shows a tab; only the active tab drives the menu bar and has focus.
    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = index.min(self.tabs.len().saturating_sub(1));
        for (k, tab) in self.tabs.iter().enumerate() {
            let active = k == self.active;
            tab.update(cx, |tab, cx| tab.set_active(active, window, cx));
        }
        cx.notify();
    }

    /// Opens a repository: in the current tab when it is empty, in the tab
    /// that already shows it, or in a new tab.
    fn open_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let showing =
            |tab: &Entity<RepoView>, cx: &App| tab.read(cx).repo_path().map(str::to_string);
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| showing(tab, cx).as_deref() == Some(path.as_str()))
        {
            self.activate(index, window, cx);
        } else if showing(&self.tabs[self.active], cx).is_none() {
            self.tabs[self.active].update(cx, |tab, cx| tab.open(path, cx));
            self.activate(self.active, window, cx);
        } else {
            self.tabs.push(Self::new_tab(Some(path), window, cx));
            self.activate(self.tabs.len() - 1, window, cx);
        }
    }

    fn add_empty_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.push(Self::new_tab(None, window, cx));
        self.activate(self.tabs.len() - 1, window, cx);
    }

    /// Closes a tab; closing the last one closes the window.
    fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() <= 1 {
            window.remove_window();
            return;
        }
        let tab = self.tabs.remove(index);
        if let Some(path) = tab.read(cx).repo_path() {
            self.closed.push(path.to_string());
            let extra = self.closed.len().saturating_sub(MAX_CLOSED);
            self.closed.drain(..extra);
        }
        let active = if self.active > index {
            self.active - 1
        } else {
            self.active
        };
        self.activate(active, window, cx);
    }

    /// Reopens the last closed tab; each further call goes back one more.
    fn reopen_closed_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.closed.pop() {
            self.open_path(path, window, cx);
        }
    }

    /// Moves a tab to where another one is, keeping the same tab active.
    fn move_tab(&mut self, from: usize, to: usize, window: &mut Window, cx: &mut Context<Self>) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let active = self.tabs[self.active].clone();
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        let index = self.tabs.iter().position(|tab| *tab == active).unwrap_or(0);
        self.activate(index, window, cx);
    }

    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len();
        let next = match forward {
            true => (self.active + 1) % count,
            false => (self.active + count - 1) % count,
        };
        self.activate(next, window, cx);
    }

    fn tab_bar(&self, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut bar = div()
            .id("tab-bar")
            .flex()
            .flex_none()
            .items_end()
            .gap(px(2.))
            .h(px(TAB_BAR_HEIGHT))
            // Full screen hides the traffic lights, so tabs start at the edge.
            .pl(px(match window.is_fullscreen() {
                true => 8.,
                false => TRAFFIC_LIGHTS,
            }))
            .pr(px(8.))
            .bg(theme.graph_background)
            .border_b_1()
            .border_color(theme.separator)
            .text_xs()
            // The empty part of the bar moves the window, as a title bar does;
            // a press on a tab or button is theirs, so a tab can be dragged.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| {
                    if !std::mem::take(&mut this.tab_pressed) {
                        window.start_window_move();
                    }
                }),
            )
            .on_click(|event: &ClickEvent, window, _| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            });
        for (index, tab) in self.tabs.iter().enumerate() {
            let view = tab.read(cx);
            let title = view.repo_name().unwrap_or_else(|| "New Tab".to_string());
            let active = index == self.active;
            let group = SharedString::from(format!("tab-{index}"));
            let dragged = DraggedTab {
                index,
                title: title.clone(),
            };
            let highlight = theme.selection;
            bar = bar.child(
                div()
                    .id(("tab", index))
                    .group(group.clone())
                    // Dragging a tab onto another moves it there.
                    .on_drag(dragged, |tab, _, _, cx| {
                        cx.new(|_| TabPreview(tab.title.clone()))
                    })
                    .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(highlight))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                        this.move_tab(dragged.index, index, window, cx)
                    }))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .max_w(px(MAX_TAB_WIDTH))
                    .h(px(TAB_BAR_HEIGHT - 6.))
                    .px(px(10.))
                    .rounded_t(px(6.))
                    .border_1()
                    .border_b_0()
                    .map(|tab| match active {
                        true => tab.bg(theme.background).border_color(theme.separator),
                        false => tab
                            .border_color(gpui::transparent_black())
                            .text_color(theme.secondary)
                            .hover(|tab| tab.bg(theme.selection)),
                    })
                    .cursor_pointer()
                    // The title bar would move the window with the tab; it holds
                    // still until the mouse comes up.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, _| {
                            this.tab_pressed = true;
                            set_window_movable(window, false);
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.activate(index, window, cx);
                    }))
                    // The close button leads, as in Safari and Finder tabs.
                    .child(
                        div()
                            .id(("tab-close", index))
                            .px(px(2.))
                            .rounded(px(3.))
                            .opacity(if active { 0.6 } else { 0. })
                            .group_hover(group, |close| close.opacity(1.))
                            .hover(|close| close.bg(theme.selection))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(index, window, cx);
                            }))
                            .child("×"),
                    )
                    .child(div().truncate().child(title)),
            );
        }
        bar.child(
            div()
                .id("tab-new")
                .flex()
                .items_center()
                .h(px(TAB_BAR_HEIGHT - 6.))
                .px(px(8.))
                .text_color(theme.secondary)
                .rounded(px(4.))
                .hover(|new| new.bg(theme.selection))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.tab_pressed = true),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.add_empty_tab(window, cx);
                }))
                .child("+"),
        )
        .child(div().flex_1())
        .when(
            self.tabs[self.active].read(cx).repo_path().is_some(),
            |bar| {
                // Centred in the bar, whose tabs sit at its bottom.
                bar.child(
                    div()
                        .h_full()
                        .flex()
                        .items_center()
                        .child(self.sync_button(false, theme, cx))
                        .child(self.sync_button(true, theme, cx))
                        .child(self.fetch_button(theme, cx)),
                )
            },
        )
    }

    /// Fetches every remote now, as the background fetches do; the graph
    /// then marks branches whose remote is gone. It deletes nothing.
    fn fetch_button(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.tabs[self.active].read(cx);
        let running = view.fetching();
        let last = view
            .fetched_at()
            .map(|at| crate::repo_view::short_age(at, crate::repo_view::now_seconds()));
        div()
            .id("fetch-run")
            .flex()
            .items_center()
            .gap(px(4.))
            .ml(px(6.))
            .px(px(8.))
            .h(px(TAB_BAR_HEIGHT - 12.))
            .rounded(px(5.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .cursor_pointer()
            .hover(|button| button.bg(theme.selection))
            .when(running, |button| button.text_color(theme.secondary))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| {
                    this.tab_pressed = true;
                    set_window_movable(window, false);
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.sync_menu = None;
                this.tabs[this.active].update(cx, |view, cx| view.fetch_now(cx));
            }))
            // ⟳ draws smaller than the arrows of Pull and Push: it is set
            // larger to match them, without growing the button.
            .child(
                div()
                    .text_size(px((crate::preferences::ui_size() * 1.25).round()))
                    .line_height(px(TAB_BAR_HEIGHT - 14.))
                    .child("⟳"),
            )
            .when(running, |button| button.child("…"))
            .tooltip(move |_, cx| {
                let text = match &last {
                    Some(age) => format!("Fetch all remotes · last {age} ago"),
                    None => "Fetch all remotes".to_string(),
                };
                cx.new(|_| TabPreview(text)).into()
            })
    }

    /// The Pull or Push button: its main part runs the chosen option, its
    /// chevron lists the options to choose from. Counts show what waits.
    fn sync_button(&self, push: bool, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.tabs[self.active].read(cx);
        let (ahead, behind) = view.sync_counts();
        let running = view.syncing() == Some(push);
        let (arrow, count) = match push {
            true => ("↑", ahead),
            false => ("↓", behind),
        };
        let option = crate::preferences::sync_option(push);
        let options = crate::sync::options(push);
        let label = options
            .get(option)
            .map(|option| crate::sync::label(push, option))
            .unwrap_or_default();
        let open = self.sync_menu == Some(push);
        let part = |id: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .h_full()
                .cursor_pointer()
                .hover(|part| part.bg(theme.selection))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, _| {
                        this.tab_pressed = true;
                        set_window_movable(window, false);
                    }),
                )
        };
        div()
            .flex()
            .items_center()
            .ml(px(6.))
            .h(px(TAB_BAR_HEIGHT - 12.))
            .rounded(px(5.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .overflow_hidden()
            .child(
                part(if push { "push-run" } else { "pull-run" })
                    .gap(px(4.))
                    .px(px(8.))
                    .when(running, |part| part.text_color(theme.secondary))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.sync_menu = None;
                        this.tabs[this.active].update(cx, |view, cx| view.sync(push, option, cx));
                    }))
                    .child(arrow)
                    .when(count > 0, |part| part.child(count.to_string()))
                    .when(running, |part| part.child("…"))
                    .tooltip(move |_, cx| cx.new(|_| TabPreview(format!("git {label}"))).into()),
            )
            .child(div().w(px(1.)).h_full().bg(theme.separator))
            .child(
                part(if push { "push-options" } else { "pull-options" })
                    .px(px(5.))
                    .when(open, |part| part.bg(theme.selection))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let just_closed = this.sync_menu_closed.take().is_some_and(|(menu, at)| {
                            menu == push && at.elapsed() < std::time::Duration::from_millis(500)
                        });
                        this.sync_menu = match (this.sync_menu, just_closed) {
                            (Some(shown), _) if shown == push => None,
                            (_, true) => None,
                            _ => Some(push),
                        };
                        cx.notify();
                    }))
                    .child(crate::repo_view::chevron(
                        true,
                        theme.secondary,
                        (crate::preferences::ui_size() * 0.75).round(),
                    )),
            )
    }

    /// The options of the open Pull or Push menu: each runs at once and
    /// becomes what its button runs.
    fn sync_menu(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let push = self.sync_menu?;
        let options = crate::sync::options(push);
        let chosen = crate::preferences::sync_option(push);
        let rows = options.iter().enumerate().map(|(k, option)| {
            div()
                .id(("sync-option", k))
                .flex()
                .items_center()
                .gap(px(24.))
                .px(px(10.))
                .py(px(5.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|row| row.bg(theme.selection))
                .when(k == chosen, |row| row.bg(theme.selection.opacity(0.5)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.sync_menu = None;
                    crate::preferences::set_sync_option(push, k, cx);
                    this.tabs[this.active].update(cx, |view, cx| view.sync(push, k, cx));
                }))
                .child(div().flex_none().child(crate::sync::label(push, option)))
                .child(div().flex_1())
                .child(
                    div()
                        .flex_none()
                        .italic()
                        .text_color(theme.secondary)
                        .child(option.about),
                )
        });
        // With more than one remote, the Push menu picks where it goes.
        let view = self.tabs[self.active].read(cx);
        let remotes = match push {
            true => view.remotes().to_vec(),
            false => Vec::new(),
        };
        let target = view.push_remote();
        let to = (remotes.len() > 1).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .pt(px(4.))
                .pb(px(6.))
                .mb(px(2.))
                .border_b_1()
                .border_color(theme.separator)
                .child(div().text_color(theme.secondary).child("Push to"))
                .children(remotes.into_iter().map(|remote| {
                    let picked = remote == target;
                    div()
                        .id(SharedString::from(format!("push-to-{remote}")))
                        .px(px(8.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(match picked {
                            true => theme.accent,
                            false => theme.separator,
                        })
                        .when(picked, |chip| chip.bg(theme.selection))
                        .cursor_pointer()
                        .hover(|chip| chip.bg(theme.selection))
                        .child(remote.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let remote = remote.clone();
                            this.tabs[this.active]
                                .update(cx, |view, cx| view.set_push_remote(remote, cx));
                        }))
                }))
        });
        let width = f32::from(window.viewport_size().width);
        let panel = div()
            .id("sync-menu")
            .occlude()
            .flex()
            .flex_col()
            .p(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .shadow_lg()
            .text_sm()
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                this.sync_menu = None;
                this.sync_menu_closed = Some((push, std::time::Instant::now()));
                cx.notify();
            }))
            .children(to)
            .children(rows);
        Some(
            gpui::deferred(
                gpui::anchored()
                    .anchor(gpui::Corner::TopRight)
                    .position(point(px(width - 8.), px(TAB_BAR_HEIGHT)))
                    .child(panel),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

/// Lets the title bar move the window, or not: AppKit moves it on any drag
/// that starts there, tabs included.
// objc's macros check a `cargo-clippy` feature this crate does not declare.
#[allow(unexpected_cfgs)]
fn set_window_movable(window: &Window, movable: bool) {
    use objc::{msg_send, runtime::Object, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    if let RawWindowHandle::AppKit(handle) = handle.as_raw() {
        let view = handle.ns_view.as_ptr() as *mut Object;
        // SAFETY: the view is the window's live content view, on the main
        // thread, and both messages exist on every macOS version.
        unsafe {
            let ns_window: *mut Object = msg_send![view, window];
            if !ns_window.is_null() {
                let movable = movable as objc::runtime::BOOL;
                let () = msg_send![ns_window, setMovable: movable];
            }
        }
    }
}

/// What a path to open is kept as: the top of its repository, else the
/// path in full, so `.`, a file inside or a link name it the same, in recent
/// tabs and the session, from any folder.
pub fn repository_path(path: &str) -> String {
    wtf_core::repository_root(path)
        .or_else(|| {
            Some(
                std::fs::canonicalize(path)
                    .ok()?
                    .to_string_lossy()
                    .into_owned(),
            )
        })
        .unwrap_or_else(|| path.to_string())
}

/// Opens paths the system hands the app, such as folders dropped on its Dock
/// icon: in the front window, or in a new one when there is none.
pub fn open_paths(paths: Vec<String>, cx: &mut App) {
    let roots: Vec<String> = paths.iter().map(|path| repository_path(path)).collect();
    let front = cx.active_window().or_else(|| cx.windows().first().copied());
    let opened = front.is_some_and(|window| {
        window
            .update(cx, |_, window, cx| {
                for path in &roots {
                    window.dispatch_action(Box::new(OpenPath { path: path.clone() }), cx);
                }
            })
            .is_ok()
    });
    if !opened {
        open_window(roots, cx);
    }
}

/// A local path from a `file://` URL, with percent escapes decoded.
pub fn path_from_url(url: &str) -> Option<String> {
    let path = url.strip_prefix("file://")?;
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|hex| std::str::from_utf8(hex).ok());
        match (
            bytes[i],
            hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()),
        ) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                i += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                i += 1;
            }
        }
    }
    let path = String::from_utf8(decoded).ok()?;
    Some(path.trim_end_matches('/').to_string())
}

/// The last folder of a path, as tabs, worktrees and menus name it.
pub fn folder_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or(path.to_string(), |name| name.to_string_lossy().into_owned())
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Text and controls scale with the interface size from Settings.
        window.set_rem_size(px(crate::preferences::ui_size()));
        let theme = Theme::for_appearance(window.appearance());
        let active = self.tabs[self.active].clone();
        // Remembers the tabs, to open them again when wtf next starts.
        let mut front = 0;
        let mut tabs = Vec::new();
        for (k, tab) in self.tabs.iter().enumerate() {
            if let Some(path) = tab.read(cx).repo_path() {
                if k == self.active {
                    front = tabs.len();
                }
                tabs.push(path.to_string());
            }
        }
        crate::session::remember(window.window_handle().window_id(), front, tabs, cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(crate::preferences::ui_font())
            .on_mouse_up(MouseButton::Left, |_, window, _| {
                set_window_movable(window, true)
            })
            .on_action(cx.listener(|this, action: &OpenPath, window, cx| {
                this.open_path(action.path.clone(), window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewTab, window, cx| this.add_empty_tab(window, cx)))
            .on_action(
                cx.listener(|this, _: &CloseTab, window, cx| {
                    this.close_tab(this.active, window, cx)
                }),
            )
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.step(true, window, cx)))
            .on_action(cx.listener(|this, action: &SelectTab, window, cx| {
                let index = match action.index {
                    8 => this.tabs.len() - 1,
                    index => index,
                };
                if index < this.tabs.len() {
                    this.activate(index, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ReopenClosedTab, window, cx| {
                this.reopen_closed_tab(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &PreviousTab, window, cx| this.step(false, window, cx)),
            )
            .on_action(cx.listener(|this, _: &ClearRecent, window, cx| {
                recent::clear();
                this.activate(this.active, window, cx);
            }))
            // Dropped folders open like ⌘O: in this tab when empty, else in a new one.
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_2().border_color(theme.accent)
            })
            .on_drop(cx.listener(|this, dropped: &ExternalPaths, window, cx| {
                for path in dropped.paths() {
                    let path = path.to_string_lossy();
                    let root = repository_path(&path);
                    this.open_path(root, window, cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.sync_menu.take().is_some() {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(self.tab_bar(&theme, window, cx))
            .child(div().flex_1().min_h_0().child(active))
            .children(self.sync_menu(&theme, window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_paths_from_file_urls() {
        assert_eq!(
            path_from_url("file:///Users/me/My%20Repo/").as_deref(),
            Some("/Users/me/My Repo")
        );
        assert_eq!(path_from_url("https://example.com"), None);
    }
}
