use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

use gpui::{
    AnyElement, App, Bounds, Context, FocusHandle, Global, KeyDownEvent, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, prelude::*, px, size,
};

use crate::line_edit::LineEdit;
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
    /// Summarize several commits with Apple's on-device model; `None` until
    /// the first run finds out whether this Mac has it.
    summaries: Option<bool>,
    /// Which of the app's commands the terminal prints, by kind.
    terminal_changes: bool,
    terminal_reads: bool,
    terminal_refresh: bool,
    /// Commands the terminal leaves out whatever their kind, a glob a line.
    terminal_hidden: Option<String>,
    /// At launch, problems with external programs open the tools window.
    tool_check: bool,
    /// File trees list folders before files; off, the two mix by name.
    folders_first: bool,
    /// How many commands run lately the palette lists first.
    palette_recent: u32,
    /// A side-by-side diff whose sides hold fewer characters than this
    /// shows in one column; 0 never does.
    one_column_below: u32,
    graph: GraphStyle,
    /// Labels show how far a branch is ahead of and behind its upstream.
    sync_badges: bool,
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
            summaries: None,
            terminal_changes: true,
            terminal_reads: false,
            terminal_refresh: false,
            terminal_hidden: None,
            tool_check: true,
            folders_first: false,
            one_column_below: 80,
            palette_recent: 5,
            graph: GraphStyle::default(),
            sync_badges: true,
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

/// Minutes between background fetches of `repo`; 0 for none.
pub fn fetch_minutes(repo: &str) -> u32 {
    ProjectSetting::FetchMinutes
        .value(&Scope::Repo(repo.to_string()))
        .parse()
        .unwrap_or(DEFAULT_FETCH_MINUTES)
}

pub fn sync_badges() -> bool {
    read().sync_badges
}

/// The option the Pull (`false`) or Push (`true`) button runs in `repo`.
pub fn sync_option(push: bool, repo: &str) -> usize {
    let setting = match push {
        true => ProjectSetting::Push,
        false => ProjectSetting::Pull,
    };
    let options = crate::sync::options(push);
    setting
        .value(&Scope::Repo(repo.to_string()))
        .parse()
        .unwrap_or(0)
        .min(options.len() - 1)
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
            Knob::FetchMinutes => DEFAULT_FETCH_MINUTES as f32,
            Knob::RowHeight => s.graph.row_height,
            Knob::Column => s.graph.column,
            Knob::Curve => s.graph.curve,
            Knob::Dot => s.graph.dot,
            Knob::BeadSpacing => s.graph.bead_spacing,
            Knob::Beads => s.graph.beads as f32,
            Knob::Line => s.graph.line,
            Knob::HoverLine => s.graph.hover_line,
            // A project's, read where the slider shows it.
            Knob::IdleDays => DEFAULT_IDLE_DAYS as f32,
        }
    }

    fn set(self, s: &mut Settings, value: f32) {
        match self {
            Knob::UiSize => s.ui_size = value,
            Knob::CodeSize => s.code_size = value,
            Knob::CodeWeight => s.code_weight = value,
            Knob::CodeLine => s.code_line = value,
            Knob::TerminalLine => s.terminal_line = value,
            Knob::FetchMinutes => {}
            Knob::RowHeight => s.graph.row_height = value,
            Knob::Column => s.graph.column = value,
            Knob::Curve => s.graph.curve = value,
            Knob::Dot => s.graph.dot = value,
            Knob::BeadSpacing => s.graph.bead_spacing = value,
            Knob::Beads => s.graph.beads = value as usize,
            Knob::Line => s.graph.line = value,
            Knob::HoverLine => s.graph.hover_line = value,
            Knob::IdleDays => {}
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
/// How many commands run lately the command palette lists first.
pub fn palette_recent() -> usize {
    read().palette_recent as usize
}

/// How narrow, in characters, a side of a split diff may get before the
/// diff shows in one column; 0 for never.
pub fn one_column_below() -> u32 {
    read().one_column_below
}

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

/// The files the structural diff leaves to the text one, built in: those
/// that are code around data, where its structure tells nothing.
pub const TEXT_ONLY: &str = "*.snap\n*.lock\npackage-lock.json\n*-lock.yaml\n*.min.js\n*.min.css";

/// Which of the app's commands the terminal prints; with `all`, every one.
pub fn terminal_shown(all: bool) -> crate::console_view::Shown {
    let settings = read();
    crate::console_view::Shown {
        changes: settings.terminal_changes,
        reads: settings.terminal_reads,
        refresh: settings.terminal_refresh,
        hidden: settings
            .terminal_hidden
            .as_deref()
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        all,
    }
}

/// What leaves a file of `repo` to the text diff: the project's globs,
/// and the size in bytes past which none is diffed by structure.
pub fn text_only_rule(repo: &str) -> wtf_core::TextOnly {
    let scope = Scope::Repo(repo.to_string());
    let kb: usize = ProjectSetting::TextDiffAbove
        .value(&scope)
        .parse()
        .unwrap_or(DEFAULT_TEXT_DIFF_ABOVE);
    wtf_core::TextOnly {
        patterns: ProjectSetting::TextDiff
            .value(&scope)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        max_bytes: match kb {
            0 => usize::MAX,
            kb => kb * 1024,
        },
    }
}

/// Days without a commit after which a branch reads as idle, unless git
/// config says otherwise.
pub const DEFAULT_IDLE_DAYS: u32 = 14;

/// Minutes between background fetches, unless git config says otherwise.
const DEFAULT_FETCH_MINUTES: u32 = 5;

/// Files larger than this, in kilobytes, get the text diff, unless git
/// config says otherwise; 0 for no limit.
const DEFAULT_TEXT_DIFF_ABOVE: usize = 200;

/// What the model is told to summarize commits of `repo`.
pub fn summary_template(repo: &str) -> String {
    ProjectSetting::Summary.value(&Scope::Repo(repo.to_string()))
}

/// What the model is told to write a commit message in `repo`.
pub fn message_template(repo: &str) -> String {
    ProjectSetting::Message.value(&Scope::Repo(repo.to_string()))
}

/// A project, as Settings show its settings: every repository's, kept in
/// the global git config, or one repository's own, kept in its config.
#[derive(Clone, PartialEq, Debug)]
enum Scope {
    Global,
    Repo(String),
}

impl Scope {
    fn config(&self) -> wtf_core::ConfigScope<'_> {
        match self {
            Scope::Global => wtf_core::ConfigScope::Global,
            Scope::Repo(repo) => wtf_core::ConfigScope::Repo(repo),
        }
    }

    /// Where it is cached: the repository, or nothing for global.
    fn key(&self) -> &str {
        match self {
            Scope::Global => "",
            Scope::Repo(repo) => repo,
        }
    }

    /// The project it goes by where it has nothing of its own.
    fn outer(&self) -> Option<Scope> {
        match self {
            Scope::Global => None,
            Scope::Repo(_) => Some(Scope::Global),
        }
    }
}

/// What a project keeps of its own, a line a value where there are several;
/// read from git config once, then as Settings change it.
#[derive(Clone, Default)]
struct Own {
    summary: Option<String>,
    message: Option<String>,
    text_diff: Option<String>,
    text_diff_above: Option<String>,
    idle_days: Option<String>,
    fetch_minutes: Option<String>,
    prune: Option<String>,
    /// The Pull and Push buttons' options, as places in their lists.
    pull: Option<String>,
    push: Option<String>,
    /// Who commits are by.
    name: Option<String>,
    email: Option<String>,
    channels: Vec<String>,
}

static OWN: LazyLock<RwLock<std::collections::HashMap<String, Own>>> =
    LazyLock::new(Default::default);

fn own(scope: &Scope) -> Own {
    if let Some(own) = OWN
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .get(scope.key())
    {
        return own.clone();
    }
    let config = scope.config();
    let lines = |key: &str| {
        let values = wtf_core::project_values(config, key);
        (!values.is_empty()).then(|| values.join("\n"))
    };
    let value = |setting: ProjectSetting| wtf_core::project_value(config, setting.key());
    let push = value(ProjectSetting::Push).and_then(|label| {
        let options = crate::sync::options(true);
        let at = options
            .iter()
            .position(|option| crate::sync::label(true, option) == label)?;
        Some(at.to_string())
    });
    let own = Own {
        summary: value(ProjectSetting::Summary),
        message: value(ProjectSetting::Message),
        text_diff: lines(ProjectSetting::TextDiff.key()),
        text_diff_above: value(ProjectSetting::TextDiffAbove),
        idle_days: value(ProjectSetting::IdleDays),
        fetch_minutes: value(ProjectSetting::FetchMinutes),
        prune: value(ProjectSetting::Prune).map(|on| git_bool(&on).to_string()),
        pull: pull_option(config).map(|at| at.to_string()),
        push,
        name: value(ProjectSetting::Name),
        email: value(ProjectSetting::Email),
        channels: wtf_core::project_values(config, "wtf.channel"),
    };
    OWN.write()
        .unwrap_or_else(|error| error.into_inner())
        .insert(scope.key().to_string(), own.clone());
    own
}

/// A boolean as git config writes it.
fn git_bool(value: &str) -> bool {
    matches!(
        value.trim().to_lowercase().as_str(),
        "true" | "yes" | "on" | "1"
    )
}

/// The Pull option git's own settings at `config` choose, when they choose
/// one: `pull.rebase`, with `rebase.autoStash`, or `pull.ff`.
fn pull_option(config: wtf_core::ConfigScope) -> Option<usize> {
    let value = |key| wtf_core::project_value(config, key);
    let rebase = value("pull.rebase");
    let autostash = value("rebase.autoStash").is_some_and(|on| git_bool(&on));
    match (rebase.as_deref().map(git_bool), value("pull.ff").as_deref()) {
        (Some(true), _) if autostash => Some(3),
        (Some(true), _) => Some(2),
        (_, Some("only")) => Some(1),
        (Some(false), _) | (_, Some(_)) => Some(0),
        (None, None) => None,
    }
}

/// Sets git's own settings at `config` for a Pull option, or with `None`
/// drops them.
fn set_pull_option(
    config: wtf_core::ConfigScope,
    option: Option<usize>,
) -> Result<(), wtf_core::CoreError> {
    let keys: &[(&str, &str)] = match option {
        None => &[],
        Some(0) => &[("pull.rebase", "false")],
        Some(1) => &[("pull.ff", "only")],
        Some(2) => &[("pull.rebase", "true")],
        Some(_) => &[("pull.rebase", "true"), ("rebase.autoStash", "true")],
    };
    for key in ["pull.rebase", "pull.ff", "rebase.autoStash"] {
        let value = keys
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, value)| *value);
        wtf_core::set_project_values(config, key, value.as_slice())?;
    }
    Ok(())
}

