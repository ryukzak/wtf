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
    Attention, ChangedFile, Commit, CoreError, DiffMode, FileDiff, LogEntry, Revision, Scope,
    WorkflowGraph,
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

const CONTEXT_ROWS: usize = 3;
const REFRESH: Duration = Duration::from_secs(2);
const MAX_GRAPH_HEIGHT: f32 = 300.;
/// The graph's starting height when it has few lanes, such as one branch.
const MIN_GRAPH_HEIGHT: f32 = 160.;
const DOCK_HEIGHT: f32 = 260.;
const STATUS_HEIGHT: f32 = 24.;
const MIN_PANEL_HEIGHT: f32 = 60.;
/// Room left for the commit, file and diff panes when panels grow.
const MIN_MAIN_HEIGHT: f32 = 200.;
const MIN_COLUMN_WIDTH: f32 = 120.;
const MIN_DIFF_WIDTH: f32 = 300.;
const MAX_LOG: usize = 5000;
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
/// How many nodes back the commit list offers under a node's commits.
const EARLIER_NODES: usize = 3;
const PROMPT_WIDTH: f32 = 420.;
const ATTENTION_PANEL_WIDTH: f32 = 240.;
const MIN_SIDEBAR_WIDTH: f32 = 180.;
const MAX_SIDEBAR_WIDTH: f32 = 500.;
/// The closed sidebar: a strip holding its toggle.
/// Room for the strip's boxes and a margin on either side of them.
const SIDEBAR_STRIP_WIDTH: f32 = 34.;
/// The all-files view falls back to one file at a time above this.
const MAX_STACKED_FILES: usize = 100;
/// The commit header shows this much of a message body.
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
        ShowOneFile,
        ShowAllFiles,
        Minimize,
        Zoom,
    ]
);

/// How the diff pane shows the changed files.
#[derive(Clone, Copy, PartialEq)]
enum ChangesLayout {
    OneFile,
    /// All files in one scrollable list, like Sublime Merge.
    AllFiles,
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

/// A folder of changed files while the tree is built.
#[derive(Default)]
struct Folder {
    folders: std::collections::BTreeMap<String, Folder>,
    files: Vec<(String, usize)>,
}

/// The file list's rows: folders first, then files, by name; a worktree's
/// files under their sections. Folded folders hide what is in them.
fn file_tree(
    files: &[ChangedFile],
    split: Option<usize>,
    collapsed: &HashSet<String>,
) -> Vec<TreeRow> {
    fn emit(
        folder: &Folder,
        path: &str,
        depth: usize,
        section: &str,
        collapsed: &HashSet<String>,
        rows: &mut Vec<TreeRow>,
    ) {
        for (name, mut inner) in &folder.folders {
            let mut label = name.clone();
            while inner.files.is_empty() && inner.folders.len() == 1 {
                let (next, deeper) = inner.folders.iter().next().unwrap();
                label = format!("{label}/{next}");
                inner = deeper;
            }
            let path = format!("{path}{label}/");
            let key = format!("{section}{path}");
            let open = !collapsed.contains(&key);
            rows.push(TreeRow::Dir { key, label, depth });
            if open {
                emit(inner, &path, depth + 1, section, collapsed, rows);
            }
        }
        for (name, index) in &folder.files {
            rows.push(TreeRow::File {
                index: *index,
                name: name.clone(),
                depth,
            });
        }
    }
    let group = |range: std::ops::Range<usize>| {
        let mut indices: Vec<usize> = range.collect();
        indices.sort_by_key(|&index| display_path(&files[index]));
        let mut root = Folder::default();
        for index in indices {
            let path = display_path(&files[index]);
            let mut parts: Vec<&str> = path.split('/').collect();
            let name = parts.pop().unwrap_or_default().to_string();
            let mut folder = &mut root;
            for part in parts {
                folder = folder.folders.entry(part.to_string()).or_default();
            }
            folder.files.push((name, index));
        }
        root
    };
    let mut rows = Vec::new();
    match split {
        None => emit(&group(0..files.len()), "", 0, "", collapsed, &mut rows),
        Some(split) => {
            let split = split.min(files.len());
            for (staged, range) in [(false, 0..split), (true, split..files.len())] {
                rows.push(TreeRow::Section(staged, range.len()));
                let section = if staged { "staged:" } else { "unstaged:" };
                emit(&group(range), "", 0, section, collapsed, &mut rows);
            }
        }
    }
    rows
}

/// A row of the all-files view.
enum StackItem {
    /// Staged or unstaged files, with how many.
    Section(bool, usize),
    Header(usize),
    Row(usize, usize),
    /// A hunk of a worktree file: its first row and the row after its last.
    Hunk(usize, usize, usize),
    Gap,
    Note(String),
    /// The row a section's header reaches up into, as a file's does.
    Space,
    /// The row a file's header reaches up into: headers are two rows tall.
    HeaderPad,
}

/// A panel border being dragged.
#[derive(Clone, Copy)]
enum Border {
    Graph,
    Dock,
    Sidebar,
    Commits,
}

/// The panels the strip at the right edge opens, one at a time.
#[derive(Clone, Copy, PartialEq)]
enum SidePanel {
    Attention,
    /// The changed files as a tree: a table of contents for all files, or
    /// the file to show one at a time.
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

/// Branch names as a tree of their `/` folders: folders first, by name, then
/// branches in the given order; folders in `folded` hide what is in them.
fn name_tree(names: &[&str], folded: &HashSet<String>) -> Vec<NameRow> {
    #[derive(Default)]
    struct Node {
        folders: std::collections::BTreeMap<String, Node>,
        leaves: Vec<(String, usize)>,
    }
    fn emit(
        node: &Node,
        path: &str,
        depth: usize,
        folded: &HashSet<String>,
        rows: &mut Vec<NameRow>,
    ) {
        for (name, mut inner) in &node.folders {
            let mut label = format!("{name}/");
            while inner.leaves.is_empty() && inner.folders.len() == 1 {
                let (next, deeper) = inner.folders.iter().next().unwrap();
                label = format!("{label}{next}/");
                inner = deeper;
            }
            let path = format!("{path}{label}");
            let open = !folded.contains(&path);
            rows.push(NameRow::Folder {
                path: path.clone(),
                label,
                depth,
            });
            if open {
                emit(inner, &path, depth + 1, folded, rows);
            }
        }
        for (name, index) in &node.leaves {
            rows.push(NameRow::Leaf {
                index: *index,
                name: name.clone(),
                depth,
            });
        }
    }
    let mut root = Node::default();
    for (index, name) in names.iter().enumerate() {
        let mut parts: Vec<&str> = name.split('/').collect();
        let leaf = parts.pop().unwrap_or_default().to_string();
        let mut node = &mut root;
        for part in parts {
            node = node.folders.entry(part.to_string()).or_default();
        }
        node.leaves.push((leaf, index));
    }
    let mut rows = Vec::new();
    emit(&root, "", 0, folded, &mut rows);
    rows
}

/// An attention checkbox: on, off, or `None` for a folder partly on.
fn attention_checkbox(
    id: impl Into<gpui::ElementId>,
    state: Option<bool>,
    enabled: bool,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    div()
        .id(id.into())
        .flex_none()
        .w(px(14.))
        .text_color(match (enabled, state) {
            (false, _) => theme.separator,
            (true, Some(false)) => theme.secondary,
            (true, _) => theme.accent,
        })
        .when(enabled, |checkbox| checkbox.cursor_pointer())
        .child(match state {
            Some(true) => "☑",
            Some(false) => "☐",
            None => "⊟",
        })
}

/// A group of branches in the attention sidebar.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Section {
    Worktrees,
    Local,
    Remote,
    Tags,
    Stashes,
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
    input: String,
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
}

/// Code colors are cached by path, appearance and all-files entry.
type ColorKey = (String, bool, Option<usize>);

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
    /// Scrolls the graph to its newest end on the next render.
    scroll_graph_to_end: bool,
    /// What the menu bar was last built from.
    menus_for: Option<MenuState>,
    worktree_names: WorktreeNames,
    worktree_order: WorktreeOrder,
    context_menu: Option<(Target, Point<Pixels>)>,
    prompt: Option<Prompt>,
    prompt_focus: FocusHandle,
    /// The prune list, when open: stale branches by name, and whether each is
    /// ticked to go.
    prune: Option<Vec<(String, bool)>>,
    prune_focus: FocusHandle,
    /// The Go To picker or the command palette, when open: the query, the
    /// highlighted result, and whether it lists commands.
    goto: Option<(String, usize, bool)>,
    /// The Stash picker, open on a worktree: its path and the highlighted
    /// row, the first one running the stash.
    stash_picker: Option<(String, usize)>,
    /// The flags the Stash picker has on, by their place in its list.
    stash_flags: [bool; popups::STASH_FLAGS.len()],
    stash_focus: FocusHandle,
    goto_focus: FocusHandle,
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
    attention_search: String,
    attention_search_focus: FocusHandle,
    collapsed_sections: HashSet<Section>,
    dropdown: Option<(Dropdown, Point<Pixels>)>,
    /// The graph's width when last drawn, to keep the view still as it changes.
    graph_width: Cell<f32>,
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
    changes_layout: ChangesLayout,
    stacked: Vec<StackItem>,
    stacked_diffs: Vec<Result<FileDiff, String>>,
    /// For a selected worktree, where staged files start in `files`.
    worktree_split: Option<usize>,
    /// Diff rows picked for staging, all in one file.
    line_selection: Option<(usize, Vec<usize>)>,
    line_anchor: Option<usize>,
    commit_message: String,
    /// Where the commit message's cursor is, as a byte offset; `None` at
    /// its end.
    message_cursor: Option<usize>,
    /// The commit message as last laid out, to place the cursor by a click
    /// and move it between lines.
    message_layout: RefCell<Option<gpui::TextLayout>>,
    commit_focus: FocusHandle,
    /// A commit picked in the graph, to select once its node's commits load.
    pending_commit: Option<String>,
    commit_scroll: UniformListScrollHandle,
    /// The commit box rewrites the worktree's latest commit instead.
    amend: bool,
    /// The model is writing a commit message for an empty box, and whether
    /// a commit waits for it.
    writing_message: bool,
    commit_when_written: bool,
    /// The message the model last put in the commit box, which it may
    /// replace as staging goes on until the user edits it; what was staged
    /// then; and the wait before writing the next one.
    suggested: Option<String>,
    suggest_key: Option<String>,
    suggest_task: Option<Task<()>>,
    message_scroll: ScrollHandle,
    /// Earlier and undone commit messages, for ⌘Z and ⇧⌘Z, and whether the
    /// last change was a typed letter, which the next one joins.
    message_undo: Vec<String>,
    /// All of the commit message is selected, by ⌘A.
    message_selected: bool,
    message_redo: Vec<String>,
    typing_run: bool,
    /// A commit message being edited: the commit, the worktree whose latest
    /// commit it is, and the text so far.
    rewording: Option<(String, String, String)>,
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
    /// The unstaged and staged sections, folded to their titles.
    folded_sections: [bool; 2],
    /// Folders folded in the file list, by section and path.
    collapsed_dirs: HashSet<String>,
    is_loading_stacked: bool,
    stacked_scroll: UniformListScrollHandle,
    /// The commit header's height, as last laid out; the diff starts below it.
    header_height: f32,
    is_loading_diff: bool,
    error_message: Option<String>,

    console_open: bool,
    hide_refresh: bool,
    log: Vec<LogEntry>,
    log_seq: u64,
    /// The directory of the last command printed in the terminal.
    announced_dir: Option<String>,
    dock_height: f32,
    terminal: Option<Entity<TerminalView>>,
    /// Set by dragging the graph's border; otherwise the graph fits its rows.
    graph_height: Option<f32>,
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
    /// When the remotes were last fetched, and why the last fetch failed.
    fetched_at: Option<i64>,
    fetch_error: Option<String>,
    commits_task: Option<Task<()>>,
    files_task: Option<Task<()>>,
    diff_task: Option<Task<()>>,
    stacked_task: Option<Task<()>>,
    diff_scroll: UniformListScrollHandle,
}

