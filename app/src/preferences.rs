use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

use gpui::{
    AnyElement, App, Bounds, Context, FocusHandle, Global, KeyDownEvent, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, prelude::*, px, size,
};

use crate::line_edit::LineEdit;
use crate::sync;
use crate::text_field::Field;
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
}

impl Default for GraphStyle {
    fn default() -> Self {
        Self {
            row_height: 22.,
            column: 32.,
            bead_spacing: 7.,
            beads: 20,
            dot: 10.,
            line: 2.,
            hover_line: 4.,
            curve: 16.,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
struct Settings {
    appearance: Appearance,
    /// The interface font; `.SystemUIFont` is macOS's own.
    ui_font: SharedString,
    code_font: SharedString,
    code_size: f32,
    /// The code font's weight, 100 (thin) to 900 (black); 400 is regular.
    code_weight: f32,
    /// Lines of code and of the terminal, as multiples of the code size.
    code_line: f32,
    terminal_line: f32,
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
    /// At launch, problems with external programs open the tools window.
    tool_check: bool,
    /// File trees list folders before files; off, the two mix by name.
    folders_first: bool,
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

impl Settings {
    /// Keeps numbers in the ranges Settings offers.
    fn clamp(&mut self) {
        self.ui_size = self.ui_size.clamp(12., 22.);
        self.code_size = self.code_size.clamp(9., 24.);
        self.code_weight = (self.code_weight / 100.).round().clamp(1., 9.) * 100.;
        // Below 1.3 the diff's section headers, a little shorter than its
        // rows, no longer fit; terminals run tighter, and box drawing joins
        // up only near 1.2.
        self.code_line = ((self.code_line * 10.).round() / 10.).clamp(1.3, 2.);
        self.terminal_line = ((self.terminal_line * 20.).round() / 20.).clamp(1., 1.6);
        self.idle_days = self.idle_days.clamp(1, 365);
        // A choice gone from the list, such as the fetches Pull once offered,
        // goes back to the default rather than to whatever took its place.
        if self.pull_option >= sync::PULL.len() {
            self.pull_option = Settings::default().pull_option;
        }
        self.push_option = self.push_option.min(sync::PUSH.len() - 1);
        let graph = &mut self.graph;
        graph.row_height = graph.row_height.clamp(16., 48.);
        graph.column = graph.column.clamp(32., 160.);
        graph.bead_spacing = graph.bead_spacing.clamp(4., 16.);
        graph.beads = graph.beads.clamp(5, 200);
        graph.dot = graph.dot.clamp(6., 18.);
        graph.line = graph.line.clamp(1., 4.);
        graph.hover_line = graph.hover_line.clamp(2., 8.);
        graph.curve = graph.curve.clamp(8., 64.);
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Appearance::System,
            ui_font: SYSTEM_FONT.into(),
            code_font: "Menlo".into(),
            code_size: 12.,
            code_weight: 400.,
            code_line: 1.6,
            terminal_line: 1.35,
            ui_size: 16.,
            idle_days: 14,
            summaries: None,
            summary_prompt: None,
            message_prompt: None,
            tool_check: true,
            folders_first: false,
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

/// The height of a line of code, such as a diff row, in points.
pub fn code_line_height() -> f32 {
    let settings = read();
    (settings.code_size * settings.code_line).round()
}

/// The height of a terminal line, in points.
pub fn terminal_line_height() -> f32 {
    let settings = read();
    (settings.code_size * settings.terminal_line).round()
}

pub fn code_weight() -> gpui::FontWeight {
    gpui::FontWeight(read().code_weight)
}

/// A weight's name, as font menus call it.
fn weight_name(weight: f32) -> &'static str {
    match weight as u32 {
        100 => "Thin",
        200 => "Extra Light",
        300 => "Light",
        400 => "Regular",
        500 => "Medium",
        600 => "Semibold",
        700 => "Bold",
        800 => "Extra Bold",
        _ => "Black",
    }
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

/// The option the Pull (`false`) or Push (`true`) button runs.
pub fn sync_option(push: bool) -> usize {
    let settings = read();
    match push {
        true => settings.push_option,
        false => settings.pull_option,
    }
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
const FETCH_MINUTES: [f32; 6] = [0., 1., 5., 15., 30., 60.];

/// Idle days offered: close together where they matter, far apart beyond.
const IDLE_DAYS: [f32; 15] = [
    1., 2., 3., 5., 7., 10., 14., 21., 30., 45., 60., 90., 120., 180., 365.,
];

/// A slider's track and its knob.
const SLIDER_WIDTH: f32 = 180.;
const KNOB_SIZE: f32 = 14.;

/// A number of the settings that a slider sets.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Knob {
    UiSize,
    CodeSize,
    CodeWeight,
    CodeLine,
    TerminalLine,
    FetchMinutes,
    RowHeight,
    Column,
    Curve,
    Dot,
    BeadSpacing,
    Beads,
    Line,
    HoverLine,
    IdleDays,
}

/// The values a slider runs over: evenly from one to another, or a list.
enum Steps {
    Even { min: f32, max: f32, step: f32 },
    List(&'static [f32]),
}

impl Knob {
    /// The same ranges [`Settings::clamp`] keeps the numbers in.
    fn steps(self) -> Steps {
        let even = |min, max, step| Steps::Even { min, max, step };
        match self {
            Knob::UiSize => even(12., 22., 1.),
            Knob::CodeSize => even(9., 24., 1.),
            Knob::CodeWeight => even(100., 900., 100.),
            Knob::CodeLine => even(1.3, 2., 0.1),
            Knob::TerminalLine => even(1., 1.6, 0.05),
            Knob::FetchMinutes => Steps::List(&FETCH_MINUTES),
            Knob::RowHeight => even(16., 48., 2.),
            Knob::Column => even(32., 160., 8.),
            Knob::Curve => even(8., 64., 4.),
            Knob::Dot => even(6., 18., 2.),
            Knob::BeadSpacing => even(4., 16., 1.),
            Knob::Beads => even(5., 200., 5.),
            Knob::Line => even(1., 4., 0.5),
            Knob::HoverLine => even(2., 8., 1.),
            Knob::IdleDays => Steps::List(&IDLE_DAYS),
        }
    }

    fn get(self, s: &Settings) -> f32 {
        match self {
            Knob::UiSize => s.ui_size,
            Knob::CodeSize => s.code_size,
            Knob::CodeWeight => s.code_weight,
            Knob::CodeLine => s.code_line,
            Knob::TerminalLine => s.terminal_line,
            Knob::FetchMinutes => s.fetch_minutes as f32,
            Knob::RowHeight => s.graph.row_height,
            Knob::Column => s.graph.column,
            Knob::Curve => s.graph.curve,
            Knob::Dot => s.graph.dot,
            Knob::BeadSpacing => s.graph.bead_spacing,
            Knob::Beads => s.graph.beads as f32,
            Knob::Line => s.graph.line,
            Knob::HoverLine => s.graph.hover_line,
            Knob::IdleDays => s.idle_days as f32,
        }
    }

    fn set(self, s: &mut Settings, value: f32) {
        match self {
            Knob::UiSize => s.ui_size = value,
            Knob::CodeSize => s.code_size = value,
            Knob::CodeWeight => s.code_weight = value,
            Knob::CodeLine => s.code_line = value,
            Knob::TerminalLine => s.terminal_line = value,
            Knob::FetchMinutes => s.fetch_minutes = value as u32,
            Knob::RowHeight => s.graph.row_height = value,
            Knob::Column => s.graph.column = value,
            Knob::Curve => s.graph.curve = value,
            Knob::Dot => s.graph.dot = value,
            Knob::BeadSpacing => s.graph.bead_spacing = value,
            Knob::Beads => s.graph.beads = value as usize,
            Knob::Line => s.graph.line = value,
            Knob::HoverLine => s.graph.hover_line = value,
            Knob::IdleDays => s.idle_days = value as u32,
        }
    }

    /// How far along its track a value sits, from 0 to 1.
    fn fraction(self, value: f32) -> f32 {
        match self.steps() {
            Steps::Even { min, max, .. } => ((value - min) / (max - min)).clamp(0., 1.),
            // A value between two of the list sits at the nearer one.
            Steps::List(list) => {
                let at = (0..list.len())
                    .min_by(|&a, &b| (list[a] - value).abs().total_cmp(&(list[b] - value).abs()))
                    .unwrap_or(0);
                at as f32 / (list.len() - 1).max(1) as f32
            }
        }
    }

    /// The value at a place along the track, on one of the steps.
    fn at(self, frac: f32) -> f32 {
        match self.steps() {
            Steps::Even { min, max, step } => {
                let value = min + ((frac * (max - min)) / step).round() * step;
                // Rounded as the settings keep it, so steps of 0.1 stay exact.
                ((value * 100.).round() / 100.).clamp(min, max)
            }
            Steps::List(list) => list[(frac * (list.len() - 1) as f32).round() as usize],
        }
    }

    /// The value as the slider says it.
    fn label(self, value: f32) -> String {
        match self {
            Knob::CodeWeight => format!("{} {value}", weight_name(value)),
            Knob::CodeLine => format!("{value:.1}"),
            Knob::TerminalLine => format!("{value:.2}"),
            Knob::FetchMinutes => match value as u32 {
                0 => "Off".to_string(),
                n => format!("every {n} min"),
            },
            Knob::IdleDays => match value as u32 {
                1 => "1 day".to_string(),
                n => format!("{n} days"),
            },
            _ => format!("{value}"),
        }
    }
}

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

/// Turns commit summaries on or off, as Settings › Review does.
pub fn set_summaries(on: bool, cx: &mut App) {
    change(|settings| settings.summaries = Some(on), cx);
}

/// Whether file trees list folders before files.
pub fn folders_first() -> bool {
    read().folders_first
}

/// Whether problems with external programs show at launch.
pub fn tool_check() -> bool {
    read().tool_check
}

pub fn set_tool_check(on: bool, cx: &mut App) {
    change(|settings| settings.tool_check = on, cx);
}

pub fn appearance() -> Appearance {
    read().appearance
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

/// macOS's interface font, offered as "System".
const SYSTEM_FONT: &str = ".SystemUIFont";

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn file() -> PathBuf {
    crate::recent::support_file("settings")
}

/// Applies the saved preferences; missing or unreadable ones keep defaults.
pub fn load() {
    let settings = parse(&fs::read_to_string(file()).unwrap_or_default());
    *SETTINGS.write().unwrap_or_else(|error| error.into_inner()) = settings;
}

/// Reads `key=value` lines; unknown keys and bad values keep defaults.
fn parse(text: &str) -> Settings {
    let mut settings = Settings::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // Prompts keep their spaces; other values are trimmed.
        let raw = value;
        let value = value.trim();
        let number = value.parse::<f32>().ok().filter(|n| n.is_finite());
        let graph = &mut settings.graph;
        match (key.trim(), number) {
            ("appearance", _) => {
                settings.appearance = match value {
                    "light" => Appearance::Light,
                    "dark" => Appearance::Dark,
                    _ => Appearance::System,
                }
            }
            ("ui_font", _) if !value.is_empty() => settings.ui_font = value.to_string().into(),
            ("code_font", _) if !value.is_empty() => settings.code_font = value.to_string().into(),
            ("code_size", Some(n)) => settings.code_size = n,
            ("code_weight", Some(n)) => settings.code_weight = n,
            ("code_line", Some(n)) => settings.code_line = n,
            ("terminal_line", Some(n)) => settings.terminal_line = n,
            ("ui_size", Some(n)) => settings.ui_size = n,
            ("idle_days", Some(n)) => settings.idle_days = n as u32,
            ("summaries", _) => settings.summaries = Some(value == "on"),
            ("fetch_minutes", Some(n)) => settings.fetch_minutes = n as u32,
            ("fetch_prune", _) => settings.fetch_prune = value == "on",
            ("sync_badges", _) => settings.sync_badges = value == "on",
            ("tool_check", _) => settings.tool_check = value == "on",
            ("folders_first", _) => settings.folders_first = value == "on",
            ("pull_option", Some(n)) => settings.pull_option = n as usize,
            ("push_option", Some(n)) => settings.push_option = n as usize,
            ("summary_prompt", _) => settings.summary_prompt = Some(unescape(raw)),
            ("message_prompt", _) => settings.message_prompt = Some(unescape(raw)),
            ("graph.row_height", Some(n)) => graph.row_height = n,
            ("graph.column", Some(n)) => graph.column = n,
            ("graph.bead_spacing", Some(n)) => graph.bead_spacing = n,
            ("graph.beads", Some(n)) => graph.beads = n as usize,
            ("graph.dot", Some(n)) => graph.dot = n,
            ("graph.line", Some(n)) => graph.line = n,
            ("graph.hover_line", Some(n)) => graph.hover_line = n,
            ("graph.curve", Some(n)) => graph.curve = n,
            _ => {}
        }
    }
    // A hand-edited file may hold numbers Settings never offers.
    settings.clamp();
    settings
}

/// A prompt on one line: line breaks as `\n`, backslashes as `\\`.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\n', "\\n")
}

/// Reads [`escape`]d text back; a backslash before anything else stays.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some('n')) => {
                chars.next();
                out.push('\n');
            }
            ('\\', Some('\\')) => {
                chars.next();
                out.push('\\');
            }
            _ => out.push(c),
        }
    }
    out
}

fn format(settings: &Settings) -> String {
    let appearance = match settings.appearance {
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
        format!("code_weight={}", settings.code_weight),
        format!("code_line={}", settings.code_line),
        format!("terminal_line={}", settings.terminal_line),
        format!("ui_size={}", settings.ui_size),
        format!("idle_days={}", settings.idle_days),
        format!("fetch_minutes={}", settings.fetch_minutes),
        format!("fetch_prune={}", on_off(settings.fetch_prune)),
        format!("sync_badges={}", on_off(settings.sync_badges)),
        format!("tool_check={}", on_off(settings.tool_check)),
        format!("folders_first={}", on_off(settings.folders_first)),
        format!("pull_option={}", settings.pull_option),
        format!("push_option={}", settings.push_option),
        match settings.summaries {
            Some(true) => "summaries=on".to_string(),
            Some(false) => "summaries=off".to_string(),
            None => String::new(),
        },
        settings
            .summary_prompt
            .as_ref()
            .map_or(String::new(), |p| format!("summary_prompt={}", escape(p))),
        settings
            .message_prompt
            .as_ref()
            .map_or(String::new(), |p| format!("message_prompt={}", escape(p))),
        format!("graph.row_height={}", g.row_height),
        format!("graph.column={}", g.column),
        format!("graph.bead_spacing={}", g.bead_spacing),
        format!("graph.beads={}", g.beads),
        format!("graph.dot={}", g.dot),
        format!("graph.line={}", g.line),
        format!("graph.hover_line={}", g.hover_line),
        format!("graph.curve={}", g.curve),
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
    let text = format(&read());
    let file = file();
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).ok();
    }
    fs::write(file, text).ok();
}

/// Changes the settings, saves them and redraws every window.
fn change(update: impl FnOnce(&mut Settings), cx: &mut App) {
    let mut settings = SETTINGS.write().unwrap_or_else(|error| error.into_inner());
    update(&mut settings);
    settings.clamp();
    drop(settings);
    save();
    cx.refresh_windows();
}

/// The open Preferences window, so a second ⌘, brings it forward.
struct PreferencesWindow(Option<WindowHandle<Preferences>>);

impl Global for PreferencesWindow {}

/// Brings a window of the app's own, such as Settings, to the front; false
/// when it is not open.
pub fn bring_forward<V: 'static>(handle: Option<WindowHandle<V>>, cx: &mut App) -> bool {
    handle.is_some_and(|handle| {
        handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    })
}