/// Reads a project's settings from git config again when next asked.
fn forget(scope: &Scope) {
    OWN.write()
        .unwrap_or_else(|error| error.into_inner())
        .remove(scope.key());
}

/// Repositories' tabs to show a change of `scope`'s settings: its own, or
/// every one for global ones.
fn reload_tabs(scope: &Scope, cx: &mut App) {
    for view in crate::workspace::repo_views(cx) {
        let shown = view.read(cx).repo_path().map(str::to_string);
        if *scope == Scope::Global || shown.as_deref() == Some(scope.key()) {
            view.update(cx, |view, cx| view.reload(cx));
        }
    }
}

/// The global channel patterns, for every repository.
fn global_channels() -> Vec<String> {
    own(&Scope::Global).channels
}

fn set_global_channels(patterns: &[String], cx: &mut App) {
    let patterns: Vec<&str> = patterns.iter().map(String::as_str).collect();
    if let Err(error) =
        wtf_core::set_project_values(wtf_core::ConfigScope::Global, "wtf.channel", &patterns)
    {
        eprintln!("wtf: {error}");
    }
    forget(&Scope::Global);
    reload_tabs(&Scope::Global, cx);
    cx.refresh_windows();
}

/// A setting of a project's, as Settings show it.
#[derive(Clone, Copy, PartialEq)]
enum ProjectSetting {
    Summary,
    Message,
    TextDiff,
    TextDiffAbove,
    IdleDays,
    FetchMinutes,
    Prune,
    Pull,
    Push,
    Name,
    Email,
}

