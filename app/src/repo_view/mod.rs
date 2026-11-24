use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    Animation, AnimationExt, AnyElement, ClickEvent, ClipboardItem, Context, CursorStyle, Entity,
    FocusHandle, HighlightStyle, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, PathPromptOptions, Pixels, Point, PromptLevel, ScrollHandle, ScrollStrategy,
    SharedString, Stateful, StyledText, Task, UniformListScrollHandle, Window, actions, anchored,
    canvas, deferred, div, point, prelude::*, pulsating_between, px, rems, uniform_list,
};
use wtf_core::{
    Attention, ChangedFile, Commit, CoreError, DiffMode, FileDiff, Revision, Scope, WorkflowGraph,
};

use crate::console_view;
use crate::diff_view;
use crate::goto;
use crate::graph_view::{self, WorktreeNames, WorktreeOrder};
use crate::menus::{
    self, AllAuthors, MenuState, ResetBranches, SetScope, SetTimeWindow, SetWorktreeNames,
    SetWorktreeOrder, ToggleAttentionPanel, ToggleAuthor, ToggleBranch, ToggleRemotes, ToggleTags,
};
use crate::palette;
use crate::popups::{self, Command, Target};
use crate::preferences;
use crate::recent;
use crate::syntax;
use crate::terminal_view::TerminalView;
use crate::theme::Theme;
use crate::workspace::{self, OpenPath};

mod attention_panel;
mod commit_box;
mod graph_band;
mod header;
mod lists;
mod overlays;
mod search;
mod stack;
mod staging;
mod time;
mod trees;
mod widgets;

use attention_panel::*;
use header::*;
use time::*;
use trees::*;
use widgets::*;

pub use time::{now_seconds, short_age};
pub use widgets::chevron;

const CONTEXT_ROWS: usize = 3;
const REFRESH: Duration = Duration::from_secs(2);
/// How often the tab in front refreshes while its window is not: often
/// enough to follow agents at work from another screen.
const BACKGROUND_REFRESH: Duration = Duration::from_secs(10);
/// A refresh waits at least this many times as long as the last one took,
/// so a large repository never keeps git busy all the time.
const REFRESH_SHARE: u32 = 3;
/// How long a stash asked for in the terminal is waited for.
const STASH_WAIT: Duration = Duration::from_secs(120);
/// The most of the window's height the graph takes by itself; dragged, it
/// takes what it is given.
const MAX_GRAPH_SHARE: f32 = 0.25;
/// Room between the graph and its band's edges as it opens; its margin
/// beyond them is there to scroll into.
const GRAPH_EDGE: f32 = 8.;
const DOCK_HEIGHT: f32 = 260.;
const MIN_PANEL_HEIGHT: f32 = 60.;
/// Room left for the commit, file and diff panes when panels grow.
const MIN_MAIN_HEIGHT: f32 = 200.;
const MIN_COLUMN_WIDTH: f32 = 120.;
const MIN_DIFF_WIDTH: f32 = 300.;
/// How many more commits each scroll to the graph's left edge brings in.
const HISTORY_STEP: usize = 100;
/// The commit message's line height, in rems.
const MESSAGE_LINE: f32 = 1.25;
/// How long staging stays put before the model writes a commit message.
const SUGGEST_DELAY: Duration = Duration::from_millis(600);
/// How far to scroll on past the graph's left edge to bring them.
const PULL_DISTANCE: f32 = 160.;
/// The Go To picker's width and how many results it lists.
const GOTO_WIDTH: f32 = 600.;
const GOTO_ROWS: usize = 14;
const PROMPT_WIDTH: f32 = 420.;
const ATTENTION_PANEL_WIDTH: f32 = 240.;
const MIN_SIDEBAR_WIDTH: f32 = 180.;
const MAX_SIDEBAR_WIDTH: f32 = 500.;
/// The closed sidebar: a strip holding its toggle.
/// Room for the strip's boxes and a margin on either side of them.
const SIDEBAR_STRIP_WIDTH: f32 = 34.;
/// The all-files view falls back to one file at a time above this; a
/// worktree's staging view then shows its files one by one.
const MAX_STACKED_FILES: usize = 100;
/// The commit header's padding and line height, in rems; the summary bar
/// repeats its first line just as it is drawn there.
const HEADER_PADDING: f32 = 8.;
const HEADER_LINE: f32 = 1.25;

actions!(
    wtf,
    [
        OpenRepository,
        StructuralDiff,
        TextDiff,
        ToggleCombineCommits,
        ToggleConsole,
        ToggleRefreshCommands,
        ToggleSidePanel,
        GoTo,
        CommandPalette,
        Prune,
        Fetch,
        StashChanges,
        OpenWorktreeWith,
        ToggleSnapshot,
        Minimize,
        Zoom,
    ]
);

/// What a file's history was read for: folder, line tip, state and file.
type HistoryKey = (String, String, String, String);

/// The repository as it is after the selection, shown in Snapshot mode:
/// every file, and the one open.
#[derive(Default)]
struct Snapshot {
    /// The state listed, its files, and a state being listed.
    revision: Option<Revision>,
    files: Vec<String>,
    /// Which of them are submodules.
    submodules: HashSet<String>,
    loading: Option<Revision>,
    /// The file open, and its text as last read: at which state, and
    /// whether it is there.
    path: Option<String>,
    text: Option<(Revision, String, Result<String, String>)>,
    text_loading: Option<(Revision, String)>,
    /// A worktree's files changed on disk: read them again.
    stale: bool,
    /// What the search field holds; the lines of text it found, for which
    /// query and state, and the query and state searched now.
    search: crate::text_field::Field,
    found: Option<(String, Revision, Vec<wtf_core::Found>)>,
    searching: Option<(String, Revision)>,
    /// A line found and opened: shown marked, scrolled to once read.
    found_line: Option<(String, u32)>,
    only_changed: bool,
    /// Folders open in the tree: by a click, or on the way to a file opened.
    /// Only a click closes one.
    open_folders: HashSet<String>,
    /// The commits that changed the open file along the selected line, newest
    /// first, for what they were read (folder, line tip, state, file), and
    /// which of them the shown state stands at.
    history: Option<(HistoryKey, Vec<String>, Option<usize>)>,
    history_loading: Option<HistoryKey>,
    /// A commit stepped to that the graph does not reach yet.
    pending: Option<String>,
    /// The commits the header last showed, kept while the next ones load.
    header: RefCell<Vec<Commit>>,
    /// The change marks and tints last shown, kept while the next ones load.
    marks: RefCell<HashMap<String, String>>,
    tints: RefCell<Option<(String, Rc<HashSet<u32>>)>>,
    /// Code colors of the open text, for the appearance they were made for.
    colors: RefCell<Option<(bool, Rc<syntax::Colors>)>>,
}

/// A row of the file list.
#[derive(Debug, PartialEq)]
enum TreeRow {
    /// Unstaged or staged files, with how many.
    Section(bool, usize),
    /// A folder; chains of folders holding only one folder show as one.
    Dir {
        key: String,
        label: String,
        depth: usize,
    },
    File {
        index: usize,
        name: String,
        depth: usize,
    },
}

const FILE_ROW_HEIGHT: f32 = 22.;

/// The commit box of a worktree: its message, and the model's part in it.
#[derive(Default)]
struct CommitDraft {
    /// The worktree the draft belongs to.
    worktree: Option<String>,
    message: String,
    /// Where the message's cursor is, as a byte offset; `None` at its end.
    message_cursor: Option<usize>,
    /// The message as last laid out, to place the cursor by a click and
    /// move it between lines.
    message_layout: RefCell<Option<gpui::TextLayout>>,
    /// The other end of the message's selection, when there is one.
    message_anchor: Option<usize>,
    /// A press in the message is dragging out a selection.
    message_dragging: bool,
    /// The next message the model writes is a new wording, asked for with
    /// its mark.
    rewrite: bool,
    /// Earlier and undone messages, for ⌘Z and ⇧⌘Z, and whether the last
    /// change was a typed letter, which the next one joins.
    message_undo: Vec<String>,
    message_redo: Vec<String>,
    typing_run: bool,
    /// The commit rewrites the worktree's latest commit instead.
    amend: bool,
    /// The model is writing a message for an empty box, and whether a
    /// commit waits for it.
    writing_message: bool,
    commit_when_written: bool,
    /// The message the model last put in the box, which it may replace as
    /// staging goes on until the user edits it; what was staged then; and
    /// the wait before writing the next one.
    suggested: Option<String>,
    suggest_key: Option<String>,
    suggest_task: Option<Task<()>>,
}

/// A row of the all-files view.
enum StackItem {
    /// Staged or unstaged files, with how many.
    Section(bool, usize),
    Header(usize),
    /// A file's diff row, and which of its lines when wrapped.
    Row(usize, usize, usize),
    /// A hunk of a worktree file: its first row and the row after its last.
    Hunk(usize, usize, usize),
    Gap,
    Note(String),
    /// The row a section's or file's header reaches up into: headers are
    /// two rows tall.
    HeaderPad,
}

/// A panel border being dragged.
#[derive(Clone, Copy)]
enum Border {
    Graph,
    Dock,
    Sidebar,
    Commits,
    /// The commit header over a snapshot's file.
    SnapshotHeader,
}

/// The panels the strip at the right edge opens, one at a time.
#[derive(Clone, Copy, PartialEq)]
enum SidePanel {
    Attention,
    /// The changed files as a tree: a table of contents for the changes, or
    /// the file a snapshot shows.
    Files,
}

/// A row of a branch tree: a folder of branch names, or a branch.
#[derive(Debug, PartialEq)]
enum NameRow {
    /// `path` ends with `/`; chains of folders holding only one folder show
    /// as one row.
    Folder {
        path: String,
        label: String,
        depth: usize,
    },
    Leaf {
        index: usize,
        name: String,
        depth: usize,
    },
}

/// A group of branches in the attention sidebar.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Section {
    Worktrees,
    Local,
    Remote,
    Tags,
    Stashes,
    Channels,
}

/// A dropdown of the attention sidebar.
#[derive(Clone, Copy)]
enum Dropdown {
    Window,
    Authors,
}

/// A small panel asking for one line of text.
struct Prompt {
    kind: PromptKind,
    field: crate::text_field::Field,
}

enum PromptKind {
    /// The new name of a branch.
    Rename(String),
    /// A command to open a worktree's path with.
    OpenWith(String),
    /// Where to put a worktree for an existing branch.
    WorktreeFor(String),
    /// The name of a new branch at a commit, with a worktree when asked.
    NewBranch { commit: String, worktree: bool },
    /// The branch of a new worktree for a stash, at the commit it was made on.
    StashWorktree { stash: String, base: String },
    /// A glob pattern naming channel branches.
    AddChannel,
}

/// Code colors are cached by path, appearance and all-files entry.
type ColorKey = (String, bool, Option<usize>);

/// What a graph's layout depends on besides the graph: the worktree order,
/// the graph's look, the settings that change label widths, and the hour,
/// as branches' idle days grow.
type LayoutKey = (WorktreeOrder, preferences::GraphStyle, u32, bool, i64);

pub struct RepoView {
    /// Keeps menu actions reaching the view when nothing else has focus.
    focus: FocusHandle,
    /// The tab shown in its window.
    active: bool,
    repo_path: Option<String>,
    graph: WorkflowGraph,
    graph_error: Option<String>,
    /// Linear runs of commits share a node.
    combine_commits: bool,
    attention: Attention,
    graph_scroll: ScrollHandle,
    /// What the menu bar was last built from.
    menus_for: Option<MenuState>,
    worktree_names: WorktreeNames,
    worktree_order: WorktreeOrder,
    context_menu: Option<(Target, Point<Pixels>)>,
    prompt: Option<Prompt>,
    prompt_focus: FocusHandle,
    /// The prompt's caret shows; it blinks while the prompt is open.
    caret_on: bool,
    blink_task: Option<Task<()>>,
    /// The prune list, when open: stale branches by name, and whether each is
    /// ticked to go.
    prune: Option<Vec<overlays::PruneItem>>,
    prune_focus: FocusHandle,
    /// The Go To picker or the command palette, when open: the query, the
    /// highlighted result, and whether it lists commands.
    goto: Option<(crate::text_field::Field, usize, bool)>,
    /// The Stash picker, open on a worktree: its path and the highlighted
    /// row, the first one running the stash.
    stash_picker: Option<(String, usize)>,
    /// The flags the Stash picker has on, by their place in its list.
    stash_flags: [bool; popups::STASH_FLAGS.len()],
    stash_focus: FocusHandle,
    goto_focus: FocusHandle,
    /// Snapshot mode: the review shows whole files as they are after the
    /// selection, the Files panel the whole tree.
    snapshot_mode: bool,
    snapshot: Snapshot,
    /// Reading the files, the open file, and its history.
    snapshot_tasks: [Option<Task<()>>; 4],
    snapshot_scroll: UniformListScrollHandle,
    snapshot_search_focus: FocusHandle,
    /// The last commands used to open a worktree, newest first.
    open_with: Vec<String>,
    /// Shell commands run from the palette, newest first.
    shell_commands: Vec<String>,
    /// File names a Tab in the palette left to choose from.
    completions: Vec<String>,
    /// The open side panel, and the one ⌘R opens.
    side_panel: Option<SidePanel>,
    last_panel: SidePanel,
    sidebar_width: f32,
    attention_search: crate::text_field::Field,
    attention_search_focus: FocusHandle,
    collapsed_sections: HashSet<Section>,
    dropdown: Option<(Dropdown, Point<Pixels>)>,
    /// The graph's width when last drawn, to keep the view still as it changes.
    graph_width: Cell<f32>,
    /// How wide the graph's view was when last drawn.
    graph_view_width: Cell<f32>,
    /// The graph's layout as last made, and what it was made from besides
    /// the graph, which drops it when it changes.
    graph_layout: RefCell<Option<(LayoutKey, Rc<graph_view::Layout>)>>,
    /// What shows is a branch the graph cannot draw, so a new graph keeps it.
    outside_graph: bool,
    /// The stash shown, read-only, in the staging view's sections.
    stash_view: Option<wtf_core::Stash>,
    /// A stash to scroll the graph to once a rebuild shows its marker.
    reveal_stash: bool,
    /// The staging view's line under the mouse, which shows its buttons.
    hovered_line: Cell<Option<(usize, usize)>>,
    /// Code colors of the diffs shown, by file, appearance and view.
    syntax: RefCell<HashMap<ColorKey, Rc<syntax::Colors>>>,
    /// A drag of the graph: where the mouse went down and the scroll then.
    graph_drag: Option<(Point<Pixels>, Point<Pixels>)>,
    /// Set when a repository opens, until the graph is first laid out.
    scroll_to_end: Cell<bool>,
    /// The graph is scrolled away from its oldest and newest work, or has
    /// older commits to load, as last drawn.
    graph_away: Cell<(bool, bool)>,
    /// Where the button back to the worktree in review points, when shown.
    worktree_arrow: Cell<Option<&'static str>>,
    /// Once the whole history is in, the graph scrolls to its first commit.
    scroll_to_start: Cell<bool>,
    /// How many commits before the graph's oldest node to bring in, grown
    /// by scrolling to its left edge; and whether more are on the way.
    history: usize,
    loading_older: bool,
    /// How far the graph has been pulled past its left edge, while a pull
    /// is on, and whether the last scroll left it at that edge.
    pull: Cell<Option<f32>>,
    at_left_edge: bool,
    /// A node selected from outside the graph, to scroll into view.
    reveal: Cell<Option<usize>>,

