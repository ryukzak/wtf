use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

use gpui::{
    AnyElement, App, Bounds, Context, FocusHandle, Global, KeyDownEvent, SharedString, Stateful,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, prelude::*, px, size,
};

use crate::theme::{Appearance, Theme};
use crate::workspace::{CloseTab, CloseWindow};

/// How the workflow graph is laid out and drawn.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GraphStyle {
    pub row_height: f32,
    /// Between a node and the next one, in topology mode.
    pub column: f32,
    /// Between the beads of a node's commits.
    pub bead_spacing: f32,
    /// The most beads one node shows.
    pub beads: usize,
    pub dot: f32,
    pub line: f32,
    pub hover_line: f32,
    /// How far a rail runs while changing rows.
    pub curve: f32,
    /// Compressed time shortens idle gaps to this.
    pub idle_minutes: u32,
}

impl Default for GraphStyle {
    fn default() -> Self {
        Self {
            row_height: 24.,
            column: 64.,
            bead_spacing: 7.,
            beads: 20,
            dot: 10.,
            line: 2.,
            hover_line: 4.,
            curve: 20.,
            idle_minutes: 60,
        }
    }
}

impl GraphStyle {
    pub fn idle_gap(&self) -> i64 {
        i64::from(self.idle_minutes) * 60
    }
}

#[derive(Clone, PartialEq, Debug)]
struct Settings {
    /// The interface font; `.SystemUIFont` is macOS's own.
    ui_font: SharedString,
    code_font: SharedString,
    code_size: f32,
    /// The interface's rem size; text and controls scale with it.
    ui_size: f32,
    /// Days without a commit after which a branch reads as idle.
    idle_days: u32,
    /// Summarize several commits with Apple's on-device model; `None` until
    /// the first run finds out whether this Mac has it.
    summaries: Option<bool>,
    /// What the model is told to summarize commits and to write commit
    /// messages; `None` for the built-in prompts.
    summary_prompt: Option<String>,
    message_prompt: Option<String>,
    graph: GraphStyle,
    /// Minutes between background fetches; 0 for none.
    fetch_minutes: u32,
    /// Fetches drop remote branches deleted on the remote.
    fetch_prune: bool,
    /// Labels show how far a branch is ahead of and behind its upstream.
    sync_badges: bool,
    /// What the Pull and Push buttons run, as places in their option lists.
    pull_option: usize,
    push_option: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_font: SYSTEM_FONT.into(),
            code_font: "Menlo".into(),
            code_size: 12.,
            ui_size: 16.,
            idle_days: 14,
            summaries: None,
            summary_prompt: None,
            message_prompt: None,
            graph: GraphStyle::default(),
            fetch_minutes: 5,
            fetch_prune: true,
            sync_badges: true,
            pull_option: 1,
            push_option: 0,
        }
    }
}

static SETTINGS: LazyLock<RwLock<Settings>> = LazyLock::new(Default::default);

fn read() -> std::sync::RwLockReadGuard<'static, Settings> {
    SETTINGS.read().unwrap_or_else(|error| error.into_inner())
}

pub fn graph() -> GraphStyle {
    read().graph
}

pub fn ui_font() -> SharedString {
    read().ui_font.clone()
}

pub fn code_font() -> SharedString {
    read().code_font.clone()
}

pub fn code_size() -> f32 {
    read().code_size
}

pub fn ui_size() -> f32 {
    read().ui_size
}

pub fn idle_days() -> u32 {
    read().idle_days
}

pub fn fetch_minutes() -> u32 {
    read().fetch_minutes
}

pub fn fetch_prune() -> bool {
    read().fetch_prune
}

pub fn sync_badges() -> bool {
    read().sync_badges
}

pub fn pull_option() -> usize {
    read().pull_option
}

pub fn push_option() -> usize {
    read().push_option
}