impl ProjectSetting {
    /// Its name in git config: wtf's own, or git's. Pull is kept as git's
    /// `pull.rebase`, `pull.ff` and `rebase.autoStash` together.
    fn key(self) -> &'static str {
        match self {
            ProjectSetting::Summary => "wtf.summaryTemplate",
            ProjectSetting::Message => "wtf.messageTemplate",
            ProjectSetting::TextDiff => "wtf.textDiff",
            ProjectSetting::TextDiffAbove => "wtf.textDiffAboveKb",
            ProjectSetting::IdleDays => "wtf.idleDays",
            ProjectSetting::FetchMinutes => "wtf.fetchMinutes",
            ProjectSetting::Prune => "fetch.prune",
            ProjectSetting::Pull => "pull.rebase",
            ProjectSetting::Push => "wtf.push",
            ProjectSetting::Name => "user.name",
            ProjectSetting::Email => "user.email",
        }
    }

    /// What it is without any project's own.
    fn built_in(self) -> String {
        match self {
            ProjectSetting::Summary => wtf_core::SUMMARY_PROMPT.to_string(),
            ProjectSetting::Message => wtf_core::MESSAGE_PROMPT.to_string(),
            ProjectSetting::TextDiff => TEXT_ONLY.to_string(),
            ProjectSetting::TextDiffAbove => DEFAULT_TEXT_DIFF_ABOVE.to_string(),
            ProjectSetting::IdleDays => DEFAULT_IDLE_DAYS.to_string(),
            ProjectSetting::FetchMinutes => DEFAULT_FETCH_MINUTES.to_string(),
            // As git has them: no pruning; Pull fast-forwards only, where
            // git's own default would merge.
            ProjectSetting::Prune => "false".to_string(),
            ProjectSetting::Pull => "1".to_string(),
            ProjectSetting::Push => "0".to_string(),
            // Git has none: commits fail until one is set.
            ProjectSetting::Name | ProjectSetting::Email => String::new(),
        }
    }

    /// The project's own value, if it has one.
    fn own(self, scope: &Scope) -> Option<String> {
        let own = own(scope);
        match self {
            ProjectSetting::Summary => own.summary,
            ProjectSetting::Message => own.message,
            ProjectSetting::TextDiff => own.text_diff,
            ProjectSetting::TextDiffAbove => own.text_diff_above,
            ProjectSetting::IdleDays => own.idle_days,
            ProjectSetting::FetchMinutes => own.fetch_minutes,
            ProjectSetting::Prune => own.prune,
            ProjectSetting::Pull => own.pull,
            ProjectSetting::Push => own.push,
            ProjectSetting::Name => own.name,
            ProjectSetting::Email => own.email,
        }
    }

    /// What the project goes by without its own: the global value, or the
    /// built-in one.
    fn inherited(self, scope: &Scope) -> String {
        scope
            .outer()
            .map_or_else(|| self.built_in(), |outer| self.value(&outer))
    }

    /// What the project goes by.
    fn value(self, scope: &Scope) -> String {
        self.own(scope).unwrap_or_else(|| self.inherited(scope))
    }

    /// Sets the project's own value, or with `None`, as typing the
    /// inherited one back does, goes by that again.
    fn set(self, scope: &Scope, text: Option<String>, cx: &mut App) {
        let text = text
            .filter(|text| *text != self.inherited(scope))
            .filter(|text| !text.trim().is_empty() || !self.built_in().is_empty());
        let push_label = text
            .as_deref()
            .and_then(|at| crate::sync::PUSH.get(at.parse::<usize>().ok()?))
            .map(|option| crate::sync::label(true, option));
        let values: Vec<&str> = match (&text, self) {
            (None, _) => Vec::new(),
            (Some(text), ProjectSetting::TextDiff) => text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect(),
            (Some(_), ProjectSetting::Push) => push_label.as_deref().into_iter().collect(),
            (Some(text), _) => vec![text.as_str()],
        };
        let written = match self {
            ProjectSetting::Pull => set_pull_option(
                scope.config(),
                text.as_deref().and_then(|at| at.parse().ok()),
            ),
            _ => wtf_core::set_project_values(scope.config(), self.key(), &values),
        };
        if let Err(error) = written {
            eprintln!("wtf: {error}");
        }
        forget(scope);
        // The graph reads idle days as it refreshes.
        if self == ProjectSetting::IdleDays {
            reload_tabs(scope, cx);
        }
        cx.refresh_windows();
    }
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
    let text = fs::read_to_string(file()).unwrap_or_default();
    let settings = parse(&text);
    *SETTINGS.write().unwrap_or_else(|error| error.into_inner()) = settings;
    // Settings once the app's, now every project's: set ones go to the
    // global git config, where it has none, and leave the file.
    let moved = moved_settings(&text);
    for (key, values) in &moved {
        let config = wtf_core::ConfigScope::Global;
        if wtf_core::project_values(config, key).is_empty() {
            let values: Vec<&str> = values.iter().map(String::as_str).collect();
            wtf_core::set_project_values(config, key, &values).ok();
        }
    }
    if !moved.is_empty() {
        save();
    }
}

/// Settings the file kept that projects keep now, with their keys in git
/// config: those set to other than the built-in values. Pull options are
/// git's own now, and prune too: those are left to git config.
fn moved_settings(text: &str) -> Vec<(&'static str, Vec<String>)> {
    let changed = |value: &str, default: &str| {
        (value.trim() != default).then(|| vec![value.trim().to_string()])
    };
    text.lines()
        .filter_map(|line| line.split_once('='))
        .filter_map(|(key, value)| match key.trim() {
            "summary_prompt" => Some(("wtf.summaryTemplate", vec![unescape(value)])),
            "message_prompt" => Some(("wtf.messageTemplate", vec![unescape(value)])),
            "text_only" => Some((
                "wtf.textDiff",
                unescape(value)
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect(),
            )),
            "idle_days" => Some((
                "wtf.idleDays",
                changed(value, &DEFAULT_IDLE_DAYS.to_string())?,
            )),
            "fetch_minutes" => Some((
                "wtf.fetchMinutes",
                changed(value, &DEFAULT_FETCH_MINUTES.to_string())?,
            )),
            "structural_max_kb" => Some((
                "wtf.textDiffAboveKb",
                changed(value, &DEFAULT_TEXT_DIFF_ABOVE.to_string())?,
            )),
            "push_option" => {
                let option = crate::sync::PUSH.get(value.trim().parse::<usize>().ok()?)?;
                let label = crate::sync::label(true, option);
                Some(("wtf.push", changed(&label, "push")?))
            }
            _ => None,
        })
        .collect()
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
            ("summaries", _) => settings.summaries = Some(value == "on"),
            ("sync_badges", _) => settings.sync_badges = value == "on",
            ("tool_check", _) => settings.tool_check = value == "on",
            ("folders_first", _) => settings.folders_first = value == "on",
            ("palette_recent", _) => {
                if let Ok(count) = value.parse() {
                    settings.palette_recent = count;
                }
            }
            ("one_column_below", _) => {
                if let Ok(chars) = value.parse() {
                    settings.one_column_below = chars;
                }
            }
            ("terminal_hidden", _) => settings.terminal_hidden = Some(unescape(raw)),
            ("terminal_changes", _) => settings.terminal_changes = value == "on",
            ("terminal_reads", _) => settings.terminal_reads = value == "on",
            ("terminal_refresh", _) => settings.terminal_refresh = value == "on",
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
        format!("sync_badges={}", on_off(settings.sync_badges)),
        format!("tool_check={}", on_off(settings.tool_check)),
        format!("folders_first={}", on_off(settings.folders_first)),
        format!("one_column_below={}", settings.one_column_below),
        format!("palette_recent={}", settings.palette_recent),
        match settings.summaries {
            Some(true) => "summaries=on".to_string(),
            Some(false) => "summaries=off".to_string(),
            None => String::new(),
        },
        settings
            .terminal_hidden
            .as_ref()
            .map_or(String::new(), |p| format!("terminal_hidden={}", escape(p))),
        format!("terminal_changes={}", on_off(settings.terminal_changes)),
        format!("terminal_reads={}", on_off(settings.terminal_reads)),
        format!("terminal_refresh={}", on_off(settings.terminal_refresh)),
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
    // The repository in front, for its own settings; with Settings in
    // front, the one they show stays.
    let front = crate::workspace::front_repo(cx);
    let existing = cx
        .try_global::<PreferencesWindow>()
        .and_then(|window| window.0);
    if let (Some(window), Some(repo)) = (existing, front.clone()) {
        window
            .update(cx, |preferences, _, cx| {
                let scope = Scope::Repo(repo);
                if preferences.scope != scope {
                    preferences.pick(scope, cx);
                    cx.notify();
                }
            })
            .ok();
    }
    if bring_forward(existing, cx) {
        return;
    }
    let scope = front
        .or_else(|| {
            crate::workspace::repo_views(cx)
                .first()
                .and_then(|view| view.read(cx).repo_path().map(str::to_string))
        })
        .map_or(Scope::Global, Scope::Repo);
    let options = panel_options("Settings", 680., 540., cx);
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let focus = cx.focus_handle();
            focus.focus(window);
            let mut preferences = Preferences {
                focus,
                fonts: installed_fonts(window, cx),
                section: Section::Appearance,
                font_menu: None,
                font_query: Default::default(),
                prompts: vec![Field::default(); PROJECT_FIELDS + 5],
                prompt_focus: (0..PROJECT_FIELDS + 5).map(|_| cx.focus_handle()).collect(),
                dragging: None,
                tracks: Default::default(),
                scope: Scope::Global,
                project_menu: None,
                watching: None,
                channel_input: Field::default(),
                channel_focus: cx.focus_handle(),
                showing_sent: None,
            };
            preferences.pick(scope, cx);
            preferences
        })
    });
    if let Ok(handle) = opened {
        cx.set_global(PreferencesWindow(Some(handle)));
    }
}