    selected_nodes: Vec<usize>,
    hovered_node: Option<usize>,
    /// The worktree whose edge chevron the pointer is over, and whether on
    /// the left.
    edge_hovered: Option<(usize, bool)>,
    /// The diff of the selected nodes, `None` when they are not on one line.
    range: Option<(Option<String>, Revision)>,
    commits: Vec<Commit>,
    /// Commits the diff is narrowed to; empty for the whole selection.
    narrowed: Vec<usize>,
    /// Where Shift-click ranges in the commit list start.
    commit_anchor: Option<usize>,
    files: Vec<ChangedFile>,
    diff: Option<FileDiff>,
    selected_file: Option<usize>,
    diff_mode: DiffMode,
    /// Long diff lines wrap; otherwise they run on and scroll sideways.
    wrap_lines: bool,
    /// How far the diff's code is scrolled sideways, and how far it can go.
    diff_x: Cell<f32>,
    diff_max_x: Cell<f32>,
    /// The diff's width when last drawn, and the characters a side then
    /// holds, which the wrapped all-files view is laid out at.
    diff_width: Cell<(f32, f32)>,
    wrap_cols: Option<usize>,
    stacked: Vec<StackItem>,
    stacked_diffs: Vec<Result<FileDiff, String>>,
    /// The files `stacked_diffs` were made from: until a refresh brings
    /// new diffs, `files` may hold others at the same places.
    stacked_files: Vec<ChangedFile>,
    /// For a selected worktree, where staged files start in `files`.
    worktree_split: Option<usize>,
    /// Submodules of the worktree in review changed only inside.
    submodules_inside: Vec<String>,
    /// Diff rows picked for staging, all in one file.
    line_selection: Option<(usize, Vec<usize>)>,
    line_anchor: Option<usize>,
    /// The commit box: what is typed in it and the model's part in it.
    draft: CommitDraft,
    /// The drafts of worktrees not selected, kept for their return.
    drafts: HashMap<String, CommitDraft>,
    commit_focus: FocusHandle,
    /// A commit picked in the graph, to select once its node's commits load.
    pending_commit: Option<String>,
    commit_list_state: gpui::ListState,
    /// The rows the commit list was last drawn with.
    commit_rows: RefCell<Vec<lists::LineRow>>,
    /// The selection the commit list last scrolled to.
    commit_list_selection: RefCell<Vec<usize>>,
    /// A listed commit to scroll to once the list is drawn.
    scroll_to_commit: Cell<Option<usize>>,
    /// The key of the node the commit list's line starts at.
    thread: Option<String>,
    /// Bars of the commit list folded or opened by hand, by node key.
    folds: HashMap<String, bool>,
    /// The bars whose summaries are wanted, top down, and whether one is
    /// being made: they are made one at a time.
    summary_wanted: RefCell<Vec<usize>>,
    bar_summary_running: bool,
    message_scroll: ScrollHandle,
    /// A commit message being edited: the commit, the worktree whose latest
    /// commit it is, and the text so far.
    rewording: Option<(String, String, crate::text_field::Field)>,
    /// Text picked in a commit's message: the commit, where the pick started
    /// and where it ends, and whether a drag is extending it.
    message_selection: Option<(String, usize, usize)>,
    /// Summaries of selections of several commits, by their commits: `None`
    /// while the model writes one.
    summaries: HashMap<String, Option<Result<wtf_core::Summary, String>>>,
    /// The commits' own subjects show under a summary.
    show_subjects: bool,
    selecting_message: bool,
    message_focus: FocusHandle,
    /// The commit whose hash was just copied, shown so for a moment.
    copied: Option<String>,
    reword_focus: FocusHandle,
    /// Paths of files folded to their headers in the all-files view.
    folded: HashSet<String>,
    /// With more files than [`MAX_STACKED_FILES`] they all start folded:
    /// these are the ones opened since.
    unfolded: HashSet<String>,
    /// The unstaged and staged sections, folded to their titles.
    folded_sections: [bool; 2],
    /// Folders folded in the file list, by section and path.
    collapsed_dirs: HashSet<String>,
    /// Folders folded in the sidebar's branch tree, by `local:` or `remote:`
    /// and path.
    collapsed_branch_dirs: HashSet<String>,
    is_loading_stacked: bool,
    stacked_scroll: UniformListScrollHandle,
    /// The commit header's height, as last laid out; the diff starts below it.
    header_height: f32,
    is_loading_diff: bool,
    error_message: Option<String>,

    console_open: bool,
    /// The window is the one in front; with `active`, how often to refresh.
    window_active: bool,
    /// The selection saved when the repository was last open, by node keys,
    /// for its first graph to show again.
    restore_keys: Vec<String>,
    /// The worktree work goes to: the last one selected, the tab's own until
    /// one is. Pull, Push, the palette and Stash act on it; selecting commits
    /// to look at them leaves it.
    current_worktree: Option<String>,
    /// A stash asked for in the terminal: the newest stash before it, and
    /// when it was asked for.
    awaiting_stash: Option<(Option<String>, std::time::Instant)>,
    hide_refresh: bool,
    log_seq: u64,
    /// The directory of the last command printed in the terminal.
    announced_dir: Option<String>,
    dock_height: f32,
    terminal: Option<Entity<TerminalView>>,
    /// Set by dragging the graph's border; otherwise the graph fits its rows.
    graph_height: Option<f32>,
    /// The commit header's height over a snapshot's file, as dragged; and
    /// where a drag of its edge began, with the height then.
    snapshot_header_height: Option<f32>,
    header_drag: Option<(f32, f32)>,
    dragging: Option<Border>,
    commits_width: f32,

    // Replacing a task drops, and so cancels, the previous one.
    refresh_task: Option<Task<()>>,
    /// A fetch of the remotes under way.
    fetch_task: Option<Task<()>>,
    /// A pull or push under way: whether it is a push, and its task.
    sync_task: Option<(bool, Task<()>)>,
    /// The remote picked in the Push menu; `None` for where the branch
    /// tracks.
    push_remote: Option<String>,
    /// When the remotes were last fetched.
    fetched_at: Option<i64>,
    commits_task: Option<Task<()>>,
    files_task: Option<Task<()>>,
    diff_task: Option<Task<()>>,
    stacked_task: Option<Task<()>>,
    diff_scroll: UniformListScrollHandle,
}

fn message(error: CoreError) -> String {
    let CoreError::Failed { message } = error;
    message
}