/// Makes an option what the Pull or Push button runs.
pub fn set_sync_option(push: bool, option: usize, cx: &mut App) {
    change(
        |settings| match push {
            true => settings.push_option = option,
            false => settings.pull_option = option,
        },
        cx,
    );
}

/// The background fetch intervals offered, in minutes; 0 is off.
const FETCH_MINUTES: [u32; 6] = [0, 1, 5, 15, 30, 60];

pub fn summaries() -> bool {
    read().summaries == Some(true)
}

/// On the first run, turns summaries on when this Mac can make them; later,
/// the user's choice stays.
pub fn decide_summaries(available: bool, cx: &mut App) {
    if read().summaries.is_none() {
        change(|settings| settings.summaries = Some(available), cx);
    }
}

pub fn summary_prompt() -> String {
    read()
        .summary_prompt
        .clone()
        .unwrap_or_else(|| wtf_core::SUMMARY_PROMPT.to_string())
}

pub fn message_prompt() -> String {
    read()
        .message_prompt
        .clone()
        .unwrap_or_else(|| wtf_core::MESSAGE_PROMPT.to_string())
}

pub fn summaries_decided() -> bool {
    read().summaries.is_some()
}

/// macOS's interface font, offered as "System".
const SYSTEM_FONT: &str = ".SystemUIFont";

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn file() -> PathBuf {
    let home = env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/wtf/settings")
}

/// Applies the saved preferences; missing or unreadable ones keep defaults.
pub fn load() {
    let (settings, appearance) = parse(&fs::read_to_string(file()).unwrap_or_default());
    appearance.set();
    *SETTINGS.write().unwrap_or_else(|error| error.into_inner()) = settings;
}

/// Reads `key=value` lines; unknown keys and bad values keep defaults.
fn parse(text: &str) -> (Settings, Appearance) {
    let mut settings = Settings::default();
    let mut appearance = Appearance::System;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let number = value.parse::<f32>().ok();
        let graph = &mut settings.graph;
        match (key.trim(), number) {
            ("appearance", _) => {
                appearance = match value {
                    "light" => Appearance::Light,
                    "dark" => Appearance::Dark,
                    _ => Appearance::System,
                }
            }
            ("ui_font", _) if !value.is_empty() => settings.ui_font = value.to_string().into(),
            ("code_font", _) if !value.is_empty() => settings.code_font = value.to_string().into(),
            ("code_size", Some(n)) => settings.code_size = n,
            ("ui_size", Some(n)) => settings.ui_size = n,
            ("idle_days", Some(n)) => settings.idle_days = n as u32,
            ("summaries", _) => settings.summaries = Some(value == "on"),
            ("fetch_minutes", Some(n)) => settings.fetch_minutes = n as u32,
            ("fetch_prune", _) => settings.fetch_prune = value == "on",
            ("sync_badges", _) => settings.sync_badges = value == "on",
            ("pull_option", Some(n)) => settings.pull_option = n as usize,
            ("push_option", Some(n)) => settings.push_option = n as usize,
            // Prompts keep their lines as `\n`.
            ("summary_prompt", _) => settings.summary_prompt = Some(value.replace("\\n", "\n")),
            ("message_prompt", _) => settings.message_prompt = Some(value.replace("\\n", "\n")),
            ("graph.row_height", Some(n)) => graph.row_height = n,
            ("graph.column", Some(n)) => graph.column = n,
            ("graph.bead_spacing", Some(n)) => graph.bead_spacing = n,
            ("graph.beads", Some(n)) => graph.beads = n as usize,
            ("graph.dot", Some(n)) => graph.dot = n,
            ("graph.line", Some(n)) => graph.line = n,
            ("graph.hover_line", Some(n)) => graph.hover_line = n,
            ("graph.curve", Some(n)) => graph.curve = n,
            ("graph.idle_minutes", Some(n)) => graph.idle_minutes = n as u32,
            _ => {}
        }
    }
    (settings, appearance)
}