/// A small fixed-size window of the app's own, centred.
pub fn panel_options(title: &'static str, width: f32, height: f32, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(width), px(height)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            ..Default::default()
        }),
        is_resizable: false,
        ..Default::default()
    }
}

pub fn open(cx: &mut App) {
    let existing = cx
        .try_global::<PreferencesWindow>()
        .and_then(|window| window.0);
    if bring_forward(existing, cx) {
        return;
    }
    let options = panel_options("Settings", 680., 540., cx);
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            Preferences {
                focus,
                fonts: installed_fonts(window, cx),
                section: Section::Appearance,
                font_menu: None,
                font_query: Default::default(),
                prompts: Default::default(),
                prompt_focus: [cx.focus_handle(), cx.focus_handle()],
                dragging: None,
                tracks: Default::default(),
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
    font_query: Field,
    /// The prompts as edited, each a multi-line field.
    prompts: [Field; 2],
    /// The prompt fields, each with its own focus.
    prompt_focus: [FocusHandle; 2],
    /// The slider being dragged, and where each slider's track was drawn.
    dragging: Option<Knob>,
    tracks: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<Knob, Bounds<gpui::Pixels>>>>,
}

/// Which prompt a field edits.
#[derive(Clone, Copy, PartialEq)]
enum PromptSetting {
    Summary,
    Message,
}

impl PromptSetting {
    /// The prompt as the model is told it.
    fn text(self) -> String {
        match self {
            PromptSetting::Summary => summary_prompt(),
            PromptSetting::Message => message_prompt(),
        }
    }

    /// Sets a custom prompt, or with `None` goes back to the built-in one,
    /// as typing the built-in text back does.
    fn set(self, settings: &mut Settings, text: Option<String>) {
        let built_in = match self {
            PromptSetting::Summary => wtf_core::SUMMARY_PROMPT,
            PromptSetting::Message => wtf_core::MESSAGE_PROMPT,
        };
        let text = text.filter(|text| text != built_in);
        match self {
            PromptSetting::Summary => settings.summary_prompt = text,
            PromptSetting::Message => settings.message_prompt = text,
        }
    }
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

/// A row of a settings group: its label at the left, and its control, the
/// child added next, at the right.
fn row(label: &'static str, theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .min_h(px(36.))
        .px(px(12.))
        .py(px(4.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme.foreground)
                .child(label),
        )
}

/// Settings that belong together, as macOS lays them out: a title over a
/// rounded box, a line between its rows, and a note under it.
fn group(
    title: Option<&'static str>,
    rows: impl IntoIterator<Item = gpui::Div>,
    note: Option<&'static str>,
    theme: &Theme,
) -> gpui::Div {
    let rows = rows
        .into_iter()
        .enumerate()
        .map(|(k, row)| row.when(k > 0, |row| row.border_t_1().border_color(theme.separator)));
    div()
        .flex()
        .flex_col()
        .children(title.map(|title| {
            div()
                .px(px(12.))
                .pb(px(6.))
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.secondary)
                .child(title)
        }))
        .child(card(theme).children(rows))
        .children(note.map(|note| {
            div()
                .px(px(12.))
                .pt(px(6.))
                .text_xs()
                .text_color(theme.secondary)
                .child(note)
        }))
}

/// The rounded box a group of settings sits in.
fn card(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(8.))
        .border_1()
        .border_color(theme.separator)
        .bg(theme.background)
}