fn display_path(file: &ChangedFile) -> &str {
    file.new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .unwrap_or_default()
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
        Self {
            focus,
            active: false,
            repo_path: None,
            graph: WorkflowGraph::default(),
            graph_error: None,
            combine_commits: true,
            attention: Attention::default(),
            graph_scroll: ScrollHandle::new(),
            scroll_graph_to_end: false,
            menus_for: None,
            worktree_names: WorktreeNames::FolderAndBranch,
            worktree_order: WorktreeOrder::LastCommit,
            context_menu: None,
            prompt: None,
            prompt_focus: cx.focus_handle(),
            goto: None,
            stash_picker: None,
            stash_flags: [true, false, false],
            stash_focus: cx.focus_handle(),
            goto_focus: cx.focus_handle(),
            prune: None,
            prune_focus: cx.focus_handle(),
            open_with: popups::load_recent("open-with"),
            shell_commands: popups::load_recent("commands"),
            completions: Vec::new(),
            side_panel: None,
            last_panel: SidePanel::Attention,
            sidebar_width: ATTENTION_PANEL_WIDTH,
            attention_search: String::new(),
            attention_search_focus: cx.focus_handle(),
            collapsed_sections: HashSet::new(),
            dropdown: None,
            graph_width: Cell::new(0.),
            outside_graph: false,
            stash_view: None,
            reveal_stash: false,
            hovered_line: Cell::new(None),
            syntax: RefCell::new(HashMap::new()),
            graph_drag: None,
            scroll_to_end: Cell::new(true),
            graph_away: Cell::new((false, false)),
            scroll_to_start: Cell::new(false),
            history: 0,
            loading_older: false,
            pull: Cell::new(None),
            at_left_edge: false,
            reveal: Cell::new(None),
            selected_nodes: Vec::new(),
            hovered_node: None,
            range: None,
            commits: Vec::new(),
            narrowed: Vec::new(),
            commit_anchor: None,
            files: Vec::new(),
            diff: None,
            selected_file: None,
            diff_mode: DiffMode::Text,
            changes_layout: ChangesLayout::AllFiles,
            stacked: Vec::new(),
            stacked_diffs: Vec::new(),
            worktree_split: None,
            line_selection: None,
            line_anchor: None,
            commit_message: String::new(),
            message_cursor: None,
            message_layout: RefCell::new(None),
            commit_focus: cx.focus_handle(),
            pending_commit: None,
            commit_scroll: UniformListScrollHandle::new(),
            amend: false,
            writing_message: false,
            commit_when_written: false,
            suggested: None,
            suggest_key: None,
            suggest_task: None,
            message_scroll: ScrollHandle::new(),
            message_undo: Vec::new(),
            message_selected: false,
            message_redo: Vec::new(),
            typing_run: false,
            rewording: None,
            message_selection: None,
            summaries: HashMap::new(),
            show_subjects: false,
            selecting_message: false,
            message_focus: cx.focus_handle(),
            copied: None,
            reword_focus: cx.focus_handle(),
            folded: HashSet::new(),
            folded_sections: [false; 2],
            collapsed_dirs: HashSet::new(),
            is_loading_stacked: false,
            stacked_scroll: UniformListScrollHandle::new(),
            header_height: 0.,
            is_loading_diff: false,
            error_message: None,
            console_open: false,
            hide_refresh: true,
            log: Vec::new(),
            log_seq: 0,
            announced_dir: None,
            dock_height: DOCK_HEIGHT,
            terminal: None,
            graph_height: None,
            dragging: None,
            commits_width: 300.,
            refresh_task: None,
            fetch_task: None,
            sync_task: None,
            push_remote: None,
            fetched_at: None,
            fetch_error: None,
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

    pub fn repo_name(&self) -> Option<String> {
        self.repo_path.as_deref().map(workspace::folder_name)
    }

    /// The workspace shows one tab at a time; the active one takes focus and
    /// the menu bar.
    pub fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.active = active;
        if active {
            self.focus.focus(window);
            self.menus_for = None;
            self.update_menus(cx);
        }
    }

    pub fn open(&mut self, path: String, cx: &mut Context<Self>) {
        self.repo_path = Some(path.clone());
        self.graph = WorkflowGraph::default();
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
        recent::remember(&path);
        self.scroll_graph_to_end = true;
        self.update_menus(cx);
        self.start_refresh(cx);
        cx.notify();
    }

    /// Rebuilds the graph now and then every few seconds; unchanged graphs are ignored.
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
                let result =
                    cx.background_executor()
                        .spawn(async move {
                            wtf_core::workflow_graph(&path, &attention, combine, history)
                        })
                        .await;
                let applied = this.update(cx, |this, cx| {
                    this.apply_graph(result, cx);
                    this.fetch_when_due(cx);
                    // Shows the refresh commands in an open console.
                    if this.console_open {
                        cx.notify();
                    }
                });
                if applied.is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH).await;
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
        let Some(path) = self.repo_path.clone() else {
            return;
        };
        let prune = preferences::fetch_prune();
        let load = cx
            .background_executor()
            .spawn(async move { wtf_core::fetch(&path, prune, background) });
        self.fetch_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                this.fetch_task = None;
                this.fetched_at = Some(now_seconds());
                this.fetch_error = result.err().map(message);
                this.start_refresh(cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Runs a Pull or Push option in the worktree commands run in, then
    /// rebuilds the graph to show what moved.
    pub fn sync(&mut self, push: bool, option: usize, cx: &mut Context<Self>) {
        let options: &[crate::sync::SyncOption] = match push {
            true => &crate::sync::PUSH,
            false => &crate::sync::PULL,
        };
        let (Some(path), Some(option)) = (self.command_path(), options.get(option)) else {
            return;
        };
        if self.sync_task.is_some() {
            return;
        }
        let args = option.args;
        let fetch = args.first() == Some(&"fetch");
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
                if fetch {
                    this.fetched_at = Some(now_seconds());
                    this.fetch_error = result.err().map(message);
                }
                // A failure shows in red in the status bar, as Git said it.
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

    fn change_attention(&mut self, change: impl FnOnce(&mut Attention), cx: &mut Context<Self>) {
        change(&mut self.attention);
        // The sidebar's checkmarks follow at once; the graph catches up when the
        // rebuild finishes.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_secs() as i64);
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

    fn attention_summary(&self) -> String {
        let attention = &self.attention;
        let mut parts = vec![match (attention.scope, attention.window_days) {
            (Scope::Worktrees, _) => "worktrees only".to_string(),
            (Scope::Branches, Some(1)) => "last day".to_string(),
            (Scope::Branches, Some(days)) => format!("last {days} days"),
            (Scope::Branches, None) => "all time".to_string(),
        }];
        if !attention.authors.is_empty() {
            parts.push(attention.authors.join(", "));
        }
        if !attention.pinned.is_empty() {
            parts.push(format!("{} pinned", attention.pinned.len()));
        }
        if !attention.hidden.is_empty() {
            parts.push(format!("{} hidden", attention.hidden.len()));
        }
        let branches = &self.graph.branches;
        let shown = branches.iter().filter(|branch| branch.shown).count();
        format!(
            "Attention: {shown} of {} branches · {}",
            branches.len(),
            parts.join(" · ")
        )
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
        // The First and Latest buttons come and go as the view leaves either end.
        if self.graph_away_now() != self.graph_away.get() {
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

    /// Whether the graph shows the First and Latest buttons.
    fn graph_away_now(&self) -> (bool, bool) {
        let full_width = self.graph_width.get() + 2. * graph_view::MARGIN;
        let view = f32::from(self.graph_scroll.bounds().size.width);
        let scrolled = -f32::from(self.graph_scroll.offset().x);
        let shown = view > 0. && !self.graph.nodes.is_empty();
        // First at the left edge, while older commits are still to load;
        // Latest only when the newest work is more than two views away.
        (
            shown && scrolled <= 2. && self.graph.older > 0,
            shown && full_width - view - scrolled > 2. * view,
        )
    }

    /// Loads the whole history of the main branch and scrolls to its first
    /// commit.
    fn go_to_first_commit(&mut self, cx: &mut Context<Self>) {
        self.scroll_to_start.set(true);
        if self.graph.older > 0 && !self.loading_older {
            self.loading_older = true;
            self.history += self.graph.older;
            self.start_refresh(cx);
        }
        cx.notify();
    }

    /// Brings older commits into the graph, a step further back each time.
    pub fn load_older(&mut self, cx: &mut Context<Self>) {
        if self.loading_older || self.graph.older == 0 {
            return;
        }
        self.loading_older = true;
        self.history += HISTORY_STEP;
        self.start_refresh(cx);
        cx.notify();
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
        let keys: Vec<String> = self
            .selected_nodes
            .iter()
            .map(|&i| self.graph.nodes[i].key.clone())
            .collect();
        self.graph = graph;
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
                let keep = self
                    .selected_file
                    .map(|i| display_path(&self.files[i]).to_string());
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
            short_id: stash.id.chars().take(7).collect(),
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
        self.worktree_split = None;
        self.line_selection = None;
        self.outside_graph = false;
        self.stash_view = None;
        self.range = self.graph.range(&selected);
        self.reveal.set(selected.last().copied());
        self.selected_nodes = selected;
        self.narrowed.clear();
        self.commit_anchor = None;
        self.commits.clear();
        self.commits_task = None;
        self.pending_commit = None;
        self.files.clear();
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
    }

    /// Narrows the diff to one listed commit.
    fn narrow_to(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(index) = self.commits.iter().position(|commit| commit.id == id) else {
            return;
        };
        self.narrowed = vec![index];
        self.commit_anchor = Some(index);
        self.commit_scroll
            .scroll_to_item(index, ScrollStrategy::Center);
        self.reload_files(cx);
    }

    /// Widens the diff back to the whole selection of nodes.
    fn show_all_commits(&mut self, cx: &mut Context<Self>) {
        self.narrowed.clear();
        self.commit_anchor = None;
        self.reload_files(cx);
    }

    fn reload_files(&mut self, cx: &mut Context<Self>) {
        self.files.clear();
        self.select_file(None, cx);
        self.load_files(None, cx);
        cx.notify();
    }

    /// Loads the changed files, keeping `keep` selected if it is still there.
    fn load_files(&mut self, keep: Option<String>, cx: &mut Context<Self>) {
        let range = commit_range(&self.commits, &self.narrowed).or_else(|| self.range.clone());
        let (Some(repo_path), Some((base, tip))) = (self.repo_path.clone(), range) else {
            return;
        };
        // A selected worktree alone shows its changes split for staging.
        let worktree = self.staging_worktree();
        let stash = self.stash_view.clone();
        let load = cx.background_executor().spawn(async move {
            if let Some(stash) = stash {
                return wtf_core::stash_files(&repo_path, &stash)
                    .map(|(files, split)| (files, Some(split)));
            }
            match worktree {
                Some(path) => wtf_core::worktree_status(&path).map(|status| {
                    let split = status.unstaged.len();
                    let mut files = status.unstaged;
                    files.extend(status.staged);
                    (files, Some(split))
                }),
                None => wtf_core::changed_files(&repo_path, base.as_deref(), &tip)
                    .map(|files| (files, None)),
            }
        });
        self.files_task = Some(cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok((files, split)) => {
                        this.files = files;
                        this.worktree_split = split;
                        this.error_message = None;
                        let kept = keep.and_then(|path| {
                            this.files
                                .iter()
                                .position(|file| display_path(file) == path)
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

    /// In the all-files view, selecting a file scrolls to it.
    fn select_file(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        self.selected_file = index;
        if !self.is_stacked() {
            self.load_diff(false, cx);
            return;
        }
        if self.file_by_file_worktree() {
            self.restack();
            self.stacked_scroll.scroll_to_item(0, ScrollStrategy::Top);
            cx.notify();
            return;
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

    fn set_changes_layout(&mut self, layout: ChangesLayout, cx: &mut Context<Self>) {
        if self.changes_layout != layout {
            self.changes_layout = layout;
            // File by file picks files in the files panel.
            if layout == ChangesLayout::OneFile {
                self.side_panel = Some(SidePanel::Files);
                self.update_menus(cx);
            }
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

    /// File by file in a worktree: the list picks the file, and its changes
    /// show in the staging view, under the sections, one file at a time.
    fn file_by_file_worktree(&self) -> bool {
        self.changes_layout == ChangesLayout::OneFile && self.worktree_split.is_some()
    }

    fn is_stacked(&self) -> bool {
        let wanted =
            self.changes_layout == ChangesLayout::AllFiles || self.worktree_split.is_some();
        wanted
            && (!self.files.is_empty() || self.worktree_split.is_some())
            && self.files.len() <= MAX_STACKED_FILES
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

    /// Stages or unstages files, all of them when `files` is empty, then
    /// shows the result.
    fn stage(&mut self, staged: bool, files: Vec<String>, cx: &mut Context<Self>) {
        let Some(path) = self.staging_worktree() else {
            return;
        };
        let run = cx.background_executor().spawn(async move {
            match staged {
                true => wtf_core::unstage(&path, &files),
                false => wtf_core::stage(&path, &files),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = run.await;
            this.update(cx, |this, cx| this.after_change(result, cx))
                .ok();
        })
        .detach();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let message = self.commit_message.trim().to_string();
        let staged = self
            .worktree_split
            .is_some_and(|split| split < self.files.len());
        let amend = self.amend;
        // With no message typed, the commit waits for the model to write one
        // from what is staged.
        if message.is_empty() && staged && !amend && preferences::summaries() {
            self.commit_when_written = true;
            if !self.writing_message {
                self.suggest_key = None;
                self.suggest_message(cx);
            }
            return;
        }
        let Some(path) = self
            .staging_worktree()
            .filter(|_| (staged || amend) && !message.is_empty())
        else {
            return;
        };
        let run = cx
            .background_executor()
            .spawn(async move { wtf_core::commit(&path, &message, amend) });
        cx.spawn(async move |this, cx| {
            let result = run.await;
            this.update(cx, |this, cx| {
                if result.is_ok() {
                    this.commit_message.clear();
                    this.amend = false;
                    (this.suggested, this.suggest_key) = (None, None);
                    this.message_undo.clear();
                    this.message_redo.clear();
                }
                this.after_change(result, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Keeps a message from the model in the commit box as staging goes on:
    /// once what is staged settles for a moment, it writes one for it. A
    /// message the user typed or edited stays.
    fn suggest_message(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.staging_worktree() else {
            return;
        };
        let own = !self.commit_message.trim().is_empty()
            && self.suggested.as_ref() != Some(&self.commit_message);
        if !preferences::summaries() || own || self.amend {
            return;
        }
        let split = self.worktree_split.unwrap_or(self.files.len());
        let key: Vec<String> = self.files[split.min(self.files.len())..]
            .iter()
            .map(|file| format!("{}:{}", display_path(file), file.new_id))
            .collect();
        if key.is_empty() {
            if self.suggested.take().as_ref() == Some(&self.commit_message) {
                self.commit_message.clear();
            }
            (self.suggest_key, self.suggest_task) = (None, None);
            self.writing_message = false;
            return;
        }
        let key = key.join(",");
        if self.suggest_key.as_ref() == Some(&key) {
            return;
        }
        self.suggest_key = Some(key);
        self.writing_message = true;
        self.suggest_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SUGGEST_DELAY).await;
            let prompt = preferences::message_prompt();
            let write = cx
                .background_executor()
                .spawn(async move { wtf_core::write_commit_message(&path, &prompt) });
            let written = write.await;
            this.update(cx, |this, cx| {
                this.writing_message = false;
                let own = !this.commit_message.trim().is_empty()
                    && this.suggested.as_ref() != Some(&this.commit_message);
                match (written, own) {
                    (Ok(message), false) => {
                        this.set_message(message.clone(), false);
                        this.suggested = Some(message);
                        // A commit asked for with the box empty goes now.
                        if std::mem::take(&mut this.commit_when_written) {
                            this.commit(cx);
                        }
                    }
                    (Err(error), _) if this.commit_when_written => {
                        this.commit_when_written = false;
                        this.error_message = Some(message(error));
                    }
                    _ => {}
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// A click picks one line, Shift-click a range and Cmd-click toggles one;
    /// picking in another file starts over.
    fn select_line(
        &mut self,
        file: usize,
        row: usize,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let rows = match &mut self.line_selection {
            Some((f, rows)) if *f == file => rows,
            _ => {
                self.line_selection = Some((file, vec![row]));
                self.line_anchor = Some(row);
                cx.notify();
                return;
            }
        };
        match self.line_anchor {
            Some(anchor) if modifiers.shift => {
                *rows = (anchor.min(row)..=anchor.max(row)).collect()
            }
            _ if modifiers.platform => match rows.iter().position(|&r| r == row) {
                Some(at) => {
                    rows.remove(at);
                }
                None => rows.push(row),
            },
            _ if *rows == [row] => rows.clear(),
            _ => *rows = vec![row],
        }
        if !modifiers.shift {
            self.line_anchor = Some(row);
        }
        if rows.is_empty() {
            self.line_selection = None;
        }
        cx.notify();
    }

    /// Stages, or unstages, just the picked lines of their file.
    fn stage_selected_lines(&mut self, staged: bool, cx: &mut Context<Self>) {
        if let Some((index, rows)) = self.line_selection.take() {
            self.stage_rows(index, rows, staged, cx);
        }
    }

    /// Stages, or unstages, the changes on some diff rows of a file.
    fn stage_rows(&mut self, index: usize, rows: Vec<usize>, staged: bool, cx: &mut Context<Self>) {
        let (Some(worktree), Some(repo)) = (self.staging_worktree(), self.repo_path.clone()) else {
            return;
        };
        let Some((file, removed, added)) = self.picked_lines(index, &rows) else {
            return;
        };
        let run = cx.background_executor().spawn(async move {
            wtf_core::stage_lines(&repo, &worktree, &file, &removed, &added, staged)
        });
        cx.spawn(async move |this, cx| {
            let result = run.await;
            this.update(cx, |this, cx| this.after_change(result, cx))
                .ok();
        })
        .detach();
    }

    /// A file's changed lines among some diff rows: removed ones by their old
    /// numbers and added ones by their new numbers.
    fn picked_lines(
        &self,
        index: usize,
        rows: &[usize],
    ) -> Option<(ChangedFile, HashSet<u32>, HashSet<u32>)> {
        let (Some(file), Some(Ok(diff))) = (self.files.get(index), self.stacked_diffs.get(index))
        else {
            return None;
        };
        let (mut removed, mut added) = (HashSet::new(), HashSet::new());
        for row in rows.iter().filter_map(|&r| diff.rows.get(r)) {
            if let Some(left) = &row.left
                && (row.right.is_none() || !left.changes.is_empty())
            {
                removed.insert(left.line_number);
            }
            if let Some(right) = &row.right
                && (row.left.is_none() || !right.changes.is_empty())
            {
                added.insert(right.line_number);
            }
        }
        Some((file.clone(), removed, added))
    }

    /// Throws away unstaged lines, once asked: some rows of a file, or the
    /// picked lines when `rows` is empty.
    fn discard_rows(
        &mut self,
        index: usize,
        rows: Vec<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = match (rows.is_empty(), &self.line_selection) {
            (true, Some((file, picked))) if *file == index => picked.clone(),
            _ => rows,
        };
        let (Some(worktree), Some(repo)) = (self.staging_worktree(), self.repo_path.clone()) else {
            return;
        };
        let Some((file, removed, added)) = self.picked_lines(index, &rows) else {
            return;
        };
        let count = removed.len() + added.len();
        let question = format!(
            "Discard {count} changed line{} in {}?",
            if count == 1 { "" } else { "s" },
            display_path(&file)
        );
        self.confirm_then(question, window, cx, move |this, cx| {
            let run = cx.background_executor().spawn(async move {
                wtf_core::discard_lines(&repo, &worktree, &file, &removed, &added)
            });
            cx.spawn(async move |this, cx| {
                let result = run.await;
                this.update(cx, |this, cx| this.after_change(result, cx))
                    .ok();
            })
            .detach();
            this.line_selection = None;
        });
    }

    /// Throws away a file's unstaged changes, once asked.
    fn discard_file(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(worktree), Some(file)) =
            (self.staging_worktree(), self.files.get(index).cloned())
        else {
            return;
        };
        let question = format!("Discard the changes to {}?", display_path(&file));
        self.confirm_then(question, window, cx, move |_, cx| {
            let run = cx
                .background_executor()
                .spawn(async move { wtf_core::discard(&worktree, &[file]) });
            cx.spawn(async move |this, cx| {
                let result = run.await;
                this.update(cx, |this, cx| this.after_change(result, cx))
                    .ok();
            })
            .detach();
        });
    }

    /// Asks before throwing changes away, then does it.
    fn confirm_then(
        &mut self,
        question: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        let detail = "The changes are not staged or committed anywhere, so they cannot be undone.";
        let answer = window.prompt(
            PromptLevel::Warning,
            &question,
            Some(detail),
            &["Discard", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| then(this, cx)).ok();
            }
        })
        .detach();
    }

    /// Reloads the files and the graph after staging or committing.
    fn after_change(&mut self, result: Result<(), CoreError>, cx: &mut Context<Self>) {
        self.line_selection = None;
        if let Err(error) = result {
            self.error_message = Some(message(error));
        }
        let keep = self
            .selected_file
            .and_then(|i| self.files.get(i))
            .map(|f| display_path(f).to_string());
        self.load_files(keep, cx);
        self.start_refresh(cx);
        cx.notify();
    }

    fn commit_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        match keystroke.key.as_str() {
            "enter" if modifiers.platform => self.commit(cx),
            "escape" => self.focus.focus(window),
            "z" if modifiers.platform && modifiers.shift => self.redo_message(),
            "z" if modifiers.platform => self.undo_message(),
            // The box's text as a whole: ⌘A selects it, ⌘C copies and ⌘X cuts
            // it.
            "a" if modifiers.platform => self.message_selected = true,
            "c" if modifiers.platform => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.commit_message.clone()))
            }
            "x" if modifiers.platform => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.commit_message.clone()));
                self.set_message(String::new(), false);
                self.message_selected = false;
            }
            "up" | "down" => {
                self.message_selected = false;
                self.cursor_line(keystroke.key == "down");
            }
            _ => {
                // Typing, pasting or deleting over a selection replaces it;
                // moving the cursor drops the selection.
                let moving = matches!(keystroke.key.as_str(), "left" | "right" | "home" | "end");
                let replace = self.message_selected && !moving;
                let (mut text, mut cursor) = match replace {
                    true => (String::new(), 0),
                    false => (self.commit_message.clone(), self.cursor()),
                };
                let paste = || cx.read_from_clipboard().and_then(|item| item.text());
                let deleting = matches!(keystroke.key.as_str(), "backspace" | "delete");
                if !(edit_key(&mut text, &mut cursor, keystroke, paste) || deleting && replace) {
                    return;
                }
                self.message_selected = false;
                if text == self.commit_message {
                    self.message_cursor = Some(cursor);
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                // The user writes: a message the model is still writing is
                // dropped.
                if self.writing_message {
                    (
                        self.suggest_task,
                        self.writing_message,
                        self.commit_when_written,
                    ) = (None, false, false);
                }
                // Letters typed in a row undo together; anything else is a
                // step of its own.
                let letter = keystroke
                    .key_char
                    .as_deref()
                    .is_some_and(|c| c.chars().all(char::is_alphanumeric))
                    && !modifiers.platform;
                let at_end = cursor == text.len();
                self.set_message(text, letter);
                self.message_cursor = Some(cursor);
                if at_end {
                    self.message_scroll.scroll_to_bottom();
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Changes the commit message, keeping what it was to undo; `letter`
    /// joins a run of typed letters into one step.
    fn set_message(&mut self, text: String, letter: bool) {
        if text == self.commit_message {
            return;
        }
        if !(letter && self.typing_run) {
            let before = std::mem::take(&mut self.commit_message);
            self.message_undo.push(before);
        }
        self.typing_run = letter;
        self.message_redo.clear();
        self.commit_message = text;
        self.message_cursor = None;
    }

    /// The commit message's cursor, on a character of the text.
    fn cursor(&self) -> usize {
        let text = &self.commit_message;
        let mut at = self.message_cursor.unwrap_or(text.len()).min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    /// Moves the cursor a line up or down, by where the text lies on screen.
    fn cursor_line(&mut self, down: bool) {
        let Some(layout) = self.message_layout.borrow().clone() else {
            return;
        };
        let cursor = self.cursor();
        let Some(at) = layout.position_for_index(cursor) else {
            return;
        };
        let line = layout.line_height();
        let target = point(
            at.x + px(1.),
            at.y + if down { line * 1.5 } else { -line * 0.5 },
        );
        let shown = layout.index_for_position(target).unwrap_or_else(|i| i);
        self.message_cursor = Some(from_shown(shown, cursor));
    }

    fn undo_message(&mut self) {
        if let Some(before) = self.message_undo.pop() {
            let now = std::mem::replace(&mut self.commit_message, before);
            self.message_redo.push(now);
            self.typing_run = false;
            self.message_cursor = None;
        }
    }

    fn redo_message(&mut self) {
        if let Some(after) = self.message_redo.pop() {
            let now = std::mem::replace(&mut self.commit_message, after);
            self.message_undo.push(now);
            self.typing_run = false;
            self.message_cursor = None;
        }
    }

    /// The commit message with its cursor where it stands; a click places
    /// the cursor.
    fn message_text(&self, focused: bool, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if self.commit_message.is_empty() {
            self.message_layout.replace(None);
            let placeholder = match self.writing_message {
                true => "",
                false => "Commit message",
            };
            return field_text("", placeholder, focused, theme);
        }
        let cursor = self.cursor();
        let mut shown = self.commit_message.clone();
        let mut highlights = Vec::new();
        if focused {
            shown.insert_str(cursor, CARET);
            highlights.push((
                cursor..cursor + CARET.len(),
                HighlightStyle {
                    color: Some(theme.accent),
                    ..Default::default()
                },
            ));
        }
        let text = StyledText::new(shown).with_highlights(highlights);
        self.message_layout.replace(Some(text.layout().clone()));
        div()
            .id("commit-message-text")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    let Some(layout) = this.message_layout.borrow().clone() else {
                        return;
                    };
                    let shown = layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|i| i);
                    let cursor = this.cursor();
                    let focused = this.commit_focus.is_focused(window);
                    this.message_cursor = Some(match focused {
                        true => from_shown(shown, cursor),
                        false => shown,
                    });
                    this.message_selected = false;
                    this.commit_focus.focus(window);
                    cx.notify();
                }),
            )
            .child(text)
            .into_any_element()
    }

    /// The commit box holds a message the model wrote, untouched.
    fn suggesting(&self) -> bool {
        !self.commit_message.is_empty() && self.suggested.as_ref() == Some(&self.commit_message)
    }

    /// Turns amending on, with the latest commit's message to edit when none
    /// is typed, or off, dropping that message if it is unchanged.
    fn toggle_amend(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.staging_worktree() else {
            return;
        };
        let head = wtf_core::head_message(&path).unwrap_or_default();
        self.amend = !self.amend;
        if self.amend && self.commit_message.trim().is_empty() {
            self.commit_message = head;
        } else if !self.amend && self.commit_message.trim() == head.trim() {
            self.commit_message.clear();
        }
        cx.notify();
    }

    /// A commit's message as text to select, its subject in bold; a
    /// right-click opens the commit's menu.
    fn selectable_message(
        &self,
        commit: &Commit,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut text = commit.summary.clone();
        if !commit.body.is_empty() {
            text.push_str("\n\n");
            text.push_str(&commit.body);
        }
        let summary = commit.summary.len();
        let styles = vec![
            (
                0..summary,
                HighlightStyle {
                    font_weight: Some(gpui::FontWeight::SEMIBOLD),
                    ..Default::default()
                },
            ),
            (
                summary..text.len(),
                HighlightStyle {
                    color: Some(theme.secondary),
                    ..Default::default()
                },
            ),
        ];
        let menu = Target::Commit {
            commit: commit.id.clone(),
            message: text.clone(),
        };
        self.selectable_text(commit.id.clone(), text, styles, Some(menu), theme, cx)
    }

    /// Text to select: drag, double-click a word or ⌘A, then ⌘C copies it.
    /// `key` tells one text from another, so a selection stays with its own;
    /// `styles` cover the text without overlapping.
    fn selectable_text(
        &self,
        key: String,
        text: String,
        styles: Vec<(std::ops::Range<usize>, HighlightStyle)>,
        menu: Option<Target>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selection = self
            .message_selection
            .as_ref()
            .filter(|(id, _, _)| *id == key)
            .map(|&(_, anchor, head)| (anchor.min(head), anchor.max(head)))
            .filter(|(start, end)| start < end);
        // The styles cut where the selection starts and ends, as runs that do
        // not overlap.
        let mut cuts = vec![0, text.len()];
        cuts.extend(
            styles
                .iter()
                .flat_map(|(range, _)| [range.start, range.end]),
        );
        cuts.extend(selection.iter().flat_map(|&(start, end)| [start, end]));
        cuts.sort_unstable();
        cuts.dedup();
        let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = cuts
            .windows(2)
            .map(|cut| {
                let (start, end) = (cut[0], cut[1]);
                let mut style = styles
                    .iter()
                    .find(|(range, _)| range.start <= start && end <= range.end)
                    .map_or_else(HighlightStyle::default, |(_, style)| *style);
                if selection.is_some_and(|(from, to)| from <= start && end <= to) {
                    style.background_color = Some(theme.selection);
                }
                (start..end, style)
            })
            .collect();
        let styled = StyledText::new(text.clone()).with_highlights(highlights);
        let layout = styled.layout().clone();
        let (down_key, down_layout, move_layout) = (key.clone(), layout.clone(), layout);
        let down_text = text.clone();
        div()
            .id(SharedString::from(format!("text-{key}")))
            .track_focus(&self.message_focus)
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.message_focus.focus(window);
                    let at = down_layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|nearest| nearest);
                    let (anchor, head) = match event.click_count {
                        1 => (at, at),
                        _ => word_around(&down_text, at),
                    };
                    this.message_selection = Some((down_key.clone(), anchor, head));
                    this.selecting_message = event.click_count == 1;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                if !this.selecting_message || event.pressed_button != Some(MouseButton::Left) {
                    this.selecting_message = false;
                    return;
                }
                let at = move_layout
                    .index_for_position(event.position)
                    .unwrap_or_else(|nearest| nearest);
                if let Some((_, _, head)) = &mut this.message_selection {
                    *head = at;
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.selecting_message = false),
            )
            .when_some(menu, |text, menu| {
                text.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_context_menu(menu.clone(), event.position, cx);
                    }),
                )
            })
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                let keystroke = &event.keystroke;
                if !keystroke.modifiers.platform {
                    return;
                }
                match (keystroke.key.as_str(), selection) {
                    ("c", Some((start, end))) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text[start..end].into()))
                    }
                    ("a", _) => {
                        this.message_selection = Some((key.clone(), 0, text.len()));
                        cx.notify();
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(styled)
            .into_any_element()
    }

    /// The worktree with commit `id` in its history, back to the first
    /// merge, where its message can be rewritten.
    fn reword_worktree(&self, id: &str) -> Option<String> {
        self.graph.nodes.iter().find_map(|worktree| {
            let Revision::Worktree(path) = &worktree.tip else {
                return None;
            };
            let mut at = worktree.parents.first().copied();
            while let Some(node) = at.and_then(|i| self.graph.nodes.get(i)) {
                if node.kind != wtf_core::NodeKind::Commits {
                    break;
                }
                if node.commits.iter().any(|commit| commit.id == id) {
                    return Some(path.clone());
                }
                at = match node.parents[..] {
                    [parent] => Some(parent),
                    _ => None,
                };
            }
            None
        })
    }

    /// Copies a commit's full hash, and says so where it was clicked.
    fn copy_sha(&mut self, sha: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()));
        self.copied = Some(sha.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            this.update(cx, |this, cx| {
                if this.copied.as_ref() == Some(&sha) {
                    this.copied = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn start_reword(&mut self, commit: &Commit, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.reword_worktree(&commit.id) else {
            return;
        };
        let mut message = commit.summary.clone();
        if !commit.body.is_empty() {
            message.push_str("\n\n");
            message.push_str(&commit.body);
        }
        self.rewording = Some((commit.id.clone(), path, message));
        self.reword_focus.focus(window);
        cx.notify();
    }

    fn reword(&mut self, cx: &mut Context<Self>) {
        let Some((commit, path, message)) = self.rewording.take() else {
            return;
        };
        let message = message.trim().to_string();
        if message.is_empty() {
            return;
        }
        let run = cx
            .background_executor()
            .spawn(async move { wtf_core::reword(&path, &commit, &message) });
        cx.spawn(async move |this, cx| {
            let result = run.await;
            this.update(cx, |this, cx| this.after_change(result, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    fn reword_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let Some((_, _, message)) = &mut self.rewording else {
            return;
        };
        match keystroke.key.as_str() {
            "enter" if keystroke.modifiers.platform => self.reword(cx),
            "escape" => {
                self.rewording = None;
                self.focus.focus(window);
            }
            _ if type_key(message, keystroke, cx) => {}
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The commit message box above a worktree's changes.
    fn commit_box(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focused = self.commit_focus.is_focused(window);
        let staged = self
            .worktree_split
            .map_or(0, |split| self.files.len().saturating_sub(split));
        // An empty message is written by the model, when summaries are on.
        let generate = self.commit_message.trim().is_empty()
            && staged > 0
            && !self.amend
            && preferences::summaries();
        let ready =
            (staged > 0 || self.amend) && (!self.commit_message.trim().is_empty() || generate);
        let label = match (self.amend, staged) {
            (true, 0) => "Amend message".to_string(),
            (true, _) => format!(
                "Amend with {staged} file{}",
                if staged == 1 { "" } else { "s" }
            ),
            (false, _) => format!("Commit {staged} file{}", if staged == 1 { "" } else { "s" }),
        };
        div()
            .flex()
            .flex_none()
            .gap(px(8.))
            .p(px(8.))
            .border_b_1()
            .border_color(theme.separator)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .id("commit-message")
                            .track_focus(&self.commit_focus)
                            .on_key_down(cx.listener(Self::commit_key))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.commit_focus.focus(window);
                                this.message_selected = false;
                                cx.notify();
                            }))
                            .flex_1()
                            .min_w_0()
                            // Four lines high whatever it holds, so the view
                            // under it stays put; a longer message scrolls.
                            .h(px(4. * MESSAGE_LINE * preferences::ui_size() + 14.))
                            .line_height(rems(MESSAGE_LINE))
                            .overflow_y_scroll()
                            .track_scroll(&self.message_scroll)
                            .p(px(6.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(if focused {
                                theme.accent
                            } else {
                                theme.separator
                            })
                            // A message being rewritten fades under the
                            // writing mark.
                            .child(
                                div()
                                    .when(self.writing_message, |text| text.opacity(0.3))
                                    // All of it selected, by ⌘A.
                                    .when(self.message_selected && focused, |text| {
                                        text.bg(theme.selection).rounded(px(2.))
                                    })
                                    .child(self.message_text(focused, theme, cx)),
                            ),
                    )
                    // While the model writes, into an empty box or over its own
                    // message as staging changes: grey words and a pulsing mark
                    // in the middle.
                    .when(
                        self.writing_message
                            && (self.commit_message.is_empty() || self.suggesting()),
                        |field| {
                            field.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(6.))
                                    .text_color(theme.secondary)
                                    .child(
                                        div().child("✦").with_animation(
                                            "writing-message",
                                            Animation::new(Duration::from_millis(1200))
                                                .repeat()
                                                .with_easing(pulsating_between(0.25, 1.)),
                                            |mark, delta| mark.opacity(delta),
                                        ),
                                    )
                                    .child("Writing a commit message…"),
                            )
                        },
                    )
                    // A model at work, or its message in the box: a mark in the
                    // corner that says so on hover.
                    .children(self.suggesting().then(|| {
                        let note = "Written by Apple Intelligence from what is staged; rewritten \
                                    as staging changes until you edit it. ⌘Z brings back what was \
                                    there.";
                        div()
                            .absolute()
                            .top(px(4.))
                            .right(px(6.))
                            .group("ai-mark")
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.accent)
                                    .cursor_default()
                                    .child("✦"),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(16.))
                                    .right_0()
                                    .w(px(240.))
                                    .p(px(6.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(theme.separator)
                                    .bg(theme.background)
                                    .shadow_md()
                                    .text_xs()
                                    .invisible()
                                    .group_hover("ai-mark", |note| note.visible())
                                    .child(note),
                            )
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .items_end()
                    .gap(px(4.))
                    .child(
                        div()
                            .id("commit-button")
                            .px(px(12.))
                            .py(px(4.))
                            .rounded(px(4.))
                            .map(|button| match ready {
                                true => button
                                    .bg(theme.accent)
                                    .text_color(theme.tag_head_text)
                                    .cursor_pointer(),
                                false => button
                                    .border_1()
                                    .border_color(theme.separator)
                                    .text_color(theme.secondary),
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.commit(cx)))
                            .child(label),
                    )
                    .child(
                        div()
                            .id("amend")
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_xs()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_amend(cx)))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(12.))
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(theme.secondary)
                                    .when(self.amend, |check| {
                                        check
                                            .bg(theme.accent)
                                            .border_color(theme.accent)
                                            .text_color(theme.tag_head_text)
                                            .child("✓")
                                    }),
                            )
                            .child("Amend"),
                    )
                    // Puts all the changes, untracked files too, aside in a
                    // stash, named by the message box when it says something.
                    .child(
                        div()
                            .id("stash-changes")
                            .text_xs()
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(path) = this.staging_worktree() {
                                    let line = popups::stash_push(&path, &this.commit_message, &[]);
                                    this.run_in_terminal(line, window, cx);
                                }
                            }))
                            .child("Stash"),
                    )
                    .child(div().text_xs().text_color(theme.secondary).child("⌘↩")),
            )
    }

    /// Loads every file's diff for the all-files view.
    fn load_stacked(&mut self, refresh: bool, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        if !refresh {
            self.stacked.clear();
            self.stacked_diffs.clear();
            self.stacked_scroll.scroll_to_item(0, ScrollStrategy::Top);
            self.is_loading_stacked = true;
        }
        let (files, mode) = (self.files.clone(), self.diff_mode);
        let load = cx.background_executor().spawn(async move {
            files
                .into_iter()
                .map(|file| wtf_core::file_diff(repo_path.clone(), file, mode).map_err(message))
                .collect::<Vec<_>>()
        });
        self.stacked_task = Some(cx.spawn(async move |this, cx| {
            let diffs = load.await;
            this.update(cx, |this, cx| {
                this.is_loading_stacked = false;
                // Picked lines only stay picked while their file's diff holds.
                if let Some((file, _)) = &this.line_selection
                    && this.stacked_diffs.get(*file) != diffs.get(*file)
                {
                    this.line_selection = None;
                }
                this.stacked_diffs = diffs;
                this.syntax.borrow_mut().clear();
                this.restack();
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
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
        self.log.extend(new);
        if self.log.len() > MAX_LOG {
            self.log.drain(..self.log.len() - MAX_LOG);
        }
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
            return;
        }
        let height = f32::from(window.viewport_size().height);
        let y = f32::from(event.position.y);
        let max = (height - MIN_MAIN_HEIGHT).max(MIN_PANEL_HEIGHT);
        match border {
            Border::Dock => {
                self.dock_height = (height - y - STATUS_HEIGHT).clamp(MIN_PANEL_HEIGHT, max)
            }
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

    fn status_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let last = self
            .log
            .iter()
            .rev()
            .find(|entry| !(self.hide_refresh && entry.refresh));
        div()
            .id("status")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .px(px(12.))
            .h(px(24.))
            .border_t_1()
            .border_color(theme.separator)
            .text_xs()
            // The terminal's button, boxed like the side panels' ones, with
            // an arrow to where it goes.
            .child(
                div()
                    .id("terminal-toggle")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(4.))
                    .px(px(6.))
                    .h(px(18.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme.separator)
                    .cursor_pointer()
                    .hover(|button| button.bg(theme.selection))
                    .when(self.console_open, |button| button.bg(theme.selection))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_console(window, cx)))
                    .child(
                        div()
                            .text_color(theme.secondary)
                            .child(if self.console_open { "▾" } else { "▴" }),
                    )
                    .child("Terminal"),
            )
            .children(last.map(|entry| {
                div()
                    .truncate()
                    .font_family(preferences::code_font())
                    .text_color(if entry.ok {
                        theme.secondary
                    } else {
                        theme.error
                    })
                    .child(entry.command.clone())
            }))
            .child(div().flex_1())
            .when(self.repo_path.is_some(), |bar| {
                bar.child(
                    div()
                        .flex_none()
                        .text_color(theme.secondary)
                        .child(self.attention_summary()),
                )
                .child(self.fetch_status(theme, cx))
            })
    }

    /// The last fetch, `⟳ 3m`, or a failed one in red; a click fetches now.
    fn fetch_status(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let (text, color) = match (&self.fetch_task, &self.fetch_error, self.fetched_at) {
            (Some(_), _, _) => ("⟳ fetching…".to_string(), theme.secondary),
            (None, Some(_), _) => ("⟳ fetch failed".to_string(), theme.error),
            (None, None, Some(at)) => (
                format!("⟳ {}", short_age(at, now_seconds())),
                theme.secondary,
            ),
            (None, None, None) => ("⟳ fetch".to_string(), theme.secondary),
        };
        div()
            .id("fetch-status")
            .flex_none()
            .px(px(6.))
            .h(px(18.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|button| button.bg(theme.selection))
            .text_color(color)
            .on_click(cx.listener(|this, _, _, cx| this.fetch(false, cx)))
            .child(text)
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

    fn graph_band(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if let Some(error) = &self.graph_error {
            return placeholder("Cannot read the repository", Some(error.clone()), theme);
        }
        let layout = graph_view::layout(&self.graph, self.worktree_order);
        // When the graph grows or shrinks, as attention changes, the view keeps
        // its distance from the newest work on the right instead of jumping.
        let previous = self.graph_width.replace(layout.width);
        if previous > 0. && previous != layout.width {
            let offset = self.graph_scroll.offset();
            let x = offset.x - px(layout.width - previous);
            self.graph_scroll.set_offset(point(x.min(px(0.)), offset.y));
        }
        // A node selected from the commit list or Go To scrolls into view
        // when it is out of it; one picked in the graph already is in view.
        let view = self.graph_scroll.bounds().size;
        // A repository opens with the graph scrolled to its newest work.
        if view.width > px(0.) && !self.graph.nodes.is_empty() && self.scroll_to_end.replace(false)
        {
            let width = layout.width + 2. * graph_view::MARGIN;
            let x = (width - f32::from(view.width)).max(0.);
            self.graph_scroll.set_offset(point(px(-x), px(0.)));
        }
        if let Some((start, end, y)) =
            self.reveal
                .get()
                .filter(|_| view.width > px(0.))
                .and_then(|node| {
                    self.reveal.set(None);
                    layout.nodes.get(node).copied()
                })
        {
            // In the scrolled content, the graph sits inside its margin.
            let pad = graph_view::MARGIN;
            let (start, end, y) = (start + pad, end + pad, y + pad);
            let (full_width, full_height) = (layout.width + 2. * pad, layout.height + 2. * pad);
            let (width, height) = (f32::from(view.width), f32::from(view.height));
            let offset = self.graph_scroll.offset();
            let (left, top) = (-f32::from(offset.x), -f32::from(offset.y));
            let margin = 24.;
            let x = match start < left + margin || end > left + width - margin {
                true => ((start + end - width) / 2.).clamp(0., (full_width - width).max(0.)),
                false => left,
            };
            let y = match y < top + margin || y > top + height - margin {
                true => (y - height / 2.).clamp(0., (full_height - height).max(0.)),
                false => top,
            };
            self.graph_scroll.set_offset(point(px(-x), px(-y)));
        }
        let height = self
            .graph_height
            .unwrap_or(layout.height.clamp(MIN_GRAPH_HEIGHT, MAX_GRAPH_HEIGHT));
        // Scrolled away from either end, a button goes there.
        if self.graph.older == 0 && !self.loading_older && self.scroll_to_start.replace(false) {
            let offset = self.graph_scroll.offset();
            self.graph_scroll.set_offset(point(px(0.), offset.y));
        }
        let (first, away) = self.graph_away_now();
        self.graph_away.set((first, away));
        let picked: Vec<String> = self
            .narrowed
            .iter()
            .filter_map(|&i| self.commits.get(i))
            .map(|commit| commit.id.clone())
            .collect();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .h(px(height))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .bg(theme.graph_background)
                    .on_scroll_wheel(cx.listener(Self::pull_older))
                    // A press on nothing in particular grabs the graph to drag.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            if this.hovered_node.is_none() {
                                this.graph_drag =
                                    Some((event.position, this.graph_scroll.offset()));
                                cx.notify();
                            }
                        }),
                    )
                    .when(self.graph_drag.is_some(), |graph| {
                        graph.cursor(CursorStyle::ClosedHand)
                    })
                    .relative()
                    .child(div().flex_1().min_w_0().h_full().child(graph_view::render(
                        &self.graph,
                        &layout,
                        graph_view::Marks {
                            selected: &self.selected_nodes,
                            picked: &picked,
                            pull: match self.loading_older {
                                true => Some(1.),
                                false => self.pull.get().map(|pull| pull / PULL_DISTANCE),
                            },
                            hovered: self.hovered_node,
                            stash: self.stash_view.as_ref().map(|stash| stash.name.as_str()),
                        },
                        &self.graph_scroll,
                        theme,
                        cx,
                    )))
                    .when(away, |graph| {
                        graph.child(
                            div()
                                .id("graph-latest")
                                .absolute()
                                .right(px(10.))
                                .bottom(px(10.))
                                .px(px(8.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(theme.separator)
                                .bg(theme.background)
                                .shadow_md()
                                .text_xs()
                                .cursor_pointer()
                                .hover(|button| button.bg(theme.selection))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.scroll_to_end.set(true);
                                    cx.notify();
                                }))
                                .child("Latest →"),
                        )
                    })
                    .when(first, |graph| {
                        graph.child(
                            div()
                                .id("graph-first")
                                .absolute()
                                .left(px(10.))
                                .bottom(px(10.))
                                .px(px(8.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(theme.separator)
                                .bg(theme.background)
                                .shadow_md()
                                .text_xs()
                                .cursor_pointer()
                                .hover(|button| button.bg(theme.selection))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(|this, _, _, cx| this.go_to_first_commit(cx)))
                                .child(match self.loading_older && self.scroll_to_start.get() {
                                    true => "Loading history…",
                                    false => "← First commit",
                                }),
                        )
                    }),
            )
            .child(border_handle("graph-border", theme).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging = Some(Border::Graph);
                    cx.stop_propagation();
                }),
            ))
            .into_any_element()
    }

    /// Sections of rows alike: uncommitted changes, the selection's commits,
    /// then earlier ones; each row a title over a dimmed detail line.
    fn commit_list(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let uncommitted = matches!(self.range, Some((_, Revision::Worktree(_))));
        let worktree = self
            .single_node()
            .filter(|(_, node)| node.kind == wtf_core::NodeKind::Uncommitted)
            .map(|(_, node)| graph_view::worktree_label(node, self.worktree_names));
        let later = self.later_worktrees(theme, cx);
        div()
            .flex()
            .flex_col()
            .size_full()
            .when(uncommitted || !later.is_empty(), |list| {
                list.child(list_heading("Uncommitted".to_string(), theme))
            })
            .when(uncommitted, |list| {
                list.child(
                    list_item("uncommitted", self.narrowed.is_empty(), theme)
                        .on_click(cx.listener(|this, _, _, cx| this.show_all_commits(cx)))
                        .child(div().truncate().child("Uncommitted changes"))
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(theme.secondary)
                                .child(worktree.unwrap_or_default()),
                        ),
                )
            })
            .children(later)
            .when(!self.commits.is_empty(), |list| {
                list.child(list_heading(
                    format!("Commits · {}", self.commits.len()),
                    theme,
                ))
            })
            .child(
                uniform_list(
                    "commits",
                    self.commits.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                        let theme = Theme::for_appearance(window.appearance());
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map_or(0, |time| time.as_secs() as i64);
                        range
                            .map(|index| {
                                let commit = &this.commits[index];
                                let selected =
                                    this.narrowed.is_empty() || this.narrowed.contains(&index);
                                let target = Target::Commit {
                                    commit: commit.id.clone(),
                                    message: match commit.body.is_empty() {
                                        true => commit.summary.clone(),
                                        false => format!("{}\n\n{}", commit.summary, commit.body),
                                    },
                                };
                                list_item(("commit", index), selected, &theme)
                                    // Focus moves to the list, for its ⌘A.
                                    .on_click(cx.listener(
                                        move |this, event: &ClickEvent, window, cx| {
                                            this.focus.focus(window);
                                            this.click_commit(index, event.modifiers(), cx)
                                        },
                                    ))
                                    // The menu leaves the selection as it is.
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                            this.open_context_menu(
                                                target.clone(),
                                                event.position,
                                                cx,
                                            );
                                            cx.stop_propagation();
                                        }),
                                    )
                                    .child(div().truncate().child(commit.summary.clone()))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_xs()
                                            .text_color(theme.secondary)
                                            .child(format!(
                                                "{} · {} · {}",
                                                commit.short_id,
                                                commit.author,
                                                short_age(commit.time, now)
                                            )),
                                    )
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.commit_scroll.clone())
                // As tall as its commits, shrinking to scroll when they run
                // past the column, so the earlier ones follow right under it.
                .h(px(self.commits.len() as f32 * LIST_ITEM_HEIGHT))
                .flex_shrink()
                .min_h_0(),
            )
            .children(self.earlier_commits(theme, cx))
            .children(self.earlier_nodes(theme, cx))
    }

    /// The selected node when it is the only one.
    fn single_node(&self) -> Option<(usize, &wtf_core::GraphNode)> {
        let [selected] = self.selected_nodes[..] else {
            return None;
        };
        Some((selected, self.graph.nodes.get(selected)?))
    }

    /// Above a node's commits: the uncommitted changes of worktrees on it,
    /// dimmed, one click away.
    fn later_worktrees(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some((selected, node)) = self.single_node() else {
            return Vec::new();
        };
        if node.kind == wtf_core::NodeKind::Uncommitted {
            return Vec::new();
        }
        self.graph
            .nodes
            .iter()
            .enumerate()
            // Only worktrees with changes: a clean one has nothing to show.
            .filter(|(_, worktree)| {
                worktree.kind == wtf_core::NodeKind::Uncommitted
                    && worktree.parents.first() == Some(&selected)
                    && (worktree.added, worktree.removed) != (0, 0)
            })
            .map(|(w, worktree)| {
                let label = graph_view::worktree_label(worktree, self.worktree_names);
                list_item(("later", w), false, theme)
                    .text_color(theme.secondary)
                    .cursor_pointer()
                    .hover(|item| item.bg(theme.selection.opacity(0.5)))
                    .on_click(cx.listener(move |this, _, _, cx| this.click_node(w, false, cx)))
                    .child(div().truncate().child("Uncommitted changes"))
                    .child(div().truncate().text_xs().child(label))
                    .into_any_element()
            })
            .collect()
    }

    /// Below a node's commits: the nodes before it, a row each, to step back
    /// along the line one click at a time.
    fn earlier_nodes(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (_, node) = self.single_node()?;
        if node.kind == wtf_core::NodeKind::Uncommitted || self.commits.is_empty() {
            return None;
        }
        let mut rows = Vec::new();
        let mut at = node.parents.first().copied();
        while let Some(index) = at.filter(|_| rows.len() < EARLIER_NODES) {
            let earlier = &self.graph.nodes[index];
            if let Some(newest) = earlier.commits.last() {
                let count = earlier.commits.len();
                let detail = match count {
                    1 => format!("{} · {}", newest.short_id, newest.author),
                    _ => format!("{count} commits · {}", newest.author),
                };
                rows.push(
                    list_item(("earlier-node", index), false, theme)
                        .text_color(theme.secondary)
                        .cursor_pointer()
                        .hover(|item| item.bg(theme.selection.opacity(0.5)))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.click_node(index, false, cx)),
                        )
                        .child(div().truncate().child(newest.summary.clone()))
                        .child(div().truncate().text_xs().child(detail)),
                );
            }
            at = earlier.parents.first().copied();
        }
        if rows.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .child(list_heading("Earlier".to_string(), theme))
                .children(rows)
                .into_any_element(),
        )
    }

    /// Below a worktree's uncommitted changes, which have no commits of their
    /// own: the commits leading to them, dimmed, back to the oldest shown.
    /// Clicking one selects its node in the graph.
    fn earlier_commits(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.commits.is_empty() {
            return None;
        }
        let [selected] = self.selected_nodes[..] else {
            return None;
        };
        let worktree = self.graph.nodes.get(selected)?;
        if worktree.kind != wtf_core::NodeKind::Uncommitted {
            return None;
        }
        let mut earlier = Vec::new();
        let mut at = worktree.parents.first().copied();
        while let Some(index) = at {
            let node = &self.graph.nodes[index];
            earlier.extend(node.commits.iter().rev().map(|commit| (index, commit)));
            at = node.parents.first().copied();
        }
        let items = earlier.into_iter().enumerate().map(|(k, (node, commit))| {
            let id = commit.id.clone();
            list_item(("earlier", k), false, theme)
                .text_color(theme.secondary)
                .cursor_pointer()
                .hover(|item| item.bg(theme.selection.opacity(0.5)))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.select_commit(node, id.clone(), cx)),
                )
                .child(div().truncate().child(commit.summary.clone()))
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .child(format!("{} · {}", commit.short_id, commit.author)),
                )
        });
        Some(
            div()
                .id("earlier-commits")
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(list_heading("Earlier".to_string(), theme))
                .children(items)
                .into_any_element(),
        )
    }

    /// Files in the order the file tree lists them, folded folders included.
    fn file_order(&self) -> Vec<usize> {
        file_tree(&self.files, self.worktree_split, &HashSet::new())
            .into_iter()
            .filter_map(|row| match row {
                TreeRow::File { index, .. } => Some(index),
                _ => None,
            })
            .collect()
    }

    /// File by file: the shown file with its place among them, and arrows to
    /// step to the previous and next one.
    fn file_stepper(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.changes_layout != ChangesLayout::OneFile || self.files.is_empty() {
            return None;
        }
        let order = self.file_order();
        let at = self
            .selected_file
            .and_then(|file| order.iter().position(|&i| i == file))
            .unwrap_or(0);
        let file = &self.files[order[at]];
        let step = |id: &'static str, label: &'static str, to: Option<usize>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(20.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.separator)
                .map(|button| match to {
                    Some(to) => button
                        .cursor_pointer()
                        .hover(|button| button.bg(theme.selection))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select_file(Some(to), cx)),
                        ),
                    None => button.text_color(theme.separator),
                })
                .child(label)
        };
        let previous = at.checked_sub(1).map(|k| order[k]);
        let next = order.get(at + 1).copied();
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .h(px(32.))
                .px(px(12.))
                .border_b_1()
                .border_color(theme.separator)
                .bg(theme.graph_background)
                .child(step("file-previous", "‹", previous))
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(format!("{} / {}", at + 1, order.len())),
                )
                .child(step("file-next", "›", next))
                .child(
                    div()
                        .text_color(theme.secondary)
                        .font_family(preferences::code_font())
                        .text_xs()
                        .child(file.status.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(display_path(file).to_string()),
                )
                .into_any_element(),
        )
    }

    /// The changed files as a tree of folders, under the unstaged and staged
    /// sections for a worktree.
    fn file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = file_tree(&self.files, self.worktree_split, &self.collapsed_dirs).len();
        uniform_list(
            "files",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                let theme = Theme::for_appearance(window.appearance());
                let rows = file_tree(&this.files, this.worktree_split, &this.collapsed_dirs);
                range
                    .filter_map(|k| rows.get(k).map(|row| (k, row)))
                    .map(|(k, row)| {
                        let item = div()
                            .id(("file-row", k))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .w_full()
                            .h(px(FILE_ROW_HEIGHT))
                            .pr(px(8.))
                            .whitespace_nowrap()
                            .overflow_hidden();
                        let indent = |depth: usize| px(8. + depth as f32 * 14.);
                        match row {
                            TreeRow::Section(staged, count) => {
                                let title = if *staged { "Staged" } else { "Unstaged" };
                                item.pl(indent(0))
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.secondary)
                                    .child(format!("{title} ({count})"))
                            }
                            TreeRow::Dir { key, label, depth } => {
                                let collapsed = this.collapsed_dirs.contains(key);
                                let key = key.clone();
                                item.pl(indent(*depth))
                                    .cursor_pointer()
                                    .hover(|item| item.bg(theme.selection.opacity(0.5)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.collapsed_dirs.remove(&key) {
                                            this.collapsed_dirs.insert(key.clone());
                                        }
                                        cx.notify();
                                    }))
                                    .child(panel_chevron(!collapsed, &theme))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_color(theme.secondary)
                                            .child(label.clone()),
                                    )
                            }
                            TreeRow::File { index, name, depth } => {
                                let index = *index;
                                let file = &this.files[index];
                                item.pl(indent(*depth))
                                    .cursor_pointer()
                                    .when(this.selected_file == Some(index), |item| {
                                        item.bg(theme.selection)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.selected_file != Some(index) {
                                            this.select_file(Some(index), cx);
                                        }
                                    }))
                                    .child(
                                        div()
                                            .w(px(10.))
                                            .font_family(preferences::code_font())
                                            .text_xs()
                                            .text_color(theme.secondary)
                                            .child(file.status.clone()),
                                    )
                                    .child(div().truncate().child(name.clone()))
                            }
                        }
                    })
                    .collect()
            }),
        )
        .size_full()
        .py(px(4.))
    }

    fn detail(&self, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(message) = &self.error_message {
            return placeholder("Something went wrong", Some(message.clone()), theme);
        }
        if self.repo_path.is_none() {
            let hint = "Open a Git repository to review its commits.";
            return placeholder("No repository", Some(hint.into()), theme);
        }
        if self.is_stacked() {
            if self.is_loading_stacked {
                return self.with_header(placeholder("Loading…", None, theme), theme, window, cx);
            }
            let spacers = self.header_rows();
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
                    self.stacked[..=top.min(self.stacked.len().saturating_sub(1))]
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
                let list = uniform_list(
                    "diff",
                    spacers + diff.rows.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, window, _| {
                        let theme = Theme::for_appearance(window.appearance());
                        let rows = this.diff.as_ref().map_or(&[][..], |diff| &diff.rows);
                        range
                            .filter_map(|index| match index.checked_sub(spacers) {
                                None => Some(spacer()),
                                Some(index) => rows.get(index).map(|row| {
                                    let colors = this.diff_colors(None, &theme);
                                    diff_view::row(row, &theme, colors.as_deref())
                                        .into_any_element()
                                }),
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.diff_scroll.clone())
                .size_full()
                .font_family(preferences::code_font())
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

    /// `content` below the commit header, when one is selected, which stays put.
    fn with_header(
        &self,
        content: AnyElement,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .when(self.range.is_some(), |pane| {
                pane.child(self.commit_header(theme, window, cx))
            })
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
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
                    .child(self.file_list(cx))
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

    /// The attention sidebar on the left of the graph, or a strip that opens it:
    /// a search field, the filters, and the branches by group.
    fn attention_panel_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let attention = &self.attention;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_secs() as i64);
        let search = self.attention_search.to_lowercase();
        let matches = |name: &str| search.is_empty() || name.to_lowercase().contains(&search);

        let time_window = match attention.window_days {
            Some(1) => "Last day".to_string(),
            Some(days) => format!("{days} days"),
            None => "All time".to_string(),
        };
        let authors = match attention.authors.len() {
            0 => "Everyone".to_string(),
            1 => attention.authors[0].clone(),
            count => format!("{count} authors"),
        };
        let dropdown = |id: &'static str, label: String, which: Dropdown| {
            div()
                .id(id)
                .px(px(6.))
                .py(px(1.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.separator)
                .hover(|chip| chip.bg(theme.selection))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.dropdown = Some((which, event.position));
                        cx.notify();
                    }),
                )
                .child(format!("{label} ▾"))
        };
        let window_chip = dropdown("attention-window", time_window, Dropdown::Window);
        let authors_chip = dropdown("attention-authors", authors, Dropdown::Authors);

        let mut body = div()
            .id("attention-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .pb(px(8.))
            .overflow_y_scroll();
        let groups = [
            (Section::Worktrees, "WORKTREES", None),
            (
                Section::Local,
                "LOCAL",
                Some(attention.scope == Scope::Branches),
            ),
            (Section::Remote, "REMOTE", Some(attention.remotes)),
            (Section::Tags, "TAGS", Some(attention.tags)),
            (Section::Stashes, "STASHES", Some(attention.stashes)),
        ];
        for (section, title, enabled) in groups {
            let branches: Vec<&wtf_core::Branch> = self
                .graph
                .branches
                .iter()
                .filter(|branch| match section {
                    Section::Worktrees => branch.worktree,
                    Section::Local => !branch.worktree && !branch.remote,
                    Section::Remote => branch.remote,
                    Section::Tags | Section::Stashes => false,
                })
                .collect();
            let mut tags: Vec<&str> = Vec::new();
            // Every tag, so they can be looked over before the graph shows
            // them.
            if section == Section::Tags {
                tags.extend(self.graph.tags.iter().map(String::as_str));
            }
            // A group's checkbox turns it on or off; worktrees are always on.
            // An open group shows its count and folds with the chevron.
            let on = enabled.unwrap_or(true);
            let count = match section {
                Section::Tags => tags.len().to_string(),
                Section::Stashes => self.graph.stashes.len().to_string(),
                _ => {
                    let shown = branches.iter().filter(|branch| branch.shown).count();
                    format!("{shown}/{}", branches.len())
                }
            };
            // A group that is on starts open, one that is off starts folded;
            // either opens or folds with its chevron.
            let collapsed = self.collapsed_sections.contains(&section) == on;
            let checkbox = attention_checkbox(
                ("attention-group", section as usize),
                Some(on),
                enabled.is_some(),
                theme,
            )
            .when(enabled.is_some(), |checkbox| {
                checkbox.on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_group(section, cx)
                }))
            });
            body = body.child(
                div()
                    .id(("attention-section", section as usize))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(FILE_ROW_HEIGHT))
                    .mt(px(6.))
                    .pl(px(8.))
                    .pr(px(8.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(if on {
                        theme.foreground
                    } else {
                        theme.secondary
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed_sections.remove(&section) {
                            this.collapsed_sections.insert(section);
                        }
                        cx.notify();
                    }))
                    .child(panel_chevron(!collapsed, theme))
                    .child(div().flex_1().child(title))
                    .child(div().text_color(theme.secondary).child(count))
                    .child(checkbox),
            );
            if collapsed {
                continue;
            }
            // A branch matches by its name, its author, or its worktree's name.
            let found = |branch: &&wtf_core::Branch| {
                matches(&branch.name)
                    || matches(&branch.author)
                    || self.graph.nodes.iter().any(|node| {
                        node.kind == wtf_core::NodeKind::Uncommitted
                            && node.refs.first() == Some(&branch.name)
                            && matches(&graph_view::worktree_label(
                                node,
                                WorktreeNames::FolderAndBranch,
                            ))
                    })
            };
            let branches: Vec<&wtf_core::Branch> = branches.into_iter().filter(found).collect();
            if section == Section::Worktrees {
                for branch in branches {
                    body = body.child(self.attention_row(branch, None, 1, now, theme, cx));
                }
                continue;
            }
            // Branches nest by the folders in their names, as files do; while
            // searching, every folder is open.
            let names: Vec<&str> = branches.iter().map(|branch| branch.name.as_str()).collect();
            let prefix = if section == Section::Remote {
                "remote:"
            } else {
                "local:"
            };
            let folded: HashSet<String> = match search.is_empty() {
                true => self
                    .collapsed_dirs
                    .iter()
                    .filter_map(|key| key.strip_prefix(prefix).map(str::to_string))
                    .collect(),
                false => HashSet::new(),
            };
            for row in name_tree(&names, &folded) {
                match row {
                    NameRow::Leaf { index, name, depth } => {
                        body = body.child(self.attention_row(
                            branches[index],
                            Some(name),
                            depth + 1,
                            now,
                            theme,
                            cx,
                        ));
                    }
                    NameRow::Folder { path, label, depth } => {
                        let inside: Vec<String> = branches
                            .iter()
                            .filter(|branch| branch.name.starts_with(&path))
                            .map(|branch| branch.name.clone())
                            .collect();
                        let shown = branches
                            .iter()
                            .filter(|branch| branch.name.starts_with(&path) && branch.shown)
                            .count();
                        let state = match shown {
                            0 => Some(false),
                            n if n == inside.len() => Some(true),
                            _ => None,
                        };
                        let key = format!("{prefix}{path}");
                        let folded = folded.contains(&path);
                        body = body.child(
                            div()
                                .id(SharedString::from(format!("attention-dir-{key}")))
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .h(px(FILE_ROW_HEIGHT))
                                .pl(px(8. + (depth + 1) as f32 * 14.))
                                .pr(px(8.))
                                .cursor_pointer()
                                .hover(|row| row.bg(theme.selection.opacity(0.5)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.collapsed_dirs.remove(&key) {
                                        this.collapsed_dirs.insert(key.clone());
                                    }
                                    cx.notify();
                                }))
                                .child(panel_chevron(!folded, theme))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(theme.secondary)
                                        .child(label),
                                )
                                .child(
                                    div()
                                        .text_color(theme.secondary)
                                        .child(format!("{shown}/{}", inside.len())),
                                )
                                .child(
                                    attention_checkbox(
                                        SharedString::from(format!("attention-dir-check-{path}")),
                                        state,
                                        true,
                                        theme,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.toggle_branches(&inside, state != Some(false), cx)
                                        },
                                    )),
                                ),
                        );
                    }
                }
            }
            // Every stash, its message and age; a click shows it.
            if section == Section::Stashes {
                for stash in self
                    .graph
                    .stashes
                    .iter()
                    .filter(|stash| matches(&stash.name) || matches(&stash.message))
                {
                    let (show, name) = (stash.name.clone(), stash.name.clone());
                    let target = Target::Stash {
                        name: stash.name.clone(),
                        id: stash.id.clone(),
                        message: stash.message.clone(),
                    };
                    let shown = self.stash_view.as_ref().is_some_and(|s| s.name == name);
                    body = body.child(
                        div()
                            .id(SharedString::from(format!("attention-{name}")))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(FILE_ROW_HEIGHT))
                            .pl(px(8. + 14. + 16.))
                            .pr(px(8.))
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .hover(|row| row.bg(theme.selection))
                            .when(shown, |row| row.bg(theme.selection))
                            .on_click(cx.listener(move |this, _, _, cx| this.show_stash(&show, cx)))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.open_context_menu(target.clone(), event.position, cx);
                                }),
                            )
                            .child(div().flex_none().text_color(theme.secondary).child(name))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(stash.message.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(theme.secondary)
                                    .child(short_age(stash.time, now)),
                            ),
                    );
                }
            }
            for tag in tags.into_iter().filter(|tag| matches(tag)) {
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .h(px(FILE_ROW_HEIGHT))
                        .pl(px(8. + 14.))
                        .child(div().w(px(10.)).text_color(theme.git_tag_text).child("◆"))
                        .child(div().truncate().child(tag.to_string())),
                );
            }
        }

        let focused = self.attention_search_focus.is_focused(window);
        let content = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(self.sidebar_width))
            .h_full()
            .pt(px(4.))
            .bg(theme.background)
            .text_xs()
            .child(
                div()
                    .id("attention-search")
                    .track_focus(&self.attention_search_focus)
                    .on_key_down(cx.listener(Self::attention_search_key))
                    .on_click(
                        cx.listener(|this, _, window, _| this.attention_search_focus.focus(window)),
                    )
                    .flex()
                    .flex_none()
                    .items_center()
                    .mx(px(8.))
                    .h(px(22.))
                    .px(px(6.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(if focused {
                        theme.accent
                    } else {
                        theme.separator
                    })
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .map(|field| match self.attention_search.is_empty() {
                        true => field
                            .text_color(theme.secondary)
                            .child("Filter worktrees, branches, tags"),
                        false => field.child(self.attention_search.clone()),
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(px(6.))
                    .px(px(8.))
                    .pt(px(6.))
                    .child(window_chip)
                    .child(authors_chip),
            )
            .child(body);
        content.into_any_element()
    }

    /// The strip at the window's right edge, the same with a panel open or
    /// not: the arrow, the panels by name, then the diff view switches.
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
            .into_any_element()
    }

    /// A branch in the sidebar, checked when shown and dimmed when not.
    /// Clicking hides a shown branch or pins a left-out one; a pin or hide
    /// shows as a note whose × clears it.
    /// A branch in the sidebar tree, `name` short for it under its folder,
    /// with its checkbox at the right.
    fn attention_row(
        &self,
        branch: &wtf_core::Branch,
        name: Option<String>,
        depth: usize,
        now: i64,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hidden = self.attention.hidden.contains(&branch.name);
        let pinned = self.attention.pinned.contains(&branch.name);
        let main = branch.name == self.graph.main;
        let overridden = pinned || hidden;
        let group = SharedString::from(format!("attention-row-{}", branch.name));
        let index = self.graph.nodes.iter().position(|node| {
            node.kind == wtf_core::NodeKind::Uncommitted && node.refs.first() == Some(&branch.name)
        });
        let worktree = index.map(|i| &self.graph.nodes[i]);
        let selected = index.is_some_and(|i| self.selected_nodes.contains(&i));
        let target = graph_view::label_target(&self.graph, &branch.name, branch.commit.clone());
        let label = match worktree {
            Some(node) => graph_view::worktree_label(node, self.worktree_names),
            None => name.unwrap_or_else(|| branch.name.clone()),
        };
        let age = (branch.time > 0).then(|| short_age(branch.time, now));
        let name = branch.name.clone();
        div()
            .id(SharedString::from(format!("attention-{}", branch.name)))
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(FILE_ROW_HEIGHT))
            .pl(px(8. + depth as f32 * 14. + 16.))
            .pr(px(8.))
            .whitespace_nowrap()
            .hover(|row| row.bg(theme.selection))
            .cursor_pointer()
            .when(!branch.shown, |row| row.text_color(theme.secondary))
            // Left behind, as in the graph: in italics, with why after it.
            .when(
                graph_view::stale_note(&self.graph, &branch.name).is_some(),
                |row| row.italic(),
            )
            .when(selected, |row| row.bg(theme.selection))
            // The row picks the branch in the graph; its checkbox shows or
            // hides it.
            .on_click({
                let name = name.clone();
                cx.listener(move |this, _, _, cx| this.reveal_branch(&name, cx))
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_context_menu(target.clone(), event.position, cx);
                }),
            )
            .child(div().flex_1().min_w_0().truncate().child(label))
            // How far it is from its upstream, as its label in the graph says.
            .children(
                graph_view::sync_badge(&self.graph, &branch.name)
                    .map(|badge| div().flex_none().text_color(theme.secondary).child(badge)),
            )
            // Why it looks left behind, as its label in the graph says, or
            // why the graph cannot draw it.
            .children(
                graph_view::stale_note(&self.graph, &branch.name)
                    .or_else(|| branch.unrelated.then(|| "no shared history".to_string()))
                    .map(|note| div().flex_none().text_color(theme.secondary).child(note)),
            )
            .children(age.map(|age| div().text_color(theme.secondary).child(age)))
            // Ticked or not by hand, against the filters: on hover, ↺ hands
            // the branch back to them.
            .when(overridden, |row| {
                let name = branch.name.clone();
                row.child(
                    div()
                        .id(SharedString::from(format!(
                            "attention-follow-{}",
                            branch.name
                        )))
                        .flex_none()
                        .px(px(2.))
                        .rounded(px(3.))
                        .opacity(0.)
                        .group_hover(group, |style| style.opacity(1.))
                        .hover(|button| button.bg(theme.tag_background))
                        .text_color(theme.secondary)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.follow_filters(&name, cx);
                        }))
                        .child("↺"),
                )
            })
            .child(
                attention_checkbox(
                    SharedString::from(format!("attention-check-{}", branch.name)),
                    Some(branch.shown),
                    !main && !branch.unrelated,
                    theme,
                )
                .when(!main && !branch.unrelated, |checkbox| {
                    checkbox.on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_branch(&name, cx)
                    }))
                })
                // The dot marks a choice made by hand.
                .when(overridden, |checkbox| {
                    checkbox.relative().child(
                        div()
                            .absolute()
                            .top(px(1.))
                            .right(px(-3.))
                            .size(px(5.))
                            .rounded(px(3.))
                            .bg(theme.accent),
                    )
                }),
            )
            .into_any_element()
    }

    /// Drops a pin or hide, so the filters decide on the branch again.
    fn follow_filters(&mut self, name: &str, cx: &mut Context<Self>) {
        self.change_attention(
            |attention| {
                attention.pinned.retain(|pinned| pinned != name);
                attention.hidden.retain(|hidden| hidden != name);
            },
            cx,
        );
    }

    /// Selects a branch's node in the graph, its worktree's when it has one;
    /// one the graph cannot draw shows in the commit area instead.
    fn reveal_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        let worktree = self.graph.nodes.iter().position(|node| {
            node.kind == wtf_core::NodeKind::Uncommitted
                && node.refs.first().map(String::as_str) == Some(name)
        });
        let tip = || {
            self.graph.nodes.iter().position(|node| {
                node.kind != wtf_core::NodeKind::Uncommitted && node.refs.iter().any(|r| r == name)
            })
        };
        if let Some(node) = worktree.or_else(tip) {
            self.click_node(node, false, cx);
        } else if self
            .graph
            .branches
            .iter()
            .any(|b| b.name == name && b.unrelated)
        {
            self.show_unrelated(name, cx);
        }
    }

    /// Hides branches from attention, or pins them into it.
    fn toggle_branches(&mut self, names: &[String], hide: bool, cx: &mut Context<Self>) {
        let main = self.graph.main.clone();
        let names: Vec<String> = names
            .iter()
            .filter(|name| **name != main)
            .cloned()
            .collect();
        self.change_attention(
            |attention| {
                attention.pinned.retain(|pinned| !names.contains(pinned));
                attention.hidden.retain(|hidden| !names.contains(hidden));
                match hide {
                    true => attention.hidden.extend(names),
                    false => attention.pinned.extend(names),
                }
            },
            cx,
        );
    }

    /// Turns a group of branches on or off in attention.
    fn toggle_group(&mut self, section: Section, cx: &mut Context<Self>) {
        self.change_attention(
            |attention| match section {
                Section::Worktrees => {}
                Section::Local => {
                    attention.scope = match attention.scope {
                        Scope::Branches => Scope::Worktrees,
                        Scope::Worktrees => Scope::Branches,
                    }
                }
                Section::Remote => attention.remotes = !attention.remotes,
                Section::Tags => attention.tags = !attention.tags,
                Section::Stashes => attention.stashes = !attention.stashes,
            },
            cx,
        );
    }

    fn attention_search_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        match keystroke.key.as_str() {
            "escape" if self.attention_search.is_empty() => self.focus.focus(window),
            "escape" => self.attention_search.clear(),
            "backspace" => {
                self.attention_search.pop();
            }
            "v" if modifiers.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.attention_search.push_str(text.trim());
                }
            }
            _ if !modifiers.platform && !modifiers.control => match &keystroke.key_char {
                Some(text) if !text.chars().any(char::is_control) => {
                    self.attention_search.push_str(text)
                }
                _ => return,
            },
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The time window and authors dropdowns of the sidebar.
    fn dropdown_view(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (which, position) = self.dropdown.as_ref()?;
        let item = |id: (&'static str, usize), checked: bool, label: String| {
            div()
                .id(id)
                .flex()
                .gap(px(6.))
                .px(px(10.))
                .py(px(2.))
                .hover(|item| item.bg(theme.selection))
                .cursor_pointer()
                .child(div().w(px(10.)).child(if checked { "✓" } else { "" }))
                .child(label)
        };
        let mut menu = div()
            .id("attention-dropdown")
            .flex()
            .flex_col()
            .min_w(px(160.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .shadow_md()
            .text_xs()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.dropdown = None;
                cx.notify();
            }));
        match which {
            Dropdown::Window => {
                let windows = [
                    (Some(1), "Last day"),
                    (Some(7), "7 days"),
                    (Some(14), "14 days"),
                    (Some(30), "30 days"),
                    (None, "All time"),
                ];
                for (k, (days, label)) in windows.into_iter().enumerate() {
                    let checked = self.attention.window_days == days;
                    menu = menu.child(item(("window", k), checked, label.to_string()).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.dropdown = None;
                            this.change_attention(|attention| attention.window_days = days, cx)
                        }),
                    ));
                }
            }
            Dropdown::Authors => {
                let everyone = self.attention.authors.is_empty();
                menu = menu.child(
                    item(("author", 0), everyone, "Everyone".to_string()).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.change_attention(|attention| attention.authors.clear(), cx)
                        },
                    )),
                );
                let mut authors: Vec<&str> = Vec::new();
                for branch in &self.graph.branches {
                    if !branch.author.is_empty() && !authors.contains(&branch.author.as_str()) {
                        authors.push(&branch.author);
                    }
                }
                for (k, author) in authors.into_iter().enumerate() {
                    let name = author.to_string();
                    let checked = self.attention.authors.contains(&name);
                    menu = menu.child(item(("author", k + 1), checked, name.clone()).on_click(
                        cx.listener(move |this, _, _, cx| {
                            let name = name.clone();
                            this.change_attention(
                                |attention| match attention.authors.iter().position(|a| *a == name)
                                {
                                    Some(at) => {
                                        attention.authors.remove(at);
                                    }
                                    None => attention.authors.push(name),
                                },
                                cx,
                            )
                        }),
                    ));
                }
            }
        }
        Some(
            deferred(anchored().position(*position).snap_to_window().child(menu))
                .with_priority(1)
                .into_any_element(),
        )
    }

    pub fn open_context_menu(
        &mut self,
        target: Target,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some((target, position));
        cx.notify();
    }

    fn run_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, _)) = self.context_menu.take() else {
            return;
        };
        let repo = self.repo_path.clone().unwrap_or_default();
        let (name, path, commit) = match &target {
            Target::Branch { name, commit, .. } => (Some(name.clone()), None, commit.clone()),
            Target::Commit { commit, .. } => (None, None, commit.clone()),
            Target::Remote {
                remote,
                branch,
                commit,
            } => (Some(format!("{remote}/{branch}")), None, commit.clone()),
            Target::Tag { name, commit } => (Some(name.clone()), None, commit.clone()),
            Target::Stash { name, id, .. } => (Some(name.clone()), None, id.clone()),
            Target::Worktree {
                path,
                branch,
                commit,
                ..
            } => (branch.clone(), Some(path.clone()), commit.clone()),
        };
        let copy = |text: String, cx: &mut Context<Self>| {
            cx.write_to_clipboard(ClipboardItem::new_string(text))
        };
        match command {
            Command::CopyName => copy(name.unwrap_or_default(), cx),
            Command::CopyHash => copy(commit, cx),
            Command::CopyShortHash => copy(commit.chars().take(7).collect(), cx),
            Command::CopyMessage => {
                if let Target::Commit { message, .. } = &target {
                    copy(message.clone(), cx)
                }
            }
            Command::CopyPath => copy(path.unwrap_or_default(), cx),
            Command::OpenWithRecent(k) => {
                if let (Some(path), Some(command)) = (path, self.open_with.get(k).cloned()) {
                    self.run_in_terminal(popups::open_with(&command, &path), window, cx);
                    self.remember_open_with(&command);
                }
            }
            Command::RevealInFinder => {
                let opened = std::process::Command::new("open")
                    .arg("-R")
                    .arg(path.unwrap_or_default())
                    .spawn();
                if let Err(error) = opened {
                    self.error_message = Some(format!("Cannot open Finder: {error}"));
                }
            }
            Command::OpenWith => {
                if let Some(path) = path {
                    let last = self.open_with.first().cloned().unwrap_or_default();
                    self.show_prompt(PromptKind::OpenWith(path), last, window);
                }
            }
            Command::Rename => {
                if let Some(name) = name {
                    self.show_prompt(PromptKind::Rename(name.clone()), name, window);
                }
            }
            Command::Delete => {
                if let Some(name) = name {
                    let question = format!("Delete branch {name}?");
                    let detail = "git refuses to delete a branch that is not merged.";
                    self.confirm(
                        question,
                        detail,
                        "Delete",
                        popups::delete_branch(&repo, &name),
                        window,
                        cx,
                    );
                }
            }
            Command::NewWorktree => match &target {
                Target::Branch { name, .. } => {
                    let path = popups::worktree_path(&repo, name);
                    self.show_prompt(PromptKind::WorktreeFor(name.clone()), path, window);
                }
                Target::Commit { .. } => {
                    let kind = PromptKind::NewBranch {
                        commit: commit.clone(),
                        worktree: true,
                    };
                    self.show_prompt(kind, name.unwrap_or_default(), window);
                }
                _ => {}
            },
            // A commit asks for a name.
            Command::NewBranch => {
                if let Target::Commit { .. } = &target {
                    let kind = PromptKind::NewBranch {
                        commit: commit.clone(),
                        worktree: false,
                    };
                    self.show_prompt(kind, name.unwrap_or_default(), window);
                }
            }
            Command::StashApply(k) | Command::StashPop(k) => {
                let pop = matches!(command, Command::StashPop(_));
                if let (Some(name), Some((path, _))) =
                    (name, self.checkout_worktrees(&target).get(k).cloned())
                {
                    self.run_in_terminal(popups::stash_apply(&path, &name, pop), window, cx);
                }
            }
            Command::StashWorktree => {
                if let Target::Stash { name, .. } = &target
                    && let Some(stash) = self.graph.stashes.iter().find(|s| s.name == *name)
                {
                    let kind = PromptKind::StashWorktree {
                        stash: stash.name.clone(),
                        base: stash.base.clone(),
                    };
                    self.show_prompt(kind, String::new(), window);
                }
            }
            Command::StashChanges => {
                if let Target::Worktree { path, .. } = &target {
                    self.open_stash_picker(path.clone(), window, cx);
                }
            }
            Command::DropStash => {
                if let Some(name) = name {
                    self.confirm(
                        format!("Drop {name}?"),
                        "Its changes are gone once dropped.",
                        "Drop",
                        popups::stash_drop(&repo, &name),
                        window,
                        cx,
                    );
                }
            }
            Command::CheckoutIn(k) => {
                if let (Some(name), Some((path, _))) =
                    (name, self.checkout_worktrees(&target).get(k).cloned())
                {
                    self.run_in_terminal(popups::switch(&path, &name), window, cx);
                }
            }
            Command::DeleteWithCommits => {
                if let Some(name) = name {
                    self.confirm(
                        format!("Delete branch {name} and its commits?"),
                        "Nothing else grows from it and the main branch has not merged it; \
                         its commits are lost unless they were pushed.",
                        "Delete",
                        popups::delete_branch_with_commits(&repo, &name),
                        window,
                        cx,
                    );
                }
            }
            Command::DeleteTag => {
                if let Some(name) = name {
                    self.confirm(
                        format!("Delete tag {name}?"),
                        "Only this repository's tag goes; a pushed tag stays on its remote.",
                        "Delete",
                        popups::delete_tag(&repo, &name),
                        window,
                        cx,
                    );
                }
            }
            Command::DeleteRemote => {
                if let Target::Remote { remote, branch, .. } = &target {
                    self.confirm(
                        format!("Delete {branch} on {remote}?"),
                        "This pushes the deletion: the branch goes from the shared repository for everyone.",
                        "Delete on Remote",
                        popups::delete_remote_branch(&repo, remote, branch),
                        window,
                        cx,
                    );
                }
            }
            Command::Hide => self.hide_branch(&popups::attention_name(&target), cx),
            Command::FollowFilters => self.follow_filters(&popups::attention_name(&target), cx),
            Command::Pin => {
                let name = popups::attention_name(&target);
                self.change_attention(
                    |attention| {
                        attention.hidden.retain(|hidden| *hidden != name);
                        if !attention.pinned.contains(&name) {
                            attention.pinned.push(name.clone());
                        }
                    },
                    cx,
                );
            }
            Command::RemoveWorktreeAndBranch => {
                let merged = matches!(
                    target,
                    Target::Worktree {
                        leaf: Some(true),
                        ..
                    }
                );
                if let (Some(path), Some(name)) = (path, name) {
                    let detail = match merged {
                        true => {
                            "The main branch has merged it, so no work is lost. git refuses to \
                             remove a worktree with uncommitted changes."
                        }
                        false => {
                            "Nothing else grows from this branch; its commits are lost unless \
                             they were pushed. git refuses to remove a worktree with uncommitted \
                             changes."
                        }
                    };
                    self.confirm(
                        format!("Remove worktree {path} and delete branch {name}?"),
                        detail,
                        "Remove Both",
                        popups::remove_worktree_and_branch(&repo, &path, &name, merged),
                        window,
                        cx,
                    );
                }
            }
            Command::RemoveWorktree => {
                if let Some(path) = path {
                    let question = format!("Remove worktree {path}?");
                    let detail = "git refuses to remove a worktree with uncommitted changes.";
                    self.confirm(
                        question,
                        detail,
                        "Remove",
                        popups::remove_worktree(&repo, &path),
                        window,
                        cx,
                    );
                }
            }
        }
        cx.notify();
    }

    /// Asks before running a command that changes the repository.
    fn confirm(
        &mut self,
        question: String,
        detail: &str,
        action: &str,
        line: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = window.prompt(
            PromptLevel::Warning,
            &question,
            Some(detail),
            &[action, "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, window, cx| {
                    this.run_in_terminal(line, window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    /// Types a command into the terminal, opening the dock to show it.
    fn run_in_terminal(&mut self, line: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.console_open {
            self.toggle_console(window, cx);
        }
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, _| terminal.run(&line));
        }
    }

    /// Lists the stale branches and worktrees, all ticked to go.
    fn open_prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stale = self
            .graph
            .branches
            .iter()
            .filter(|branch| !branch.remote && !branch.unrelated && branch.name != self.graph.main)
            .filter(|branch| graph_view::stale_note(&self.graph, &branch.name).is_some())
            // Merged ones start ticked; the rest, idle or with a gone remote,
            // may still hold work, so they wait to be ticked.
            .map(|branch| {
                let merged = branch.stale.iter().any(|reason| reason == "merged");
                (branch.name.clone(), merged)
            })
            .collect();
        self.prune = Some(stale);
        self.prune_focus.focus(window);
        cx.notify();
    }

    fn close_prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prune = None;
        self.focus.focus(window);
        cx.notify();
    }

    /// Deletes the ticked branches, each worktree first, in the terminal: a
    /// merged branch as git checks it, others with their commits. One that
    /// fails leaves the rest to go on.
    fn run_prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(list), Some(repo)) = (self.prune.take(), self.repo_path.clone()) else {
            return;
        };
        let lines: Vec<String> = list
            .into_iter()
            .filter(|(_, ticked)| *ticked)
            .filter_map(|(name, _)| {
                let branch = self.graph.branches.iter().find(|b| b.name == name)?;
                let merged = branch.stale.iter().any(|reason| reason == "merged");
                let delete = match merged {
                    true => popups::delete_branch(&repo, &name),
                    false => popups::delete_branch_with_commits(&repo, &name),
                };
                let worktree = self.graph.nodes.iter().find_map(|node| match &node.tip {
                    Revision::Worktree(path) if node.branch.as_ref() == Some(&name) => {
                        Some(path.clone())
                    }
                    _ => None,
                });
                Some(match worktree {
                    Some(path) => format!("{} && {delete}", popups::remove_worktree(&repo, &path)),
                    None => delete,
                })
            })
            .collect();
        self.focus.focus(window);
        if !lines.is_empty() {
            self.run_in_terminal(lines.join(" ; "), window, cx);
        }
        cx.notify();
    }

    fn prune_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_prune(window, cx),
            "enter" => self.run_prune(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The prune list: each stale branch with why, ticked to go, and a button
    /// that deletes the ticked ones.
    fn prune_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let list = self.prune.as_ref()?;
        let ticked = list.iter().filter(|(_, ticked)| *ticked).count();
        let rows = list.iter().enumerate().map(|(k, (name, on))| {
            let worktree = self.graph.nodes.iter().find(|node| {
                node.kind == wtf_core::NodeKind::Uncommitted && node.branch.as_ref() == Some(name)
            });
            let note = graph_view::stale_note(&self.graph, name).unwrap_or_default();
            div()
                .id(("prune", k))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(26.))
                .px(px(8.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|row| row.bg(theme.selection.opacity(0.5)))
                .when(!on, |row| row.text_color(theme.secondary))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some((_, on)) = this.prune.as_mut().and_then(|list| list.get_mut(k)) {
                        *on = !*on;
                        cx.notify();
                    }
                }))
                .child(attention_checkbox(
                    ("prune-check", k),
                    Some(*on),
                    true,
                    theme,
                ))
                .child(div().min_w_0().truncate().child(name.clone()))
                .children(worktree.map(|node| {
                    div()
                        .flex_none()
                        .px(px(5.))
                        .rounded(px(3.))
                        .bg(theme.tag_background)
                        .text_xs()
                        .child(format!(
                            "▣ {}",
                            graph_view::worktree_label(node, WorktreeNames::Folder)
                        ))
                }))
                .child(div().flex_1())
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.secondary)
                        .italic()
                        .child(note),
                )
        });
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(4.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.separator)
                .cursor_pointer()
                .hover(|button| button.bg(theme.selection))
                .child(label)
        };
        let width = f32::from(window.viewport_size().width);
        let panel_width = GOTO_WIDTH.min(width - 32.);
        let position = Point::new(px(((width - panel_width) / 2.).max(0.)), px(60.));
        let panel =
            div()
                .id("prune-panel")
                .track_focus(&self.prune_focus)
                .on_key_down(cx.listener(Self::prune_key))
                .occlude()
                .flex()
                .flex_col()
                .gap(px(6.))
                .w(px(panel_width))
                .p(px(12.))
                .rounded(px(8.))
                .border_1()
                .border_color(theme.separator)
                .bg(theme.background)
                .shadow_lg()
                .text_sm()
                .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_prune(window, cx)))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Prune stale branches and worktrees"),
                )
                .child(div().text_xs().text_color(theme.secondary).child(
                    "Untick what to keep. Worktrees go first, then their branches; merged ones \
                 as git checks them, others with their commits.",
                ))
                // Everything at once, then untick or tick from there.
                .when(!list.is_empty(), |panel| {
                    let all = |id: &'static str, label: &'static str, on: bool| {
                        div()
                            .id(id)
                            .text_xs()
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                for (_, ticked) in this.prune.iter_mut().flatten() {
                                    *ticked = on;
                                }
                                cx.notify();
                            }))
                            .child(label)
                    };
                    panel.child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(all("prune-all", "Select all", true))
                            .child(all("prune-none", "Deselect all", false)),
                    )
                })
                .child(
                    div()
                        .id("prune-list")
                        .flex()
                        .flex_col()
                        .max_h(px(360.))
                        .overflow_y_scroll()
                        .children(rows),
                )
                .when(list.is_empty(), |panel| {
                    panel.child(
                        div()
                            .text_color(theme.secondary)
                            .child("Nothing looks stale."),
                    )
                })
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(button("prune-cancel", "Cancel".to_string()).on_click(
                            cx.listener(|this, _, window, cx| this.close_prune(window, cx)),
                        ))
                        .when(ticked > 0, |buttons| {
                            buttons.child(
                                button("prune-run", format!("Delete {ticked}"))
                                    .bg(theme.error)
                                    .border_color(theme.error)
                                    .text_color(theme.tag_head_text)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.run_prune(window, cx)
                                    })),
                            )
                        }),
                );
        Some(
            deferred(anchored().position(position).child(panel))
                .with_priority(2)
                .into_any_element(),
        )
    }

    /// Asks for a command to open a worktree with, the last one offered:
    /// worktree node `node`, or the one selected, or the repository's.
    fn open_worktree_with(
        &mut self,
        node: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let node = node.or_else(|| match self.selected_nodes[..] {
            [selected] => Some(selected),
            _ => None,
        });
        let path = node
            .and_then(|node| match &self.graph.nodes.get(node)?.tip {
                Revision::Worktree(path) => Some(path.clone()),
                Revision::Commit(_) => None,
            })
            .or_else(|| self.repo_path.clone());
        if let Some(path) = path {
            let last = self.open_with.first().cloned().unwrap_or_default();
            self.show_prompt(PromptKind::OpenWith(path), last, window);
            cx.notify();
        }
    }

    fn remember_open_with(&mut self, command: &str) {
        popups::remember(&mut self.open_with, command);
        popups::save_recent("open-with", &self.open_with);
    }

    /// The worktree the palette runs commands on: the selected one, or
    /// the repository's.
    fn command_path(&self) -> Option<String> {
        let selected = match self.selected_nodes[..] {
            [selected] => self.graph.nodes.get(selected),
            _ => None,
        };
        match selected.map(|node| &node.tip) {
            Some(Revision::Worktree(path)) => Some(path.clone()),
            _ => self.repo_path.clone(),
        }
    }

    /// What the picker lists for its query. Go To: the places it matches.
    /// The palette: the commands it matches and those run before on the
    /// worktree; with ⌥ held, the query itself run on the worktree.
    fn goto_found(&self, window: &Window) -> Vec<goto::Entry> {
        let Some((query, _, palette)) = &self.goto else {
            return Vec::new();
        };
        if !*palette {
            let entries = goto::entries(&self.graph, self.worktree_names);
            return goto::filter(&entries, query).into_iter().cloned().collect();
        }
        let path = self.command_path();
        if window.modifiers().alt && !query.trim().is_empty() {
            return path
                .map(|path| goto::shell_entry(query, &path))
                .into_iter()
                .collect();
        }
        // The last commands run come first, then the app's.
        let entries: Vec<goto::Entry> = path
            .iter()
            .flat_map(|path| {
                self.shell_commands
                    .iter()
                    .map(move |command| goto::shell_entry(command, path))
            })
            .chain(palette::entries(window))
            .collect();
        goto::filter(&entries, query).into_iter().cloned().collect()
    }

    fn open_goto(&mut self, palette: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.goto = Some((String::new(), 0, palette));
        self.goto_focus.focus(window);
        cx.notify();
    }

    fn close_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.goto = None;
        self.focus.focus(window);
        cx.notify();
    }

    /// Jumps to a picked place: a worktree's or branch's node, or a commit.
    fn pick_goto(&mut self, entry: goto::Entry, window: &mut Window, cx: &mut Context<Self>) {
        self.close_goto(window, cx);
        if entry.kind == goto::Kind::Command {
            if let Some((_, action)) = palette::commands().into_iter().nth(entry.node) {
                window.dispatch_action(action, cx);
            }
            return;
        }
        if let Some((command, path)) = &entry.run {
            // Run in the terminal without opening the dock over the review.
            let line = popups::run_in(command, path);
            match &self.terminal {
                Some(terminal) => terminal.update(cx, |terminal, _| terminal.run(&line)),
                None => self.run_in_terminal(line, window, cx),
            }
            popups::remember(&mut self.shell_commands, command);
            popups::save_recent("commands", &self.shell_commands);
            return;
        }
        match entry.commit {
            Some(commit) => self.select_commit(entry.node, commit, cx),
            None => self.click_node(entry.node, false, cx),
        }
    }

    fn goto_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.goto_found(window);
        let path = self.command_path();
        let completions = std::mem::take(&mut self.completions);
        let Some((query, highlighted, palette)) = &mut self.goto else {
            return;
        };
        let keystroke = &event.keystroke;
        let found = entries.len();
        match keystroke.key.as_str() {
            "escape" => return self.close_goto(window, cx),
            // In the palette, Tab completes a file name in the worktree.
            "tab" if *palette && !query.trim().is_empty() => {
                let Some(path) = path else { return };
                let (completed, matches) = goto::complete(query, |folder| {
                    std::fs::read_dir(Path::new(&path).join(folder))
                        .into_iter()
                        .flatten()
                        .flatten()
                        .map(|entry| {
                            let folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
                            (entry.file_name().to_string_lossy().into_owned(), folder)
                        })
                        .collect()
                });
                // A second Tab with nothing more to add keeps the choices shown.
                self.completions = match matches.is_empty() && completed == *query {
                    true => completions,
                    false => matches,
                };
                *query = completed;
            }
            "enter" => {
                let picked = entries.get(*highlighted).cloned();
                // ⌘↩ on a worktree opens it with a command instead.
                let open_with = keystroke.modifiers.platform;
                match picked {
                    Some(entry) if open_with && entry.kind == goto::Kind::Worktree => {
                        self.close_goto(window, cx);
                        self.open_worktree_with(Some(entry.node), window, cx);
                    }
                    Some(entry) => self.pick_goto(entry, window, cx),
                    None => {}
                }
                return;
            }
            "down" => *highlighted = (*highlighted + 1).min(found.saturating_sub(1)),
            "up" => *highlighted = highlighted.saturating_sub(1),
            _ if type_key(query, keystroke, cx) => *highlighted = 0,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The Go To picker: a query above the worktrees, branches and commits
    /// it matches.
    fn goto_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (query, highlighted, commands) = self.goto.as_ref()?;
        let found = self.goto_found(window);
        let running = *commands && window.modifiers().alt && !query.trim().is_empty();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_secs() as i64);
        let rows = found.iter().take(GOTO_ROWS).enumerate().map(|(k, entry)| {
            let (icon, kind) = match entry.kind {
                goto::Kind::Worktree => ("▣", "worktree · ⌘↩ open with"),
                goto::Kind::Shell => ("$", "run in terminal"),
                goto::Kind::Branch => ("⎇", "branch"),
                goto::Kind::Commit => ("●", "commit"),
                goto::Kind::Command => ("›", ""),
            };
            let picked = entry.clone();
            div()
                .id(("goto", k))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(28.))
                .px(px(10.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(k == *highlighted, |row| row.bg(theme.selection))
                .hover(|row| row.bg(theme.selection.opacity(0.5)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.pick_goto(picked.clone(), window, cx)
                }))
                .child(div().w(px(14.)).text_color(theme.secondary).child(icon))
                .child(div().min_w_0().truncate().child(entry.label.clone()))
                .child(match entry.kind {
                    // A command's folder keeps its end in view: one too long
                    // is cut from the start, against the right edge.
                    goto::Kind::Shell => div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .justify_end()
                        .overflow_hidden()
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(
                            div()
                                .flex_none()
                                .whitespace_nowrap()
                                .child(entry.detail.clone()),
                        ),
                    _ => div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(entry.detail.clone()),
                })
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(match entry.time {
                            0 => kind.to_string(),
                            time => format!("{kind} · {}", short_age(time, now)),
                        }),
                )
        });
        let width = f32::from(window.viewport_size().width);
        let panel_width = GOTO_WIDTH.min(width - 32.);
        let position = Point::new(px(((width - panel_width) / 2.).max(0.)), px(60.));
        let panel = div()
            .id("goto-panel")
            .occlude()
            .flex()
            .flex_col()
            .gap(px(4.))
            .w(px(panel_width))
            .p(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .shadow_lg()
            .text_sm()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_goto(window, cx)))
            .child(
                div()
                    .id("goto-input")
                    .track_focus(&self.goto_focus)
                    .on_key_down(cx.listener(Self::goto_key))
                    // Holding ⌥ turns the query into a command to run.
                    .on_modifiers_changed(cx.listener(|_, _, _, cx| cx.notify()))
                    .flex()
                    .items_center()
                    .h(px(30.))
                    .px(px(10.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme.accent)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(field_text(
                        query,
                        if *commands {
                            "Run a command"
                        } else {
                            "Go to a worktree, branch or commit"
                        },
                        true,
                        theme,
                    )),
            )
            .children(rows)
            .when(found.is_empty(), |panel| {
                panel.child(
                    div()
                        .px(px(10.))
                        .py(px(6.))
                        .text_color(theme.secondary)
                        .child("Nothing matches"),
                )
            })
            .when(*commands, |panel| {
                let path = self.command_path().unwrap_or_default();
                let worktree = path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default();
                let hint = match (running, query.trim()) {
                    _ if !self.completions.is_empty() => self.completions.join("   "),
                    (true, _) => format!(
                        "↩ runs it in {} · ⇥ completes file names",
                        popups::tilde(&path)
                    ),
                    (false, "") => {
                        format!("Type a shell command and hold ⌥ to run it in {worktree}")
                    }
                    (false, query) => {
                        format!("Hold ⌥ to run “{query}” in {worktree} · ⇥ completes file names")
                    }
                };
                panel.child(
                    div()
                        .mt(px(2.))
                        .pt(px(6.))
                        .px(px(10.))
                        .border_t_1()
                        .border_color(theme.separator)
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(hint),
                )
            });
        Some(
            deferred(anchored().position(position).child(panel))
                .with_priority(2)
                .into_any_element(),
        )
    }

    fn show_prompt(&mut self, kind: PromptKind, input: String, window: &mut Window) {
        self.prompt = Some(Prompt { kind, input });
        self.prompt_focus.focus(window);
    }

    fn prompt_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        match keystroke.key.as_str() {
            "enter" => self.submit_prompt(window, cx),
            "escape" => {
                self.prompt = None;
                self.focus.focus(window);
            }
            "backspace" => {
                prompt.input.pop();
            }
            "v" if modifiers.platform => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    prompt.input.push_str(&text.replace('\n', " "));
                }
            }
            _ if !modifiers.platform && !modifiers.control => match &keystroke.key_char {
                Some(text) if !text.chars().any(char::is_control) => prompt.input.push_str(text),
                _ => return,
            },
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        self.focus.focus(window);
        let input = prompt.input.trim().to_string();
        if input.is_empty() {
            return;
        }
        let repo = self.repo_path.clone().unwrap_or_default();
        match prompt.kind {
            PromptKind::Rename(old) if old != input => {
                self.run_in_terminal(popups::rename_branch(&repo, &old, &input), window, cx);
                // Pins and hides follow the branch to its new name.
                self.change_attention(
                    |attention| {
                        for name in attention.pinned.iter_mut().chain(&mut attention.hidden) {
                            if *name == old {
                                *name = input.clone();
                            }
                        }
                    },
                    cx,
                );
            }
            PromptKind::Rename(_) => {}
            PromptKind::OpenWith(path) => {
                self.run_in_terminal(popups::open_with(&input, &path), window, cx);
                self.remember_open_with(&input);
            }
            PromptKind::StashWorktree { stash, base } => {
                let path = popups::worktree_path(&repo, &input);
                let line = popups::stash_worktree(&repo, &path, &input, &base, &stash);
                self.run_in_terminal(line, window, cx);
            }
            PromptKind::WorktreeFor(branch) => {
                self.run_in_terminal(popups::add_worktree(&repo, &input, &branch), window, cx);
            }
            PromptKind::NewBranch {
                commit,
                worktree: false,
            } => self.run_in_terminal(popups::new_branch(&repo, &input, &commit), window, cx),
            PromptKind::NewBranch {
                commit,
                worktree: true,
            } => {
                let path = popups::worktree_path(&repo, &input);
                let line = popups::add_worktree_at(&repo, &path, &input, &commit);
                self.run_in_terminal(line, window, cx);
            }
        }
    }

    /// The worktrees a branch can be checked out in: all but one that has it.
    fn checkout_worktrees(&self, target: &Target) -> Vec<(String, String)> {
        let name = match target {
            Target::Branch { name, .. } => Some(name),
            // A stash goes into any worktree.
            Target::Stash { .. } => None,
            _ => return Vec::new(),
        };
        self.graph
            .nodes
            .iter()
            .filter(|node| node.kind == wtf_core::NodeKind::Uncommitted)
            .filter(|node| name.is_none() || node.branch.as_ref() != name)
            .filter_map(|node| match &node.tip {
                Revision::Worktree(path) => Some((
                    path.clone(),
                    graph_view::worktree_label(node, self.worktree_names),
                )),
                Revision::Commit(_) => None,
            })
            .collect()
    }

    fn context_menu_view(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (target, position) = self.context_menu.as_ref()?;
        let main = self.graph.main.as_str();
        let mut menu = div()
            .id("context-menu")
            .flex()
            .flex_col()
            .min_w(px(200.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .shadow_md()
            .text_sm()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                cx.notify();
            }));
        let name = popups::attention_name(target);
        let shown = self
            .graph
            .branches
            .iter()
            .find(|branch| branch.name == name)
            .is_none_or(|branch| branch.shown);
        let worktrees: Vec<String> = self
            .checkout_worktrees(target)
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        let overridden =
            self.attention.pinned.contains(&name) || self.attention.hidden.contains(&name);
        for (k, item) in
            popups::commands(target, main, &self.open_with, shown, overridden, &worktrees)
                .into_iter()
                .enumerate()
        {
            menu = match item {
                None => menu.child(div().h(px(1.)).my(px(4.)).bg(theme.separator)),
                Some((command, label)) => menu.child(
                    div()
                        .id(("context-item", k))
                        .px(px(12.))
                        .py(px(3.))
                        .hover(|item| item.bg(theme.selection))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run_command(command, window, cx)
                        }))
                        .child(label),
                ),
            };
        }
        Some(
            deferred(anchored().position(*position).snap_to_window().child(menu))
                .with_priority(1)
                .into_any_element(),
        )
    }

    fn prompt_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let prompt = self.prompt.as_ref()?;
        let title = match &prompt.kind {
            PromptKind::Rename(name) => format!("Rename branch {name}"),
            PromptKind::OpenWith(_) => {
                "Open worktree with a command; its path is appended".to_string()
            }
            PromptKind::WorktreeFor(branch) => format!("New worktree for {branch} at"),
            PromptKind::StashWorktree { stash, .. } => {
                format!("New worktree with {stash}, on a new branch named")
            }
            PromptKind::NewBranch {
                commit,
                worktree: false,
            } => format!("New branch at {}", &commit[..commit.len().min(7)]),
            PromptKind::NewBranch {
                commit,
                worktree: true,
            } => format!(
                "New worktree at {} on a new branch named",
                &commit[..commit.len().min(7)]
            ),
        };
        let width = f32::from(window.viewport_size().width);
        let position = Point::new(px(((width - PROMPT_WIDTH) / 2.).max(0.)), px(80.));
        let panel = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .w(px(PROMPT_WIDTH))
            .p(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .shadow_lg()
            .text_sm()
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
            .child(
                div()
                    .id("prompt-input")
                    .track_focus(&self.prompt_focus)
                    .on_key_down(cx.listener(Self::prompt_key))
                    .flex()
                    .items_center()
                    .h(px(26.))
                    .px(px(8.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme.accent)
                    .font_family(preferences::code_font())
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(prompt.input.clone())
                    .child(div().w(px(1.5)).h(px(14.)).bg(theme.foreground)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.secondary)
                    .child("Enter to run in the terminal · Esc to cancel"),
            );
        Some(
            deferred(anchored().position(position).child(panel))
                .with_priority(2)
                .into_any_element(),
        )
    }

    /// What the diff shows, above it: the selected commit's message, or the
    /// subjects of several, next to the layout and diff mode toggles.
    /// Diff rows the commit header covers before the first real one.
    fn header_rows(&self) -> usize {
        match self.range {
            Some(_) => (self.header_height / diff_view::row_height()).ceil() as usize,
            None => 0,
        }
    }

    /// The list row to scroll to for diff row `row`, so it shows just below
    /// the summary bar.
    fn below_bar(&self, row: usize) -> usize {
        let bar = (summary_bar_height() / diff_view::row_height()).ceil() as usize;
        (row + self.header_rows()).saturating_sub(bar)
    }

    /// The commits the header describes.
    /// The key a selection of several commits has its summary by.
    fn summary_key(&self) -> Option<String> {
        let commits = self.header_commits();
        (commits.len() >= 2).then(|| {
            commits
                .iter()
                .map(|commit| commit.id.as_str())
                .collect::<Vec<_>>()
                .join(",")
        })
    }

    /// Asks the model for a summary of the selected commits, when there are
    /// several, summaries are on, and none is made or on its way.
    fn request_summary(&mut self, cx: &mut Context<Self>) {
        let (Some(key), Some(repo)) = (self.summary_key(), self.repo_path.clone()) else {
            return;
        };
        if !preferences::summaries() || self.summaries.contains_key(&key) {
            return;
        }
        let mut indices = self.narrowed.clone();
        if indices.is_empty() {
            indices = (0..self.commits.len()).collect();
        }
        let Some((base, tip)) = commit_range(&self.commits, &indices) else {
            return;
        };
        let commits: Vec<Commit> = self.header_commits().into_iter().cloned().collect();
        self.summaries.insert(key.clone(), None);
        let prompt = preferences::summary_prompt();
        let run = cx.background_executor().spawn(async move {
            wtf_core::summarize_commits(&repo, &commits, base.as_deref(), &tip, &prompt)
                .map_err(message)
        });
        cx.spawn(async move |this, cx| {
            let summary = run.await;
            this.update(cx, |this, cx| {
                this.summaries.insert(key, Some(summary));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The summary of the selected commits, once the model has written it.
    fn summary(&self) -> Option<&wtf_core::Summary> {
        match self.summaries.get(&self.summary_key()?)? {
            Some(Ok(summary)) if preferences::summaries() => Some(summary),
            _ => None,
        }
    }

    fn header_commits(&self) -> Vec<&Commit> {
        let mut indices = self.narrowed.clone();
        if indices.is_empty() {
            indices = (0..self.commits.len()).collect();
        }
        indices.sort_unstable();
        indices
            .iter()
            .filter_map(|&i| self.commits.get(i))
            .collect()
    }

    /// The one-file or all-files and structural or text switches, reading
    /// down the strip.
    fn view_toggles(&self, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let option = |label: &'static str, selected: bool| {
            strip_option(label, selected, theme).child(vertical_label(
                label,
                theme.foreground,
                window,
            ))
        };
        let group = || strip_group(theme);
        let layout = |label: &'static str, layout: ChangesLayout| {
            option(label, self.changes_layout == layout)
                .on_click(cx.listener(move |this, _, _, cx| this.set_changes_layout(layout, cx)))
        };
        let mode = |label: &'static str, mode: DiffMode| {
            option(label, self.diff_mode == mode)
                .on_click(cx.listener(move |this, _, _, cx| this.set_diff_mode(mode, cx)))
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(STRIP_GAP))
            .child(
                group()
                    .child(layout("File by file", ChangesLayout::OneFile))
                    .child(layout("All files", ChangesLayout::AllFiles)),
            )
            .child(
                group()
                    .child(mode("Structural", DiffMode::Structural))
                    .child(mode("Text", DiffMode::Text)),
            )
    }

    /// The commit header's first line, kept at the top once the rest of it
    /// scrolls away.
    fn summary_bar(&self, theme: &Theme) -> impl IntoElement {
        // The newest commit's first line: in bold when it is the only one.
        let (summary, bold) = match self.header_commits()[..] {
            [] => ("Uncommitted changes".to_string(), true),
            [commit] => (commit.summary.clone(), true),
            [commit, ..] => match self.summary() {
                Some(summary) => (summary.header.clone(), true),
                None => (commit.summary.clone(), false),
            },
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(summary_bar_height()))
            .px(px(12.))
            .py(px(HEADER_PADDING))
            .bg(theme.background)
            .border_b_1()
            .border_color(theme.separator)
            .font_family(preferences::ui_font())
            .text_sm()
            .line_height(rems(HEADER_LINE))
            .flex()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(bold, |line| line.font_weight(gpui::FontWeight::SEMIBOLD))
                    .child(summary),
            )
    }

    fn commit_header(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let commits = self.header_commits();
        let uncommitted =
            self.narrowed.is_empty() && matches!(self.range, Some((_, Revision::Worktree(_))));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_secs() as i64);

        let mut description = div().flex().flex_col().gap(px(4.)).min_w_0();
        match commits[..] {
            [] => {}
            [commit] => {
                let editable = self.reword_worktree(&commit.id).is_some();
                let edited = (*commit).clone();
                let editing = self
                    .rewording
                    .as_ref()
                    .filter(|(id, _, _)| *id == commit.id)
                    .map(|(_, _, message)| message.as_str());
                let link = |id: &'static str, label: &'static str| {
                    div()
                        .id(id)
                        .text_color(theme.accent)
                        .cursor_pointer()
                        .child(label)
                };
                // Editing keeps the message's layout: the first line in bold,
                // the rest below it, so the header does not change height.
                let message = match editing {
                    None => self.selectable_message(commit, theme, cx),
                    Some(text) => {
                        let focused = self.reword_focus.is_focused(window);
                        let (first, rest) = match text.split_once('\n') {
                            Some((first, rest)) => {
                                (first, Some(rest.strip_prefix('\n').unwrap_or(rest)))
                            }
                            None => (text, None),
                        };
                        let first_line = match rest {
                            None => field_text(first, "Commit message", focused, theme),
                            Some(_) => field_text(first, "", false, theme),
                        };
                        div()
                            .id("reword-message")
                            .track_focus(&self.reword_focus)
                            .on_key_down(cx.listener(Self::reword_key))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .mx(px(-4.))
                            .px(px(4.))
                            .rounded(px(4.))
                            .bg(theme.selection)
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(first_line),
                            )
                            .children(rest.map(|rest| {
                                div()
                                    .text_color(theme.secondary)
                                    .child(field_text(rest, "", focused, theme))
                            }))
                            .into_any_element()
                    }
                };
                let sha = commit.id.clone();
                let copied = self.copied.as_deref() == Some(commit.id.as_str());
                description = description.child(message).child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(
                            div()
                                .flex()
                                .child(
                                    div()
                                        .id("copy-sha")
                                        .cursor_pointer()
                                        .hover(|sha| sha.text_color(theme.foreground))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.copy_sha(sha.clone(), cx)
                                        }))
                                        .child(match copied {
                                            true => "Copied".to_string(),
                                            false => commit.short_id.clone(),
                                        }),
                                )
                                .child(format!(
                                    " · {} · {}",
                                    commit.author,
                                    relative_time(commit.time, now)
                                )),
                        )
                        .map(|line| match editing {
                            Some(_) => line
                                .child(
                                    link("reword-save", "Save")
                                        .on_click(cx.listener(|this, _, _, cx| this.reword(cx))),
                                )
                                .child(link("reword-cancel", "Cancel").on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.rewording = None;
                                        this.focus.focus(window);
                                        cx.notify();
                                    },
                                )))
                                .child("⌘↩ saves; later commits are copied"),
                            None if editable => {
                                line.child(link("reword", "Edit message").on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.start_reword(&edited, window, cx)
                                    },
                                )))
                            }
                            None => line,
                        }),
                );
            }
            ref several => {
                // A summary from the model leads, when summaries are on; the
                // commits' own subjects show while it is written, when it
                // failed, or when asked for.
                let state = self
                    .summary_key()
                    .and_then(|key| self.summaries.get(&key).cloned());
                let summarizing = preferences::summaries() && matches!(state, Some(None) | None);
                let summary = self.summary().cloned();
                if let Some(summary) = &summary {
                    let mut text = summary.header.clone();
                    let header = text.len();
                    for detail in &summary.details {
                        text.push_str("\n• ");
                        text.push_str(detail);
                    }
                    let styles = vec![
                        (
                            0..header,
                            HighlightStyle {
                                font_weight: Some(gpui::FontWeight::SEMIBOLD),
                                ..Default::default()
                            },
                        ),
                        (
                            header..text.len(),
                            HighlightStyle {
                                color: Some(theme.secondary),
                                ..Default::default()
                            },
                        ),
                    ];
                    let key = format!("summary-{}", self.summary_key().unwrap_or_default());
                    description =
                        description.child(self.selectable_text(key, text, styles, None, theme, cx));
                }
                let failed = match &state {
                    Some(Some(Err(error))) if preferences::summaries() => Some(error.clone()),
                    _ => None,
                };
                let show_subjects = summary.is_none() || self.show_subjects;
                // Each commit's first line, marked when its message goes on,
                // as one text to select; their hashes and times beside them.
                let subjects: Vec<String> = several
                    .iter()
                    .map(|commit| {
                        let more = if commit.body.is_empty() { "" } else { " …" };
                        format!("{}{more}", commit.summary)
                    })
                    .collect();
                let key = several
                    .iter()
                    .map(|commit| commit.short_id.as_str())
                    .collect();
                let stamps = several.iter().map(|commit| {
                    div().child(format!(
                        "{} · {}",
                        commit.short_id,
                        relative_time(commit.time, now)
                    ))
                });
                description = description
                    .when(show_subjects, |description| {
                        description.child(
                            div()
                                .flex()
                                .gap(px(12.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(self.selectable_text(
                                            key,
                                            subjects.join("\n"),
                                            Vec::new(),
                                            None,
                                            theme,
                                            cx,
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(theme.secondary)
                                        .children(stamps),
                                ),
                        )
                    })
                    // Who and when, and what the summary is and how to see
                    // the commits under it.
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .text_xs()
                            .text_color(theme.secondary)
                            .child(aggregate(several, now))
                            .when(summarizing, |line| line.child("· ✦ Summarizing…"))
                            .children(failed.map(|error| {
                                div().truncate().child(format!("· no summary: {error}"))
                            }))
                            .when(summary.is_some(), |line| {
                                line.child("· ✦ summary by Apple Intelligence ·").child(
                                    div()
                                        .id("show-subjects")
                                        .text_color(theme.accent)
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.show_subjects = !this.show_subjects;
                                            cx.notify();
                                        }))
                                        .child(match self.show_subjects {
                                            true => "Hide commits",
                                            false => "Show commits",
                                        }),
                                )
                            }),
                    );
            }
        }
        if uncommitted {
            let label = if commits.is_empty() {
                "Uncommitted changes"
            } else {
                "and uncommitted changes"
            };
            description = description.child(
                div()
                    .when(commits.is_empty(), |label| {
                        label.font_weight(gpui::FontWeight::SEMIBOLD)
                    })
                    .when(!commits.is_empty(), |label| {
                        label.text_xs().text_color(theme.secondary)
                    })
                    .child(label),
            );
        }

        // The diff leaves room for the header as it lays out.
        let view = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let height = f32::from(bounds.size.height);
                view.update(cx, |this, cx| {
                    if (this.header_height - height).abs() > 0.5 {
                        this.header_height = height;
                        cx.notify();
                    }
                })
                .ok();
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        div()
            .relative()
            .flex()
            .flex_none()
            .items_start()
            .px(px(12.))
            .py(px(HEADER_PADDING))
            .bg(theme.background)
            .border_b_1()
            .border_color(theme.separator)
            .font_family(preferences::ui_font())
            .text_sm()
            .line_height(rems(HEADER_LINE))
            .child(measure)
            .child(div().flex_1().min_w_0().child(description))
    }

    /// The diff `list` with the commit header scrolling above its first rows,
    /// and the summary bar and a `pinned` file header kept on top.
    fn under_header(
        &self,
        list: AnyElement,
        scroll_y: f32,
        pinned: Option<AnyElement>,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let header = self.range.is_some();
        let scrolled = header && scroll_y > 0.;
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .child(list)
            .when(header && scroll_y < self.header_height, |pane| {
                pane.child(
                    div()
                        .absolute()
                        .top(px(-scroll_y))
                        .left_0()
                        .right_0()
                        .child(self.commit_header(theme, window, cx)),
                )
            })
            .children(pinned.map(|pinned| {
                div()
                    .absolute()
                    .top(px(if scrolled { summary_bar_height() } else { 0. }))
                    .left_0()
                    .right_0()
                    .child(pinned)
            }))
            .when(scrolled, |pane| pane.child(self.summary_bar(theme)))
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

    fn restack(&mut self) {
        let folded: Vec<bool> = self
            .files
            .iter()
            .map(|file| self.folded.contains(display_path(file)))
            .collect();
        let only = match self.file_by_file_worktree() {
            true => Some(self.selected_file.unwrap_or(0)),
            false => None,
        };
        self.stacked = stack_items(
            &self.stacked_diffs,
            &folded,
            self.worktree_split,
            self.folded_sections,
            only,
        );
    }

    fn toggle_section(&mut self, staged: bool, cx: &mut Context<Self>) {
        let folded = &mut self.folded_sections[usize::from(staged)];
        *folded = !*folded;
        self.restack();
        cx.notify();
    }

    /// The files of the unstaged or staged section.
    fn section_files(&self, staged: bool) -> std::ops::Range<usize> {
        let split = self.worktree_split.unwrap_or(0).min(self.files.len());
        match staged {
            true => split..self.files.len(),
            false => 0..split,
        }
    }

    /// Folds every file of a section, or unfolds them all when all are folded.
    fn fold_section_files(&mut self, staged: bool, cx: &mut Context<Self>) {
        let paths: Vec<String> = self.files[self.section_files(staged)]
            .iter()
            .map(|file| display_path(file).to_string())
            .collect();
        if paths.iter().all(|path| self.folded.contains(path)) {
            for path in &paths {
                self.folded.remove(path);
            }
        } else {
            self.folded.extend(paths);
        }
        self.restack();
        cx.notify();
    }

    fn toggle_fold(&mut self, file: usize, cx: &mut Context<Self>) {
        let Some(path) = self
            .files
            .get(file)
            .map(|file| display_path(file).to_string())
        else {
            return;
        };
        if !self.folded.remove(&path) {
            self.folded.insert(path);
        }
        self.restack();
        cx.notify();
    }

    /// Code colors for a file's diff: one of the all-files view by its index,
    /// or the one-file view's. Worked out once per file and appearance.
    fn diff_colors(&self, file: Option<usize>, theme: &Theme) -> Option<Rc<syntax::Colors>> {
        let (path, diff) = match file {
            Some(file) => (
                self.files.get(file)?,
                self.stacked_diffs.get(file)?.as_ref().ok()?,
            ),
            None => (self.files.get(self.selected_file?)?, self.diff.as_ref()?),
        };
        let path = display_path(path).to_string();
        let dark = theme.background.l < 0.5;
        // A file staged in part is listed twice, its two diffs apart, so
        // the all-files view keys colors by entry rather than by path.
        let key = (path.clone(), dark, file);
        if let Some(colors) = self.syntax.borrow().get(&key) {
            return Some(colors.clone());
        }
        let colors = Rc::new(syntax::colors(&path, diff, dark));
        self.syntax.borrow_mut().insert(key, colors.clone());
        Some(colors)
    }

    /// A row of the all-files view. Rows can still name files and lines of
    /// the last listing while a new one loads; those draw empty.
    fn stack_row(&self, item: &StackItem, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if !self.stack_item_fits(item) {
            return div()
                .w_full()
                .h(px(diff_view::row_height()))
                .into_any_element();
        }
        let element = self.stack_row_unchecked(item, theme, cx);
        // Rows share one height, so a section's or file's header fills its
        // own row and the pad row above it, from where it is drawn.
        match item {
            StackItem::Header(_) | StackItem::Section(..) => {
                let height = diff_view::row_height();
                div()
                    .relative()
                    .w_full()
                    .h(px(height))
                    .child(
                        div()
                            .absolute()
                            .top(px(-height))
                            .left_0()
                            .right_0()
                            .child(element),
                    )
                    .into_any_element()
            }
            _ => element,
        }
    }

    /// Whether a row's file and lines are in the files and diffs now held.
    fn stack_item_fits(&self, item: &StackItem) -> bool {
        let rows = |file: usize| match self.stacked_diffs.get(file) {
            Some(Ok(diff)) => Some(diff.rows.len()),
            Some(Err(_)) => Some(0),
            None => None,
        };
        match *item {
            StackItem::Header(file) => file < self.files.len() && rows(file).is_some(),
            StackItem::Row(file, index) => {
                file < self.files.len() && rows(file).is_some_and(|n| index < n || n == 0)
            }
            StackItem::Hunk(file, _, end) => {
                file < self.files.len() && rows(file).is_some_and(|n| end <= n)
            }
            _ => true,
        }
    }

    fn stack_row_unchecked(
        &self,
        item: &StackItem,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = || {
            div()
                .w_full()
                .h(px(diff_view::row_height()))
                .overflow_hidden()
                .whitespace_nowrap()
        };
        // Sublime Merge's buttons: on sections for all files, on headers for one.
        // Shorter than a row, so buttons on rows next to each other never touch.
        let button = |id: SharedString, label: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .h(px(diff_view::row_height() - 3.))
                .line_height(px(diff_view::row_height() - 5.))
                .px(px(10.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.separator)
                .font_family(crate::preferences::ui_font())
                // As large as the code they sit by, a little lighter.
                .text_size(px(crate::preferences::code_size() * 0.95))
                .hover(|button| button.bg(theme.selection))
                .cursor_pointer()
                .child(label)
        };
        // Section and file headers are two rows tall: their buttons get
        // room to match.
        let tall = |id: SharedString, label: &'static str| {
            button(id, label)
                .h(px(1.4 * diff_view::row_height()))
                .px(px(12.))
                .rounded(px(5.))
        };
        match item {
            &StackItem::Section(staged, count) => {
                let button = tall;
                let title = if staged {
                    "Staged Files"
                } else {
                    "Unstaged Files"
                };
                let label = if staged { "Unstage All" } else { "Stage All" };
                let section_folded = self.folded_sections[usize::from(staged)];
                let all_folded = self.files[self.section_files(staged)]
                    .iter()
                    .all(|file| self.folded.contains(display_path(file)));
                let fold_label = if all_folded {
                    "Expand Files"
                } else {
                    "Collapse Files"
                };
                div()
                    .w_full()
                    .h(px(2. * diff_view::row_height()))
                    .whitespace_nowrap()
                    .border_t_1()
                    .border_color(theme.separator)
                    .id(SharedString::from(format!("section-row-{staged}")))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .font_family(crate::preferences::ui_font())
                    // The interface font reads smaller than code at one size,
                    // so titles go up to stand over the diff.
                    .text_size(px(crate::preferences::code_size() * 1.2))
                    .font_weight(gpui::FontWeight::BOLD)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(staged, cx)))
                    .child(chevron(
                        !section_folded,
                        theme.secondary,
                        preferences::code_size(),
                    ))
                    .child(format!("{title} ({count})"))
                    .child(div().flex_1())
                    .when(count > 0 && !section_folded, |section| {
                        section.child(
                            button(
                                SharedString::from(format!("fold-files-{staged}")),
                                fold_label,
                            )
                            .font_weight(gpui::FontWeight::NORMAL)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.fold_section_files(staged, cx)
                                },
                            )),
                        )
                    })
                    .when(count > 0 && self.stash_view.is_none(), |section| {
                        section.child(
                            button(SharedString::from(format!("section-{staged}")), label)
                                .font_weight(gpui::FontWeight::NORMAL)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.stage(staged, Vec::new(), cx)
                                })),
                        )
                    })
                    .into_any_element()
            }
            &StackItem::Header(index) => {
                let button = tall;
                let file = &self.files[index];
                let folded = self.folded.contains(display_path(file));
                // A stash shows read-only: no buttons.
                let staged = self
                    .worktree_split
                    .map(|split| index >= split)
                    .filter(|_| self.stash_view.is_none());
                let path = display_path(file).to_string();
                div()
                    .w_full()
                    .h(px(2. * diff_view::row_height()))
                    .whitespace_nowrap()
                    .id(("file-header", index))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .bg(theme.graph_background)
                    .border_t_1()
                    .border_color(theme.separator)
                    .font_family(crate::preferences::ui_font())
                    .text_size(px(crate::preferences::code_size() * 1.1))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_fold(index, cx)))
                    .child(chevron(!folded, theme.secondary, preferences::code_size()))
                    .child(div().text_color(theme.secondary).child(file.status.clone()))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(display_path(file).to_string()),
                    )
                    .children(staged.map(|staged| {
                        let label = if staged { "Unstage File" } else { "Stage File" };
                        let lines = match &self.line_selection {
                            Some((file, rows)) if *file == index => rows.len(),
                            _ => 0,
                        };
                        let lines_label = if staged {
                            "Unstage Lines"
                        } else {
                            "Stage Lines"
                        };
                        div()
                            .flex_1()
                            .flex()
                            .justify_end()
                            .gap(px(6.))
                            // Unstaged changes can be thrown away, picked
                            // lines or the whole file.
                            .when(lines > 0 && !staged, |buttons| {
                                buttons.child(
                                    button(
                                        SharedString::from(format!("discard-lines-{index}")),
                                        "Discard Lines",
                                    )
                                    .text_color(theme.error)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.discard_rows(index, Vec::new(), window, cx)
                                    }))
                                    .child(format!(" ({lines})")),
                                )
                            })
                            .when(!staged, |buttons| {
                                let stash_path = path.clone();
                                buttons.child(
                                    button(
                                        SharedString::from(format!("stash-{index}")),
                                        "Stash File",
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            if let Some(worktree) = this.staging_worktree() {
                                                let line = popups::stash_push(
                                                    &worktree,
                                                    "",
                                                    std::slice::from_ref(&stash_path),
                                                );
                                                this.run_in_terminal(line, window, cx);
                                            }
                                        },
                                    )),
                                )
                            })
                            .when(!staged, |buttons| {
                                buttons.child(
                                    button(
                                        SharedString::from(format!("discard-{index}")),
                                        "Discard File",
                                    )
                                    .text_color(theme.error)
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.discard_file(index, window, cx)
                                        },
                                    )),
                                )
                            })
                            .when(lines > 0, |buttons| {
                                buttons.child(
                                    button(
                                        SharedString::from(format!("lines-{index}")),
                                        lines_label,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.stage_selected_lines(staged, cx)
                                    }))
                                    .child(format!(" ({lines})")),
                                )
                            })
                            .child(
                                button(SharedString::from(format!("stage-{index}")), label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.stage(staged, vec![path.clone()], cx)
                                    })),
                            )
                    }))
                    .into_any_element()
            }
            &StackItem::Row(file, index) => {
                let colors = self.diff_colors(Some(file), theme);
                match &self.stacked_diffs[file] {
                    Ok(diff) if self.worktree_split.is_some() && self.stash_view.is_none() => {
                        // Lines are picked for staging: a bar marks the selection.
                        let selected = matches!(&self.line_selection, Some((f, rows)) if *f == file && rows.contains(&index));
                        let staged = self.worktree_split.is_some_and(|split| file >= split);
                        let group = SharedString::from(format!("line-{file}-{index}"));
                        let changed = diff_view::is_change(&diff.rows[index]);
                        let label = if staged { "Unstage Line" } else { "Stage Line" };
                        let hovered = self.hovered_line.get() == Some((file, index));
                        div()
                            .id(("line", file * 100_000 + index))
                            .group(group.clone())
                            .relative()
                            .flex()
                            .w_full()
                            .cursor_pointer()
                            .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                                let line = Some((file, index));
                                match over {
                                    true => this.hovered_line.set(line),
                                    false if this.hovered_line.get() == line => {
                                        this.hovered_line.set(None)
                                    }
                                    false => return,
                                }
                                cx.notify();
                            }))
                            // A changed line offers its own buttons while the
                            // mouse is over it; other lines carry nothing.
                            .when(changed && hovered, |line| {
                                line.child(
                                    div()
                                        .absolute()
                                        .top(px(2.))
                                        .right(px(8.))
                                        .flex()
                                        .gap(px(6.))
                                        .when(!staged, |buttons| {
                                            buttons.child(
                                                button(
                                                    SharedString::from(format!(
                                                        "discard-line-{file}-{index}"
                                                    )),
                                                    "Discard",
                                                )
                                                .h(px(diff_view::row_height() - 4.))
                                                .bg(theme.background)
                                                .text_color(theme.error)
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        this.discard_rows(
                                                            file,
                                                            vec![index],
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                )),
                                            )
                                        })
                                        .child(
                                            button(
                                                SharedString::from(format!(
                                                    "stage-line-{file}-{index}"
                                                )),
                                                label,
                                            )
                                            .h(px(diff_view::row_height() - 4.))
                                            .bg(theme.background)
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.stage_rows(file, vec![index], staged, cx)
                                                }),
                                            ),
                                        ),
                                )
                            })
                            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                this.select_line(file, index, event.modifiers(), cx)
                            }))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(3.))
                                    .h_full()
                                    .when(selected, |bar| bar.bg(theme.accent)),
                            )
                            .child(
                                diff_view::row(&diff.rows[index], theme, colors.as_deref())
                                    .flex_1(),
                            )
                            .into_any_element()
                    }
                    Ok(diff) => diff_view::row(&diff.rows[index], theme, colors.as_deref())
                        .into_any_element(),
                    Err(_) => row().into_any_element(),
                }
            }
            &StackItem::Hunk(file, start, end) => {
                let staged = self.worktree_split.is_some_and(|split| file >= split);
                let label = if staged { "Unstage Hunk" } else { "Stage Hunk" };
                // The hunk's first lines on each side, as `@@` headers show them.
                let lines = match &self.stacked_diffs[file] {
                    Ok(diff) => {
                        let rows = &diff.rows[start..end];
                        let first = |side: fn(&wtf_core::DiffRow) -> Option<u32>| {
                            rows.iter()
                                .find_map(side)
                                .map_or("—".to_string(), |n| n.to_string())
                        };
                        format!(
                            "@@ −{} +{} @@",
                            first(|row| row.left.as_ref().map(|side| side.line_number)),
                            first(|row| row.right.as_ref().map(|side| side.line_number)),
                        )
                    }
                    Err(_) => String::new(),
                };
                row()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(diff_view::gutter_width()))
                    .pr(px(8.))
                    .bg(theme.filler)
                    .text_color(theme.secondary)
                    .child(div().flex_1().child(lines))
                    .when(!staged && self.stash_view.is_none(), |hunk| {
                        hunk.child(
                            button(
                                SharedString::from(format!("discard-hunk-{file}-{start}")),
                                "Discard Hunk",
                            )
                            .text_color(theme.error)
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.discard_rows(file, (start..end).collect(), window, cx)
                                },
                            )),
                        )
                    })
                    .when(self.stash_view.is_none(), |hunk| {
                        hunk.child(
                            button(SharedString::from(format!("hunk-{file}-{start}")), label)
                                .text_color(theme.foreground)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.stage_rows(file, (start..end).collect(), staged, cx)
                                })),
                        )
                    })
                    .into_any_element()
            }
            StackItem::Gap => diff_view::gap(theme).into_any_element(),
            StackItem::Space | StackItem::HeaderPad => row().into_any_element(),
            StackItem::Note(text) => row()
                .pl(px(48.))
                .text_color(theme.secondary)
                .child(text.clone())
                .into_any_element(),
        }
    }
}