fn format(settings: &Settings, appearance: Appearance) -> String {
    let appearance = match appearance {
        Appearance::System => "system",
        Appearance::Light => "light",
        Appearance::Dark => "dark",
    };
    let g = settings.graph;
    [
        format!("appearance={appearance}"),
        format!("ui_font={}", settings.ui_font),
        format!("code_font={}", settings.code_font),
        format!("code_size={}", settings.code_size),
        format!("ui_size={}", settings.ui_size),
        format!("idle_days={}", settings.idle_days),
        format!("fetch_minutes={}", settings.fetch_minutes),
        format!("fetch_prune={}", on_off(settings.fetch_prune)),
        format!("sync_badges={}", on_off(settings.sync_badges)),
        format!("pull_option={}", settings.pull_option),
        format!("push_option={}", settings.push_option),
        match settings.summaries {
            Some(true) => "summaries=on".to_string(),
            Some(false) => "summaries=off".to_string(),
            None => String::new(),
        },
        settings.summary_prompt.as_ref().map_or(String::new(), |p| {
            format!("summary_prompt={}", p.replace('\n', "\\n"))
        }),
        settings.message_prompt.as_ref().map_or(String::new(), |p| {
            format!("message_prompt={}", p.replace('\n', "\\n"))
        }),
        format!("graph.row_height={}", g.row_height),
        format!("graph.column={}", g.column),
        format!("graph.bead_spacing={}", g.bead_spacing),
        format!("graph.beads={}", g.beads),
        format!("graph.dot={}", g.dot),
        format!("graph.line={}", g.line),
        format!("graph.hover_line={}", g.hover_line),
        format!("graph.curve={}", g.curve),
        format!("graph.idle_minutes={}", g.idle_minutes),
    ]
    .into_iter()
    .filter(|line| !line.is_empty())
    .collect::<Vec<_>>()
    .join("\n")
        + "\n"
}

/// Saves the preferences. Failing to save only loses them on restart, so
/// errors are ignored.
fn save() {
    let text = format(&read(), Appearance::current());
    let file = file();
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).ok();
    }
    fs::write(file, text).ok();
}

/// Changes the settings, saves them and redraws every window.
fn change(update: impl FnOnce(&mut Settings), cx: &mut App) {
    update(&mut SETTINGS.write().unwrap_or_else(|error| error.into_inner()));
    save();
    cx.refresh_windows();
}

/// The open Preferences window, so a second ⌘, brings it forward.
struct PreferencesWindow(Option<WindowHandle<Preferences>>);

impl Global for PreferencesWindow {}

pub fn open(cx: &mut App) {
    let existing = cx
        .try_global::<PreferencesWindow>()
        .and_then(|window| window.0);
    if let Some(handle) = existing
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(600.), px(480.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            ..Default::default()
        }),
        is_resizable: false,
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            Preferences {
                focus,
                fonts: installed_fonts(window, cx),
                section: Section::Appearance,
                font_menu: None,
                font_query: String::new(),
                prompt_focus: [cx.focus_handle(), cx.focus_handle()],
            }
        })
    });
    if let Ok(handle) = opened {
        cx.set_global(PreferencesWindow(Some(handle)));
    }
}

pub struct Preferences {
    focus: FocusHandle,
    /// Installed font families, each with whether it is monospace.
    fonts: Vec<(String, bool)>,
    /// The section shown, picked in the sidebar.
    section: Section,
    /// The font dropdown, when open, and where.
    font_menu: Option<(FontSetting, gpui::Point<gpui::Pixels>)>,
    font_query: String,
    /// The prompt fields, each with its own focus.
    prompt_focus: [FocusHandle; 2],
}

/// Which prompt a field edits.
#[derive(Clone, Copy, PartialEq)]
enum PromptSetting {
    Summary,
    Message,
}

/// The settings window's sections, in sidebar order.
#[derive(Clone, Copy, PartialEq)]
enum Section {
    Appearance,
    Fonts,
    Graph,
    Review,
    Remote,
}