impl RepoView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        let focus = cx.focus_handle();
        focus.focus(window);
        // Panels open at the sizes they were last dragged to.
        let sizes = crate::panels::load(crate::panels::Sizes {
            sidebar: ATTENTION_PANEL_WIDTH,
            dock: DOCK_HEIGHT,
            graph: None,
            commits: 300.,
            snapshot_header: None,
        });
        Self {
            focus,
            active: false,
            repo_path: None,
            graph: WorkflowGraph::default(),
            graph_error: None,
            combine_commits: true,
            attention: Attention::default(),
            graph_scroll: ScrollHandle::new(),
            menus_for: None,
            worktree_names: WorktreeNames::FolderAndBranch,
            worktree_order: WorktreeOrder::ForkPoint,
            context_menu: None,
            prompt: None,
            prompt_focus: cx.focus_handle(),
            caret_on: true,
            blink_task: None,
            goto: None,
            stash_picker: None,
            stash_flags: [true, false, false],
            stash_focus: cx.focus_handle(),
            goto_focus: cx.focus_handle(),
            snapshot_mode: false,
            snapshot: Snapshot::default(),
            snapshot_tasks: [None, None, None, None],
            snapshot_scroll: UniformListScrollHandle::new(),
            snapshot_search_focus: cx.focus_handle(),
            prune: None,
            prune_focus: cx.focus_handle(),
            open_with: popups::load_recent("open-with"),
            shell_commands: popups::load_recent("commands"),
            completions: Vec::new(),
            side_panel: None,
            last_panel: SidePanel::Attention,
            sidebar_width: sizes.sidebar,
            attention_search: Default::default(),
            attention_search_focus: cx.focus_handle(),
            collapsed_sections: HashSet::new(),
            dropdown: None,
            graph_width: Cell::new(0.),
            graph_view_width: Cell::new(0.),
            graph_layout: RefCell::new(None),
            outside_graph: false,
            stash_view: None,
            reveal_stash: false,
            hovered_line: Cell::new(None),
            syntax: RefCell::new(HashMap::new()),
            graph_drag: None,
            scroll_to_end: Cell::new(true),
            graph_away: Cell::new((false, false)),
            worktree_arrow: Cell::new(None),
            scroll_to_start: Cell::new(false),
            history: 0,
            loading_older: false,
            pull: Cell::new(None),
            at_left_edge: false,
            reveal: Cell::new(None),
            selected_nodes: Vec::new(),
            hovered_node: None,
            edge_hovered: None,
            range: None,
            commits: Vec::new(),
            narrowed: Vec::new(),
            commit_anchor: None,
            files: Vec::new(),
            diff: None,
            selected_file: None,
            diff_mode: DiffMode::Text,
            wrap_lines: false,
            diff_x: Cell::new(0.),
            diff_max_x: Cell::new(0.),
            diff_width: Cell::new((-1., -1.)),
            wrap_cols: None,
            stacked: Vec::new(),
            stacked_diffs: Vec::new(),
            stacked_files: Vec::new(),
            worktree_split: None,
            submodules_inside: Vec::new(),
            line_selection: None,
            line_anchor: None,
            draft: CommitDraft::default(),
            drafts: HashMap::new(),
            commit_focus: cx.focus_handle(),
            pending_commit: None,
            commit_list_state: gpui::ListState::new(0, gpui::ListAlignment::Top, px(400.)),
            commit_rows: RefCell::new(Vec::new()),
            commit_list_selection: RefCell::new(Vec::new()),
            scroll_to_commit: Cell::new(None),
            thread: None,
            folds: HashMap::new(),
            summary_wanted: RefCell::new(Vec::new()),
            bar_summary_running: false,
            message_scroll: ScrollHandle::new(),
            rewording: None,
            message_selection: None,
            summaries: HashMap::new(),
            show_subjects: false,
            selecting_message: false,
            message_focus: cx.focus_handle(),
            copied: None,
            reword_focus: cx.focus_handle(),
            folded: HashSet::new(),
            unfolded: HashSet::new(),
            folded_sections: [false; 2],
            collapsed_dirs: HashSet::new(),
            collapsed_branch_dirs: HashSet::new(),
            is_loading_stacked: false,
            stacked_scroll: UniformListScrollHandle::new(),
            header_height: 0.,
            is_loading_diff: false,
            error_message: None,
            console_open: false,
            hide_refresh: true,
            window_active: true,
            restore_keys: Vec::new(),
            current_worktree: None,
            awaiting_stash: None,
            log_seq: 0,
            announced_dir: None,
            dock_height: sizes.dock,
            terminal: None,
            graph_height: sizes.graph,
            snapshot_header_height: sizes.snapshot_header,
            header_drag: None,
            dragging: None,
            commits_width: sizes.commits,
            refresh_task: None,
            fetch_task: None,
            sync_task: None,
            push_remote: None,
            fetched_at: None,
            commits_task: None,
            files_task: None,
            diff_task: None,
            stacked_task: None,
            diff_scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn repo_path(&self) -> Option<&str> {
        self.repo_path.as_deref()
    }

    /// DEBUG: the worktree commands run in, and why: the one selected, or
    /// the tab's own. Delete with its one use, `debug_worktree_label` in the
    /// workspace.
    pub fn debug_worktree(&self) -> Option<String> {
        let path = self.command_path()?;
        let why = match self.current_worktree_path().is_some() {
            true => "chosen",
            false => "tab",
        };
        Some(format!("{} · {why}", workspace::folder_name(&path)))
    }

    /// Notes that a stash is on its way, made in the terminal: the next graph
    /// with a newer stash shows it, as picking it in the attention panel does.
    pub(super) fn expect_stash(&mut self) {
        let newest = self.graph.stashes.first().map(|stash| stash.id.clone());
        self.awaiting_stash = Some((newest, std::time::Instant::now()));
    }

    /// The current worktree's path, when one was chosen and the graph still
    /// has it; `None` for the tab's own.
    pub(super) fn current_worktree_path(&self) -> Option<&str> {
        let path = self.current_worktree.as_deref()?;
        let tab = self.repo_path.as_deref().map(|p| p.trim_end_matches('/'));
        let alive = self.graph.nodes.iter().any(|node| {
            matches!(&node.tip, Revision::Worktree(p) if p.trim_end_matches('/') == path.trim_end_matches('/'))
        });
        (alive && tab != Some(path.trim_end_matches('/'))).then_some(path)
    }

    /// The tab's title: its repository.
    pub fn tab_title(&self) -> Option<String> {
        self.repo_name()
    }

    pub fn repo_name(&self) -> Option<String> {
        self.repo_path.as_deref().map(workspace::folder_name)
    }

    /// The workspace shows one tab at a time; the active one takes focus and
    /// the menu bar.
    /// Shows or hides the tab. Only the tab shown refreshes, so one coming
    /// to the front catches up at once.
    pub fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        let was_refreshing = self.active && self.window_active;
        self.active = active;
        self.window_active = window.is_window_active();
        if active {
            self.focus.focus(window);
            self.menus_for = None;
            self.update_menus(cx);
            if !was_refreshing {
                self.start_refresh(cx);
            }
        }
    }

    /// The window went to the back: the tab refreshes less often.
    pub fn set_window_inactive(&mut self) {
        self.window_active = false;
    }

    /// How long to wait for the next refresh; `None` stops refreshing, for a
    /// tab not shown.
    fn refresh_interval(&self) -> Option<Duration> {
        match (self.active, self.window_active) {
            (false, _) => None,
            (true, true) => Some(REFRESH),
            (true, false) => Some(BACKGROUND_REFRESH),
        }
    }

    pub fn open(&mut self, path: String, cx: &mut Context<Self>) {
        self.repo_path = Some(path.clone());
        self.graph = WorkflowGraph::default();
        self.graph_layout.take();
        self.scroll_to_end.set(true);
        self.history = 0;
        self.graph_error = None;
        // A shell for this repository from the start, so the terminal shows
        // every command the window runs; older commands are skipped.
        self.log_seq = wtf_core::log_since(self.log_seq)
            .last()
            .map_or(self.log_seq, |entry| entry.seq);
        self.announced_dir = None;
        let dir = path.clone();
        self.terminal = Some(cx.new(|cx| TerminalView::new(dir, cx)));
        self.selected_nodes.clear();
        self.range = None;
        self.narrowed.clear();
        self.commit_anchor = None;
        self.commits.clear();
        self.files.clear();
        self.select_file(None, cx);
        self.attention = Attention::load(&path);
        self.restore_keys = saved_selection(&path);
        self.current_worktree = None;
        recent::remember(&path);
        self.update_menus(cx);
        self.start_refresh(cx);
        cx.notify();
    }

    /// Rebuilds the graph now and then every few seconds while the tab is
    /// shown; unchanged graphs are ignored, and nothing is redrawn for them.
    fn start_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.repo_path.clone() else {
            return;
        };
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let settings = this.update(cx, |this, _| {
                    (this.attention.clone(), this.combine_commits, this.history)
                });
                let Ok((attention, combine, history)) = settings else {
                    break;
                };
                let path = path.clone();
                let started = std::time::Instant::now();
                let result =
                    cx.background_executor()
                        .spawn(async move {
                            wtf_core::workflow_graph(&path, &attention, combine, history)
                        })
                        .await;
                let took = started.elapsed();
                let interval = this.update(cx, |this, cx| {
                    this.apply_graph(result, cx);
                    this.fetch_when_due(cx);
                    // The terminal prints the refresh's commands itself; the
                    // view is not redrawn for them.
                    this.sync_log(cx);
                    this.refresh_interval()
                });
                let Ok(Some(interval)) = interval else {
                    break;
                };
                cx.background_executor()
                    .timer(interval.max(took * REFRESH_SHARE))
                    .await;
            }
        }));
    }

    /// Fetches in the background as often as Settings ask, first when the
    /// repository opens.
    fn fetch_when_due(&mut self, cx: &mut Context<Self>) {
        let minutes = preferences::fetch_minutes();
        if minutes == 0 || self.fetch_task.is_some() {
            return;
        }
        let due = self
            .fetched_at
            .is_none_or(|at| now_seconds() - at >= i64::from(minutes) * 60);
        if due {
            self.fetch(true, cx);
        }
    }

    /// Fetches every remote, then rebuilds the graph to show what came.
    fn fetch(&mut self, background: bool, cx: &mut Context<Self>) {
        self.fetch_with(background, preferences::fetch_prune(), cx);
    }

    fn fetch_with(&mut self, background: bool, prune: bool, cx: &mut Context<Self>) {
        // A fetch under way already brings what another would.
        let Some(path) = self.repo_path.clone().filter(|_| self.fetch_task.is_none()) else {
            return;
        };
        let load = cx
            .background_executor()
            .spawn(async move { wtf_core::fetch(&path, prune, background) });
        self.fetch_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                this.fetch_task = None;
                this.fetched_at = Some(now_seconds());
                // A background fetch that failed is only logged, in red in
                // the terminal: it is tried again soon. One the user asked
                // for says why.
                if let (false, Err(error)) = (background, result) {
                    this.error_message = Some(message(error));
                }
                this.start_refresh(cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Fetches now, as the background fetches do: the graph then marks
    /// branches whose remote is gone. Nothing is deleted.
    pub fn fetch_now(&mut self, cx: &mut Context<Self>) {
        self.fetch(false, cx);
    }

    pub fn fetching(&self) -> bool {
        self.fetch_task.is_some()
    }

    /// When the remotes were last fetched, for the Fetch button's tooltip.
    pub fn fetched_at(&self) -> Option<i64> {
        self.fetched_at
    }

    /// Runs a Pull or Push option in the worktree commands run in, then
    /// rebuilds the graph to show what moved.
    pub fn sync(&mut self, push: bool, option: usize, cx: &mut Context<Self>) {
        let options = crate::sync::options(push);
        let (Some(path), Some(option)) = (self.command_path(), options.get(option)) else {
            return;
        };
        if self.sync_task.is_some() {
            return;
        }
        let args = option.args;
        let to = self.push_remote.clone();
        let run = cx.background_executor().spawn(async move {
            match push {
                true => wtf_core::push(&path, to.as_deref(), args),
                false => wtf_core::pull(&path, args),
            }
        });
        let task = cx.spawn(async move |this, cx| {
            let result = run.await;
            this.update(cx, |this, cx| {
                this.sync_task = None;
                // A pull fetches its remote on the way: the next background
                // fetch waits from now.
                if !push {
                    this.fetched_at = Some(now_seconds());
                }
                // The user asked for it, so a failure shows in the review
                // area, as Git said it.
                if let Err(error) = result {
                    this.error_message = Some(message(error));
                }
                this.start_refresh(cx);
                cx.notify();
            })
            .ok();
        });
        self.sync_task = Some((push, task));
        cx.notify();
    }

    /// The repository's remotes, to push to.
    pub fn remotes(&self) -> &[String] {
        &self.graph.remotes
    }

    /// Where a push goes: the remote picked in the menu, else the one the
    /// branch tracks, else `origin`.
    pub fn push_remote(&self) -> String {
        if let Some(remote) = &self.push_remote {
            return remote.clone();
        }
        self.sync_branch()
            .and_then(|branch| branch.upstream.as_deref())
            .and_then(|upstream| upstream.split_once('/'))
            .map(|(remote, _)| remote.to_string())
            .unwrap_or_else(|| "origin".to_string())
    }

    pub fn set_push_remote(&mut self, remote: String, cx: &mut Context<Self>) {
        self.push_remote = Some(remote);
        cx.notify();
    }

    /// A pull (`false`) or push (`true`) under way.
    pub fn syncing(&self) -> Option<bool> {
        self.sync_task.as_ref().map(|(push, _)| *push)
    }

    /// The branch checked out where commands run.
    fn sync_branch(&self) -> Option<&wtf_core::Branch> {
        let path = self.command_path();
        self.graph
            .nodes
            .iter()
            .find(|node| matches!(&node.tip, Revision::Worktree(p) if Some(p) == path.as_ref()))
            .and_then(|node| node.branch.as_ref())
            .and_then(|name| {
                self.graph
                    .branches
                    .iter()
                    .find(|b| &b.name == name && !b.remote)
            })
    }

    /// How far the branch commands run on is ahead of and behind its upstream.
    pub fn sync_counts(&self) -> (u32, u32) {
        self.sync_branch()
            .map_or((0, 0), |branch| (branch.ahead, branch.behind))
    }

    /// Rebuilds the menu bar when the attention or the branches change.
    /// The menu bar is shared by all tabs and windows, so only the active tab
    /// builds it.
    fn update_menus(&mut self, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        let state = MenuState {
            recent: recent::load(),
            attention: self.attention.clone(),
            branches: self.graph.branches.clone(),
            combine: self.combine_commits,
            names: self.worktree_names,
            order: self.worktree_order,
            panel: self.side_panel == Some(SidePanel::Attention),
        };
        if self.menus_for.as_ref() != Some(&state) {
            cx.set_menus(menus::menus(&state));
            self.menus_for = Some(state);
        }
    }

    /// Adds a channel pattern to the repository's git config, then shows
    /// what it makes of the branches.
    pub(super) fn add_channel(&mut self, pattern: &str, cx: &mut Context<Self>) {
        self.change_channels(|repo| wtf_core::add_channel(repo, pattern), cx);
    }

    pub(super) fn remove_channel(&mut self, pattern: &str, cx: &mut Context<Self>) {
        self.change_channels(|repo| wtf_core::remove_channel(repo, pattern), cx);
    }

    fn change_channels(
        &mut self,
        change: impl FnOnce(&str) -> Result<(), CoreError>,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo_path.clone() else {
            return;
        };
        if let Err(error) = change(&repo) {
            self.error_message = Some(message(error));
        }
        self.start_refresh(cx);
        cx.notify();
    }

    /// The branches a channel pattern names, each once, without its remote:
    /// `v2410` for `origin/v2410` and a local `v2410` alike.
    pub(super) fn channel_matches(&self, pattern: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .graph
            .branches
            .iter()
            .map(|branch| match branch.remote {
                true => branch
                    .name
                    .split_once('/')
                    .map_or(&*branch.name, |(_, n)| n),
                false => &branch.name,
            })
            .filter(|name| wtf_core::glob(pattern, name))
            .map(str::to_string)
            .collect();
        names.sort();
        names.dedup();
        names
    }

    fn change_attention(&mut self, change: impl FnOnce(&mut Attention), cx: &mut Context<Self>) {
        change(&mut self.attention);
        // The sidebar's checkmarks follow at once; the graph catches up when the
        // rebuild finishes.
        let now = now_seconds();
        let main = self.graph.main.clone();
        for branch in &mut self.graph.branches {
            branch.shown = branch.name == main || self.attention.shows_branch(branch, now);
        }
        if let Some(path) = &self.repo_path
            && let Err(error) = self.attention.save(path)
        {
            self.error_message = Some(message(error));
        }
        self.update_menus(cx);
        self.start_refresh(cx);
        cx.notify();
    }

    pub fn hide_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        self.change_attention(
            |attention| {
                attention.pinned.retain(|pinned| pinned != name);
                if !attention.hidden.iter().any(|hidden| hidden == name) {
                    attention.hidden.push(name.to_string());
                }
            },
            cx,
        );
    }

    fn toggle_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        // The main branch is always shown; one with no history in common with
        // it cannot be.
        let unrelated = self
            .graph
            .branches
            .iter()
            .any(|branch| branch.name == name && branch.unrelated);
        if name == self.graph.main {
            return;
        }
        if unrelated {
            return;
        }
        let shown = self
            .graph
            .branches
            .iter()
            .any(|branch| branch.name == name && branch.shown);
        self.change_attention(
            |attention| {
                attention.pinned.retain(|pinned| pinned != name);
                attention.hidden.retain(|hidden| hidden != name);
                if shown {
                    attention.hidden.push(name.to_string());
                } else {
                    attention.pinned.push(name.to_string());
                }
            },
            cx,
        );
    }

    /// Like pulling a list down to refresh: scrolling stops at the graph's
    /// left edge, and only scrolling on past it, in a new swipe or with more
    /// wheel clicks, brings older commits in. A swipe's momentum that carries
    /// it to the edge does not.
    fn pull_older(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The First and Latest buttons come and go as the view leaves either
        // end, and the one back to the worktree as it leaves the view.
        let arrow = self
            .worktree_away(&self.graph_layout())
            .map(|(_, arrow)| arrow);
        if self.graph_away_now() != self.graph_away.get() || arrow != self.worktree_arrow.get() {
            cx.notify();
        }
        let was_at_edge = self.at_left_edge;
        self.at_left_edge = self.graph_scroll.offset().x >= px(-2.);
        let dx = f32::from(event.delta.pixel_delta(px(20.)).x);
        let pull = self.pull.get();
        let next = match (&event.delta, event.touch_phase) {
            _ if !self.at_left_edge || self.graph.older == 0 => None,
            // A trackpad: a swipe that starts at the edge may pull; it ends
            // with the fingers lifting.
            (gpui::ScrollDelta::Pixels(_), gpui::TouchPhase::Started) => Some(0.),
            (gpui::ScrollDelta::Pixels(_), gpui::TouchPhase::Ended) => None,
            (gpui::ScrollDelta::Pixels(_), _) => pull.map(|pull| pull + dx),
            // A wheel: clicks once at the edge pull.
            (gpui::ScrollDelta::Lines(_), _) if was_at_edge && dx > 0. => {
                Some(pull.unwrap_or(0.) + dx)
            }
            (gpui::ScrollDelta::Lines(_), _) => pull.filter(|_| dx >= 0.),
        };
        let next = next.map(|pull| pull.max(0.));
        if next.is_some_and(|pull| pull >= PULL_DISTANCE) {
            self.pull.set(None);
            self.load_older(cx);
        } else if next != pull {
            self.pull.set(next);
            cx.notify();
        }
    }

    fn apply_graph(&mut self, result: Result<WorkflowGraph, CoreError>, cx: &mut Context<Self>) {
        self.loading_older = false;
        let graph = match result {
            Ok(graph) => graph,
            // A failed refresh keeps the last graph; the terminal shows why.
            Err(_) if !self.graph.nodes.is_empty() => return,
            Err(error) => {
                self.graph_error = Some(message(error));
                cx.notify();
                return;
            }
        };
        self.graph_error = None;
        if graph == self.graph {
            return;
        }

        // Keeps the selection by node identity, falling back to the opened
        // worktree's current state, or else the main branch.
        let mut keys: Vec<String> = self
            .selected_nodes
            .iter()
            .map(|&i| self.graph.nodes[i].key.clone())
            .collect();
        // The first graph of a repository opened before shows what was
        // selected then.
        if keys.is_empty() {
            keys = std::mem::take(&mut self.restore_keys);
        }
        self.graph = graph;
        self.graph_layout.take();
        // Drafts of worktrees the graph no longer has go, so a new one at
        // the same path starts empty.
        let worktrees: HashSet<&str> = self
            .graph
            .nodes
            .iter()
            .filter_map(|node| match &node.tip {
                Revision::Worktree(path) => Some(path.as_str()),
                Revision::Commit(_) => None,
            })
            .collect();
        self.drafts
            .retain(|path, _| worktrees.contains(path.as_str()));
        self.hovered_node = None;
        self.update_menus(cx);
        let mut selected: Vec<usize> = keys
            .iter()
            .filter_map(|key| self.graph.nodes.iter().position(|node| &node.key == key))
            .collect();
        if selected.is_empty() {
            let root = self
                .repo_path
                .as_deref()
                .map(|path| path.trim_end_matches('/'));
            let current = self.graph.nodes.iter().position(|node| {
                matches!(&node.tip, Revision::Worktree(path)
                    if Some(path.trim_end_matches('/')) == root)
            });
            selected.extend(current.or(self.graph.main_node));
        }

        // A stash made from here, once in, is shown and picked in the
        // attention panel, its group open. One that never comes, failed or
        // cancelled, is given up after a while.
        if let Some((before, asked)) = &self.awaiting_stash {
            let newest = self
                .graph
                .stashes
                .first()
                .map(|stash| (stash.id.clone(), stash.name.clone()));
            match newest {
                Some((id, name)) if Some(&id) != before.as_ref() => {
                    self.awaiting_stash = None;
                    self.collapsed_sections.remove(&Section::Stashes);
                    self.show_stash(&name, cx);
                    cx.notify();
                    return;
                }
                _ if asked.elapsed() > STASH_WAIT => self.awaiting_stash = None,
                _ => {}
            }
        }

        // A branch shown from outside the graph stays shown.
        if self.outside_graph {
            if std::mem::take(&mut self.reveal_stash)
                && let Some(stash) = &self.stash_view
            {
                self.reveal.set(self.stash_node(&stash.name));
            }
            cx.notify();
            return;
        }
        let range = self.graph.range(&selected);
        if range != self.range {
            self.set_selection(selected, cx);
        } else {
            self.selected_nodes = selected;
            // Uncommitted changes may have changed under the same range.
            let live = matches!(range, Some((_, Revision::Worktree(_))));
            if live && self.narrowed.is_empty() {
                let keep = self.selected_file.map(|i| self.files[i].path().to_string());
                self.load_files(keep, cx);
            }
        }
        cx.notify();
    }

    fn choose_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        // The workspace decides the tab: this one when empty, otherwise a new one.
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let path = path.to_string_lossy().into_owned();
            this.update_in(cx, |_, window, cx| {
                window.dispatch_action(Box::new(OpenPath { path }), cx)
            })
            .ok();
        })
        .detach();
    }

    pub fn hover_node(&mut self, index: usize, hovered: bool, cx: &mut Context<Self>) {
        let next = match hovered {
            true => Some(index),
            false if self.hovered_node == Some(index) => None,
            false => self.hovered_node,
        };

        if next != self.hovered_node {
            self.hovered_node = next;
            cx.notify();
        }
    }

    /// A plain click selects one node; Cmd-click toggles it in the selection.
    /// Selects a node from outside the graph, such as a list or Go To, and
    /// scrolls the graph to it. A click in the graph leaves the graph where
    /// it is: what was clicked is in view, or meant to be left there.
    pub(super) fn select_and_reveal(&mut self, index: usize, cx: &mut Context<Self>) {
        self.click_node(index, false, cx);
        self.reveal.set(Some(index));
        cx.notify();
    }

    pub fn click_node(&mut self, index: usize, toggle: bool, cx: &mut Context<Self>) {
        let mut selected = self.selected_nodes.clone();
        if !toggle {
            selected = vec![index];
        } else if let Some(at) = selected.iter().position(|&i| i == index) {
            selected.remove(at);
        } else {
            selected.push(index);
        }
        if selected == self.selected_nodes && self.narrowed.is_empty() {
            return;
        }
        self.set_selection(selected, cx);
    }

    /// Shows a branch with no history in common with the main branch, which
    /// the graph cannot draw: all its commits, from nothing to its tip.
    fn show_unrelated(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(tip) = self
            .graph
            .branches
            .iter()
            .find(|branch| branch.name == name)
            .map(|branch| branch.commit.clone())
        else {
            return;
        };
        self.set_selection(Vec::new(), cx);
        self.range = Some((None, Revision::Commit(tip)));
        self.outside_graph = true;
        self.load_commits(cx);
        self.load_files(None, cx);
        cx.notify();
    }

    /// Shows a stash: what it held staged and unstaged, untracked files
    /// with the unstaged, as the staging view would, but read-only.
    pub fn show_stash(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(stash) = self.graph.stashes.iter().find(|s| s.name == name).cloned() else {
            return;
        };
        self.set_selection(Vec::new(), cx);
        self.range = Some((Some(stash.base.clone()), Revision::Commit(stash.id.clone())));
        self.outside_graph = true;
        self.commits = vec![Commit {
            id: stash.id.clone(),
            short_id: popups::short_hash(&stash.id),
            summary: format!("{} — {}", stash.name, stash.message),
            body: match &stash.branch {
                Some(branch) => format!("Stashed on {branch}."),
                None => "Stashed on a detached HEAD.".to_string(),
            },
            author: String::new(),
            time: stash.time,
            parent: Some(stash.base.clone()),
        }];
        self.stash_view = Some(stash);
        self.load_files(None, cx);
        // The graph scrolls to its marker, turning stashes on to draw it.
        if self.attention.stashes {
            self.reveal.set(self.stash_node(name));
        } else {
            self.reveal_stash = true;
            self.change_attention(|attention| attention.stashes = true, cx);
        }
        cx.notify();
    }

    /// The node a stash was made on, when the graph reaches back to it.
    fn stash_node(&self, name: &str) -> Option<usize> {
        self.graph
            .nodes
            .iter()
            .position(|node| node.stashes.iter().any(|stash| stash == name))
    }

    fn set_selection(&mut self, selected: Vec<usize>, cx: &mut Context<Self>) {
        // The last worktree among those selected becomes the current one.
        if let Some(path) =
            selected
                .iter()
                .rev()
                .find_map(|&i| match &self.graph.nodes.get(i)?.tip {
                    Revision::Worktree(path) => Some(path.clone()),
                    Revision::Commit(_) => None,
                })
        {
            self.current_worktree = Some(path);
        }
        if let Some(repo) = &self.repo_path {
            let keys: Vec<&str> = selected
                .iter()
                .filter_map(|&i| self.graph.nodes.get(i))
                .map(|node| node.key.as_str())
                .collect();
            save_selection(repo, &keys);
        }
        self.worktree_split = None;
        self.line_selection = None;
        self.outside_graph = false;
        self.stash_view = None;
        self.range = self.graph.range(&selected);
        self.selected_nodes = selected;
        self.narrowed.clear();
        self.commit_anchor = None;
        self.follow_thread();
        // A snapshot keeps the last commits and files in view until the new
        // ones come, so stepping through versions does not flash.
        if !self.snapshot_mode {
            self.commits.clear();
            self.files.clear();
        }
        self.commits_task = None;
        self.pending_commit = None;
        // A commit waiting for the model's message goes only where it was
        // asked for.
        self.draft.commit_when_written = false;
        // Each worktree has a commit box of its own.
        let worktree = self.staging_worktree();
        if let Some(path) = worktree.filter(|path| self.draft.worktree.as_ref() != Some(path)) {
            let next = self.drafts.remove(&path).unwrap_or_else(|| CommitDraft {
                worktree: Some(path),
                ..CommitDraft::default()
            });
            let mut previous = std::mem::replace(&mut self.draft, next);
            // What was typed waits for the worktree's return; a message the
            // model is still writing stops, as it would land in this one,
            // and is asked for again on the return.
            if std::mem::take(&mut previous.writing_message) {
                (previous.suggest_task, previous.suggest_key) = (None, None);
            }
            self.message_scroll.set_offset(point(px(0.), px(0.)));
            if let Some(path) = previous.worktree.clone() {
                self.drafts.insert(path, previous);
            }
        }
        self.select_file(None, cx);
        if self.range.is_none() && !self.selected_nodes.is_empty() {
            self.error_message = Some("Select nodes on one line of history.".to_string());
        } else {
            self.error_message = None;
            self.load_commits(cx);
            self.load_files(None, cx);
        }
        cx.notify();
    }

    fn load_commits(&mut self, cx: &mut Context<Self>) {
        let (Some(repo_path), Some((base, tip))) = (self.repo_path.clone(), self.range.clone())
        else {
            return;
        };
        let load = cx
            .background_executor()
            .spawn(async move { wtf_core::range_commits(&repo_path, base.as_deref(), &tip) });
        self.commits_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                this.commits_task = None;
                match result {
                    Ok(commits) => this.commits = commits,
                    Err(error) => this.error_message = Some(message(error)),
                }
                if let Some(id) = this.pending_commit.take() {
                    this.narrow_to(&id, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// A click narrows the diff to one commit, or widens it back when that was
    /// the only one; Cmd-click toggles a commit and Shift-click selects a range.
    fn click_commit(&mut self, index: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        match self.commit_anchor {
            Some(anchor) if modifiers.shift => {
                self.narrowed = (anchor.min(index)..=anchor.max(index)).collect();
            }
            _ if modifiers.platform => {
                match self.narrowed.iter().position(|&i| i == index) {
                    Some(at) => {
                        self.narrowed.remove(at);
                    }
                    None => self.narrowed.push(index),
                }
                self.commit_anchor = Some(index);
            }
            _ => {
                self.narrowed = if self.narrowed == [index] {
                    Vec::new()
                } else {
                    vec![index]
                };
                self.commit_anchor = Some(index);
            }
        }
        self.reload_files(cx);
    }

    /// A bead in the graph: the first click selects its node with all its
    /// commits; on a selected node, a click picks the bead's commit, and
    /// another on the same bead goes back to all of them.
    pub fn click_bead(&mut self, node: usize, commit: String, cx: &mut Context<Self>) {
        if self.selected_nodes != [node] {
            self.set_selection(vec![node], cx);
            return;
        }
        let picked = self.commits.iter().position(|c| c.id == commit);
        if picked.is_some() && self.narrowed == picked.into_iter().collect::<Vec<_>>() {
            self.show_all_commits(cx);
        } else {
            self.narrow_to(&commit, cx);
        }
    }

    /// Selects a commit from elsewhere, such as Go To: its node, then the
    /// commit once the node's commits are loaded.
    fn select_commit(&mut self, node: usize, commit: String, cx: &mut Context<Self>) {
        if self.selected_nodes == [node] && !self.commits.is_empty() {
            self.narrow_to(&commit, cx);
        } else {
            self.set_selection(vec![node], cx);
            self.pending_commit = Some(commit);
        }
        // From outside the graph: the graph goes to it.
        self.reveal.set(Some(node));
    }

    /// Narrows the diff to one listed commit.
    fn narrow_to(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(index) = self.commits.iter().position(|commit| commit.id == id) else {
            return;
        };
        self.narrowed = vec![index];
        self.commit_anchor = Some(index);
        self.scroll_to_commit.set(Some(index));
        self.reload_files(cx);
    }

    /// Widens the diff back to the whole selection of nodes.
    fn show_all_commits(&mut self, cx: &mut Context<Self>) {
        self.narrowed.clear();
        self.commit_anchor = None;
        self.reload_files(cx);
    }

    fn reload_files(&mut self, cx: &mut Context<Self>) {
        if !self.snapshot_mode {
            self.files.clear();
        }
        self.select_file(None, cx);
        self.load_files(None, cx);
        cx.notify();
    }

    /// Loads the changed files, keeping `keep` selected if it is still there.
    fn load_files(&mut self, keep: Option<String>, cx: &mut Context<Self>) {
        // A snapshot's open file is the one whose changes load, for its tints.
        let keep = match self.snapshot_mode {
            true => self.snapshot.path.clone().or(keep),
            false => keep,
        };
        let range = commit_range(&self.commits, &self.narrowed).or_else(|| self.range.clone());
        // A worktree's files change on disk; a commit's never do.
        if let Some((_, Revision::Worktree(_))) = &range {
            self.snapshot.stale = true;
        }
        let (Some(repo_path), Some((base, tip))) = (self.repo_path.clone(), range) else {
            return;
        };
        // A selected worktree alone shows its changes split for staging.
        let worktree = self.staging_worktree();
        let stash = self.stash_view.clone();
        let load = cx.background_executor().spawn(async move {
            if let Some(stash) = stash {
                return wtf_core::stash_files(&repo_path, &stash)
                    .map(|(files, split)| (files, Some(split), Vec::new()));
            }
            match worktree {
                Some(path) => wtf_core::worktree_status(&path).map(|status| {
                    let split = status.unstaged.len();
                    let mut files = status.unstaged;
                    files.extend(status.staged);
                    (files, Some(split), status.inside)
                }),
                None => wtf_core::changed_files(&repo_path, base.as_deref(), &tip)
                    .map(|files| (files, None, Vec::new())),
            }
        });
        self.files_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                this.files_task = None;
                match result {
                    Ok((files, split, inside)) => {
                        this.files = files;
                        this.worktree_split = split;
                        this.submodules_inside = inside;
                        this.error_message = None;
                        let kept = keep.and_then(|path| {
                            this.files.iter().position(|file| file.path() == path)
                        });
                        if split.is_some() {
                            this.suggest_message(cx);
                        }
                        if this.is_stacked() {
                            // Without one to keep, the first file as the tree lists them.
                            this.selected_file =
                                kept.or_else(|| this.file_order().first().copied());
                            this.load_stacked(kept.is_some(), cx);
                        } else {
                            match kept {
                                Some(index) => {
                                    this.selected_file = Some(index);
                                    this.load_diff(true, cx);
                                }
                                None => this.select_file((!this.files.is_empty()).then_some(0), cx),
                            }
                        }
                    }
                    Err(error) => this.error_message = Some(message(error)),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Selecting a file scrolls the changes to it; among too many files to
    /// open at once, it opens it too.
    fn select_file(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        self.selected_file = index;
        if !self.is_stacked() {
            self.load_diff(false, cx);
            return;
        }
        if self.many_files()
            && let Some(path) = index
                .and_then(|i| self.files.get(i))
                .map(|f| f.path().to_string())
            && self.is_folded(&path)
        {
            self.set_folded(path, false);
            self.after_fold(cx);
        }
        let header = index.and_then(|file| {
            self.stacked
                .iter()
                .position(|item| matches!(item, StackItem::Header(i) if *i == file))
                .map(|header| header.saturating_sub(1))
        });
        if let Some(header) = header {
            self.stacked_scroll
                .scroll_to_item_strict(self.below_bar(header), ScrollStrategy::Top);
        }
        cx.notify();
    }

    fn set_diff_mode(&mut self, mode: DiffMode, cx: &mut Context<Self>) {
        if self.diff_mode != mode {
            self.diff_mode = mode;
            self.reload_changes(cx);
        }
    }

    fn reload_changes(&mut self, cx: &mut Context<Self>) {
        if self.is_stacked() {
            self.load_stacked(false, cx);
        } else {
            self.load_diff(false, cx);
        }
    }

    /// Whether changes show in the stacked view: always, once there are
    /// files; a worktree's even without, as the commit box lives there.
    fn is_stacked(&self) -> bool {
        self.worktree_split.is_some() || !self.files.is_empty()
    }

    /// Too many files to open at once: they all start folded, and each
    /// loads its diff when opened.
    pub(super) fn many_files(&self) -> bool {
        self.files.len() > MAX_STACKED_FILES
    }

    /// Whether a file of the stacked view shows only its header.
    pub(super) fn is_folded(&self, path: &str) -> bool {
        match self.many_files() {
            true => !self.unfolded.contains(path),
            false => self.folded.contains(path),
        }
    }

    /// The worktree to stage in: the only selected node, when it is one.
    fn staging_worktree(&self) -> Option<String> {
        if !self.narrowed.is_empty() {
            return None;
        }
        let [index] = self.selected_nodes[..] else {
            return None;
        };
        match &self.graph.nodes.get(index)?.tip {
            Revision::Worktree(path) => Some(path.clone()),
            Revision::Commit(_) => None,
        }
    }

    /// A refresh keeps the current diff and scroll position until the new one is ready.
    fn load_diff(&mut self, refresh: bool, cx: &mut Context<Self>) {
        self.diff_task = None;
        if !refresh {
            self.diff = None;
            self.is_loading_diff = false;
        }
        let (Some(repo_path), Some(file)) = (
            self.repo_path.clone(),
            self.selected_file.and_then(|index| self.files.get(index)),
        ) else {
            self.diff = None;
            return;
        };
        self.is_loading_diff = !refresh;
        let (file, mode) = (file.clone(), self.diff_mode);
        let load = cx
            .background_executor()
            .spawn(async move { wtf_core::file_diff(repo_path, file, mode) });
        self.diff_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                this.is_loading_diff = false;
                match result {
                    Ok(diff) => {
                        if !refresh {
                            let first = diff_view::first_change(&diff.rows).unwrap_or(0);
                            // Changes near the top leave the commit header in view.
                            let row = match first.saturating_sub(CONTEXT_ROWS) {
                                0 => 0,
                                row => this.below_bar(row),
                            };
                            this.diff_scroll
                                .scroll_to_item_strict(row, ScrollStrategy::Top);
                        }
                        this.diff = Some(diff);
                        this.syntax.borrow_mut().clear();
                        this.error_message = None;
                    }
                    Err(error) => this.error_message = Some(message(error)),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Picks up commands recorded since the last call and prints them in the
    /// terminal, which also flushes lines held during full-screen programs.
    fn sync_log(&mut self, cx: &mut Context<Self>) {
        let mut new = wtf_core::log_since(self.log_seq);
        if let Some(last) = new.last() {
            self.log_seq = last.seq;
        }
        // The log is shared by all windows; this one shows its repository's.
        new.retain(|entry| self.runs_here(&entry.dir));
        let lines = console_view::lines(&new, self.hide_refresh, &mut self.announced_dir);
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, cx| terminal.announce(lines, cx));
        }
    }

    /// Whether a command ran in this window's repository or one of its worktrees.
    fn runs_here(&self, dir: &str) -> bool {
        let dir = dir.trim_end_matches('/');
        let repo = self
            .repo_path
            .as_deref()
            .map(|path| path.trim_end_matches('/'));
        repo == Some(dir)
            || self.graph.nodes.iter().any(|node| {
                matches!(&node.tip, Revision::Worktree(path) if path.trim_end_matches('/') == dir)
            })
    }

    /// Opens the terminal dock, starting the shell the first time.
    fn toggle_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.console_open = !self.console_open;
        if self.console_open {
            let dir = self
                .repo_path
                .clone()
                .or_else(|| std::env::var("HOME").ok())
                .unwrap_or_else(|| "/".to_string());
            let terminal = self
                .terminal
                .get_or_insert_with(|| cx.new(|cx| TerminalView::new(dir, cx)));
            terminal.read(cx).focus(window);
        } else {
            self.focus.focus(window);
        }
        cx.notify();
    }

    /// Follows the mouse while a panel border is dragged.
    fn drag_border(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The graph follows a drag that started on its empty part.
        if let Some((start, offset)) = self.graph_drag {
            if event.pressed_button != Some(MouseButton::Left) {
                self.graph_drag = None;
                return;
            }
            let max = self.graph_scroll.max_offset();
            let moved = event.position - start;
            let x = (offset.x + moved.x).clamp(-max.width, px(0.));
            let y = (offset.y + moved.y).clamp(-max.height, px(0.));
            self.graph_scroll.set_offset(point(x, y));
            cx.notify();
            return;
        }
        let Some(border) = self.dragging else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.dragging = None;
            crate::panels::save(self.panel_sizes());
            return;
        }
        let height = f32::from(window.viewport_size().height);
        let y = f32::from(event.position.y);
        let max = (height - MIN_MAIN_HEIGHT).max(MIN_PANEL_HEIGHT);
        match border {
            Border::SnapshotHeader => {
                if let Some((from, start)) = self.header_drag {
                    let lines = HEADER_LINE * preferences::ui_size();
                    self.snapshot_header_height = Some((start + y - from).clamp(2. * lines, max));
                }
            }
            Border::Dock => self.dock_height = (height - y).clamp(MIN_PANEL_HEIGHT, max),
            Border::Graph => {
                let top = workspace::TAB_BAR_HEIGHT;
                self.graph_height = Some((y - top).clamp(MIN_PANEL_HEIGHT, max))
            }
            Border::Sidebar => {
                let x = f32::from(window.viewport_size().width - event.position.x);
                let width = x - SIDEBAR_STRIP_WIDTH;
                self.sidebar_width = width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
            }
            Border::Commits => {
                let right = match (self.repo_path.is_some(), self.side_panel) {
                    (false, _) => 0.,
                    (true, Some(_)) => SIDEBAR_STRIP_WIDTH + self.sidebar_width + 8.,
                    (true, None) => SIDEBAR_STRIP_WIDTH,
                };
                let width = f32::from(window.viewport_size().width) - right;
                let max = (width - MIN_DIFF_WIDTH).max(MIN_COLUMN_WIDTH);
                self.commits_width = f32::from(event.position.x).clamp(MIN_COLUMN_WIDTH, max);
            }
        }
        cx.notify();
    }

    /// Opens a submodule, by its path in the worktree in review or else the
    /// repository, as a tab of its own.
    pub(super) fn open_submodule(&self, path: &str, cx: &mut Context<Self>) {
        let root = self
            .staging_worktree()
            .or_else(|| self.current_worktree_path().map(str::to_string))
            .or_else(|| self.repo_path.clone());
        let Some(root) = root else {
            return;
        };
        let full = Path::new(&root).join(path).to_string_lossy().into_owned();
        // After this update: opening a tab updates the window this runs in.
        cx.defer(move |cx| workspace::open_paths(vec![full], cx));
    }

    /// The panels' sizes as they are now, to keep.
    fn panel_sizes(&self) -> crate::panels::Sizes {
        crate::panels::Sizes {
            sidebar: self.sidebar_width,
            dock: self.dock_height,
            graph: self.graph_height,
            commits: self.commits_width,
            snapshot_header: self.snapshot_header_height,
        }
    }

    fn dock(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_none()
            .h(px(self.dock_height))
            .child(border_handle("dock-border", theme).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging = Some(Border::Dock);
                    cx.stop_propagation();
                }),
            ))
            .child(div().flex_1().min_h_0().children(self.terminal.clone()))
    }

    fn title(&self) -> String {
        let name = self
            .repo_path
            .as_deref()
            .and_then(|path| Path::new(path).file_name());
        let name = name.map_or("wtf".into(), |name| name.to_string_lossy());
        match &self.diff {
            Some(diff) => format!("{name} — {}", diff.language),
            None => name.into_owned(),
        }
    }

    fn toggle_combine_commits(&mut self, cx: &mut Context<Self>) {
        self.combine_commits = !self.combine_commits;
        self.update_menus(cx);
        self.start_refresh(cx);
        cx.notify();
    }

    fn toggle_refresh_commands(&mut self, cx: &mut Context<Self>) {
        self.hide_refresh = !self.hide_refresh;
        cx.notify();
    }

    fn detail(&self, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(message) = &self.error_message {
            return placeholder("Something went wrong", Some(message.clone()), theme);
        }
        if self.repo_path.is_none() {
            let hint = "Open a Git repository to review its commits.";
            return placeholder("No repository", Some(hint.into()), theme);
        }
        if self.snapshot_mode {
            let view = self.snapshot_view(theme, cx);
            // The message keeps one height, four lines unless dragged, so
            // the file under it stays put from version to version.
            let height = self
                .snapshot_header_height
                .unwrap_or(2. * HEADER_PADDING + 4. * HEADER_LINE * preferences::ui_size());
            return div()
                .flex()
                .flex_col()
                .size_full()
                .when(self.range.is_some(), |pane| {
                    pane.child(
                        div()
                            .id("snapshot-header")
                            .flex_none()
                            .h(px(height))
                            .overflow_y_scroll()
                            .child(self.commit_header(theme, window, cx)),
                    )
                    .child(
                        border_handle("snapshot-header-border", theme).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                this.dragging = Some(Border::SnapshotHeader);
                                this.header_drag = Some((f32::from(event.position.y), height));
                                cx.stop_propagation();
                            }),
                        ),
                    )
                })
                .children(
                    self.snapshot
                        .path
                        .clone()
                        .map(|path| self.file_steps_bar(&path, theme, cx)),
                )
                .child(div().flex_1().min_h_0().child(view))
                .into_any_element();
        }
        if self.is_stacked() {
            if self.is_loading_stacked {
                return self.with_header(placeholder("Loading…", None, theme), theme, window, cx);
            }
            let spacers = self.header_rows();
            let width = self
                .stacked_scroll
                .0
                .borrow()
                .base_handle
                .bounds()
                .size
                .width;
            let widest = self
                .stacked_diffs
                .iter()
                .flatten()
                .map(|diff| diff_view::widest(&diff.rows))
                .max()
                .unwrap_or(0);
            self.track_widest(widest, f32::from(width), true, window);
            let list = uniform_list(
                "stacked",
                spacers + self.stacked.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                    let theme = Theme::for_appearance(window.appearance());
                    range
                        .filter_map(|index| match index.checked_sub(spacers) {
                            None => Some(spacer()),
                            Some(index) => this
                                .stacked
                                .get(index)
                                .map(|item| this.stack_row(item, &theme, cx)),
                        })
                        .collect()
                }),
            )
            .track_scroll(self.stacked_scroll.clone())
            .flex_1()
            .size_full()
            .font_family(preferences::code_font())
            .font_weight(preferences::code_weight())
            .text_size(px(preferences::code_size()))
            // Headers and code lines share one line height, whatever the font.
            .line_height(px(diff_view::row_height()));
            // The header of the file at the top stays pinned while its lines
            // scroll under it.
            // Rows have one height, so the offset tells the top row. Past the
            // top, the list holds still as it lays out, and so does the header.
            let scroll_y =
                f32::from(-self.stacked_scroll.0.borrow().base_handle.offset().y).max(0.);
            let bar = match self.range.is_some() && scroll_y > 0. {
                true => summary_bar_height(),
                false => 0.,
            };
            let top = ((scroll_y + bar) / diff_view::row_height()).max(0.) as usize;
            let pinned = top
                .checked_sub(spacers)
                .and_then(|top| {
                    self.stacked[..(top + 1).min(self.stacked.len())]
                        .iter()
                        .rposition(|item| {
                            matches!(item, StackItem::Header(_) | StackItem::Section(..))
                        })
                        .filter(|&at| at < top)
                })
                .and_then(|at| match self.stacked[at] {
                    StackItem::Header(file) => Some(file),
                    _ => None,
                })
                .filter(|&file| self.stack_item_fits(&StackItem::Header(file)));
            return div()
                .flex()
                .flex_col()
                .size_full()
                .on_scroll_wheel(cx.listener(Self::scroll_diff_sideways))
                .when(
                    self.worktree_split.is_some() && self.stash_view.is_none(),
                    |pane| pane.child(self.commit_box(theme, window, cx)),
                )
                .child(self.under_header(
                    list.into_any_element(),
                    scroll_y,
                    pinned.map(|file| {
                        div()
                            .font_family(preferences::code_font())
                            .font_weight(preferences::code_weight())
                            .text_size(px(preferences::code_size()))
                            .line_height(px(diff_view::row_height()))
                            .border_b_1()
                            .border_color(theme.separator)
                            .child(self.stack_row_unchecked(&StackItem::Header(file), theme, cx))
                            .into_any_element()
                    }),
                    theme,
                    window,
                    cx,
                ))
                .into_any_element();
        }
        if self.is_loading_diff {
            return self.with_header(placeholder("Loading…", None, theme), theme, window, cx);
        }
        let shown = match &self.diff {
            Some(diff) if !diff.rows.is_empty() => {
                let spacers = self.header_rows();
                let width = self.diff_scroll.0.borrow().base_handle.bounds().size.width;
                let cols = self.wrap_cols_for(f32::from(width), false, window);
                let widest = diff_view::widest(&diff.rows);
                self.track_widest(widest, f32::from(width), false, window);
                // Wrapped, each row takes as many list rows as it has lines.
                let lines: Rc<Vec<(usize, usize)>> = Rc::new(match cols {
                    Some(cols) => diff
                        .rows
                        .iter()
                        .enumerate()
                        .flat_map(|(i, row)| {
                            (0..diff_view::parts(row, cols)).map(move |part| (i, part))
                        })
                        .collect(),
                    None => (0..diff.rows.len()).map(|i| (i, 0)).collect(),
                });
                let count = lines.len();
                let list = uniform_list(
                    "diff",
                    spacers + count,
                    cx.processor(move |this, range: std::ops::Range<usize>, window, _| {
                        let theme = Theme::for_appearance(window.appearance());
                        let rows = this.diff.as_ref().map_or(&[][..], |diff| &diff.rows);
                        range
                            .filter_map(|index| match index.checked_sub(spacers) {
                                None => Some(spacer()),
                                Some(index) => {
                                    let &(i, part) = lines.get(index)?;
                                    rows.get(i).map(|row| {
                                        let colors = this.diff_colors(None, &theme);
                                        diff_view::row(
                                            row,
                                            &theme,
                                            colors.as_deref(),
                                            this.fit(cols, part),
                                        )
                                        .into_any_element()
                                    })
                                }
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.diff_scroll.clone())
                .size_full()
                .font_family(preferences::code_font())
                .font_weight(preferences::code_weight())
                .text_size(px(preferences::code_size()))
                .line_height(px(diff_view::row_height()));
                let scroll_y =
                    f32::from(-self.diff_scroll.0.borrow().base_handle.offset().y).max(0.);
                // The pane takes its parent's height, as the all-files one does
                // in its column; otherwise the list has none and shows nothing.
                return div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .on_scroll_wheel(cx.listener(Self::scroll_diff_sideways))
                    .child(self.under_header(
                        list.into_any_element(),
                        scroll_y,
                        None,
                        theme,
                        window,
                        cx,
                    ))
                    .into_any_element();
            }
            Some(_) => {
                let hint = "Difftastic found no differences to show in this file.";
                placeholder("No visible changes", Some(hint.into()), theme)
            }
            None if self.range.is_some() && self.files.is_empty() => {
                placeholder("No changes", None, theme)
            }
            None => placeholder("No file selected", None, theme),
        };
        self.with_header(shown, theme, window, cx)
    }
}

impl RepoView {
    fn toggle_attention_panel(&mut self, cx: &mut Context<Self>) {
        self.toggle_panel(SidePanel::Attention, cx);
    }

    /// Opens a side panel, or closes it when it is the open one.
    fn toggle_panel(&mut self, panel: SidePanel, cx: &mut Context<Self>) {
        self.side_panel = match self.side_panel == Some(panel) {
            true => None,
            false => Some(panel),
        };
        self.last_panel = panel;
        self.update_menus(cx);
        cx.notify();
    }

    /// Closes the open side panel, or opens the last one.
    fn toggle_side_panel(&mut self, cx: &mut Context<Self>) {
        let panel = self.side_panel.unwrap_or(self.last_panel);
        self.toggle_panel(panel, cx);
    }

    /// The open side panel and the strip beside it.
    fn side_panel_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = match self.side_panel {
            Some(SidePanel::Attention) => Some(self.attention_panel_view(theme, window, cx)),
            Some(SidePanel::Files) => Some(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(px(self.sidebar_width))
                    .h_full()
                    .bg(theme.background)
                    .text_sm()
                    .map(|panel| match self.snapshot_mode {
                        true => panel.child(self.snapshot_list(theme, window, cx)),
                        false => panel.child(self.file_list(cx)),
                    })
                    .into_any_element(),
            ),
            None => None,
        };
        div()
            .flex()
            .flex_none()
            .h_full()
            .children(content)
            .child(self.side_strip(theme, window, cx))
            .into_any_element()
    }

    /// The strip at the window's right edge, the same with a panel open or
    /// not: the arrow, the panels by name, then the diff view switches; the
    /// terminal's button at the foot, nearest the terminal.
    fn side_strip(&self, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        // Each panel: a box like the mode groups below, of one button
        // holding its arrow and its name.
        let tab = |panel: SidePanel, label: &'static str| {
            let open = self.side_panel == Some(panel);
            strip_group(theme).child(
                strip_option(label, open, theme)
                    .gap(px(2.))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_panel(panel, cx)))
                    .child(side_chevron(!open, theme.secondary, 10.))
                    .child(vertical_label(label, theme.foreground, window)),
            )
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(px(STRIP_GAP))
            .pt(px(STRIP_GAP))
            .w(px(SIDEBAR_STRIP_WIDTH))
            .h_full()
            .border_l_1()
            .border_color(theme.separator)
            .bg(theme.graph_background)
            .child(tab(SidePanel::Attention, "Attention"))
            .child(tab(SidePanel::Files, "Files"))
            .child(div().w(px(16.)).h(px(1.)).bg(theme.separator))
            .child(self.view_toggles(theme, window, cx))
            .child(div().flex_1())
            .child(
                strip_group(theme).mb(px(STRIP_GAP)).child(
                    strip_option("Terminal", self.console_open, theme)
                        .gap(px(2.))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_console(window, cx)),
                        )
                        .child(vertical_label("Terminal", theme.foreground, window))
                        // At the foot, towards the terminal: down closes
                        // it, up opens it.
                        .child(stroke_icon(
                            match self.console_open {
                                true => "M3.5 6 L8 10.5 L12.5 6",
                                false => "M3.5 10 L8 5.5 L12.5 10",
                            },
                            theme.secondary,
                            10.,
                        )),
                ),
            )
            .into_any_element()
    }

    /// A column's right edge, with the same grip as the other borders,
    /// that resizes the column when dragged.
    fn column_handle(
        &self,
        id: &'static str,
        border: Border,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        side_border_handle(id, theme).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.dragging = Some(border);
                cx.stop_propagation();
            }),
        )
    }
}