/// Opens Settings at `repo`'s branch settings, its channels among them,
/// once the window asking is done, so its tabs can be read.
pub fn open_channels(repo: String, cx: &mut App) {
    cx.defer(move |cx| {
        open(cx);
        let Some(handle) = cx
            .try_global::<PreferencesWindow>()
            .and_then(|window| window.0)
        else {
            return;
        };
        handle
            .update(cx, |preferences, _, cx| {
                preferences.section = Section::Branches;
                preferences.pick(Scope::Repo(repo), cx);
                cx.notify();
            })
            .ok();
    });
}

/// The field of the commands the terminal never shows.
const HIDDEN_FIELD: usize = 0;

/// Where the fields of a project's settings start, after the app's: the
/// text diff files, the summary and commit message templates, then the
/// author's name and email, each a line.
const PROJECT_FIELDS: usize = 1;

pub struct Preferences {
    focus: FocusHandle,
    /// Installed font families, each with whether it is monospace.
    fonts: Vec<(String, bool)>,
    /// The section shown, picked in the sidebar.
    section: Section,
    /// The font dropdown, when open, and where.
    font_menu: Option<(FontSetting, gpui::Point<gpui::Pixels>)>,
    font_query: Field,
    /// The prompts as edited, each a multi-line field: the app's, then from
    /// `PROJECT_FIELDS` the repository's.
    prompts: Vec<Field>,
    /// The prompt fields, each with its own focus.
    prompt_focus: Vec<FocusHandle>,
    /// The slider being dragged, and where each slider's track was drawn.
    dragging: Option<Knob>,
    tracks: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<Knob, Bounds<gpui::Pixels>>>>,
    /// The project whose settings show: global, or a repository open in a
    /// tab.
    scope: Scope,
    /// The list of projects to pick one from, when open, and where.
    project_menu: Option<gpui::Point<gpui::Pixels>>,
    /// The repository's tab, followed so its channels show as they change.
    watching: Option<gpui::Subscription>,
    /// A channel pattern being typed.
    channel_input: Field,
    channel_focus: FocusHandle,
    /// The request whose last sending shows under its template.
    showing_sent: Option<wtf_core::Request>,
}

/// The settings window's sections, in sidebar order: the app's, then a
/// repository's own.
#[derive(Clone, Copy, PartialEq)]
enum Section {
    Appearance,
    Fonts,
    Graph,
    Review,
    Terminal,
    Author,
    Branches,
    Diffs,
    Messages,
    Sync,
}

impl Section {
    const ALL: [Section; 5] = [
        Section::Appearance,
        Section::Fonts,
        Section::Graph,
        Section::Review,
        Section::Terminal,
    ];

    const PROJECT: [Section; 5] = [
        Section::Author,
        Section::Branches,
        Section::Diffs,
        Section::Messages,
        Section::Sync,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::Appearance => "Appearance",
            Section::Fonts => "Fonts",
            Section::Graph => "Workflow graph",
            Section::Review => "Review",
            Section::Terminal => "Terminal",
            Section::Author => "Author",
            Section::Branches => "Branches",
            Section::Diffs => "Diffs",
            Section::Messages => "Messages",
            Section::Sync => "Sync",
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
fn row(label: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
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
                .child(label.into()),
        )
}

/// A row of a project's setting: its name and where its value comes from,
/// and the control under them, as there is no room for all in a line.
fn project_row(
    label: &'static str,
    origin: AnyElement,
    control: impl IntoElement,
    theme: &Theme,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(12.))
        .py(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().text_color(theme.foreground).child(label))
                .child(origin),
        )
        .child(control)
}

/// A small label saying where a value comes from; `strong` when it is
/// one of the user's own.
fn pill(label: &'static str, strong: bool, theme: &Theme) -> gpui::Div {
    div()
        .px(px(6.))
        .rounded(px(4.))
        .text_xs()
        .border_1()
        .when(strong, |pill| {
            pill.border_color(theme.accent).text_color(theme.accent)
        })
        .when(!strong, |pill| {
            pill.border_color(theme.separator)
                .text_color(theme.secondary)
        })
        .child(label)
}

/// A box that scrolls on its own inside the scrolling page: gpui hands a
/// wheel to every scrolling box under the pointer, so the page would move
/// along with it.
fn scroll_alone(scrolling: impl IntoElement) -> gpui::Div {
    div()
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(scrolling)
}