impl Section {
    const ALL: [Section; 5] = [
        Section::Appearance,
        Section::Fonts,
        Section::Graph,
        Section::Review,
        Section::Remote,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Fonts => "Fonts",
            Section::Graph => "Workflow graph",
            Section::Review => "Review",
            Section::Remote => "Remote",
        }
    }
}

/// Which font a dropdown sets.
#[derive(Clone, Copy, PartialEq)]
enum FontSetting {
    Interface,
    Code,
}

/// Installed font families, sorted, each with whether its letters are all
/// one width.
fn installed_fonts(window: &Window, cx: &App) -> Vec<(String, bool)> {
    let mut names = cx.text_system().all_font_names();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    let text = window.text_system();
    names
        .into_iter()
        .filter(|name| !name.starts_with('.'))
        .map(|name| {
            let id = text.resolve_font(&gpui::font(name.clone()));
            let width = |c| text.advance(id, px(12.), c).map(|size| size.width).ok();
            let mono = width('i').is_some() && width('i') == width('M');
            (name, mono)
        })
        .collect()
}

/// A labeled row of the settings window.
fn row(label: &'static str, theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(26.))
        .child(div().w(px(150.)).text_color(theme.secondary).child(label))
}

fn small_button(id: SharedString, label: &'static str, theme: &Theme) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(22.))
        .h(px(20.))
        .rounded(px(4.))
        .border_1()
        .border_color(theme.separator)
        .hover(|button| button.bg(theme.selection))
        .cursor_pointer()
        .child(label)
}