/// Lays out the all-files view: each file's header, then, unless the file is
/// folded, its changes with context or a note when there is nothing to show.
/// With `split`, the files before it are unstaged and the rest staged, each
/// group under its section.
fn stack_items(
    diffs: &[Result<FileDiff, String>],
    folded: &[bool],
    split: Option<usize>,
    folded_sections: [bool; 2],
    only: Option<usize>,
) -> Vec<StackItem> {
    let mut items = Vec::new();
    if let Some(split) = split {
        items.extend([StackItem::Space, StackItem::Section(false, split)]);
    }
    for (file, diff) in diffs.iter().enumerate() {
        if split == Some(file) {
            items.extend([
                StackItem::Space,
                StackItem::Section(true, diffs.len() - file),
            ]);
        }
        if let Some(split) = split
            && folded_sections[usize::from(file >= split)]
        {
            continue;
        }
        if only.is_some_and(|only| only != file) {
            continue;
        }
        items.extend([StackItem::HeaderPad, StackItem::Header(file)]);
        if folded.get(file).copied().unwrap_or(false) {
            continue;
        }
        let hunks = match diff {
            Ok(diff) => diff_view::hunks(&diff.rows, CONTEXT_ROWS),
            Err(error) => {
                items.push(StackItem::Note(error.clone()));
                continue;
            }
        };
        if hunks.is_empty() {
            items.push(StackItem::Note("No visible changes".to_string()));
        }
        if split.is_none() {
            items.extend(hunks.into_iter().map(|row| match row {
                Some(row) => StackItem::Row(file, row),
                None => StackItem::Gap,
            }));
            continue;
        }
        // A worktree file's hunks each get a header to stage them by.
        for hunk in hunks.split(Option::is_none).filter(|hunk| !hunk.is_empty()) {
            let rows: Vec<usize> = hunk.iter().flatten().copied().collect();
            items.push(StackItem::Hunk(file, rows[0], rows[rows.len() - 1] + 1));
            items.extend(rows.into_iter().map(|row| StackItem::Row(file, row)));
        }
    }
    if split.is_some_and(|split| split >= diffs.len()) {
        items.extend([StackItem::Space, StackItem::Section(true, 0)]);
    }
    items
}