/// A switch for a setting that is on or off; a click flips it.
fn switch(
    id: &'static str,
    on: bool,
    pick: impl Fn(bool, &mut App) + 'static,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .w(px(34.))
        .h(px(20.))
        .p(px(2.))
        .rounded_full()
        .bg(match on {
            true => theme.accent,
            false => theme.secondary.opacity(0.35),
        })
        .when(on, |track| track.justify_end())
        .cursor_pointer()
        .on_click(move |_, _, cx| pick(!on, cx))
        .child(
            div()
                .size(px(16.))
                .rounded_full()
                .bg(gpui::white())
                .shadow_sm(),
        )
}

/// A small graph drawn with the graph settings as they stand, at the size
/// the graph has: a main line with a bar of commits, a branch that leaves
/// and comes back, and one hovered, in its wider line.
fn graph_preview(g: GraphStyle, theme: &Theme) -> impl IntoElement {
    let rgb = |color: gpui::Hsla| {
        let rgba = gpui::Rgba::from(color);
        let byte = |channel: f32| (channel * 255.).round() as u8;
        format!("rgb({},{},{})", byte(rgba.r), byte(rgba.g), byte(rgba.b))
    };
    let lanes = theme.lanes.map(rgb);
    // Beads are upright ellipses, a little over half as wide as tall.
    let bead = g.dot * 0.6;
    let pad = 14.;
    let y = |row: f32| pad + g.row_height * (row + 0.5);
    // A rail leaves its row and arrives in the next as the graph draws it:
    // two half-curves over at most the curve length.
    let s_curve = |(x1, y1): (f32, f32), (x2, y2): (f32, f32)| {
        let (mx, my) = ((x1 + x2) / 2., (y1 + y2) / 2.);
        format!("Q{mx},{y1} {mx},{my} Q{mx},{y2} {x2},{y2}")
    };
    let fork = |from: (f32, f32), to: (f32, f32)| {
        let bend = (to.0 - from.0).clamp(0., g.curve);
        let corner = (from.0 + bend, to.1);
        format!(
            "M{},{} {} L{},{}",
            from.0,
            from.1,
            s_curve(from, corner),
            to.0,
            to.1
        )
    };
    let merge = |from: (f32, f32), to: (f32, f32)| {
        let bend = (to.0 - from.0).clamp(0., g.curve);
        let corner = (to.0 - bend, from.1);
        format!(
            "M{},{} L{},{} {}",
            from.0,
            from.1,
            corner.0,
            corner.1,
            s_curve(corner, to)
        )
    };
    let rail = |d: String, width: f32, color: &str| {
        format!(
            r#"<path d="{d}" fill="none" stroke="{color}" stroke-width="{width}" stroke-linecap="round"/>"#
        )
    };
    let mark = |x: f32, y: f32, color: &str| {
        format!(
            r#"<rect x="{}" y="{}" width="{bead}" height="{}" rx="{}" fill="{color}"/>"#,
            x - bead / 2.,
            y - g.dot / 2.,
            g.dot,
            bead / 2.
        )
    };
    // Main: a commit, a bar of many, two commits.
    let count = g.beads.min(24);
    let x0 = pad + g.dot;
    let bar = (
        x0 + g.column,
        x0 + g.column + (count - 1) as f32 * g.bead_spacing,
    );
    let xc = bar.1 + g.column;
    let xd = xc + g.column;
    let (y0, y1, y2) = (y(0.), y(1.), y(2.));
    let mut rails = vec![
        rail(format!("M{x0},{y0} L{xd},{y0}"), g.line, &lanes[0]),
        // A branch off the first commit, back into the main line.
        rail(fork((x0, y0), (bar.0, y1)), g.line, &lanes[1]),
        rail(
            format!("M{},{y1} L{},{y1}", bar.0, bar.1),
            g.line,
            &lanes[1],
        ),
        rail(merge((bar.1, y1), (xc, y0)), g.line, &lanes[1]),
    ];
    // A hovered branch, off the third commit.
    rails.push(rail(fork((xc, y0), (xd, y2)), g.hover_line, &lanes[2]));
    rails.push(rail(
        format!("M{xd},{y2} L{},{y2}", xd + g.column),
        g.hover_line,
        &lanes[2],
    ));
    let mut marks = vec![
        mark(x0, y0, &lanes[0]),
        mark(xc, y0, &lanes[0]),
        mark(xd, y0, &lanes[0]),
    ];
    marks.extend((0..count).map(|k| mark(bar.0 + k as f32 * g.bead_spacing, y0, &lanes[0])));
    marks.extend([mark(bar.0, y1, &lanes[1]), mark(bar.1, y1, &lanes[1])]);
    marks.extend([mark(xd, y2, &lanes[2]), mark(xd + g.column, y2, &lanes[2])]);
    let (w, h) = (
        (xd + g.column + pad + g.dot).ceil(),
        (y(2.) + g.row_height / 2. + pad).ceil(),
    );
    // Drawn at twice its size to stay sharp, as the strip's labels are.
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {w} {h}">{}{}</svg>"#,
        w * 2.,
        h * 2.,
        rails.concat(),
        marks.concat()
    );
    div()
        .w_full()
        .h(px(h))
        .rounded(px(8.))
        .border_1()
        .border_color(theme.separator)
        .bg(theme.graph_background)
        .overflow_hidden()
        .child(
            gpui::img(std::sync::Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Svg,
                svg.into_bytes(),
            )))
            .w(px(w))
            .h(px(h)),
        )
}