/// A small action in a card's corner, such as a reset.
fn link(id: &'static str, label: &'static str, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .text_xs()
        .text_color(theme.accent)
        .cursor_pointer()
        .child(label)
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
        let value = self.knob_value(knob);
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
        if self.knob_value(knob) != value {
            match knob {
                Knob::IdleDays => {
                    ProjectSetting::IdleDays.set(&self.scope, Some(value.to_string()), cx)
                }
                Knob::FetchMinutes => {
                    ProjectSetting::FetchMinutes.set(&self.scope, Some(value.to_string()), cx)
                }
                _ => change(|settings| knob.set(settings, value), cx),
            }
        }
        cx.notify();
    }

    /// A slider's number: the app's, or the project's for idle days and
    /// fetches.
    fn knob_value(&self, knob: Knob) -> f32 {
        let project = |setting: ProjectSetting, default: u32| {
            setting.value(&self.scope).parse().unwrap_or(default as f32)
        };
        match knob {
            Knob::IdleDays => project(ProjectSetting::IdleDays, DEFAULT_IDLE_DAYS),
            Knob::FetchMinutes => project(ProjectSetting::FetchMinutes, DEFAULT_FETCH_MINUTES),
            _ => knob.get(&read()),
        }
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

    /// Shows a project's settings, following a repository's tab for its
    /// channels.
    fn pick(&mut self, scope: Scope, cx: &mut Context<Self>) {
        self.watching = match &scope {
            Scope::Repo(repo) => crate::workspace::repo_view(repo, cx)
                .map(|view| cx.observe(&view, |_, _, cx| cx.notify())),
            Scope::Global => None,
        };
        self.scope = scope;
        self.project_menu = None;
        self.channel_input.edit.set_text("");
    }

    /// The project's branch settings. Its channels: for a repository, the
    /// main branch, the global patterns and its own, each with how many
    /// branches it names; then a field to add one, telling what it would
    /// name as it is typed. Then how long until a branch is idle.
    fn branches_content(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let scope = self.scope.clone();
        let view = match &scope {
            Scope::Repo(repo) => crate::workspace::repo_view(repo, cx),
            Scope::Global => None,
        };
        let global = global_channels();
        let tab = view.as_ref().map(|view| view.read(cx));
        let count = |pattern: &str| tab.map(|tab| tab.channel_matches(pattern).len());
        let line = || {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .min_h(px(36.))
                .px(px(12.))
                .py(px(4.))
        };
        let mut rows = Vec::new();
        if let Some(tab) = tab {
            rows.push(
                line()
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .child(tab.channels().0.to_string()),
                    )
                    .child(div().text_color(theme.secondary).child("main branch")),
            );
        }
        // A repository's own come after the global ones, which only Global
        // drops.
        let own: Vec<String> = match tab {
            Some(tab) => tab
                .channels()
                .1
                .iter()
                .filter(|pattern| !global.contains(pattern))
                .cloned()
                .collect(),
            None => global.clone(),
        };
        let inherited: Vec<String> = match scope {
            Scope::Repo(_) => global.clone(),
            Scope::Global => Vec::new(),
        };
        let patterns = inherited
            .into_iter()
            .map(|pattern| (pattern, false))
            .chain(own.into_iter().map(|pattern| (pattern, true)));
        for (pattern, removable) in patterns {
            let found = count(&pattern);
            let remove = pattern.clone();
            let view = view.clone();
            rows.push(
                line()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(code_font())
                            .child(pattern.clone()),
                    )
                    .when(!removable, |row| row.child(pill("Global", false, theme)))
                    .children(found.map(|found| {
                        div().text_color(theme.secondary).child(match found {
                            1 => "1 branch".to_string(),
                            n => format!("{n} branches"),
                        })
                    }))
                    .when(removable, |row| {
                        row.child(
                            div()
                                .id(SharedString::from(format!("channel-remove-{pattern}")))
                                .px(px(4.))
                                .rounded(px(3.))
                                .text_color(theme.secondary)
                                .cursor_pointer()
                                .hover(|button| button.bg(theme.selection))
                                .on_click(move |_, _, cx| match &view {
                                    Some(view) => {
                                        view.update(cx, |view, cx| view.remove_channel(&remove, cx))
                                    }
                                    None => {
                                        let mut patterns = global_channels();
                                        patterns.retain(|pattern| *pattern != remove);
                                        set_global_channels(&patterns, cx);
                                    }
                                })
                                .child("✕"),
                        )
                    }),
            );
        }
        let hint = match tab {
            Some(tab) => tab.channel_hint(self.channel_input.text()),
            None => "* matches any run of characters, ? one, [0-9] one of a set. \
                     Enter to add."
                .to_string(),
        };
        let focused = self.channel_focus.is_focused(window);
        let add = div()
            .id("channel-add")
            .track_focus(&self.channel_focus)
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "enter" {
                    let pattern = this.channel_input.text().trim().to_string();
                    match &this.scope {
                        Scope::Repo(repo) => {
                            if let Some(view) = crate::workspace::repo_view(repo, cx) {
                                view.update(cx, |view, cx| view.add_channel(&pattern, cx));
                            }
                        }
                        Scope::Global => {
                            let mut patterns = global_channels();
                            if !pattern.is_empty() && !patterns.contains(&pattern) {
                                patterns.push(pattern);
                                set_global_channels(&patterns, cx);
                            }
                        }
                    }
                    this.channel_input.edit.set_text("");
                } else if !this.channel_input.key(&event.keystroke, cx) {
                    return;
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .w_full()
            .px(px(8.))
            .py(px(5.))
            .rounded(px(6.))
            .border_1()
            .border_color(if focused {
                theme.accent
            } else {
                theme.separator
            })
            .bg(theme.background)
            .whitespace_nowrap()
            .overflow_hidden()
            .child(self.channel_input.render(
                "channel-add-text",
                "Add a branch or a pattern, such as v[0-9]*",
                focused,
                true,
                Vec::new(),
                &self.channel_focus,
                theme,
                |this: &mut Self| Some(&mut this.channel_input),
                cx,
            ));
        let channels_note = match self.scope {
            Scope::Global => {
                "Branches finished work goes to, in every repository, besides its main \
                 branch. Work merged into one is done, and Prune offers it."
            }
            Scope::Repo(_) => {
                "Branches finished work goes to. Work merged into one is done, and \
                 Prune offers it."
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(group(
                Some("Channels"),
                rows.into_iter().chain([line().child(add)]),
                None,
                theme,
            ))
            .child(
                div()
                    .px(px(12.))
                    .mt(px(-14.))
                    .text_xs()
                    .text_color(theme.secondary)
                    .child(hint)
                    .child(div().pt(px(4.)).child(channels_note)),
            )
            .child(group(
                Some("Branches"),
                [project_row(
                    "Idle after",
                    self.origin(ProjectSetting::IdleDays, theme, cx),
                    self.slider(Knob::IdleDays, theme, cx),
                    theme,
                )],
                Some("A branch without commits for this long reads as idle, and Prune offers it."),
                theme,
            ))
    }

    /// The project's name: its folder's, or Global.
    fn scope_name(&self) -> String {
        match &self.scope {
            Scope::Global => "Global".to_string(),
            Scope::Repo(repo) => crate::workspace::folder_name(repo),
        }
    }

    /// Where the project's settings are kept, over its sections.
    fn scope_note(&self, theme: &Theme) -> gpui::Div {
        div()
            .px(px(12.))
            .text_xs()
            .text_color(theme.secondary)
            .child(match self.scope {
                Scope::Global => {
                    "Every repository's, kept in your global git config, ~/.gitconfig. A \
                     repository's own come first."
                }
                Scope::Repo(_) => {
                    "This repository's, kept in its git config and shared by its \
                     worktrees. What it has none of comes from Global."
                }
            })
    }

    /// The open list of projects to show the settings of: global, then each
    /// repository open in a tab.
    fn project_menu(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let position = self.project_menu?;
        let mut repos: Vec<String> = Vec::new();
        for view in crate::workspace::repo_views(cx) {
            if let Some(path) = view.read(cx).repo_path()
                && !repos.iter().any(|repo| repo == path)
            {
                repos.push(path.to_string());
            }
        }
        let scopes = [Scope::Global]
            .into_iter()
            .chain(repos.into_iter().map(Scope::Repo));
        let items = scopes.enumerate().map(|(k, scope)| {
            let chosen = self.scope == scope;
            let (name, place) = match &scope {
                Scope::Global => ("Global".to_string(), "every repository".to_string()),
                Scope::Repo(repo) => (crate::workspace::folder_name(repo), repo.clone()),
            };
            div()
                .id(("project", k))
                .flex()
                .gap(px(6.))
                .px(px(8.))
                .py(px(2.))
                .hover(|item| item.bg(theme.selection))
                .cursor_pointer()
                .child(div().w(px(10.)).child(if chosen { "✓" } else { "" }))
                .child(div().child(name))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_right()
                        .text_color(theme.secondary)
                        .child(place),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pick(scope.clone(), cx);
                    cx.notify();
                }))
        });
        let menu = div()
            .id("project-menu")
            .occlude()
            .flex()
            .flex_col()
            .w(px(320.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .shadow_md()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.project_menu = None;
                cx.notify();
            }))
            .children(items);
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

    /// The last request of a kind the model got this run, filled in as
    /// sent, with its answer; folded under a line saying when it went.
    fn last_sent(
        &self,
        request: wtf_core::Request,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let note = div().px(px(12.)).text_xs().text_color(theme.secondary);
        let Some(sent) = wtf_core::last_sent(request) else {
            return note.child("Nothing sent with it yet since wtf started.");
        };
        let open = self.showing_sent == Some(request);
        let age = crate::repo_view::short_age(sent.time, crate::repo_view::now_seconds());
        let answer = match &sent.answer {
            Ok(answer) => answer.trim().to_string(),
            Err(error) => format!("Failed: {error}"),
        };
        let parts = [
            ("Instructions", sent.instructions.trim().to_string()),
            ("Input", sent.input.trim().to_string()),
            ("Answer", answer),
        ];
        let whole: String = parts
            .iter()
            .filter(|(_, text)| !text.is_empty())
            .map(|(title, text)| format!("{}:\n{text}", title.to_uppercase()))
            .collect::<Vec<_>>()
            .join("\n\n");
        let toggle = div()
            .id(("last-sent", request as usize))
            .text_color(theme.accent)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.showing_sent = (this.showing_sent != Some(request)).then_some(request);
                cx.notify();
            }))
            .child(format!(
                "{} Last sent, {age} ago, as the model got it",
                if open { "▾" } else { "▸" }
            ));
        let line = note.flex().gap(px(12.)).child(toggle).when(open, |line| {
            line.child(link("copy-sent", "Copy", theme).on_click(move |_, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(whole.clone()))
            }))
        });
        if !open {
            return line;
        }
        let blocks = parts
            .into_iter()
            .filter(|(_, text)| !text.is_empty())
            .map(|(title, text)| {
                // At full height, for the box to scroll rather than squeeze.
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .font_family(ui_font())
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.secondary)
                            .child(title),
                    )
                    .child(div().whitespace_normal().child(text))
            });
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(line)
            .child(scroll_alone(
                card(theme)
                    .id(("sent-text", request as usize))
                    .max_h(px(280.))
                    .overflow_y_scroll()
                    .gap(px(10.))
                    .p(px(10.))
                    .text_xs()
                    .font_family(code_font())
                    .children(blocks),
            ))
    }

    /// The commands the terminal never shows, a pattern a line.
    fn hidden_commands_field(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = "Never show commands matching, a pattern a line, such as git rev-parse*";
        self.field_card(
            HIDDEN_FIELD,
            label,
            None,
            theme,
            window,
            cx,
            |_, text, cx| {
                let text = (!text.trim().is_empty()).then_some(text);
                change(|settings| settings.terminal_hidden = text, cx)
            },
        )
    }

    /// Where the project's value of `setting` comes from, and a way back to
    /// what it would go by when it is its own.
    fn origin(&self, setting: ProjectSetting, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let scope = self.scope.clone();
        let own = setting.own(&scope).is_some();
        let from_global = scope
            .outer()
            .is_some_and(|outer| setting.own(&outer).is_some());
        let none = setting.built_in().is_empty();
        let (label, back) = match (&scope, own, from_global) {
            (Scope::Global, true, _) if none => (pill("Set", true, theme), "Clear"),
            (Scope::Repo(_), true, false) if none => (pill("This repo's", true, theme), "Clear"),
            (_, false, false) if none => (pill("Not set", false, theme), ""),
            (Scope::Global, true, _) => (pill("Custom", true, theme), "Reset to built-in"),
            (Scope::Repo(_), true, true) => (pill("This repo's", true, theme), "Use global"),
            (Scope::Repo(_), true, false) => {
                (pill("This repo's", true, theme), "Reset to built-in")
            }
            (Scope::Repo(_), false, true) => (pill("Global", false, theme), ""),
            (_, false, _) => (pill("Built-in", false, theme), ""),
        };
        div()
            .flex()
            .gap(px(8.))
            .child(label)
            .when(own, |aside| {
                aside.child(
                    link(setting.key(), back, theme)
                        .on_click(cx.listener(move |_, _, _, cx| setting.set(&scope, None, cx))),
                )
            })
            .into_any_element()
    }

    /// A project's setting of one line, such as the author's email: its
    /// name and where it comes from over the field.
    fn line_field(
        &self,
        setting: ProjectSetting,
        label: &'static str,
        k: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let focus = &self.prompt_focus[k];
        let focused = focus.is_focused(window);
        let scope = self.scope.clone();
        let input = div()
            .id(("line-field", k))
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
                    setting.set(&scope, Some(text), cx);
                }
                cx.notify();
            }))
            .w_full()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(if focused {
                theme.accent
            } else {
                theme.separator
            })
            .bg(theme.background)
            .whitespace_nowrap()
            .overflow_hidden()
            .child(self.prompts[k].render(
                ("line-field-text", k),
                "Not set",
                focused,
                true,
                Vec::new(),
                focus,
                theme,
                move |this: &mut Self| this.prompts.get_mut(k),
                cx,
            ));
        project_row(label, self.origin(setting, theme, cx), input, theme)
    }

    /// A project's setting, showing what it goes by, editing its own.
    fn project_field(
        &self,
        setting: ProjectSetting,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (label, k) = match setting {
            ProjectSetting::Summary => ("Summary template", PROJECT_FIELDS + 1),
            ProjectSetting::Message => ("Commit message template", PROJECT_FIELDS + 2),
            _ => (
                "Text diff for these files, a pattern a line",
                PROJECT_FIELDS,
            ),
        };
        let origin = self.origin(setting, theme, cx);
        let scope = self.scope.clone();
        self.field_card(
            k,
            label,
            Some(origin),
            theme,
            window,
            cx,
            move |_, text, cx| setting.set(&scope, Some(text), cx),
        )
    }

    /// Field `k` in a card under `label`, with `aside` at its right, telling
    /// `edited` each change of its text.
    #[allow(clippy::too_many_arguments)]
    fn field_card(
        &self,
        k: usize,
        label: &'static str,
        aside: Option<AnyElement>,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
        edited: impl Fn(&mut Self, String, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
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
                    .children(aside),
            )
            .child(scroll_alone(
                div()
                    .id(("field", k))
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
                            edited(this, text, cx);
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
                        ("field-text", k),
                        "",
                        focused,
                        true,
                        Vec::new(),
                        focus,
                        theme,
                        move |this: &mut Self| this.prompts.get_mut(k),
                        cx,
                    )),
            ))
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
        // A field not being edited shows the setting as it stands, which
        // may have been reset, or be another project's.
        let hidden = read().terminal_hidden.clone().unwrap_or_default();
        let fields = [
            (HIDDEN_FIELD, hidden),
            (PROJECT_FIELDS, ProjectSetting::TextDiff.value(&self.scope)),
            (
                PROJECT_FIELDS + 1,
                ProjectSetting::Summary.value(&self.scope),
            ),
            (
                PROJECT_FIELDS + 2,
                ProjectSetting::Message.value(&self.scope),
            ),
        ];
        for (k, text) in fields {
            if !self.prompt_focus[k].is_focused(window) && self.prompts[k].text() != text {
                self.prompts[k].edit = LineEdit::multiline(text);
            }
        }
        for (k, setting) in [ProjectSetting::Name, ProjectSetting::Email]
            .into_iter()
            .enumerate()
        {
            let (k, text) = (PROJECT_FIELDS + 3 + k, setting.value(&self.scope));
            if !self.prompt_focus[k].is_focused(window) && self.prompts[k].text() != text {
                self.prompts[k].edit = LineEdit::new(text);
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
                    Some("Command palette"),
                    [row("Commands run lately, first", &theme).child(segmented(
                        "palette-recent",
                        &[("None", 0), ("3", 3), ("5", 5), ("10", 10)],
                        settings.palette_recent,
                        |count, cx| change(|settings| settings.palette_recent = count, cx),
                        &theme,
                    ))],
                    Some("The app's commands and the shell's alike, newest first."),
                    &theme,
                ))
                .child(group(
                    Some("Diff"),
                    [
                        row("One column when a side is under", &theme).child(segmented(
                            "one-column-below",
                            &[
                                ("Never", 0),
                                ("60", 60),
                                ("80", 80),
                                ("100", 100),
                                ("120", 120),
                            ],
                            settings.one_column_below,
                            |chars, cx| change(|settings| settings.one_column_below = chars, cx),
                            &theme,
                        )),
                    ],
                    Some(
                        "Characters of code. A split diff narrower than this shows old and \
                         new lines in one column; Split and Unified in the strip choose by hand.",
                    ),
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
                         Nothing leaves this Mac. What it is told, and which files get \
                         the text diff, are a project's: Review under Global or a \
                         repository.",
                    ),
                    &theme,
                )),
            Section::Terminal => div()
                .child(group(
                    Some("Show the app's commands"),
                    [
                        row("Changes it makes", &theme).child(switch_for(
                            "terminal-changes",
                            settings.terminal_changes,
                            |s, on| s.terminal_changes = on,
                        )),
                        row("Reads for what you look at", &theme).child(switch_for(
                            "terminal-reads",
                            settings.terminal_reads,
                            |s, on| s.terminal_reads = on,
                        )),
                        row("Background refresh", &theme).child(switch_for(
                            "terminal-refresh",
                            settings.terminal_refresh,
                            |s, on| s.terminal_refresh = on,
                        )),
                    ],
                    Some(
                        "Changes stage, commit, check out, stash, push and pull. Reads are \
                         the diffs, logs and files the screen shows; the refresh keeps the \
                         graph current. Commands you run show always, and so does any \
                         command that fails. Show All Commands, in the palette, shows \
                         everything for a while.",
                    ),
                    &theme,
                ))
                .child(self.hidden_commands_field(&theme, window, cx)),
            Section::Author => div().child(self.scope_note(&theme)).child(group(
                None,
                [
                    self.line_field(
                        ProjectSetting::Name,
                        "Name",
                        PROJECT_FIELDS + 3,
                        &theme,
                        window,
                        cx,
                    ),
                    self.line_field(
                        ProjectSetting::Email,
                        "Email",
                        PROJECT_FIELDS + 4,
                        &theme,
                        window,
                        cx,
                    ),
                ],
                Some(
                    "Who commits are by: git's user.name and user.email, which git \
                     commit in a terminal goes by too.",
                ),
                &theme,
            )),
            Section::Branches => div()
                .child(self.scope_note(&theme))
                .child(self.branches_content(&theme, window, cx)),
            Section::Diffs => {
                let scope = self.scope.clone();
                let kb = ProjectSetting::TextDiffAbove
                    .value(&scope)
                    .parse()
                    .unwrap_or(DEFAULT_TEXT_DIFF_ABOVE);
                div()
                    .child(self.scope_note(&theme))
                    .child(group(
                        None,
                        [project_row(
                            "Text diff above",
                            self.origin(ProjectSetting::TextDiffAbove, &theme, cx),
                            segmented(
                                "text-diff-above",
                                &[
                                    ("100 KB", 100),
                                    ("200 KB", 200),
                                    ("500 KB", 500),
                                    ("1 MB", 1024),
                                    ("Any size", 0),
                                ],
                                kb,
                                move |kb: usize, cx| {
                                    ProjectSetting::TextDiffAbove.set(
                                        &scope,
                                        Some(kb.to_string()),
                                        cx,
                                    )
                                },
                                &theme,
                            ),
                            &theme,
                        )],
                        Some(
                            "Larger files, and those below, get the text diff even in \
                             Structural mode: their structure is slow to find and tells little.",
                        ),
                        &theme,
                    ))
                    .child(self.project_field(ProjectSetting::TextDiff, &theme, window, cx))
            }
            Section::Messages => div()
                .child(self.scope_note(&theme))
                .child(self.project_field(ProjectSetting::Summary, &theme, window, cx))
                .child(self.last_sent(wtf_core::Request::Summary, &theme, cx))
                .child(self.project_field(ProjectSetting::Message, &theme, window, cx))
                .child(self.last_sent(wtf_core::Request::Message, &theme, cx))
                .child(
                    div()
                        .px(px(12.))
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(TEMPLATE_NOTE),
                ),
            Section::Sync => {
                let scope = self.scope.clone();
                let prune = git_bool(&ProjectSetting::Prune.value(&scope));
                let option = |setting: ProjectSetting| -> usize {
                    setting.value(&scope).parse().unwrap_or(0)
                };
                let (pull, push) = (option(ProjectSetting::Pull), option(ProjectSetting::Push));
                let set = |setting: ProjectSetting| {
                    let scope = scope.clone();
                    move |at: usize, cx: &mut App| setting.set(&scope, Some(at.to_string()), cx)
                };
                let set_prune = {
                    let scope = scope.clone();
                    move |on: bool, cx: &mut App| {
                        ProjectSetting::Prune.set(&scope, Some(on.to_string()), cx)
                    }
                };
                div()
                    .child(self.scope_note(&theme))
                    .child(group(
                        Some("Fetch"),
                        [
                            project_row(
                                "In the background",
                                self.origin(ProjectSetting::FetchMinutes, &theme, cx),
                                self.slider(Knob::FetchMinutes, &theme, cx),
                                &theme,
                            ),
                            project_row(
                                "Prune",
                                self.origin(ProjectSetting::Prune, &theme, cx),
                                switch("prune", prune, set_prune, &theme),
                                &theme,
                            ),
                        ],
                        Some(
                            "Fetches every remote. Pruning drops remote branches deleted there, \
                             so local ones tracking them read as remote gone; kept as git's \
                             fetch.prune, which git fetch in a terminal goes by too.",
                        ),
                        &theme,
                    ))
                    .child(group(
                        Some("Buttons"),
                        [
                            project_row(
                                "Pull runs",
                                self.origin(ProjectSetting::Pull, &theme, cx),
                                segmented(
                                    "pull-option",
                                    &[
                                        ("Merge", 0),
                                        ("Fast-forward", 1),
                                        ("Rebase", 2),
                                        ("Rebase, stash", 3),
                                    ],
                                    pull,
                                    set(ProjectSetting::Pull),
                                    &theme,
                                ),
                                &theme,
                            ),
                            project_row(
                                "Push runs",
                                self.origin(ProjectSetting::Push, &theme, cx),
                                segmented(
                                    "push-option",
                                    &[("Push", 0), ("Force with lease", 1), ("No verify", 2)],
                                    push,
                                    set(ProjectSetting::Push),
                                    &theme,
                                ),
                                &theme,
                            ),
                        ],
                        Some(
                            "What the Pull and Push buttons run; their menus run the others \
                             once. Pull is kept as git's pull.rebase, pull.ff and \
                             rebase.autoStash, which git pull in a terminal goes by too.",
                        ),
                        &theme,
                    ))
            }
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
                    Some("Labels"),
                    [row("Ahead/behind badges", &theme).child(switch_for(
                        "badges",
                        settings.sync_badges,
                        |s, on| s.sync_badges = on,
                    ))],
                    Some("How far a branch is from its upstream, as ↑2 ↓1."),
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
                                // Everything in this section.
                                change(|settings| settings.graph = GraphStyle::default(), cx)
                            }))
                            .child("Reset to Defaults"),
                    ),
                ),
        }
        // Groups apart, as in System Settings.
        .flex()
        .flex_col()
        .gap(px(20.));
        let item = |section: Section| {
            div()
                .id(("section", section as usize))
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
        };
        let sections = Section::ALL.map(&item);
        // The app's settings, then a project's: global ones or a
        // repository's, under a button to pick which.
        let heading = |text: &'static str| {
            div()
                .px(px(10.))
                .pb(px(2.))
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.secondary)
                .child(text)
        };
        let picker = div()
            .id("project-picker")
            .flex()
            .items_center()
            .gap(px(4.))
            .mx(px(4.))
            .mb(px(2.))
            .px(px(6.))
            .py(px(2.))
            .rounded(px(5.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .cursor_pointer()
            .hover(|button| button.bg(theme.selection))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.project_menu = Some(event.position);
                    cx.notify();
                }),
            )
            .child(div().flex_1().truncate().child(self.scope_name()))
            .child(div().text_color(theme.secondary).child("▾"));
        let project_sections = Section::PROJECT.map(&item);

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
                    .child(heading("App"))
                    .children(sections)
                    .child(heading("Project").mt(px(14.)))
                    .child(picker)
                    .children(project_sections)
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
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(self.section.title()),
                            )
                            .when(Section::PROJECT.contains(&self.section), |title| {
                                title.child(
                                    div().text_color(theme.secondary).child(self.scope_name()),
                                )
                            }),
                    )
                    .child(content),
            )
            .children(self.font_menu(&theme, cx))
            .children(self.project_menu(&theme, cx))
    }
}