impl Preferences {
    /// A − value + stepper changing one number of the settings.
    fn stepper(
        &self,
        id: &'static str,
        value: String,
        step: impl Fn(&mut Settings, f32) + Clone + 'static,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let down = step.clone();
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                small_button(SharedString::from(format!("{id}-down")), "−", theme).on_click(
                    cx.listener(move |_, _, _, cx| {
                        let down = down.clone();
                        change(move |settings| down(settings, -1.), cx)
                    }),
                ),
            )
            .child(div().w(px(48.)).flex().justify_center().child(value))
            .child(
                small_button(SharedString::from(format!("{id}-up")), "+", theme).on_click(
                    cx.listener(move |_, _, _, cx| {
                        let step = step.clone();
                        change(move |settings| step(settings, 1.), cx)
                    }),
                ),
            )
    }

    /// Typing filters an open font dropdown; Esc closes it, then the window.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if self.font_menu.is_none() {
            if keystroke.key == "escape" {
                window.remove_window();
            }
            return;
        }
        match keystroke.key.as_str() {
            "escape" => {
                self.font_menu = None;
                self.font_query.clear();
            }
            "backspace" => {
                self.font_query.pop();
            }
            _ if !keystroke.modifiers.platform && !keystroke.modifiers.control => {
                match &keystroke.key_char {
                    Some(text) if !text.chars().any(char::is_control) => {
                        self.font_query.push_str(text)
                    }
                    _ => return,
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// A prompt the model is told, to edit in place, with a way back to the
    /// built-in one.
    fn prompt_field(
        &self,
        setting: PromptSetting,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (label, text, custom, focus) = match setting {
            PromptSetting::Summary => (
                "Summary prompt",
                summary_prompt(),
                read().summary_prompt.is_some(),
                &self.prompt_focus[0],
            ),
            PromptSetting::Message => (
                "Commit message prompt",
                message_prompt(),
                read().message_prompt.is_some(),
                &self.prompt_focus[1],
            ),
        };
        let focused = focus.is_focused(window);
        let caret = if focused { "▏" } else { "" };
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(px(4.))
            .mt(px(10.))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(div().text_color(theme.secondary).child(label))
                    .child(div().flex_1())
                    .when(custom, |row| {
                        row.child(
                            div()
                                .id(label)
                                .text_xs()
                                .text_color(theme.accent)
                                .cursor_pointer()
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    change(
                                        |settings| match setting {
                                            PromptSetting::Summary => {
                                                settings.summary_prompt = None
                                            }
                                            PromptSetting::Message => {
                                                settings.message_prompt = None
                                            }
                                        },
                                        cx,
                                    )
                                }))
                                .child("Reset to built-in"),
                        )
                    }),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{label}-field")))
                    .track_focus(focus)
                    .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                        let keystroke = &event.keystroke;
                        let mut text = match setting {
                            PromptSetting::Summary => summary_prompt(),
                            PromptSetting::Message => message_prompt(),
                        };
                        let modifiers = keystroke.modifiers;
                        match keystroke.key.as_str() {
                            "enter" if !modifiers.platform => text.push('\n'),
                            "backspace" => {
                                text.pop();
                            }
                            "v" if modifiers.platform => {
                                if let Some(pasted) =
                                    cx.read_from_clipboard().and_then(|item| item.text())
                                {
                                    text.push_str(&pasted);
                                }
                            }
                            _ if !modifiers.platform && !modifiers.control => {
                                match &keystroke.key_char {
                                    Some(typed) if !typed.chars().any(char::is_control) => {
                                        text.push_str(typed)
                                    }
                                    _ => return,
                                }
                            }
                            _ => return,
                        }
                        cx.stop_propagation();
                        change(
                            |settings| match setting {
                                PromptSetting::Summary => settings.summary_prompt = Some(text),
                                PromptSetting::Message => settings.message_prompt = Some(text),
                            },
                            cx,
                        );
                    }))
                    .w_full()
                    .min_h(px(64.))
                    .max_h(px(120.))
                    .overflow_y_scroll()
                    .whitespace_normal()
                    .p(px(6.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(if focused {
                        theme.accent
                    } else {
                        theme.separator
                    })
                    .text_xs()
                    .child(format!("{text}{caret}")),
            )
    }

    /// A button showing a font, opening the dropdown to choose another.
    fn font_button(
        &self,
        setting: FontSetting,
        current: SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label: SharedString = match current.as_ref() {
            SYSTEM_FONT => "System".into(),
            _ => current.clone(),
        };
        div()
            .id(match setting {
                FontSetting::Interface => "ui-font",
                FontSetting::Code => "code-font",
            })
            .flex()
            .items_center()
            .justify_between()
            .w(px(200.))
            .h(px(22.))
            .px(px(8.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme.separator)
            .hover(|button| button.bg(theme.selection))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    this.font_menu = Some((setting, event.position));
                    this.font_query.clear();
                    this.focus.focus(window);
                    cx.notify();
                }),
            )
            .child(div().truncate().font_family(current).child(label))
            .child(div().text_color(theme.secondary).child("▾"))
    }

    /// The open font dropdown: the filter as typed, then matching fonts, with
    /// monospace ones first for code.
    fn font_menu(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (setting, position) = self.font_menu?;
        let query = self.font_query.to_lowercase();
        let mut fonts: Vec<(String, bool)> = self
            .fonts
            .iter()
            .filter(|(name, _)| name.to_lowercase().contains(&query))
            .cloned()
            .collect();
        if setting == FontSetting::Code {
            fonts.sort_by_key(|(_, mono)| !mono);
        } else if "system".contains(&query) {
            fonts.insert(0, (SYSTEM_FONT.to_string(), false));
        }
        let current = match setting {
            FontSetting::Interface => ui_font(),
            FontSetting::Code => code_font(),
        };
        let items = fonts.into_iter().enumerate().map(|(k, (name, mono))| {
            let label = if name == SYSTEM_FONT {
                "System".to_string()
            } else {
                name.clone()
            };
            let chosen = current.as_ref() == name;
            let font: SharedString = name.into();
            div()
                .id(("font", k))
                .flex()
                .gap(px(6.))
                .px(px(8.))
                .py(px(2.))
                .hover(|item| item.bg(theme.selection))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    let font = font.clone();
                    this.font_menu = None;
                    change(
                        move |settings| match setting {
                            FontSetting::Interface => settings.ui_font = font,
                            FontSetting::Code => settings.code_font = font,
                        },
                        cx,
                    );
                }))
                .child(div().w(px(10.)).child(if chosen { "✓" } else { "" }))
                .child(div().flex_1().truncate().child(label))
                .when(mono && setting == FontSetting::Code, |item| {
                    item.child(div().text_color(theme.secondary).child("mono"))
                })
        });
        let menu = div()
            .id("font-menu")
            // Clicks and hovers stop here instead of reaching the rows beneath.
            .occlude()
            .flex()
            .flex_col()
            .w(px(240.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .shadow_md()
            .text_sm()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.font_menu = None;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(26.))
                    .px(px(8.))
                    .border_b_1()
                    .border_color(theme.separator)
                    .map(|filter| match self.font_query.is_empty() {
                        true => filter
                            .text_color(theme.secondary)
                            .child("Type to filter fonts"),
                        false => filter.child(self.font_query.clone()),
                    }),
            )
            .child(
                div()
                    .id("font-list")
                    .flex()
                    .flex_col()
                    .max_h(px(260.))
                    .py(px(4.))
                    .overflow_y_scroll()
                    .children(items),
            );
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(position)
                    .snap_to_window()
                    .child(menu),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