/// The diff covering selected commits of a newest-first list: from the oldest
/// one's parent to the newest one; `None` when nothing is selected.
fn commit_range(commits: &[Commit], selected: &[usize]) -> Option<(Option<String>, Revision)> {
    let newest = commits.get(*selected.iter().min()?)?;
    let oldest = commits.get(*selected.iter().max()?)?;
    Some((oldest.parent.clone(), Revision::Commit(newest.id.clone())))
}

/// A short "3 days ago" style age.
/// A time as the clock here showed it: `2026-09-14 15:32`.
fn local_datetime(time: i64) -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let seconds: libc::time_t = time;
    // SAFETY: localtime_r writes only into `tm`, which outlives the call.
    if unsafe { libc::localtime_r(&seconds, &mut tm) }.is_null() {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

/// The height of a commit list row.
const LIST_ITEM_HEIGHT: f32 = 40.;

impl Render for RepoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // An open prompt's caret blinks, from when it opened or was typed in.
        if self.prompt.is_some() && self.blink_task.is_none() {
            self.blink(cx);
        }
        let theme = Theme::for_appearance(window.appearance());
        window.set_window_title(&self.title());
        self.request_summary(cx);
        self.sync_snapshot(cx);
        self.sync_log(cx);
        self.sync_wrap(window, cx);
        div()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &OpenRepository, window, cx| {
                this.choose_repository(window, cx)
            }))
            .on_action(cx.listener(|this, _: &StructuralDiff, _, cx| {
                this.set_diff_mode(DiffMode::Structural, cx)
            }))
            .on_action(
                cx.listener(|this, _: &TextDiff, _, cx| this.set_diff_mode(DiffMode::Text, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleCombineCommits, _, cx| {
                    this.toggle_combine_commits(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &GoTo, window, cx| this.open_goto(false, window, cx)))
            .on_action(cx.listener(|this, _: &Prune, window, cx| this.open_prune(window, cx)))
            .on_action(cx.listener(|this, _: &Fetch, _, cx| this.fetch(false, cx)))
            // Stashes the changes of the worktree commands run in.
            .on_action(cx.listener(|this, _: &StashChanges, window, cx| {
                if let Some(path) = this.command_path() {
                    this.open_stash_picker(path, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSnapshot, window, cx| {
                let on = !this.snapshot_mode;
                this.set_snapshot_mode(on, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenWorktreeWith, window, cx| {
                this.open_worktree_with(None, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &CommandPalette, window, cx| {
                    this.open_goto(true, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleSidePanel, _, cx| this.toggle_side_panel(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleConsole, window, cx| this.toggle_console(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleRefreshCommands, _, cx| {
                this.toggle_refresh_commands(cx)
            }))
            .on_action(cx.listener(|this, action: &SetTimeWindow, _, cx| {
                let days = action.days;
                this.change_attention(|attention| attention.window_days = days, cx)
            }))
            .on_action(cx.listener(|this, action: &ToggleAuthor, _, cx| {
                let name = action.name.clone();
                this.change_attention(
                    |attention| match attention.authors.iter().position(|a| *a == name) {
                        Some(at) => {
                            attention.authors.remove(at);
                        }
                        None => attention.authors.push(name),
                    },
                    cx,
                )
            }))
            .on_action(
                cx.listener(|this, _: &ToggleAttentionPanel, _, cx| {
                    this.toggle_attention_panel(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleRemotes, _, cx| {
                this.change_attention(|attention| attention.remotes = !attention.remotes, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleTags, _, cx| {
                this.change_attention(|attention| attention.tags = !attention.tags, cx)
            }))
            .on_action(cx.listener(|this, action: &SetWorktreeOrder, _, cx| {
                this.worktree_order = action.order;
                this.update_menus(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, action: &SetWorktreeNames, _, cx| {
                this.worktree_names = action.names;
                this.update_menus(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, action: &SetScope, _, cx| {
                let scope = action.scope;
                this.change_attention(|attention| attention.scope = scope, cx)
            }))
            .on_action(cx.listener(|this, _: &AllAuthors, _, cx| {
                this.change_attention(|attention| attention.authors.clear(), cx)
            }))
            .on_action(cx.listener(|this, action: &ToggleBranch, _, cx| {
                this.toggle_branch(&action.name, cx)
            }))
            .on_action(cx.listener(|this, _: &ResetBranches, _, cx| {
                this.change_attention(
                    |attention| {
                        attention.pinned.clear();
                        attention.hidden.clear();
                    },
                    cx,
                )
            }))
            // ⌘A, with nothing more particular in focus, picks every commit
            // of the selection again, as one picked in the list narrows it.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;
                // Esc closes an open menu.
                if keystroke.key == "escape"
                    && (this.context_menu.is_some() || this.dropdown.is_some())
                {
                    this.context_menu = None;
                    this.dropdown = None;
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                // Text fields keep ⌘A to themselves.
                let typing = [
                    &this.prompt_focus,
                    &this.goto_focus,
                    &this.attention_search_focus,
                    &this.prune_focus,
                    &this.reword_focus,
                ]
                .iter()
                .any(|focus| focus.is_focused(window));
                // Arrows move about the graph while nothing else has the keys;
                // ⌥← and ⌥→ through the selected node's commits.
                let modifiers = keystroke.modifiers;
                let alt = modifiers.alt && !modifiers.platform && !modifiers.control;
                let arrow = matches!(keystroke.key.as_str(), "left" | "right" | "up" | "down");
                if arrow && this.focus.is_focused(window) && this.context_menu.is_none() {
                    match (
                        modifiers.modified(),
                        alt && !modifiers.shift,
                        keystroke.key.as_str(),
                    ) {
                        (false, _, key) => drop(this.step_node(key, cx)),
                        (_, true, "left") => this.step_commit(false, cx),
                        (_, true, "right") => this.step_commit(true, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                    return;
                }
                if keystroke.modifiers.platform && keystroke.key == "a" && !typing {
                    this.message_selection = None;
                    if !this.narrowed.is_empty() {
                        this.show_all_commits(cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .on_mouse_move(cx.listener(Self::drag_border))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    // A border let go: its panel keeps that size from now on.
                    if this.dragging.take().is_some() {
                        crate::panels::save(this.panel_sizes());
                    }
                    // The view stays where the drag left it.
                    if this.graph_drag.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &Zoom, window, _| window.zoom_window())
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_sm()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .when(self.repo_path.is_some(), |view| {
                                view.child(self.graph_band(&theme, window, cx))
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_h_0()
                                    .child(column(self.commits_width).child(self.commit_list(cx)))
                                    .child(self.column_handle(
                                        "commits-border",
                                        Border::Commits,
                                        &theme,
                                        cx,
                                    ))
                                    .child(
                                        div().flex().flex_col().flex_1().min_w_0().h_full().child(
                                            div()
                                                .flex_1()
                                                .min_h_0()
                                                .child(self.detail(&theme, window, cx)),
                                        ),
                                    ),
                            )
                            .when(self.console_open, |view| view.child(self.dock(&theme, cx))),
                    )
                    // The open sidebar's left edge resizes it.
                    .when(
                        self.repo_path.is_some() && self.side_panel.is_some(),
                        |row| {
                            // The same grip as the graph's border, standing up.
                            row.child(side_border_handle("sidebar-border", &theme).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.dragging = Some(Border::Sidebar);
                                    cx.stop_propagation();
                                }),
                            ))
                        },
                    )
                    .when(self.repo_path.is_some(), |row| {
                        row.child(self.side_panel_view(&theme, window, cx))
                    }),
            )
            .children(self.context_menu_view(&theme, cx))
            .children(self.dropdown_view(&theme, cx))
            .children(self.prompt_view(&theme, window, cx))
            .children(self.goto_view(&theme, window, cx))
            .children(self.stash_picker_view(&theme, window, cx))
            .children(self.prune_view(&theme, window, cx))
    }
}

/// What each repository had selected, by node keys: a line a repository,
/// its path then its keys, tab-separated.
const SELECTIONS: &str = "selections";

fn saved_selection(repo: &str) -> Vec<String> {
    recent::load_list(SELECTIONS)
        .iter()
        .find_map(|line| {
            let mut fields = line.split('\t');
            (fields.next() == Some(repo)).then(|| fields.map(str::to_string).collect())
        })
        .unwrap_or_default()
}

/// Saves a repository's selection, the most recent first, for some
/// repositories; a selection that cannot be saved only starts over.
fn save_selection(repo: &str, keys: &[&str]) {
    let mut lines = recent::load_list(SELECTIONS);
    lines.retain(|line| line.split('\t').next() != Some(repo));
    if !keys.is_empty() {
        lines.insert(
            0,
            std::iter::once(repo)
                .chain(keys.iter().copied())
                .collect::<Vec<_>>()
                .join("\t"),
        );
    }
    lines.truncate(50);
    recent::save_list(SELECTIONS, &lines);
}

/// The folders of a snapshot's files, each with whether a file in it changed.
fn snapshot_folders(entries: &[ChangedFile]) -> HashMap<String, bool> {
    let mut folders: HashMap<String, bool> = HashMap::new();
    for entry in entries {
        let path = entry.path();
        let mut at = 0;
        while let Some(slash) = path[at..].find('/') {
            at += slash + 1;
            let changed = folders.entry(path[..at].to_string()).or_default();
            *changed |= !entry.status.is_empty();
        }
    }
    folders
}

/// Space between the side strip's boxes, and above the first.
const STRIP_GAP: f32 = 6.;

impl RepoView {
    /// Switches between the changes and the snapshot; a snapshot opens on
    /// the file in view, or the first changed one.
    fn set_snapshot_mode(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        // A snapshot shows one file at a time, picked in the files panel.
        if on && self.side_panel != Some(SidePanel::Files) {
            self.side_panel = Some(SidePanel::Files);
            self.update_menus(cx);
        }
        self.snapshot_mode = on;
        if on && self.snapshot.path.is_none() {
            let first = self
                .selected_file
                .and_then(|i| self.files.get(i))
                .or_else(|| self.files.first())
                .map(|file| file.path().to_string());
            if let Some(path) = first {
                self.open_snapshot_file(path);
            }
        }
        if !on {
            self.focus.focus(window);
        }
        self.update_menus(cx);
        cx.notify();
    }

    /// The state a snapshot shows: after the commits picked in the list,
    /// else after the selection, or a worktree's files on disk.
    fn snapshot_revision(&self) -> Option<Revision> {
        // A commit stepped to is shown at once, before the list narrows.
        if let Some(id) = &self.pending_commit {
            return Some(Revision::Commit(id.clone()));
        }
        commit_range(&self.commits, &self.narrowed)
            .or_else(|| self.range.clone())
            .map(|(_, tip)| tip)
    }

    /// Lists the snapshot's files and reads the open one, when the state
    /// or the file changed, or a worktree's files did. What was read last
    /// stays in view meanwhile.
    fn sync_snapshot(&mut self, cx: &mut Context<Self>) {
        if !self.snapshot_mode {
            return;
        }
        let (Some(repo), Some(revision)) = (self.repo_path.clone(), self.snapshot_revision())
        else {
            return;
        };
        let stale = std::mem::take(&mut self.snapshot.stale);
        self.sync_search(cx);
        let listed = self.snapshot.revision.as_ref() == Some(&revision) && !stale;
        if !listed && self.snapshot.loading.as_ref() != Some(&revision) {
            self.snapshot.loading = Some(revision.clone());
            let (repo, wanted, listing) = (repo.clone(), revision.clone(), revision.clone());
            let load = cx.background_executor().spawn(async move {
                let files = wtf_core::snapshot_files(&repo, &wanted);
                (files, wtf_core::snapshot_submodules(&repo, &wanted))
            });
            self.snapshot_tasks[0] = Some(cx.spawn(async move |this, cx| {
                let (files, submodules) = load.await;
                this.update(cx, |this, cx| {
                    if this.snapshot.loading.as_ref() == Some(&listing) {
                        this.snapshot.loading = None;
                        this.snapshot.files = files.unwrap_or_default();
                        this.snapshot.submodules = submodules.into_iter().collect();
                        this.snapshot.revision = Some(listing);
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
        let Some(path) = self.snapshot.path.clone() else {
            return;
        };
        // A commit stepped to beyond the graph is selected once it is in.
        if let Some(id) = self.snapshot.pending.clone()
            && let Some(node) = self.node_of_commit(&id)
        {
            self.snapshot.pending = None;
            self.select_commit(node, id, cx);
        }
        self.sync_history(&repo, &revision, &path, cx);
        let wanted = (revision.clone(), path.clone());
        let read = self
            .snapshot
            .text
            .as_ref()
            .is_some_and(|(r, p, _)| (r, p) == (&wanted.0, &wanted.1))
            && !stale;
        if read || self.snapshot.text_loading.as_ref() == Some(&wanted) {
            return;
        }
        self.snapshot.text_loading = Some(wanted.clone());
        let load = cx
            .background_executor()
            .spawn(async move { wtf_core::snapshot_file(&repo, &revision, &path) });
        self.snapshot_tasks[1] = Some(cx.spawn(async move |this, cx| {
            let text = load.await.map_err(message);
            this.update(cx, |this, cx| {
                if this.snapshot.text_loading.as_ref() == Some(&wanted) {
                    this.snapshot.text_loading = None;
                    let (revision, path) = wanted;
                    this.keep_reading_position(&path, &text);
                    let found = this
                        .snapshot
                        .found_line
                        .as_ref()
                        .is_some_and(|(p, _)| *p == path);
                    this.snapshot.text = Some((revision, path, text));
                    if found {
                        this.scroll_to_found();
                    }
                    this.snapshot.colors.replace(None);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Whether the snapshot's text of `path` is of the state now selected.
    fn snapshot_text_is_current(&self, path: &str) -> bool {
        let revision = self.snapshot_revision();
        self.snapshot
            .text
            .as_ref()
            .is_some_and(|(r, p, _)| p == path && Some(r) == revision.as_ref())
    }

    /// The lines of `path`, as it is after the selection, that the selection
    /// added or changed: from the diff the review loaded for it.
    fn changed_lines(&self, path: &str) -> HashSet<u32> {
        let index = self.files.iter().position(|file| file.path() == path);
        let diff = match index {
            Some(i) if self.selected_file == Some(i) && self.diff.is_some() => self.diff.as_ref(),
            Some(i) => self
                .stacked_diffs
                .get(i)
                .and_then(|diff| diff.as_ref().ok()),
            None => None,
        };
        diff.into_iter()
            .flat_map(|diff| &diff.rows)
            .filter(|row| diff_view::is_change(row))
            .filter_map(|row| row.right.as_ref().map(|side| side.line_number))
            .collect()
    }

    /// Reads which commits changed the open file along the selected line,
    /// when the file, the line or the state changed.
    fn sync_history(
        &mut self,
        repo: &str,
        revision: &Revision,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        let (dir, at) = match revision {
            Revision::Commit(id) => (repo.to_string(), id.clone()),
            Revision::Worktree(worktree) => (worktree.clone(), "HEAD".to_string()),
        };
        // Stepping through the file's own history keeps the line it was read
        // along, so its count holds; selecting elsewhere reads it anew.
        let kept_tip = self
            .snapshot
            .history
            .as_ref()
            .filter(|((d, _, _, p), history, _)| *d == dir && p == path && history.contains(&at))
            .map(|((_, tip, _, _), _, _)| tip.clone());
        let tip = kept_tip
            .or_else(|| self.line_tip())
            .unwrap_or_else(|| at.clone());
        let key: HistoryKey = (dir, tip, at, path.to_string());
        let read = self
            .snapshot
            .history
            .as_ref()
            .is_some_and(|(k, _, _)| *k == key);
        if read || self.snapshot.history_loading.as_ref() == Some(&key) {
            return;
        }
        self.snapshot.history_loading = Some(key.clone());
        let wanted = key.clone();
        let load = cx.background_executor().spawn(async move {
            let (dir, tip, at, file) = wanted;
            wtf_core::file_history(&dir, &tip, &at, &file)
        });
        self.snapshot_tasks[2] = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                if this.snapshot.history_loading.as_ref() == Some(&key) {
                    this.snapshot.history_loading = None;
                    let (history, at) = result.unwrap_or_default();
                    this.snapshot.history = Some((key, history, at));
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// The newest commit of the selected node's line: its lane followed to
    /// its end, so later changes to the file can be stepped to.
    fn line_tip(&self) -> Option<String> {
        let (mut at, node) = self.single_node()?;
        if node.kind == wtf_core::NodeKind::Uncommitted {
            return None;
        }
        while let Some(next) = self.graph.nodes.iter().position(|child| {
            child.kind != wtf_core::NodeKind::Uncommitted
                && child.lane == node.lane
                && child.parents.first() == Some(&at)
        }) {
            at = next;
        }
        Some(self.graph.nodes[at].commits.last()?.id.clone())
    }

    /// The graph node holding a commit.
    fn node_of_commit(&self, id: &str) -> Option<usize> {
        self.graph.nodes.iter().position(|node| {
            node.kind != wtf_core::NodeKind::Uncommitted
                && node.commits.iter().any(|commit| commit.id == id)
        })
    }

    /// The commits the open file changed in before and after the shown
    /// state, and which change of how many it stands at.
    fn file_steps(&self) -> Option<(Option<String>, Option<String>, usize, usize)> {
        let (_, history, at) = self.snapshot.history.as_ref()?;
        // Before the file's first change: the way on is to where it is added.
        let Some(at) = *at else {
            return Some((None, history.last().cloned(), 0, history.len()));
        };
        let current = match self.snapshot_revision()? {
            Revision::Commit(id) => Some(id),
            Revision::Worktree(_) => None,
        };
        let on_change = current.as_deref() == history.get(at).map(String::as_str);
        let older = match on_change {
            true => history.get(at + 1),
            false => history.get(at),
        };
        let newer = match current {
            Some(_) => at.checked_sub(1).and_then(|newer| history.get(newer)),
            None => None,
        };
        Some((
            older.cloned(),
            newer.cloned(),
            history.len() - at,
            history.len(),
        ))
    }

    /// Steps to the commit the open file changed in before or after this
    /// state: selects it, so the graph and the commit list follow.
    fn step_file(&mut self, older: bool, cx: &mut Context<Self>) {
        let Some((before, after, _, _)) = self.file_steps() else {
            return;
        };
        let Some(id) = (if older { before } else { after }) else {
            return;
        };
        match self.node_of_commit(&id) {
            Some(node) => self.select_commit(node, id, cx),
            // Beyond the graph: its history comes in first.
            None => {
                self.snapshot.pending = Some(id);
                if self.graph.older > 0 && !self.loading_older {
                    self.loading_older = true;
                    self.history += self.graph.older;
                    self.start_refresh(cx);
                }
            }
        }
        cx.notify();
    }

    /// The bar over a snapshot's file: its path, and buttons to the commits
    /// it changed in before and after.
    fn file_steps_bar(&self, path: &str, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let steps = self.file_steps();
        let (older, newer) = steps
            .as_ref()
            .map_or((false, false), |(o, n, _, _)| (o.is_some(), n.is_some()));
        let button = |id: &'static str, label: &'static str, on: bool, older: bool| {
            div()
                .id(id)
                .px(px(8.))
                .py(px(2.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.separator)
                .when(!on, |button| {
                    button.text_color(theme.secondary).opacity(0.5)
                })
                .when(on, |button| {
                    button
                        .cursor_pointer()
                        .hover(|button| button.bg(theme.selection))
                        .on_click(cx.listener(move |this, _, _, cx| this.step_file(older, cx)))
                })
                .child(label)
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(6.))
            .border_b_1()
            .border_color(theme.separator)
            .text_sm()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(path.to_string()),
            )
            .children(steps.as_ref().map(|(_, _, at, of)| {
                div().text_xs().text_color(theme.secondary).child(match at {
                    0 => "not added yet".to_string(),
                    at => format!("change {at} of {of}"),
                })
            }))
            .child(button("file-older", "◀ Older change", older, true))
            .child(button("file-newer", "Newer change ▶", newer, false))
            .into_any_element()
    }

    /// Before another version of the open file replaces the one shown, moves
    /// the view so the line at its top stays there: carried through what
    /// changed between them, so the reader keeps their place.
    fn keep_reading_position(&self, path: &str, text: &Result<String, String>) {
        let (Some((_, shown, Ok(old))), Ok(new)) = (&self.snapshot.text, text) else {
            return;
        };
        if shown != path || old == new {
            return;
        }
        let handle = &self.snapshot_scroll.0.borrow().base_handle;
        let offset = handle.offset();
        let row = diff_view::row_height();
        let scrolled = f32::from(-offset.y).max(0.);
        let top = (scrolled / row) as usize;
        let within = scrolled - top as f32 * row;
        let carried = wtf_core::carry_line(old, new, top);
        handle.set_offset(point(offset.x, px(-(carried as f32 * row + within))));
    }

    /// Opens a file in the snapshot, with the folders on the way to it.
    fn open_snapshot_file(&mut self, path: String) {
        let mut at = 0;
        while let Some(slash) = path[at..].find('/') {
            at += slash + 1;
            self.snapshot.open_folders.insert(path[..at].to_string());
        }
        self.snapshot.path = Some(path);
    }

    /// The selection's changes by path: how each file changed.
    fn changed_paths(&self) -> HashMap<String, String> {
        if self.files_task.is_some() {
            return self.snapshot.marks.borrow().clone();
        }
        let marks: HashMap<String, String> = self
            .files
            .iter()
            .map(|file| (file.path().to_string(), file.status.clone()))
            .collect();
        self.snapshot.marks.replace(marks.clone());
        marks
    }

    /// The snapshot's files as the tree lists them, with how the
    /// selection changed them; files it deleted are listed too.
    fn snapshot_entries(&self) -> Vec<ChangedFile> {
        let changed = self.changed_paths();
        let deleted = changed
            .iter()
            .filter(|(_, status)| status.as_str() == "D")
            .map(|(path, _)| path.clone());
        // The open file stays listed, there or not, so it is never lost.
        let open = self.snapshot.path.clone();
        let mut paths: Vec<String> = self
            .snapshot
            .files
            .iter()
            .cloned()
            .chain(deleted)
            .chain(open)
            .collect();
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .filter(|path| {
                let open = self.snapshot.path.as_ref() == Some(path);
                open || !self.snapshot.only_changed || changed.contains_key(path)
            })
            .map(|path| ChangedFile {
                status: changed.get(&path).cloned().unwrap_or_default(),
                old_path: None,
                new_path: Some(path),
                old_id: String::new(),
                new_id: String::new(),
                worktree: None,
                submodule: false,
            })
            .collect()
    }

    /// Folders of the snapshot tree that are closed: those without changes,
    /// unless opened, and those with changes the user closed.
    /// `folders` are the tree's, from [`snapshot_folders`]: worked out once
    /// per drawing, as they take a walk over every file.
    fn snapshot_closed(&self, folders: &HashMap<String, bool>) -> HashSet<String> {
        // Folders open and close by hand only, so stepping through versions
        // leaves the tree as it is: closed at first, but for the way to the
        // open file. A click turns one from how it starts.
        folders
            .keys()
            .filter(|folder| !self.snapshot.open_folders.contains(*folder))
            .cloned()
            .collect()
    }

    /// The Files panel in Snapshot mode: a search, and the whole tree with
    /// the selection's changes marked, or what the search found.
    fn snapshot_list(&self, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let focused = self.snapshot_search_focus.is_focused(window);
        let only = self.snapshot.only_changed;
        let count = {
            let entries = self.snapshot_entries();
            let closed = self.snapshot_closed(&snapshot_folders(&entries));
            file_tree(&entries, None, &closed).len()
        };
        let list = uniform_list(
            "snapshot-files",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                let theme = Theme::for_appearance(window.appearance());
                let entries = this.snapshot_entries();
                // Once for all the rows drawn, not once a row.
                let folders = snapshot_folders(&entries);
                let closed = this.snapshot_closed(&folders);
                let rows = file_tree(&entries, None, &closed);
                range
                    .filter_map(|k| rows.get(k).map(|row| (k, row)))
                    .map(|(k, row)| {
                        let item = div()
                            .id(("snapshot-row", k))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .w_full()
                            .h(px(FILE_ROW_HEIGHT))
                            .pr(px(8.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .cursor_pointer()
                            .hover(|item| item.bg(theme.selection.opacity(0.5)));
                        let indent = |depth: usize| px(8. + depth as f32 * 14.);
                        match row {
                            TreeRow::Dir { key, label, depth } => {
                                let closed = closed.contains(key);
                                // Something changed inside: a mark, the
                                // folder stays as it is.
                                let changed = folders.get(key).copied().unwrap_or(false);
                                let key = key.clone();
                                item.pl(indent(*depth))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.snapshot.open_folders.remove(&key) {
                                            this.snapshot.open_folders.insert(key.clone());
                                        }
                                        cx.notify();
                                    }))
                                    .child(panel_chevron(!closed, &theme))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_color(theme.secondary)
                                            .child(label.clone()),
                                    )
                                    .when(changed, |item| {
                                        item.child(
                                            div()
                                                .flex_none()
                                                .size(px(6.))
                                                .rounded(px(3.))
                                                .bg(theme.accent),
                                        )
                                    })
                            }
                            TreeRow::File { index, name, depth } => {
                                let file = &entries[*index];
                                let path = file.path().to_string();
                                let open = this.snapshot.path.as_deref() == Some(path.as_str());
                                let deleted = file.status == "D";
                                let missing = this.snapshot.revision.is_some()
                                    && !deleted
                                    && !this.snapshot.files.contains(&path);
                                let submodule = this.snapshot.submodules.contains(&path);
                                let open_path = path.clone();
                                item.pl(indent(*depth))
                                    .when(open, |item| item.bg(theme.selection))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_snapshot_file(path.clone());
                                        this.snapshot_scroll.scroll_to_item(0, ScrollStrategy::Top);
                                        // Its changes load too, for its tints.
                                        let index =
                                            this.files.iter().position(|file| file.path() == path);
                                        if index.is_some() && this.selected_file != index {
                                            this.select_file(index, cx);
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .w(px(10.))
                                            .font_family(preferences::code_font())
                                            .font_weight(preferences::code_weight())
                                            .text_xs()
                                            .text_color(theme.accent)
                                            .child(match submodule {
                                                true => "⊞".to_string(),
                                                false => file.status.clone(),
                                            }),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .when(file.status.is_empty(), |name| {
                                                name.text_color(theme.secondary)
                                            })
                                            .when(deleted, |name| name.line_through())
                                            .when(missing, |name| {
                                                name.italic().text_color(theme.secondary)
                                            })
                                            .child(name.clone()),
                                    )
                                    // A submodule opens as a tab of its own.
                                    .when(submodule, |item| {
                                        item.child(div().flex_1()).child(
                                            div()
                                                .id(("snapshot-open-sub", k))
                                                .flex_none()
                                                .text_xs()
                                                .text_color(theme.accent)
                                                .cursor_pointer()
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.open_submodule(&open_path, cx)
                                                }))
                                                .child("Open"),
                                        )
                                    })
                            }
                            TreeRow::Section(..) => item,
                        }
                    })
                    .collect()
            }),
        )
        .flex_1()
        .py(px(4.));
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .p(px(6.))
                    .border_b_1()
                    .border_color(theme.separator)
                    .child(
                        div()
                            .id("snapshot-search")
                            .track_focus(&self.snapshot_search_focus)
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                                let keystroke = &event.keystroke;
                                match keystroke.key.as_str() {
                                    "escape" if this.snapshot.search.text().is_empty() => {
                                        this.focus.focus(window);
                                    }
                                    "escape" => this.snapshot.search.edit.set_text(""),
                                    "enter" => {}
                                    _ if this.snapshot.search.key(keystroke, cx) => {}
                                    _ => return,
                                }
                                this.snapshot.found_line = None;
                                this.sync_search(cx);
                                cx.stop_propagation();
                                cx.notify();
                            }))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.snapshot_search_focus.focus(window);
                                cx.notify();
                            }))
                            .flex_1()
                            .min_w_0()
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .px(px(6.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if focused {
                                theme.accent
                            } else {
                                theme.separator
                            })
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(self.snapshot.search.render(
                                "snapshot-search-text",
                                "Search",
                                focused,
                                true,
                                Vec::new(),
                                &self.snapshot_search_focus,
                                theme,
                                |this: &mut Self| Some(&mut this.snapshot.search),
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .id("snapshot-only-changed")
                            .flex_none()
                            .px(px(6.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme.separator)
                            .text_xs()
                            .cursor_pointer()
                            .when(only, |toggle| toggle.bg(theme.selection))
                            .hover(|toggle| toggle.bg(theme.selection))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.snapshot.only_changed = !this.snapshot.only_changed;
                                cx.notify();
                            }))
                            .child("Changed"),
                    ),
            )
            .child(match self.snapshot.search.text().trim().is_empty() {
                true => list.into_any_element(),
                false => self.search_list(cx),
            })
            .into_any_element()
    }

    /// The open file, whole, as it is in the snapshot; while another state
    /// is read, the last one read stays.
    fn snapshot_view(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(revision) = self.snapshot_revision() else {
            return placeholder("Nothing selected", None, theme);
        };
        let Some(path) = self.snapshot.path.clone() else {
            let hint = "Pick a file in the Files panel (⌘R).";
            return placeholder("No file open", Some(hint.into()), theme);
        };
        let text = match &self.snapshot.text {
            Some((_, shown, text)) if *shown == path => text,
            _ => return placeholder("Loading…", None, theme),
        };
        let listed = self.snapshot.revision.as_ref() == Some(&revision);
        let missing = listed && !self.snapshot.files.contains(&path);
        let text = match (text, missing) {
            // The file stays open where it is missing: it is added later, or
            // was deleted by here.
            (_, true) => {
                let not_yet = self
                    .snapshot
                    .history
                    .as_ref()
                    .is_some_and(|(_, history, at)| at.is_none() && !history.is_empty());
                let (title, hint) = match not_yet {
                    true => (
                        "Doesn't exist yet",
                        format!("{path} is added in a later commit: Newer change ▶ goes there."),
                    ),
                    false => (
                        "Deleted",
                        format!("{path} was deleted before this version."),
                    ),
                };
                return placeholder(title, Some(hint), theme);
            }
            (Err(error), _) => return placeholder(&path, Some(error.clone()), theme),
            (Ok(text), _) => text.clone(),
        };
        let lines: Rc<Vec<String>> = Rc::new(text.lines().map(str::to_string).collect());
        let dark = theme.background.l < 0.5;
        let colors = {
            let cached = self.snapshot.colors.borrow().clone();
            match cached {
                Some((made_dark, colors)) if made_dark == dark => colors,
                _ => {
                    let diff = FileDiff {
                        language: String::new(),
                        rows: lines
                            .iter()
                            .enumerate()
                            .map(|(i, line)| wtf_core::DiffRow {
                                left: None,
                                right: Some(wtf_core::DiffSide {
                                    line_number: i as u32 + 1,
                                    text: line.clone(),
                                    changes: Vec::new(),
                                }),
                            })
                            .collect(),
                    };
                    let colors = Rc::new(syntax::colors(&path, &diff, dark));
                    self.snapshot.colors.replace(Some((dark, colors.clone())));
                    colors
                }
            }
        };
        let foreground = theme.foreground;
        let secondary = theme.secondary;
        let inserted = theme.inserted;
        // A line a search found and opened shows marked.
        let marked = self
            .snapshot
            .found_line
            .as_ref()
            .filter(|(found, _)| *found == path)
            .map(|(_, line)| *line);
        let selection = theme.selection;
        // Lines the selection added or changed, from its diff of the file,
        // when the state shown is the selection's own.
        let fresh = self.snapshot_text_is_current(&path)
            && self.files_task.is_none()
            && !self.is_loading_stacked
            && !self.is_loading_diff;
        let kept = self.snapshot.tints.borrow().clone();
        let changed: Rc<HashSet<u32>> = match (fresh, kept) {
            (true, _) => {
                let lines = Rc::new(self.changed_lines(&path));
                self.snapshot
                    .tints
                    .replace(Some((path.clone(), lines.clone())));
                lines
            }
            (false, Some((shown, lines))) if shown == path => lines,
            (false, _) => Rc::new(HashSet::new()),
        };
        uniform_list(
            "snapshot-text",
            lines.len(),
            cx.processor(move |_, range: std::ops::Range<usize>, _, _| {
                range
                    .map(|i| {
                        let line = &lines[i];
                        let spans = colors.right.get(&(i as u32 + 1));
                        let highlights: Vec<_> = spans
                            .into_iter()
                            .flatten()
                            .filter(|(range, _)| {
                                range.end <= line.len()
                                    && line.is_char_boundary(range.start)
                                    && line.is_char_boundary(range.end)
                            })
                            .map(|(range, color)| {
                                (
                                    range.clone(),
                                    HighlightStyle {
                                        color: Some(*color),
                                        ..Default::default()
                                    },
                                )
                            })
                            .collect();
                        div()
                            .flex()
                            .w_full()
                            .h(px(diff_view::row_height()))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .when(changed.contains(&(i as u32 + 1)), |row| row.bg(inserted))
                            .when(marked == Some(i as u32 + 1), |row| row.bg(selection))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(diff_view::gutter_width()))
                                    .pr(px(8.))
                                    .text_right()
                                    .text_color(secondary)
                                    .child((i + 1).to_string()),
                            )
                            .child(
                                div().text_color(foreground).child(
                                    StyledText::new(line.clone()).with_highlights(highlights),
                                ),
                            )
                    })
                    .collect()
            }),
        )
        .track_scroll(self.snapshot_scroll.clone())
        .size_full()
        .font_family(preferences::code_font())
        .font_weight(preferences::code_weight())
        .text_size(px(preferences::code_size()))
        .line_height(px(diff_view::row_height()))
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use wtf_core::{DiffRow, DiffSide, TextRange};

    use super::stack::stack_items;
    use super::*;

    fn diff(changed: &[bool]) -> FileDiff {
        let rows = changed
            .iter()
            .map(|&changed| {
                let side = DiffSide {
                    line_number: 1,
                    text: String::new(),
                    changes: if changed {
                        vec![TextRange { start: 0, end: 1 }]
                    } else {
                        Vec::new()
                    },
                };
                DiffRow {
                    left: Some(side.clone()),
                    right: Some(side),
                }
            })
            .collect();
        FileDiff {
            language: "Text".to_string(),
            rows,
        }
    }

    fn commit(id: &str, parent: &str) -> Commit {
        Commit {
            id: id.to_string(),
            short_id: id.to_string(),
            summary: String::new(),
            body: String::new(),
            author: String::new(),
            time: 0,
            parent: Some(parent.to_string()),
        }
    }

    #[test]
    fn selected_commits_span_from_the_oldest_parent_to_the_newest() {
        let commits = [commit("c3", "c2"), commit("c2", "c1"), commit("c1", "c0")];
        assert_eq!(commit_range(&commits, &[]), None);
        assert_eq!(
            commit_range(&commits, &[2, 0]),
            Some((Some("c0".to_string()), Revision::Commit("c3".to_string())))
        );
        assert_eq!(
            commit_range(&commits, &[1]),
            Some((Some("c1".to_string()), Revision::Commit("c2".to_string())))
        );
    }

    #[test]
    fn double_click_picks_a_word() {
        let text = "Fix the parser-bug now";
        assert_eq!(word_around(text, 10), (8, 18));
        assert_eq!(&text[8..18], "parser-bug");
        assert_eq!(word_around(text, 0), (0, 3));
    }

    #[test]
    fn branch_names_nest_by_folder() {
        let names = ["feature/parser", "bot", "feature/lexer", "origin/x/y/z"];
        let show = |rows: Vec<NameRow>| -> Vec<String> {
            rows.into_iter()
                .map(|row| match row {
                    NameRow::Folder { label, depth, .. } => format!("{depth}{label}"),
                    NameRow::Leaf { name, depth, .. } => format!("{depth}{name}"),
                })
                .collect()
        };
        assert_eq!(
            show(name_tree(&names, &HashSet::new(), false)),
            [
                "0feature/",
                "1parser",
                "1lexer",
                "0origin/x/y/",
                "1z",
                "0bot"
            ]
        );
        let folded = HashSet::from(["feature/".to_string()]);
        assert_eq!(
            show(name_tree(&names, &folded, false)),
            ["0feature/", "0origin/x/y/", "1z", "0bot"]
        );
    }

    #[test]
    fn mixed_trees_sort_folders_and_files_together() {
        let names = ["a.rs", "b/c.rs", "z.rs", "d/e.rs"];
        let show = |mixed: bool| -> Vec<String> {
            name_tree(&names, &HashSet::new(), mixed)
                .into_iter()
                .map(|row| match row {
                    NameRow::Folder { label, .. } => label,
                    NameRow::Leaf { name, .. } => name,
                })
                .collect()
        };
        assert_eq!(show(false), ["b/", "c.rs", "d/", "e.rs", "a.rs", "z.rs"]);
        assert_eq!(show(true), ["a.rs", "b/", "c.rs", "d/", "e.rs", "z.rs"]);
    }

    #[test]
    fn file_tree_nests_folders_and_joins_lone_ones() {
        let file = |path: &str| ChangedFile {
            status: "M".to_string(),
            old_path: None,
            new_path: Some(path.to_string()),
            old_id: String::new(),
            new_id: String::new(),
            worktree: None,
            submodule: false,
        };
        let files = [
            file("app/src/main.rs"),
            file("README.md"),
            file("app/src/view/a.rs"),
            file("core/src/lib/x/y.rs"),
        ];
        let show = |rows: Vec<TreeRow>| -> Vec<String> {
            rows.into_iter()
                .map(|row| match row {
                    TreeRow::Section(staged, count) => format!("{staged}:{count}"),
                    TreeRow::Dir { label, depth, .. } => format!("{depth}{label}/"),
                    TreeRow::File { name, depth, .. } => format!("{depth}{name}"),
                })
                .collect()
        };
        let none = HashSet::new();
        // Folders and files together by name, as Settings have it at first.
        assert_eq!(
            show(file_tree(&files, None, &none)),
            [
                "0README.md",
                "0app/src/",
                "1main.rs",
                "1view/",
                "2a.rs",
                "0core/src/lib/x/",
                "1y.rs"
            ]
        );
        let folded = HashSet::from(["staged:app/src/".to_string()]);
        assert_eq!(
            show(file_tree(&files[..2], Some(1), &folded)),
            ["false:1", "0app/src/", "1main.rs", "true:1", "0README.md"]
        );
    }

    #[test]
    fn staging_splits_files_into_sections() {
        let diffs = vec![Ok(diff(&[true])), Ok(diff(&[true])), Ok(diff(&[true]))];
        let kinds: Vec<String> = stack_items(
            &diffs,
            &[true, true, true],
            Some(2),
            [false; 2],
            false,
            None,
        )
        .iter()
        .map(|item| match item {
            StackItem::Section(staged, count) => format!("{staged}:{count}"),
            StackItem::Header(file) => format!("H{file}"),
            _ => "?".to_string(),
        })
        .collect();
        assert_eq!(
            kinds,
            [
                "?", "false:2", "?", "H0", "?", "H1", "?", "true:1", "?", "H2"
            ]
        );

        let folded_unstaged: Vec<String> = stack_items(
            &diffs,
            &[true, true, true],
            Some(2),
            [true, false],
            false,
            None,
        )
        .iter()
        .map(|item| match item {
            StackItem::Section(staged, count) => format!("{staged}:{count}"),
            StackItem::Header(file) => format!("H{file}"),
            _ => "?".to_string(),
        })
        .collect();
        assert_eq!(folded_unstaged, ["?", "false:2", "?", "true:1", "?", "H2"]);

        // Each hunk of a worktree file gets a header before its rows.
        let open: Vec<String> =
            stack_items(&diffs[2..], &[false], Some(0), [false; 2], false, None)
                .iter()
                .map(|item| match item {
                    StackItem::Section(staged, count) => format!("{staged}:{count}"),
                    StackItem::Header(file) => format!("H{file}"),
                    StackItem::Hunk(file, start, end) => format!("@{file}:{start}-{end}"),
                    StackItem::Row(file, row, _) => format!("{file}:{row}"),
                    _ => "?".to_string(),
                })
                .collect();
        assert_eq!(
            open,
            ["?", "false:0", "?", "true:1", "?", "H0", "@0:0-1", "0:0"]
        );

        // Too many files to open at once: a note says why, and every file
        // keeps its header, folded.
        let many = stack_items(&diffs, &[true; 3], Some(2), [false; 2], true, None);
        assert!(matches!(&many[0], StackItem::Note(note) if note.starts_with("3 files")));
        let headers = many
            .iter()
            .filter(|item| matches!(item, StackItem::Header(_)))
            .count();
        assert_eq!(headers, 3);
        assert!(!many.iter().any(|item| matches!(item, StackItem::Row(..))));

        let nothing_staged = stack_items(&diffs[..1], &[true], Some(1), [false; 2], false, None);
        assert!(matches!(
            nothing_staged.last(),
            Some(StackItem::Section(true, 0))
        ));
    }

    #[test]
    fn aggregates_authors_and_time_span() {
        let now = 10_000_000;
        let mut commits = [commit("c3", "c2"), commit("c2", "c1"), commit("c1", "c0")];
        let days = [1, 3, 3];
        let authors = ["Bot", "Ann", "Bot"];
        for ((commit, days), author) in commits.iter_mut().zip(days).zip(authors) {
            commit.time = now - days * 86_400;
            commit.author = author.to_string();
        }
        let refs: Vec<&Commit> = commits.iter().collect();
        assert_eq!(
            aggregate(&refs, now),
            "3 commits · Bot, Ann · 3 days ago – 1 day ago"
        );
    }

    #[test]
    fn dates_read_as_local_date_and_time() {
        // The hours depend on the time zone here; the shape does not.
        let text = local_datetime(1_700_000_000);
        let shape: String = text
            .chars()
            .map(|c| if c.is_ascii_digit() { '9' } else { c })
            .collect();
        assert_eq!(shape, "9999-99-99 99:99");
        assert!(text.starts_with("2023-11-1"));
    }

    #[test]
    fn short_ages_fit_a_list() {
        let now = 100_000_000;
        assert_eq!(short_age(now - 120, now), "2m");
        assert_eq!(short_age(now - 5 * 3_600, now), "5h");
        assert_eq!(short_age(now - 3 * 86_400, now), "3d");
        assert_eq!(short_age(now - 21 * 86_400, now), "3w");
        assert_eq!(short_age(now - 400 * 86_400, now), "1y");
    }

    #[test]
    fn ages_read_like_people_say_them() {
        let now = 10_000_000;
        assert_eq!(relative_time(now - 5, now), "just now");
        assert_eq!(relative_time(now - 60, now), "1 minute ago");
        assert_eq!(relative_time(now - 3 * 86_400, now), "3 days ago");
        assert_eq!(relative_time(now - 400 * 86_400, now), "1 year ago");
    }

    #[test]
    fn stacks_headers_hunks_and_notes() {
        let mut changed = vec![false; 10];
        changed[8] = true;
        let diffs = vec![
            Ok(diff(&changed)),
            Ok(diff(&[false, false])),
            Err("binary".to_string()),
        ];
        let kinds = |folded: &[bool]| -> Vec<String> {
            stack_items(&diffs, folded, None, [false; 2], false, None)
                .iter()
                .map(|item| match item {
                    StackItem::Section(staged, count) => format!("S{staged}:{count}"),
                    StackItem::Header(file) => format!("H{file}"),
                    StackItem::Row(file, row, _) => format!("{file}:{row}"),
                    StackItem::Hunk(..) => "@".to_string(),
                    StackItem::Gap => "…".to_string(),
                    StackItem::Note(text) => text.clone(),
                    StackItem::HeaderPad => " ".to_string(),
                })
                .collect()
        };
        assert_eq!(
            kinds(&[true, true, false]),
            [" ", "H0", " ", "H1", " ", "H2", "binary"]
        );
        assert_eq!(
            kinds(&[]),
            [
                " ",
                "H0",
                "…",
                "0:5",
                "0:6",
                "0:7",
                "0:8",
                "0:9",
                " ",
                "H1",
                "No visible changes",
                " ",
                "H2",
                "binary"
            ]
        );
    }
}