/// What a prompt template may hold, said under the prompts.
const TEMPLATE_NOTE: &str = "Above a line of ---, the model's instructions; below it, what \
it reads. Without the line, all of it is what it reads. {{diff}} the changes, {{files}} the files \
changed, {{commits}} the commits summarized, {{examples 3}} the latest 3 commit subjects. The \
model reads about 6000 characters at most: put {{diff}} last.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_saving_and_loading() {
        let settings = Settings {
            terminal_hidden: Some(" git log\\n, not C:\\dir\nTwo".to_string()),
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
    fn settings_projects_keep_now_leave_the_file() {
        let text =
            "idle_days=14\nsummary_prompt=One\\ntwo\ntext_only=*.lock\\n*.snap\nidle_days=30\n";
        assert_eq!(
            moved_settings(text),
            [
                ("wtf.summaryTemplate", vec!["One\ntwo".to_string()]),
                (
                    "wtf.textDiff",
                    vec!["*.lock".to_string(), "*.snap".to_string()]
                ),
                ("wtf.idleDays", vec!["30".to_string()]),
            ]
        );
    }

    #[test]
    fn pull_options_go_by_git_config_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_string_lossy().into_owned();
        std::process::Command::new("git")
            .args(["init", "-q", &repo])
            .status()
            .unwrap();
        let config = wtf_core::ConfigScope::Repo(&repo);
        assert_eq!(pull_option(config), None);
        for option in 0..4 {
            set_pull_option(config, Some(option)).unwrap();
            assert_eq!(pull_option(config), Some(option));
        }
        set_pull_option(config, None).unwrap();
        assert_eq!(pull_option(config), None);
        assert!(wtf_core::project_values(config, "rebase.autoStash").is_empty());
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
        let settings = parse("ui_size=0\ncode_size=NaN\n");
        assert_eq!(settings.ui_size, 12.);
        assert_eq!(settings.code_size, Settings::default().code_size);
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
            Knob::RowHeight,
            Knob::Column,
            Knob::Curve,
            Knob::Dot,
            Knob::BeadSpacing,
            Knob::Beads,
            Knob::Line,
            Knob::HoverLine,
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