/// The diff covering selected commits of a newest-first list: from the oldest
/// one's parent to the newest one; `None` when nothing is selected.
fn commit_range(commits: &[Commit], selected: &[usize]) -> Option<(Option<String>, Revision)> {
    let newest = commits.get(*selected.iter().min()?)?;
    let oldest = commits.get(*selected.iter().max()?)?;
    Some((oldest.parent.clone(), Revision::Commit(newest.id.clone())))
}

/// "5 commits · Ann, Bot · 3 days ago – 2 hours ago": the authors, most
/// commits first, and the time span of several commits.
fn aggregate(commits: &[&Commit], now: i64) -> String {
    let mut authors: Vec<(&str, usize)> = Vec::new();
    for commit in commits {
        match authors.iter_mut().find(|(name, _)| *name == commit.author) {
            Some((_, count)) => *count += 1,
            None => authors.push((&commit.author, 1)),
        }
    }
    authors.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
    let authors: Vec<&str> = authors.into_iter().map(|(name, _)| name).collect();
    let oldest = commits
        .iter()
        .map(|commit| commit.time)
        .min()
        .unwrap_or(now);
    let newest = commits
        .iter()
        .map(|commit| commit.time)
        .max()
        .unwrap_or(now);
    let (from, to) = (relative_time(oldest, now), relative_time(newest, now));
    let span = if from == to {
        from
    } else {
        format!("{from} – {to}")
    };
    format!(
        "{} commits · {} · {span}",
        commits.len(),
        authors.join(", ")
    )
}