/// A segmented control: a button per option, the `selected` one lit;
/// clicking one runs `pick` with its value.
fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(&'static str, T)],
    selected: T,
    pick: impl Fn(T, &mut App) + Clone + 'static,
    theme: &Theme,
) -> gpui::Div {
    let buttons = options.iter().map(|&(label, value)| {
        let pick = pick.clone();
        div()
            .id(SharedString::from(format!("{id}-{label}")))
            .px(px(12.))
            .py(px(2.))
            .rounded(px(5.))
            // The chosen one stands up from the track, as in macOS.
            .when(selected == value, |button| {
                button.bg(theme.background).shadow_sm()
            })
            .cursor_pointer()
            .on_click(move |_, _, cx| pick(value, cx))
            .child(label)
    });
    div()
        .flex()
        .flex_none()
        .p(px(2.))
        .rounded(px(7.))
        .bg(theme.filler)
        .border_1()
        .border_color(theme.separator)
        .children(buttons)
}

impl Preferences {
    /// A slider for one number of the settings: drag along it, or click
    /// where the value should be; the value reads after it.
    fn slider(&self, knob: Knob, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let value = knob.get(&read());
        let frac = knob.fraction(value);
        let tracks = self.tracks.clone();
        let travel = SLIDER_WIDTH - KNOB_SIZE;
        let track = div()
            .id(("slider", knob as usize))
            .relative()
            .flex_none()
            .w(px(SLIDER_WIDTH))
            .h(px(20.))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.dragging = Some(knob);
                    this.drag_to(knob, event.position.x, cx);
                }),
            )
            // Where the track is drawn, to read a pointer's place along it.
            .child(
                gpui::canvas(
                    move |bounds, _, _| {
                        tracks.borrow_mut().insert(knob, bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(KNOB_SIZE / 2.))
                    .right(px(KNOB_SIZE / 2.))
                    .top(px(8.))
                    .h(px(4.))
                    .rounded(px(2.))
                    .bg(theme.separator),
            )
            .child(
                div()
                    .absolute()
                    .left(px(KNOB_SIZE / 2.))
                    .top(px(8.))
                    .h(px(4.))
                    .w(px(frac * travel))
                    .rounded(px(2.))
                    .bg(theme.accent),
            )
            .child(
                div()
                    .absolute()
                    .left(px(frac * travel))
                    .top(px((20. - KNOB_SIZE) / 2.))
                    .size(px(KNOB_SIZE))
                    .rounded_full()
                    .border_1()
                    .border_color(match self.dragging == Some(knob) {
                        true => theme.accent,
                        false => theme.secondary,
                    })
                    .bg(theme.background)
                    .shadow_sm(),
            );
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(track)
            // Values in one column, so the tracks line up.
            .child(
                div()
                    .w(px(72.))
                    .whitespace_nowrap()
                    .child(knob.label(value)),
            )
    }

    /// Sets a slider's number from a pointer's place along its track, in
    /// the slider's steps; settings change only when the number does.
    fn drag_to(&mut self, knob: Knob, x: gpui::Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = self.tracks.borrow().get(&knob).copied() else {
            return;
        };
        let travel = f32::from(bounds.size.width) - KNOB_SIZE;
        let frac = (f32::from(x - bounds.left()) - KNOB_SIZE / 2.) / travel.max(1.);
        let value = knob.at(frac.clamp(0., 1.));
        if knob.get(&read()) != value {
            change(|settings| knob.set(settings, value), cx);
        }
        cx.notify();
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
                self.font_query.edit.set_text("");
            }
            _ if self.font_query.key(keystroke, cx) => {}
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
        let (label, custom, k) = match setting {
            PromptSetting::Summary => ("Summary prompt", read().summary_prompt.is_some(), 0),
            PromptSetting::Message => ("Commit message prompt", read().message_prompt.is_some(), 1),
        };
        let focus = &self.prompt_focus[k];
        let focused = focus.is_focused(window);
        card(theme)
            .w_full()
            .min_w_0()
            .gap(px(6.))
            .p(px(12.))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(div().child(label))
                    .child(div().flex_1())
                    .when(custom, |row| {
                        row.child(
                            div()
                                .id(label)
                                .text_xs()
                                .text_color(theme.accent)
                                .cursor_pointer()
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    change(|settings| setting.set(settings, None), cx)
                                }))
                                .child("Reset to built-in"),
                        )
                    }),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{label}-field")))
                    .track_focus(focus)
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                        let field = &mut this.prompts[k];
                        let before = field.text().to_string();
                        if !field.key(&event.keystroke, cx) {
                            return;
                        }
                        cx.stop_propagation();
                        let text = field.text().to_string();
                        if text != before {
                            change(|settings| setting.set(settings, Some(text)), cx);
                        }
                        cx.notify();
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
                    .child(self.prompts[k].render(
                        SharedString::from(format!("{label}-text")),
                        "",
                        focused,
                        true,
                        Vec::new(),
                        focus,
                        theme,
                        match k {
                            0 => |this: &mut Self| Some(&mut this.prompts[0]),
                            _ => |this: &mut Self| Some(&mut this.prompts[1]),
                        },
                        cx,
                    )),
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
                    this.font_query.edit.set_text("");
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
        let query = self.font_query.text().to_lowercase();
        // The window keeps the keys while the menu is open, for the filter.
        let filter = self.font_query.render(
            "font-filter",
            "Type to filter fonts",
            true,
            true,
            Vec::new(),
            &self.focus,
            theme,
            |this: &mut Self| Some(&mut this.font_query),
            cx,
        );
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
                    .child(filter),
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
        // A prompt not being edited shows the one in the settings, which
        // may have been reset.
        for (k, setting) in [PromptSetting::Summary, PromptSetting::Message]
            .into_iter()
            .enumerate()
        {
            let text = setting.text();
            if !self.prompt_focus[k].is_focused(window) && self.prompts[k].text() != text {
                self.prompts[k].edit = LineEdit::multiline(text);
            }
        }
        let theme = Theme::for_appearance(window.appearance());
        let settings = read().clone();
        let current = settings.appearance;
        let graph = settings.graph;

        let switch_for = |id: &'static str, on: bool, set: fn(&mut Settings, bool)| {
            switch(
                id,
                on,
                move |on, cx| change(|settings| set(settings, on), cx),
                &theme,
            )
        };
        let content = match self.section {
            Section::Appearance => div().child(group(
                None,
                [row("Theme", &theme).child(segmented(
                    "theme",
                    &[
                        ("System", Appearance::System),
                        ("Light", Appearance::Light),
                        ("Dark", Appearance::Dark),
                    ],
                    current,
                    |appearance: Appearance, cx| {
                        change(|settings| settings.appearance = appearance, cx)
                    },
                    &theme,
                ))],
                None,
                &theme,
            )),
            Section::Fonts => div()
                .child(group(
                    Some("Interface"),
                    [
                        row("Font", &theme).child(self.font_button(
                            FontSetting::Interface,
                            settings.ui_font.clone(),
                            &theme,
                            cx,
                        )),
                        row("Size", &theme).child(self.slider(Knob::UiSize, &theme, cx)),
                    ],
                    None,
                    &theme,
                ))
                .child(group(
                    Some("Code"),
                    [
                        row("Font", &theme).child(self.font_button(
                            FontSetting::Code,
                            settings.code_font.clone(),
                            &theme,
                            cx,
                        )),
                        row("Size", &theme).child(self.slider(Knob::CodeSize, &theme, cx)),
                        row("Weight", &theme).child(self.slider(Knob::CodeWeight, &theme, cx)),
                    ],
                    Some(
                        "A font without this weight shows its nearest one; many monospace \
                         fonts have only Regular and Bold.",
                    ),
                    &theme,
                ))
                .child(group(
                    Some("Line height"),
                    [
                        row("Code", &theme).child(self.slider(Knob::CodeLine, &theme, cx)),
                        row("Terminal", &theme).child(self.slider(Knob::TerminalLine, &theme, cx)),
                    ],
                    Some(
                        "Times the code size. Diffs read well at 1.5 to 1.7; terminals at \
                         1.15 to 1.25, where box drawing joins up.",
                    ),
                    &theme,
                )),
            Section::Review => div()
                .child(group(
                    Some("Files"),
                    [row("Folders in file trees", &theme).child(segmented(
                        "folders-first",
                        &[("With files", false), ("First", true)],
                        settings.folders_first,
                        |first, cx| change(|settings| settings.folders_first = first, cx),
                        &theme,
                    ))],
                    None,
                    &theme,
                ))
                .child(group(
                    Some("Apple Intelligence"),
                    [
                        row("Commit summaries and messages", &theme).child(switch_for(
                            "summaries",
                            settings.summaries == Some(true),
                            |s, on| s.summaries = Some(on),
                        )),
                    ],
                    Some(
                        "Several selected commits are summarized, and commit messages \
                         written, by Apple's on-device model, through the fm command. \
                         Nothing leaves this Mac.",
                    ),
                    &theme,
                ))
                .child(self.prompt_field(PromptSetting::Summary, &theme, window, cx))
                .child(self.prompt_field(PromptSetting::Message, &theme, window, cx)),
            Section::Remote => div().child(group(
                None,
                [
                    row("Fetch in background", &theme).child(self.slider(
                        Knob::FetchMinutes,
                        &theme,
                        cx,
                    )),
                    row("Prune on fetch", &theme).child(switch_for(
                        "prune",
                        settings.fetch_prune,
                        |s, on| s.fetch_prune = on,
                    )),
                    row("Ahead/behind badges", &theme).child(switch_for(
                        "badges",
                        settings.sync_badges,
                        |s, on| s.sync_badges = on,
                    )),
                ],
                Some(
                    "Fetches every remote. Pruning drops remote branches deleted there, so \
                     local ones tracking them read as remote gone.",
                ),
                &theme,
            )),
            // A sample graph, drawn as the settings stand, over them in groups:
            // where things go, how commits look, how lines look.
            Section::Graph => div()
                .child(graph_preview(graph, &theme))
                .child(group(
                    Some("Layout"),
                    [
                        row("Row height", &theme).child(self.slider(Knob::RowHeight, &theme, cx)),
                        row("Node spacing", &theme).child(self.slider(Knob::Column, &theme, cx)),
                        row("Curve length", &theme).child(self.slider(Knob::Curve, &theme, cx)),
                    ],
                    None,
                    &theme,
                ))
                .child(group(
                    Some("Commits"),
                    [
                        row("Dot size", &theme).child(self.slider(Knob::Dot, &theme, cx)),
                        row("Bead spacing", &theme).child(self.slider(
                            Knob::BeadSpacing,
                            &theme,
                            cx,
                        )),
                        row("Beads per node", &theme).child(self.slider(Knob::Beads, &theme, cx)),
                    ],
                    None,
                    &theme,
                ))
                .child(group(
                    Some("Lines"),
                    [
                        row("Width", &theme).child(self.slider(Knob::Line, &theme, cx)),
                        row("Width on hover", &theme).child(self.slider(
                            Knob::HoverLine,
                            &theme,
                            cx,
                        )),
                    ],
                    None,
                    &theme,
                ))
                .child(group(
                    Some("Branches"),
                    [row("Idle after", &theme).child(self.slider(Knob::IdleDays, &theme, cx))],
                    None,
                    &theme,
                ))
                .child(
                    div().flex().justify_end().child(
                        div()
                            .id("graph-defaults")
                            .px(px(10.))
                            .py(px(3.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(theme.separator)
                            .bg(theme.background)
                            .hover(|button| button.bg(theme.selection))
                            .cursor_pointer()
                            .on_click(cx.listener(|_, _, _, cx| {
                                // Everything in this section, idle days too.
                                change(
                                    |settings| {
                                        settings.graph = GraphStyle::default();
                                        settings.idle_days = Settings::default().idle_days;
                                    },
                                    cx,
                                )
                            }))
                            .child("Reset to Defaults"),
                    ),
                ),
        }
        // Groups apart, as in System Settings.
        .flex()
        .flex_col()
        .gap(px(20.));
        let sections = Section::ALL.map(|section| {
            div()
                .id(section.title())
                .px(px(10.))
                .py(px(5.))
                .rounded(px(6.))
                .when(section == self.section, |item| {
                    item.bg(theme.selection)
                        .font_weight(gpui::FontWeight::MEDIUM)
                })
                .when(section != self.section, |item| {
                    item.hover(|item| item.bg(theme.selection.opacity(0.5)))
                })
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
            // A slider follows the pointer anywhere in the window while the
            // button is down, and lets go with it.
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                let Some(knob) = this.dragging else {
                    return;
                };
                match event.pressed_button {
                    Some(gpui::MouseButton::Left) => this.drag_to(knob, event.position.x, cx),
                    _ => {
                        this.dragging = None;
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.dragging.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_action(|_: &CloseTab, window, _| window.remove_window())
            .on_action(|_: &CloseWindow, window, _| window.remove_window())
            .flex()
            .size_full()
            .bg(theme.graph_background)
            .text_color(theme.foreground)
            .font_family(ui_font())
            .text_sm()
            // The sections down the side, and how changes apply under them.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .w(px(170.))
                    .p(px(10.))
                    .bg(theme.filler)
                    .border_r_1()
                    .border_color(theme.separator)
                    .children(sections)
                    .child(div().flex_1())
                    .child(
                        div()
                            .px(px(10.))
                            .text_xs()
                            .text_color(theme.secondary)
                            .child("Changes apply to every window at once. Esc or ⌘W closes."),
                    ),
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
                    .gap(px(16.))
                    .px(px(24.))
                    .py(px(20.))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(self.section.title()),
                    )
                    .child(content),
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
            message_prompt: Some(" Write \\n, not C:\\dir ".to_string()),
            summaries: Some(true),
            ui_font: "Helvetica".into(),
            code_font: "SF Mono".into(),
            code_size: 14.,
            code_weight: 300.,
            graph: GraphStyle {
                dot: 12.,
                ..GraphStyle::default()
            },
            ..Settings::default()
        };
        let text = format(&settings);
        assert_eq!(parse(&text), settings);
    }

    #[test]
    fn unknown_and_bad_lines_keep_defaults() {
        let settings = parse("graph.dot=big\nnope=1\ncode_size=13\n");
        assert_eq!(settings.graph.dot, GraphStyle::default().dot);
        assert_eq!(settings.code_size, 13.);
        assert_eq!(settings.appearance, Appearance::System);
    }

    #[test]
    fn numbers_out_of_range_are_clamped() {
        let settings = parse("ui_size=0\ncode_size=NaN\npush_option=99\n");
        assert_eq!(settings.ui_size, 12.);
        assert_eq!(settings.code_size, Settings::default().code_size);
        assert_eq!(settings.push_option, sync::PUSH.len() - 1);
        // A pull option no longer offered goes back to the default.
        assert_eq!(
            parse("pull_option=4\n").pull_option,
            Settings::default().pull_option
        );
    }

    #[test]
    fn code_weight_is_a_named_step() {
        assert_eq!(parse("code_weight=540\n").code_weight, 500.);
        assert_eq!(parse("code_weight=5\n").code_weight, 100.);
        assert_eq!(parse("code_weight=2000\n").code_weight, 900.);
        assert_eq!(weight_name(Settings::default().code_weight), "Regular");
    }

    #[test]
    fn line_heights_keep_their_steps_and_ranges() {
        let settings = parse("code_line=1.63\nterminal_line=1.22\n");
        assert_eq!(settings.code_line, 1.6);
        assert_eq!(settings.terminal_line, 1.2);
        let settings = parse("code_line=1\nterminal_line=3\n");
        assert_eq!(settings.code_line, 1.3);
        assert_eq!(settings.terminal_line, 1.6);
    }

    #[test]
    fn sliders_stay_on_what_settings_keep() {
        let knobs = [
            Knob::UiSize,
            Knob::CodeSize,
            Knob::CodeWeight,
            Knob::CodeLine,
            Knob::TerminalLine,
            Knob::FetchMinutes,
            Knob::RowHeight,
            Knob::Column,
            Knob::Curve,
            Knob::Dot,
            Knob::BeadSpacing,
            Knob::Beads,
            Knob::Line,
            Knob::HoverLine,
            Knob::IdleDays,
        ];
        for knob in knobs {
            for k in 0..=40 {
                let frac = k as f32 / 40.;
                let value = knob.at(frac);
                let mut settings = Settings::default();
                knob.set(&mut settings, value);
                settings.clamp();
                assert_eq!(knob.get(&settings), value, "{knob:?} at {frac}");
                // The knob sits where the value is.
                assert_eq!(knob.at(knob.fraction(value)), value, "{knob:?} {value}");
            }
            assert!(knob.at(0.) < knob.at(1.), "{knob:?} runs one way");
        }
        assert_eq!(Knob::CodeLine.at(Knob::CodeLine.fraction(1.6)), 1.6);
        assert_eq!(Knob::IdleDays.at(Knob::IdleDays.fraction(14.)), 14.);
    }
}