impl Render for Preferences {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(ui_size()));
        let theme = Theme::for_appearance(window.appearance());
        let current = Appearance::current();
        let settings = read().clone();
        let graph = settings.graph;
        let choice = |label: &'static str, appearance: Appearance| {
            div()
                .id(label)
                .px(px(14.))
                .py(px(3.))
                .rounded(px(4.))
                .when(current == appearance, |button| button.bg(theme.selection))
                .cursor_pointer()
                .on_click(cx.listener(move |_, _, _, cx| {
                    appearance.set();
                    save();
                    cx.refresh_windows();
                }))
                .child(label)
        };
        let clamp = |value: f32, low: f32, high: f32| value.clamp(low, high);

        let content = match self.section {
            Section::Appearance => div().flex().flex_col().gap(px(4.)).child(
                row("Theme", &theme).child(
                    div()
                        .flex()
                        .p(px(2.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.separator)
                        .child(choice("System", Appearance::System))
                        .child(choice("Light", Appearance::Light))
                        .child(choice("Dark", Appearance::Dark)),
                ),
            ),
            Section::Fonts => div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(row("Interface font", &theme).child(self.font_button(
                    FontSetting::Interface,
                    settings.ui_font.clone(),
                    &theme,
                    cx,
                )))
                .child(row("Interface size", &theme).child(self.stepper(
                    "ui",
                    format!("{}", settings.ui_size),
                    move |s, d| s.ui_size = clamp(s.ui_size + d, 12., 22.),
                    &theme,
                    cx,
                )))
                .child(row("Code font", &theme).child(self.font_button(
                    FontSetting::Code,
                    settings.code_font.clone(),
                    &theme,
                    cx,
                )))
                .child(row("Code size", &theme).child(self.stepper(
                    "code",
                    format!("{}", settings.code_size),
                    move |s, d| s.code_size = clamp(s.code_size + d, 9., 24.),
                    &theme,
                    cx,
                ))),
            Section::Review => {
                let on = settings.summaries == Some(true);
                let switch = |label: &'static str, value: bool| {
                    div()
                        .id(label)
                        .px(px(14.))
                        .py(px(3.))
                        .rounded(px(4.))
                        .when(on == value, |button| button.bg(theme.selection))
                        .cursor_pointer()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            change(|settings| settings.summaries = Some(value), cx)
                        }))
                        .child(label)
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        row("Commit summaries", &theme).child(
                            div()
                                .flex()
                                .p(px(2.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(theme.separator)
                                .child(switch("On", true))
                                .child(switch("Off", false)),
                        ),
                    )
                    .child(
                        div()
                            .pl(px(162.))
                            .text_xs()
                            .text_color(theme.secondary)
                            .child(
                                "Several selected commits are summarized by Apple's on-device \
                             model, through the fm command. Nothing leaves this Mac.",
                            ),
                    )
                    .child(self.prompt_field(PromptSetting::Summary, &theme, window, cx))
                    .child(self.prompt_field(PromptSetting::Message, &theme, window, cx))
            }
            Section::Remote => {
                let minutes = settings.fetch_minutes;
                let fetch = row("Fetch in background", &theme).child(self.stepper(
                    "fetch",
                    match minutes {
                        0 => "Off".to_string(),
                        n => format!("every {n} min"),
                    },
                    move |s, d| {
                        let at = FETCH_MINUTES
                            .iter()
                            .position(|&m| m >= s.fetch_minutes)
                            .unwrap_or(FETCH_MINUTES.len() - 1);
                        let at = (at as f32 + d).clamp(0., (FETCH_MINUTES.len() - 1) as f32);
                        s.fetch_minutes = FETCH_MINUTES[at as usize];
                    },
                    &theme,
                    cx,
                ));
                let switch = |id: &'static str, on: bool, set: fn(&mut Settings, bool)| {
                    let option = |label: &'static str, value: bool| {
                        div()
                            .id(SharedString::from(format!("{id}-{label}")))
                            .px(px(14.))
                            .py(px(3.))
                            .rounded(px(4.))
                            .when(on == value, |button| button.bg(theme.selection))
                            .cursor_pointer()
                            .on_click(cx.listener(move |_, _, _, cx| {
                                change(|settings| set(settings, value), cx)
                            }))
                            .child(label)
                    };
                    div()
                        .flex()
                        .p(px(2.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.separator)
                        .child(option("On", true))
                        .child(option("Off", false))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(fetch)
                    .child(row("Prune on fetch", &theme).child(switch(
                        "prune",
                        settings.fetch_prune,
                        |s, on| s.fetch_prune = on,
                    )))
                    .child(row("Ahead/behind badges", &theme).child(switch(
                        "badges",
                        settings.sync_badges,
                        |s, on| s.sync_badges = on,
                    )))
                    .child(
                        div()
                            .pl(px(162.))
                            .text_xs()
                            .text_color(theme.secondary)
                            .child(
                                "Fetches every remote. Pruning drops remote branches deleted \
                                 there, so local ones tracking them read as remote gone.",
                            ),
                    )
            }
            Section::Graph => div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(row("Row height", &theme).child(self.stepper(
                    "row",
                    format!("{}", graph.row_height),
                    move |s, d| s.graph.row_height = clamp(s.graph.row_height + 2. * d, 16., 48.),
                    &theme,
                    cx,
                )))
                .child(row("Node spacing", &theme).child(self.stepper(
                    "column",
                    format!("{}", graph.column),
                    move |s, d| s.graph.column = clamp(s.graph.column + 8. * d, 32., 160.),
                    &theme,
                    cx,
                )))
                .child(row("Bead spacing", &theme).child(self.stepper(
                    "bead",
                    format!("{}", graph.bead_spacing),
                    move |s, d| s.graph.bead_spacing = clamp(s.graph.bead_spacing + d, 4., 16.),
                    &theme,
                    cx,
                )))
                .child(row("Beads per node", &theme).child(self.stepper(
                    "beads",
                    format!("{}", graph.beads),
                    move |s, d| {
                        s.graph.beads = clamp(s.graph.beads as f32 + 5. * d, 5., 200.) as usize
                    },
                    &theme,
                    cx,
                )))
                .child(row("Dot size", &theme).child(self.stepper(
                    "dot",
                    format!("{}", graph.dot),
                    move |s, d| s.graph.dot = clamp(s.graph.dot + 2. * d, 6., 18.),
                    &theme,
                    cx,
                )))
                .child(row("Line width", &theme).child(self.stepper(
                    "line",
                    format!("{}", graph.line),
                    move |s, d| s.graph.line = clamp(s.graph.line + 0.5 * d, 1., 4.),
                    &theme,
                    cx,
                )))
                .child(row("Hover line width", &theme).child(self.stepper(
                    "hover",
                    format!("{}", graph.hover_line),
                    move |s, d| s.graph.hover_line = clamp(s.graph.hover_line + d, 2., 8.),
                    &theme,
                    cx,
                )))
                .child(row("Curve length", &theme).child(self.stepper(
                    "curve",
                    format!("{}", graph.curve),
                    move |s, d| s.graph.curve = clamp(s.graph.curve + 4. * d, 8., 64.),
                    &theme,
                    cx,
                )))
                .child(row("Branch idle after", &theme).child(self.stepper(
                    "idle-days",
                    format!("{} days", settings.idle_days),
                    move |s, d| s.idle_days = clamp(s.idle_days as f32 + d, 1., 365.) as u32,
                    &theme,
                    cx,
                )))
                .child(row("Compressed idle gap", &theme).child(self.stepper(
                    "idle",
                    format!("{} min", graph.idle_minutes),
                    move |s, d| {
                        s.graph.idle_minutes =
                            clamp(s.graph.idle_minutes as f32 + 15. * d, 5., 1440.) as u32
                    },
                    &theme,
                    cx,
                )))
                .child(
                    div().mt(px(8.)).child(
                        div()
                            .id("graph-defaults")
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .on_click(cx.listener(|_, _, _, cx| {
                                change(|settings| settings.graph = GraphStyle::default(), cx)
                            }))
                            .child("Reset graph to defaults"),
                    ),
                ),
        };
        let sections = Section::ALL.map(|section| {
            div()
                .id(section.title())
                .px(px(10.))
                .py(px(4.))
                .rounded(px(5.))
                .when(section == self.section, |item| item.bg(theme.selection))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.section = section;
                    this.font_menu = None;
                    cx.notify();
                }))
                .child(section.title())
        });

        div()
            .id("preferences")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(|_: &CloseTab, window, _| window.remove_window())
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(ui_font())
            .text_sm()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .w(px(150.))
                    .p(px(10.))
                    .border_r_1()
                    .border_color(theme.separator)
                    .children(sections),
            )
            // The section's settings take the rest of the width, wrapping in
            // it, and scroll when they run longer than the window.
            .child(
                div()
                    .id("section-content")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .gap(px(4.))
                    .p(px(20.))
                    .child(
                        div()
                            .mb(px(8.))
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(self.section.title()),
                    )
                    .child(content)
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.secondary)
                            .child("Changes apply to every window at once. Esc or ⌘W closes."),
                    ),
            )
            .children(self.font_menu(&theme, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_saving_and_loading() {
        let settings = Settings {
            summary_prompt: Some("Two\nlines".to_string()),
            summaries: Some(true),
            ui_font: "Helvetica".into(),
            code_font: "SF Mono".into(),
            code_size: 14.,
            graph: GraphStyle {
                dot: 12.,
                idle_minutes: 90,
                ..GraphStyle::default()
            },
            ..Settings::default()
        };
        let text = format(&settings, Appearance::Dark);
        assert_eq!(parse(&text), (settings, Appearance::Dark));
    }

    #[test]
    fn unknown_and_bad_lines_keep_defaults() {
        let (settings, appearance) = parse("graph.dot=big\nnope=1\ncode_size=13\n");
        assert_eq!(settings.graph.dot, GraphStyle::default().dot);
        assert_eq!(settings.code_size, 13.);
        assert_eq!(appearance, Appearance::System);
    }
}