/// A compact age for lists: "5h", "3d", "2w", "4mo", "1y".
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs() as i64)
}

fn short_age(time: i64, now: i64) -> String {
    let seconds = (now - time).max(0);
    match seconds {
        0..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        86_400..1_209_600 => format!("{}d", seconds / 86_400),
        1_209_600..5_184_000 => format!("{}w", seconds / 604_800),
        5_184_000..31_536_000 => format!("{}mo", seconds / 2_592_000),
        _ => format!("{}y", seconds / 31_536_000),
    }
}

/// A short "3 days ago" style age.
fn relative_time(time: i64, now: i64) -> String {
    let seconds = (now - time).max(0);
    let (count, unit) = match seconds {
        0..60 => return "just now".to_string(),
        60..3_600 => (seconds / 60, "minute"),
        3_600..86_400 => (seconds / 3_600, "hour"),
        86_400..2_592_000 => (seconds / 86_400, "day"),
        2_592_000..31_536_000 => (seconds / 2_592_000, "month"),
        _ => (seconds / 31_536_000, "year"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

/// A strip on a panel border that resizes the panel when dragged; a grip in
/// its middle shows it can be dragged.
fn border_handle(id: &'static str, theme: &Theme) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w_full()
        .h(px(8.))
        .border_t_1()
        .border_b_1()
        .border_color(theme.separator)
        .bg(theme.graph_background)
        .hover(|strip| strip.bg(theme.selection))
        .cursor(CursorStyle::ResizeUpDown)
        .child(
            div()
                .w(px(28.))
                .h(px(3.))
                .rounded(px(2.))
                .bg(theme.secondary),
        )
}

/// The graph border's grip turned upright, for the side panel's left edge.
fn side_border_handle(id: &'static str, theme: &Theme) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(8.))
        .h_full()
        .border_l_1()
        .border_r_1()
        .border_color(theme.separator)
        .bg(theme.graph_background)
        .hover(|strip| strip.bg(theme.selection))
        .cursor(CursorStyle::ResizeLeftRight)
        .group(id)
        // The grip shows on hover only: beside diff rows, a lone mark reads
        // as a fault in one of them.
        .child(
            div()
                .w(px(3.))
                .h(px(28.))
                .rounded(px(2.))
                .bg(theme.secondary)
                .invisible()
                .group_hover(id, |grip| grip.visible()),
        )
}

/// A section title in the commit list.
fn list_heading(text: String, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .px(px(10.))
        .pt(px(8.))
        .pb(px(2.))
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.secondary)
        .child(text)
}

/// The height of a commit list row.
const LIST_ITEM_HEIGHT: f32 = 40.;

fn list_item(id: impl Into<gpui::ElementId>, selected: bool, theme: &Theme) -> Stateful<gpui::Div> {
    div()
        .id(id.into())
        .flex()
        .flex_col()
        .flex_none()
        .justify_center()
        .w_full()
        .h(px(LIST_ITEM_HEIGHT))
        .px(px(10.))
        .overflow_hidden()
        .when(selected, |item| item.bg(theme.selection))
}

fn placeholder(title: &str, description: Option<String>, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .child(div().text_lg().child(SharedString::from(title.to_string())))
        .children(description.map(|text| div().text_color(theme.secondary).child(text)))
        .into_any_element()
}

fn column(width: f32) -> gpui::Div {
    div().flex_none().w(px(width)).h_full()
}

impl Render for RepoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_appearance(window.appearance());
        window.set_window_title(&self.title());
        self.request_summary(cx);
        self.sync_log(cx);
        // Layout clamps the offset, so this lands at the right edge.
        if self.scroll_graph_to_end && !self.graph.nodes.is_empty() {
            let y = self.graph_scroll.offset().y;
            self.graph_scroll.set_offset(point(px(-1e7), y));
            self.scroll_graph_to_end = false;
        }
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
            .on_action(cx.listener(|this, _: &OpenWorktreeWith, window, cx| {
                this.open_worktree_with(None, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &CommandPalette, window, cx| {
                    this.open_goto(true, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleSidePanel, _, cx| this.toggle_side_panel(cx)))
            .on_action(cx.listener(|this, _: &ShowOneFile, _, cx| {
                this.set_changes_layout(ChangesLayout::OneFile, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowAllFiles, _, cx| {
                this.set_changes_layout(ChangesLayout::AllFiles, cx)
            }))
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
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
                if keystroke.modifiers.platform && keystroke.key == "a" {
                    this.message_selection = None;
                    this.show_all_commits(cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_move(cx.listener(Self::drag_border))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dragging = None;
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
                                view.child(self.graph_band(&theme, cx))
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_h_0()
                                    .child(
                                        column(self.commits_width)
                                            .child(self.commit_list(&theme, cx)),
                                    )
                                    .child(self.column_handle(
                                        "commits-border",
                                        Border::Commits,
                                        &theme,
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .flex_1()
                                            .min_w_0()
                                            .h_full()
                                            .children(self.file_stepper(&theme, cx))
                                            .child(
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
            .child(self.status_bar(&theme, cx))
            .children(self.context_menu_view(&theme, cx))
            .children(self.dropdown_view(&theme, cx))
            .children(self.prompt_view(&theme, window, cx))
            .children(self.goto_view(&theme, window, cx))
            .children(self.stash_picker_view(&theme, window, cx))
            .children(self.prune_view(&theme, window, cx))
    }
}

/// Space between the side strip's boxes, and above the first.
const STRIP_GAP: f32 = 6.;

/// A box of the side strip: a panel's button, or a group of modes.
fn strip_group(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(6.))
        .border_1()
        .border_color(theme.separator)
}

/// A button in a box of the side strip, filled while on.
fn strip_option(id: &'static str, on: bool, theme: &Theme) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .items_center()
        .px(px(2.))
        .py(px(6.))
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|option| option.bg(theme.selection.opacity(0.5)))
        .when(on, |option| option.bg(theme.selection))
}

impl RepoView {
    fn open_stash_picker(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.stash_picker = Some((path, 0));
        self.stash_focus.focus(window);
        cx.notify();
    }

    fn close_stash_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stash_picker = None;
        self.focus.focus(window);
        cx.notify();
    }

    /// A row of the Stash picker taken: the first runs the stash with the
    /// flags on, the others turn their flag on or off.
    fn pick_stash_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, _)) = self.stash_picker.clone() else {
            return;
        };
        if row == 0 {
            let flags: Vec<&str> = popups::STASH_FLAGS
                .iter()
                .zip(self.stash_flags)
                .filter(|(_, on)| *on)
                .map(|((flag, _), _)| *flag)
                .collect();
            self.close_stash_picker(window, cx);
            self.run_in_terminal(popups::stash_with(&path, &flags), window, cx);
            return;
        }
        if let Some(flag) = self.stash_flags.get_mut(row - 1) {
            *flag = !*flag;
        }
        if let Some((_, highlighted)) = &mut self.stash_picker {
            *highlighted = row;
        }
        cx.notify();
    }

    fn stash_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, highlighted)) = &mut self.stash_picker else {
            return;
        };
        let rows = popups::STASH_FLAGS.len() + 1;
        match event.keystroke.key.as_str() {
            "escape" => return self.close_stash_picker(window, cx),
            "enter" => {
                let row = *highlighted;
                return self.pick_stash_row(row, window, cx);
            }
            "down" => *highlighted = (*highlighted + 1).min(rows - 1),
            "up" => *highlighted = highlighted.saturating_sub(1),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The Stash picker, as Sublime Merge's: `stash` to run, then its flags
    /// to turn on or off, each with what it does.
    fn stash_picker_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (path, highlighted) = self.stash_picker.as_ref()?;
        let highlighted = *highlighted;
        let folder = std::path::Path::new(path)
            .file_name()
            .map_or(path.clone(), |name| name.to_string_lossy().into_owned());
        let row = |k: usize, cx: &mut Context<Self>| {
            div()
                .id(("stash-row", k))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(28.))
                .px(px(10.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(k == highlighted, |row| row.bg(theme.selection))
                .hover(|row| row.bg(theme.selection.opacity(0.5)))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.pick_stash_row(k, window, cx)),
                )
        };
        let mut rows = vec![
            row(0, cx)
                .child(div().w(px(14.)))
                .child(div().italic().child("stash"))
                .child(div().flex_1())
                .child(
                    div()
                        .text_xs()
                        .italic()
                        .text_color(theme.secondary)
                        .child(format!("Select to run in {folder}")),
                ),
        ];
        for (k, (flag, about)) in popups::STASH_FLAGS.iter().enumerate() {
            rows.push(
                row(k + 1, cx)
                    .child(
                        div()
                            .w(px(14.))
                            .text_color(theme.accent)
                            .child(if self.stash_flags[k] { "✓" } else { "" }),
                    )
                    .child(div().font_family(preferences::code_font()).child(*flag))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .italic()
                            .text_color(theme.secondary)
                            .child(*about),
                    ),
            );
        }
        let width = f32::from(window.viewport_size().width);
        let panel_width = GOTO_WIDTH.min(width - 32.);
        let position = Point::new(px(((width - panel_width) / 2.).max(0.)), px(60.));
        let panel = div()
            .id("stash-picker")
            .track_focus(&self.stash_focus)
            .on_key_down(cx.listener(Self::stash_key))
            .occlude()
            .flex()
            .flex_col()
            .gap(px(2.))
            .w(px(panel_width))
            .p(px(8.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.separator)
            .bg(theme.background)
            .shadow_lg()
            .text_sm()
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.close_stash_picker(window, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .pb(px(6.))
                    .mb(px(2.))
                    .border_b_1()
                    .border_color(theme.separator)
                    .child(
                        div()
                            .px(px(6.))
                            .rounded(px(4.))
                            .bg(theme.selection)
                            .child("Stash"),
                    )
                    .child(div().text_color(theme.secondary).child("Choose flags")),
            )
            .children(rows);
        Some(
            deferred(anchored().position(position).child(panel))
                .with_priority(2)
                .into_any_element(),
        )
    }
}

/// Text turned to read downwards, for the sidebar strip. GPUI draws text
/// level only, so it is an SVG image, drawn at twice its size to stay sharp.
fn vertical_label(text: &str, color: gpui::Hsla, window: &Window) -> impl IntoElement {
    let size = f32::from(window.rem_size()) * 0.8;
    let family = match preferences::ui_font() {
        font if font.starts_with('.') => "Helvetica Neue".to_string(),
        font => format!("{font}, Helvetica Neue"),
    };
    let run = gpui::TextRun {
        len: text.len(),
        font: gpui::font(preferences::ui_font()),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let length = window
        .text_system()
        .shape_line(text.to_string().into(), px(size), &[run], None)
        .width;
    let (w, h) = ((size * 1.4).ceil(), (f32::from(length) + 4.).ceil());
    let rgba = gpui::Rgba::from(color);
    let fill = format!(
        "rgb({},{},{})",
        (rgba.r * 255.) as u8,
        (rgba.g * 255.) as u8,
        (rgba.b * 255.) as u8
    );
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {w} {h}"><text transform="translate({} {}) rotate(90)" text-anchor="middle" dy="0.35em" font-family="{family}" font-size="{size}" fill="{fill}" fill-opacity="{}">{text}</text></svg>"#,
        w * 2.,
        h * 2.,
        w / 2.,
        h / 2.,
        rgba.a,
    );
    gpui::img(std::sync::Arc::new(gpui::Image::from_bytes(
        gpui::ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .w(px(w))
    .h(px(h))
}

/// A fold chevron in the side panels: as large as their text, drawn a little
/// into the gap after it so the name sits close.
fn panel_chevron(open: bool, theme: &Theme) -> impl IntoElement {
    div().flex_none().mr(px(-3.)).child(chevron(
        open,
        theme.secondary,
        (preferences::ui_size() * 0.875).round(),
    ))
}

/// A fold chevron, `›` when folded and `⌄` when open, drawn as an image so
/// it is crisp at any size and the same in every font.
pub fn chevron(open: bool, color: gpui::Hsla, size: f32) -> impl IntoElement {
    let path = match open {
        true => "M3.5 6 L8 10.5 L12.5 6",
        false => "M6 3.5 L10.5 8 L6 12.5",
    };
    stroke_icon(path, color, size)
}

/// A chevron pointing left or right, for the side strip's buttons.
fn side_chevron(left: bool, color: gpui::Hsla, size: f32) -> impl IntoElement {
    let path = match left {
        true => "M10 3.5 L5.5 8 L10 12.5",
        false => "M6 3.5 L10.5 8 L6 12.5",
    };
    stroke_icon(path, color, size)
}

/// A line drawing on a 16-unit grid, as an image of `size` points.
fn stroke_icon(path: &str, color: gpui::Hsla, size: f32) -> impl IntoElement {
    let rgba = gpui::Rgba::from(color);
    let stroke = format!(
        "rgb({},{},{})",
        (rgba.r * 255.) as u8,
        (rgba.g * 255.) as u8,
        (rgba.b * 255.) as u8
    );
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32" viewBox="2.5 2.5 11 11"><path d="{path}" fill="none" stroke="{stroke}" stroke-opacity="{}" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"#,
        rgba.a,
    );
    gpui::img(std::sync::Arc::new(gpui::Image::from_bytes(
        gpui::ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .flex_none()
    .w(px(size))
    .h(px(size))
}

/// The summary bar's height: the header's first line with its padding.
fn summary_bar_height() -> f32 {
    2. * HEADER_PADDING + HEADER_LINE * preferences::ui_size() + 1.
}

/// An empty diff row under the commit header.
fn spacer() -> AnyElement {
    div().h(px(diff_view::row_height())).into_any_element()
}

/// The word around byte `at` of `text`, for a double click.
fn word_around(text: &str, at: usize) -> (usize, usize) {
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let at = at.min(text.len());
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| word(c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .find(|&(_, c)| !word(c))
        .map_or(text.len(), |(i, _)| at + i);
    (start, end)
}

/// A text field's content: the text, or a placeholder, with a caret at the
/// end while it has focus. Over a placeholder the caret is laid on top, so
/// the placeholder starts where typed text will.
fn field_text(text: &str, placeholder: &str, focused: bool, theme: &Theme) -> AnyElement {
    if text.is_empty() {
        return div()
            .relative()
            .text_color(theme.secondary)
            .child(placeholder.to_string())
            .when(focused, |field| {
                field.child(
                    div()
                        .absolute()
                        .left(px(-1.))
                        .top_0()
                        .bottom_0()
                        .w(px(1.5))
                        .bg(theme.accent),
                )
            })
            .into_any_element();
    }
    let caret = if focused { "▏" } else { "" };
    let shown = format!("{text}{caret}");
    let highlights = focused.then(|| {
        (
            text.len()..shown.len(),
            HighlightStyle {
                color: Some(theme.accent),
                ..Default::default()
            },
        )
    });
    StyledText::new(shown)
        .with_highlights(highlights)
        .into_any_element()
}

/// The cursor drawn in a text field.
const CARET: &str = "▏";

/// A place in text shown with the caret at `cursor`, as a place in the text.
fn from_shown(shown: usize, cursor: usize) -> usize {
    match shown {
        at if at <= cursor => at,
        at if at < cursor + CARET.len() => cursor,
        at => at - CARET.len(),
    }
}

/// Edits `text` at its `cursor` by a key: typing, Return, ⌘V, Backspace and
/// Delete, by a word with ⌥ and to the line's start with ⌘; ← and → move by
/// a character, a word with ⌥ or to the line's end with ⌘, as Home and End
/// do. False for keys it leaves alone.
fn edit_key(
    text: &mut String,
    cursor: &mut usize,
    keystroke: &gpui::Keystroke,
    paste: impl FnOnce() -> Option<String>,
) -> bool {
    let modifiers = keystroke.modifiers;
    let at = *cursor;
    let before = |at: usize| {
        text[..at]
            .chars()
            .next_back()
            .map_or(0, |c| at - c.len_utf8())
    };
    let after = |at: usize| text[at..].chars().next().map_or(at, |c| at + c.len_utf8());
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let word_start = {
        let trimmed = text[..at].trim_end_matches(char::is_whitespace);
        trimmed.rfind(char::is_whitespace).map_or(0, |i| {
            i + trimmed[i..].chars().next().map_or(1, char::len_utf8)
        })
    };
    let word_end = {
        let rest = &text[at..];
        let skip = rest.len() - rest.trim_start_matches(char::is_whitespace).len();
        let word = rest[skip..]
            .find(char::is_whitespace)
            .unwrap_or(rest.len() - skip);
        at + skip + word
    };
    let mut insert = |text: &mut String, typed: &str| {
        text.insert_str(*cursor, typed);
        *cursor += typed.len();
    };
    match keystroke.key.as_str() {
        "left" if modifiers.platform => *cursor = line_start,
        "left" if modifiers.alt => *cursor = word_start,
        "left" => *cursor = before(at),
        "right" if modifiers.platform => *cursor = line_end,
        "right" if modifiers.alt => *cursor = word_end,
        "right" => *cursor = after(at),
        "home" => *cursor = line_start,
        "end" => *cursor = line_end,
        "backspace" => {
            let from = match (modifiers.platform, modifiers.alt) {
                (true, _) => line_start,
                (_, true) => word_start,
                _ => before(at),
            };
            text.replace_range(from..at, "");
            *cursor = from;
        }
        "delete" => {
            let to = if modifiers.alt { word_end } else { after(at) };
            text.replace_range(at..to, "");
        }
        "enter" if !modifiers.platform => insert(text, "\n"),
        "v" if modifiers.platform => match paste() {
            Some(pasted) => insert(text, &pasted),
            None => return false,
        },
        _ if !modifiers.platform && !modifiers.control => match &keystroke.key_char {
            Some(typed) if !typed.chars().any(char::is_control) => insert(text, typed),
            _ => return false,
        },
        _ => return false,
    }
    true
}

/// Types a key into a plain text field: Return breaks the line, Backspace
/// deletes and ⌘V pastes. False for keys it leaves alone.
fn type_key(text: &mut String, keystroke: &gpui::Keystroke, cx: &gpui::App) -> bool {
    let modifiers = keystroke.modifiers;
    match keystroke.key.as_str() {
        "enter" if !modifiers.platform => text.push('\n'),
        "backspace" => {
            text.pop();
        }
        "v" if modifiers.platform => {
            if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
                text.push_str(&pasted);
            }
        }
        _ if !modifiers.platform && !modifiers.control => match &keystroke.key_char {
            Some(typed) if !typed.chars().any(char::is_control) => text.push_str(typed),
            _ => return false,
        },
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use wtf_core::{DiffRow, DiffSide, TextRange};

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
            show(name_tree(&names, &HashSet::new())),
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
            show(name_tree(&names, &folded)),
            ["0feature/", "0origin/x/y/", "1z", "0bot"]
        );
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
        assert_eq!(
            show(file_tree(&files, None, &none)),
            [
                "0app/src/",
                "1view/",
                "2a.rs",
                "1main.rs",
                "0core/src/lib/x/",
                "1y.rs",
                "0README.md"
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
        let kinds: Vec<String> =
            stack_items(&diffs, &[true, true, true], Some(2), [false; 2], None)
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

        let folded_unstaged: Vec<String> =
            stack_items(&diffs, &[true, true, true], Some(2), [true, false], None)
                .iter()
                .map(|item| match item {
                    StackItem::Section(staged, count) => format!("{staged}:{count}"),
                    StackItem::Header(file) => format!("H{file}"),
                    _ => "?".to_string(),
                })
                .collect();
        assert_eq!(folded_unstaged, ["?", "false:2", "?", "true:1", "?", "H2"]);

        // Each hunk of a worktree file gets a header before its rows.
        let open: Vec<String> = stack_items(&diffs[2..], &[false], Some(0), [false; 2], None)
            .iter()
            .map(|item| match item {
                StackItem::Section(staged, count) => format!("{staged}:{count}"),
                StackItem::Header(file) => format!("H{file}"),
                StackItem::Hunk(file, start, end) => format!("@{file}:{start}-{end}"),
                StackItem::Row(file, row) => format!("{file}:{row}"),
                _ => "?".to_string(),
            })
            .collect();
        assert_eq!(
            open,
            ["?", "false:0", "?", "true:1", "?", "H0", "@0:0-1", "0:0"]
        );

        let nothing_staged = stack_items(&diffs[..1], &[true], Some(1), [false; 2], None);
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
            stack_items(&diffs, folded, None, [false; 2], None)
                .iter()
                .map(|item| match item {
                    StackItem::Section(staged, count) => format!("S{staged}:{count}"),
                    StackItem::Header(file) => format!("H{file}"),
                    StackItem::Row(file, row) => format!("{file}:{row}"),
                    StackItem::Hunk(..) => "@".to_string(),
                    StackItem::Gap => "…".to_string(),
                    StackItem::Note(text) => text.clone(),
                    StackItem::Space | StackItem::HeaderPad => " ".to_string(),
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

    #[test]
    fn edits_text_at_its_cursor() {
        let key = |keys: &str| {
            let mut keystroke = gpui::Keystroke::parse(keys).unwrap();
            if keystroke.key.chars().count() == 1 && !keystroke.modifiers.platform {
                keystroke.key_char = Some(keystroke.key.clone());
            }
            keystroke
        };
        let edit = |text: &str, cursor: usize, keys: &str| {
            let (mut text, mut cursor) = (text.to_string(), cursor);
            edit_key(&mut text, &mut cursor, &key(keys), || Some("XY".into()));
            (text, cursor)
        };
        assert_eq!(edit("ac", 1, "b"), ("abc".into(), 2));
        assert_eq!(edit("abc", 1, "backspace"), ("bc".into(), 0));
        assert_eq!(edit("abc", 1, "delete"), ("ac".into(), 1));
        assert_eq!(edit("ab", 1, "cmd-v"), ("aXYb".into(), 3));
        assert_eq!(edit("ab", 1, "enter"), ("a\nb".into(), 2));
        assert_eq!(edit("привет", 2, "right"), ("привет".into(), 4));
        assert_eq!(edit("one two", 7, "alt-left"), ("one two".into(), 4));
        assert_eq!(edit("one two", 0, "alt-right"), ("one two".into(), 3));
        assert_eq!(edit("one two", 7, "alt-backspace"), ("one ".into(), 4));
        assert_eq!(edit("a\nbc", 4, "cmd-left"), ("a\nbc".into(), 2));
        assert_eq!(edit("a\nbc", 2, "cmd-right"), ("a\nbc".into(), 4));
        assert_eq!(edit("a\nbc", 4, "cmd-backspace"), ("a\n".into(), 2));
        // The caret drawn at 1 takes room that is no place in the text.
        assert_eq!(from_shown(1, 1), 1);
        assert_eq!(from_shown(2, 1), 1);
        assert_eq!(from_shown(1 + CARET.len() + 1, 1), 2);
    }
}
