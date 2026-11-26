//! Changes to history: cherry-pick, drop, squash, fixup, reorder, reword,
//! edit and rebase. An action that goes through cleanly is done at once,
//! with a notice to undo it; one with conflicts, or one taken while the Plan
//! tab is open, goes into a rewrite there. The plan's steps can be changed,
//! their conflicts resolved and their code edited in the middle; nothing
//! changes until Apply. Meanwhile a progress branch shows the steps settled
//! so far in the graph.

use super::*;
use crate::line_edit::LineEdit;
use crate::text_field::Field;
use wtf_core::plan::{
    self, Action, Base, Choice, FileConflict, Onto, Outcome, Plan, Region, Resolution, State, Undo,
    WorkedOut,
};

/// A change to history asked for from a menu.
#[derive(Debug, Clone)]
pub(super) enum HistoryAction {
    Pick {
        onto: Onto,
        commits: Vec<String>,
    },
    Drop(Vec<String>),
    /// Into the commit before; several are squashed together, into the
    /// oldest of them.
    Squash(Vec<String>),
    Fixup(Vec<String>),
    /// Later in history, or earlier.
    MoveUp(String),
    MoveDown(String),
    Reword(String),
    Edit(String),
    /// The branch's own commits moved to start from `base`.
    Rebase {
        onto: Onto,
        base: String,
    },
}

impl HistoryAction {
    /// It needs words or code from the user: the plan opens for them.
    fn needs_input(&self) -> bool {
        matches!(self, HistoryAction::Reword(_) | HistoryAction::Edit(_))
    }

    /// What was done, for the notice.
    fn done(&self, place: &str) -> String {
        let commits = |n: usize| match n {
            1 => "1 commit".to_string(),
            n => format!("{n} commits"),
        };
        match self {
            HistoryAction::Pick { commits: c, .. } => {
                format!("Cherry-picked {} into {place}", commits(c.len()))
            }
            HistoryAction::Drop(c) => format!("Dropped {} from {place}", commits(c.len())),
            HistoryAction::Squash(c) => format!("Squashed {} in {place}", commits(c.len())),
            HistoryAction::Fixup(c) => format!("Fixed up {} in {place}", commits(c.len())),
            HistoryAction::MoveUp(_) | HistoryAction::MoveDown(_) => {
                format!("Moved a commit in {place}")
            }
            HistoryAction::Reword(_) => format!("Reworded a commit in {place}"),
            HistoryAction::Edit(_) => format!("Edited a commit in {place}"),
            HistoryAction::Rebase { .. } => format!("Rebased {place}"),
        }
    }
}

/// What the one editor in the middle edits.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum EditTarget {
    /// The lines of a conflicting part of a step's file.
    Part {
        step: usize,
        path: String,
        part: usize,
    },
    /// A step's message.
    Message { step: usize },
    /// A whole file of an edit step.
    File { step: usize, path: String },
}

pub(super) struct Editor {
    pub target: EditTarget,
    pub field: Field,
    /// The file it is code of, for its colors and line numbers; none for
    /// a message.
    pub code: Option<String>,
    /// The file's line number of the editor's first line.
    pub first_line: usize,
    pub colors: crate::syntax::Spans,
}

impl Editor {
    fn new(
        target: EditTarget,
        text: String,
        code: Option<String>,
        first_line: usize,
        dark: bool,
    ) -> Self {
        let mut editor = Editor {
            target,
            field: Field::new(LineEdit::multiline(text)),
            code,
            first_line,
            colors: Vec::new(),
        };
        editor.recolor(dark);
        editor
    }

    /// Colors the code as it now reads.
    fn recolor(&mut self, dark: bool) {
        self.colors = match &self.code {
            Some(path) => crate::syntax::text_colors(path, self.field.text(), dark),
            None => Vec::new(),
        };
    }
}

fn dark(window: &Window) -> bool {
    Theme::for_appearance(window.appearance()).dark
}

/// A plan being prepared in the Rewrite tab.
pub(super) struct PlanView {
    pub plan: Plan,
    /// Each step's commit's first line, by commit.
    pub subjects: HashMap<String, String>,
    /// Where it goes, as named: `⌂ app`, `release`.
    pub place: String,
    /// The progress branch's name.
    pub wip: String,
    pub worked: Option<WorkedOut>,
    /// Counts the times it was worked out, so an older answer is dropped.
    pub generation: usize,
    pub working: bool,
    pub selected: Option<usize>,
    /// The step's file shown: in conflict, or edited.
    pub file: Option<String>,
    pub regions: Option<(plan::Sides, Vec<Region>)>,
    /// The step under its action's choices.
    pub choosing: Option<usize>,
    /// The step shows in the middle; anything else selected closes it.
    pub middle: bool,
    pub editor: Option<Editor>,
    /// An edit step's files, to pick one to change.
    pub files: Vec<String>,
    /// The files the selected step's commit changes.
    pub changed: Vec<String>,
    /// Applied: how to take it back.
    pub applied: Option<Undo>,
    pub error: Option<String>,
}

/// A step being dragged to another place in the Rewrite tab.
#[derive(Clone)]
pub(super) struct DraggedStep {
    pub step: usize,
    pub subject: String,
}

/// A notice of a change done at once, with the way back.
pub(super) struct Toast {
    pub text: String,
    pub undo: Option<Undo>,
}

/// How long a notice stays.
const TOAST_TIME: Duration = Duration::from_secs(10);
/// A pause in typing before the plan is worked out again.
const TYPING_PAUSE: Duration = Duration::from_millis(500);

impl RepoView {
    /// Where a change to `commit`'s history goes: the current worktree when
    /// its history holds it, else a branch that does, by its worktree when
    /// it has one.
    fn history_onto(&self, repo: &str, commit: &str) -> Result<Onto, String> {
        let current = self
            .current_worktree_path()
            .or(self.repo_path.as_deref())
            .map(str::to_string);
        if let Some(path) = current
            && wtf_core::has_commit(&path, "HEAD", commit)
        {
            return Ok(Onto::Worktree(path));
        }
        let branches = plan::branches_with(repo, commit).map_err(message)?;
        let name = branches
            .first()
            .ok_or("No branch holds this commit".to_string())?;
        let path = self.graph.nodes.iter().find_map(|node| match &node.tip {
            Revision::Worktree(path) if node.branch.as_ref() == Some(name) => Some(path.clone()),
            _ => None,
        });
        Ok(match path {
            Some(path) => Onto::Worktree(path),
            None => Onto::Branch(name.clone()),
        })
    }

    fn place_name(onto: &Onto) -> String {
        match onto {
            Onto::Worktree(path) => format!("⌂ {}", workspace::folder_name(path)),
            Onto::Branch(branch) => branch.clone(),
        }
    }

    /// A plan still to apply.
    fn open_plan(&self) -> Option<&PlanView> {
        self.plan.as_ref().filter(|v| v.applied.is_none())
    }

    /// Does a change to history: at once when it goes through cleanly and
    /// no plan is open; otherwise into the plan, to be applied from there.
    pub(super) fn history_action(&mut self, action: HistoryAction, cx: &mut Context<Self>) {
        let Some(repo) = self.repo_path.clone() else {
            return;
        };
        let deferred = self.open_plan().is_some() || self.side_panel == Some(SidePanel::Plan);
        let (mut plan, focus) = match self.plan_for(&repo, &action) {
            Ok(found) => found,
            Err(error) => {
                self.show_toast(error, None, cx);
                return;
            }
        };
        if let Err(error) = shape(&repo, &mut plan, &action) {
            self.show_toast(error, None, cx);
            return;
        }
        let place = Self::place_name(&plan.onto);
        if deferred || action.needs_input() {
            self.install_plan(&repo, plan, cx);
            self.side_panel = Some(SidePanel::Plan);
            self.work_out_plan(cx);
            let step = focus.and_then(|c| self.plan.as_ref()?.plan.step_of(&c));
            match step {
                Some(step) if action.needs_input() => self.select_plan_step(step, cx),
                _ => {
                    if let Some(view) = self.plan.as_mut() {
                        view.selected = step;
                    }
                }
            }
            return;
        }
        // At once: done when it goes through cleanly, else a plan.
        let task = cx.background_executor().spawn({
            let (repo, plan) = (repo.clone(), plan.clone());
            async move {
                let worked = plan::work_out(&repo, &plan)?;
                match worked.ready() {
                    true => plan::apply(&repo, &plan.onto, &worked).map(Some),
                    false => Ok(None),
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(Some(undo)) => {
                    this.show_toast(action.done(&place), Some(undo), cx);
                    this.start_refresh(cx);
                }
                Ok(None) => {
                    this.install_plan(&repo, plan, cx);
                    this.side_panel = Some(SidePanel::Plan);
                    this.update_menus(cx);
                    this.work_out_plan(cx);
                }
                Err(error) => this.show_toast(message(error), None, cx),
            })
            .ok();
        })
        .detach();
    }

    /// The plan an action goes into: the open one, when it changes the same
    /// branch, or a new one; and the commit it is about, to select.
    fn plan_for(
        &self,
        repo: &str,
        action: &HistoryAction,
    ) -> Result<(Plan, Option<String>), String> {
        let (onto, focus) = match action {
            HistoryAction::Pick { onto, commits } => (onto.clone(), commits.first().cloned()),
            HistoryAction::Rebase { onto, .. } => (onto.clone(), None),
            HistoryAction::Drop(c) | HistoryAction::Squash(c) | HistoryAction::Fixup(c) => {
                let first = c.first().ok_or("No commit".to_string())?;
                (self.history_onto(repo, first)?, Some(first.clone()))
            }
            HistoryAction::MoveUp(c)
            | HistoryAction::MoveDown(c)
            | HistoryAction::Reword(c)
            | HistoryAction::Edit(c) => (self.history_onto(repo, c)?, Some(c.clone())),
        };
        if let Some(view) = self.open_plan() {
            if view.plan.onto != onto {
                return Err(format!(
                    "The rewrite open is for {}: apply or cancel it first",
                    view.place
                ));
            }
            if matches!(action, HistoryAction::Rebase { .. }) && !view.plan.steps.is_empty() {
                return Err("Apply or cancel the open rewrite before a rebase".into());
            }
            return Ok((view.plan.clone(), focus));
        }
        let plan = match action {
            HistoryAction::Rebase { onto, base } => {
                plan::rebase(repo, onto.clone(), base).map_err(message)?
            }
            _ => Plan::pick(onto, &[]),
        };
        Ok((plan, focus))
    }

    /// Puts a plan in the Rewrite tab, its commits' subjects read.
    fn install_plan(&mut self, repo: &str, plan: Plan, cx: &mut Context<Self>) {
        let ids: Vec<String> = plan.steps.iter().map(|s| s.commit.clone()).collect();
        let subjects = plan::subjects(repo, &ids).unwrap_or_default();
        let place = Self::place_name(&plan.onto);
        let wip = plan::REWRITE_BRANCH.to_string();
        match self.plan.as_mut().filter(|v| v.applied.is_none()) {
            Some(view) => {
                view.plan = plan;
                view.subjects = subjects;
            }
            None => {
                self.plan = Some(PlanView {
                    plan,
                    subjects,
                    place,
                    wip,
                    worked: None,
                    generation: 0,
                    working: false,
                    selected: None,
                    file: None,
                    regions: None,
                    choosing: None,
                    middle: false,
                    editor: None,
                    files: Vec::new(),
                    changed: Vec::new(),
                    applied: None,
                    error: None,
                });
            }
        }
        self.update_menus(cx);
        cx.notify();
    }

    pub(super) fn show_toast(&mut self, text: String, undo: Option<Undo>, cx: &mut Context<Self>) {
        self.toast = Some(Toast { text, undo });
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOAST_TIME).await;
            this.update(cx, |this, cx| {
                this.toast = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn undo_toast(&mut self, cx: &mut Context<Self>) {
        let (Some(repo), Some(toast)) = (self.repo_path.clone(), self.toast.take()) else {
            return;
        };
        let Some(undo) = toast.undo else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { plan::undo(&repo, &undo) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.show_toast("Undone".into(), None, cx),
                    Err(error) => this.show_toast(message(error), None, cx),
                }
                this.start_refresh(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The notice at the foot of the window.
    pub(super) fn toast_view(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let toast = self.toast.as_ref()?;
        let panel = div()
            .id("toast")
            .occlude()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(8.))
            .rounded(px(8.))
            .map(|panel| floating(panel, theme))
            .text_sm()
            .child(toast.text.clone())
            .when(toast.undo.is_some(), |panel| {
                panel.child(
                    div()
                        .id("toast-undo")
                        .px(px(8.))
                        .rounded(px(4.))
                        .text_color(theme.accent)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .cursor_pointer()
                        .hover(|b| b.bg(theme.selection))
                        .on_click(cx.listener(|this, _, _, cx| this.undo_toast(cx)))
                        .child("Undo"),
                )
            })
            .child(
                div()
                    .id("toast-close")
                    .px(px(4.))
                    .text_color(theme.secondary)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toast = None;
                        cx.notify();
                    }))
                    .child("×"),
            );
        Some(
            div()
                .absolute()
                .bottom(px(24.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(panel)
                .into_any_element(),
        )
    }

    /// Works the plan out again, in the background: after a change, or when
    /// where it goes may have moved. The progress branch follows.
    pub(super) fn work_out_plan(&mut self, cx: &mut Context<Self>) {
        let (Some(repo), Some(view)) = (self.repo_path.clone(), self.plan.as_mut()) else {
            return;
        };
        view.generation += 1;
        view.working = true;
        let generation = view.generation;
        let plan = view.plan.clone();
        let wip = view.wip.clone();
        let task = cx.background_executor().spawn(async move {
            let worked = plan::work_out(&repo, &plan)?;
            // Only steps settled into commits of their own: on its own row in
            // the graph, never a label on another branch's commit.
            let settled = (worked.settled() != worked.base).then(|| worked.settled().to_string());
            plan::mark_wip(&repo, &wip, settled.as_deref())?;
            Ok::<_, CoreError>(worked)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(view) = this.plan.as_mut().filter(|v| v.generation == generation) else {
                    return;
                };
                view.working = false;
                match result {
                    Ok(worked) => {
                        view.worked = Some(worked);
                        view.error = None;
                    }
                    Err(error) => view.error = Some(message(error)),
                }
                this.load_plan_file(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Works the plan out again once typing pauses.
    fn work_out_soon(&mut self, cx: &mut Context<Self>) {
        self.plan_typing = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPING_PAUSE).await;
            this.update(cx, |this, cx| this.work_out_plan(cx)).ok();
        }));
    }

    fn plan_outcome(&self, step: usize) -> Option<&Outcome> {
        self.plan.as_ref()?.worked.as_ref()?.outcomes.get(step)
    }

    /// The selected step's file in conflict: its parts, read again when its
    /// versions changed.
    fn load_plan_file(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo_path.clone() else {
            return;
        };
        let Some(view) = &self.plan else {
            return;
        };
        let (Some(step), Some(path)) = (view.selected, view.file.clone()) else {
            return;
        };
        let Some(file) = self
            .plan_outcome(step)
            .and_then(|o| o.files.iter().find(|f| f.path == path))
            .cloned()
        else {
            return;
        };
        if view
            .regions
            .as_ref()
            .is_some_and(|(sides, _)| *sides == file.sides)
        {
            return;
        }
        let task = cx
            .background_executor()
            .spawn(async move { plan::regions(&repo, &file.sides).map(|r| (file.sides, r)) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if let Some(view) = this.plan.as_mut() {
                    match result {
                        Ok(regions) => view.regions = Some(regions),
                        Err(error) => view.error = Some(message(error)),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Selects a step: its conflicts, message or files open in the middle;
    /// a step with none of them shows the commit it makes.
    /// Selects a step: the middle shows it, the commit history stays as
    /// it is.
    pub(super) fn select_plan_step(&mut self, step: usize, cx: &mut Context<Self>) {
        self.focus_step(step, cx);
    }

    /// Makes `step` the one the tab and the middle show: its conflicts,
    /// message or files, or nothing more for one with none of them.
    pub(super) fn focus_step(&mut self, step: usize, cx: &mut Context<Self>) {
        let Some(repo) = self.repo_path.clone() else {
            return;
        };
        let Some(view) = self.plan.as_ref() else {
            return;
        };
        let Some(entry) = view.plan.steps.get(step).cloned() else {
            return;
        };
        let outcome = self.plan_outcome(step).cloned();
        let files = match entry.action {
            Action::Edit => {
                let mut files = plan::files_of(&repo, &entry.commit).unwrap_or_default();
                let edited: Vec<String> = entry
                    .edits
                    .keys()
                    .filter(|f| !files.contains(f))
                    .cloned()
                    .collect();
                files.extend(edited);
                files
            }
            _ => Vec::new(),
        };
        let changed = outcome
            .as_ref()
            .and_then(|o| o.shown())
            .map(str::to_string)
            .or(Some(entry.commit.clone()))
            .and_then(|commit| plan::files_of(&repo, &commit).ok())
            .unwrap_or_default();
        let Some(view) = self.plan.as_mut() else {
            return;
        };
        view.selected = Some(step);
        view.changed = changed;
        let conflicted = outcome
            .as_ref()
            .and_then(|o| o.files.iter().find(|f| !f.resolved).or(o.files.first()))
            .map(|f| f.path.clone());
        let first = conflicted.or_else(|| files.first().cloned());
        if view.file != first {
            view.regions = None;
        }
        view.file = first;
        view.files = files;
        view.editor = None;
        view.middle = true;
        self.load_plan_file(cx);
        cx.notify();
    }

    fn select_plan_file(&mut self, path: String, cx: &mut Context<Self>) {
        if let Some(view) = self.plan.as_mut() {
            if view.file.as_ref() != Some(&path) {
                view.regions = None;
            }
            view.file = Some(path);
            view.editor = None;
            view.middle = true;
        }
        self.load_plan_file(cx);
        cx.notify();
    }

    /// Takes `choice` for the `part`th conflict of the shown file, or drops
    /// it when it was the one taken; then works the plan out again.
    fn choose(&mut self, part: usize, choice: Choice, cx: &mut Context<Self>) {
        let Some(view) = self.plan.as_mut() else {
            return;
        };
        let (Some(step), Some(path), Some((sides, regions))) =
            (view.selected, view.file.clone(), view.regions.as_ref())
        else {
            return;
        };
        let count = plan::conflicts(regions);
        let resolution = view.plan.steps[step]
            .resolutions
            .entry(path)
            .or_insert_with(|| Resolution {
                sides: sides.clone(),
                choices: Vec::new(),
            });
        if resolution.sides != *sides {
            resolution.sides = sides.clone();
            resolution.choices.clear();
        }
        resolution.choices.resize(count, None);
        let typed = matches!(choice, Choice::Text(_));
        resolution.choices[part] = match resolution.choices[part] == Some(choice.clone()) && !typed
        {
            true => None,
            false => Some(choice),
        };
        if !typed {
            view.editor = None;
        }
        self.work_out_plan(cx);
    }

    /// The choices made for the shown file.
    fn shown_choices(&self) -> Vec<Option<Choice>> {
        let Some(view) = &self.plan else {
            return Vec::new();
        };
        let (Some(step), Some(path), Some((sides, _))) =
            (view.selected, view.file.as_ref(), view.regions.as_ref())
        else {
            return Vec::new();
        };
        view.plan.steps[step]
            .resolutions
            .get(path)
            .filter(|r| r.sides == *sides)
            .map(|r| r.choices.clone())
            .unwrap_or_default()
    }

    /// Starts typing a conflict's block, the lines around it with it: from
    /// what it takes now, or from where the commits go.
    fn edit_part(&mut self, part: usize, window: &mut Window, cx: &mut Context<Self>) {
        let choices = self.shown_choices();
        let Some(view) = self.plan.as_mut() else {
            return;
        };
        let (Some(step), Some(path), Some((_, regions))) =
            (view.selected, view.file.clone(), view.regions.as_ref())
        else {
            return;
        };
        if view
            .editor
            .as_ref()
            .is_some_and(|e| matches!(&e.target, EditTarget::Part { part: p, .. } if *p == part))
        {
            return;
        }
        let current = choices.get(part).cloned().flatten().unwrap_or(Choice::Ours);
        let text = plan::block(regions, part, &current);
        let text = text.strip_suffix('\n').unwrap_or(&text).to_string();
        let first_line = plan::block_start(regions, &choices, part) + 1;
        view.editor = Some(Editor::new(
            EditTarget::Part {
                step,
                path: path.clone(),
                part,
            },
            text.clone(),
            Some(path),
            first_line,
            dark(window),
        ));
        // A click only places the caret: the block counts as typed, and
        // resolved, once something is typed.
        self.start_editing(window, cx);
    }

    /// Starts writing a step's message, from what it has now.
    fn edit_message(&mut self, step: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo_path.clone() else {
            return;
        };
        let Some(view) = self.plan.as_mut() else {
            return;
        };
        let entry = &view.plan.steps[step];
        let text = entry
            .message
            .clone()
            .or_else(|| plan::message_of(&repo, &entry.commit).ok())
            .unwrap_or_default();
        view.editor = Some(Editor::new(
            EditTarget::Message { step },
            text,
            None,
            1,
            dark(window),
        ));
        self.start_editing(window, cx);
    }

    /// Starts editing a whole file of an edit step, from its content there.
    fn edit_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repo_path.clone() else {
            return;
        };
        let Some(view) = self.plan.as_mut() else {
            return;
        };
        let Some(step) = view.selected else {
            return;
        };
        let entry = &view.plan.steps[step];
        let text = match entry.edits.get(&path) {
            Some(Some(text)) => text.clone(),
            Some(None) => String::new(),
            None => plan::file_at(&repo, &entry.commit, &path)
                .ok()
                .flatten()
                .unwrap_or_default(),
        };
        view.file = Some(path.clone());
        view.editor = Some(Editor::new(
            EditTarget::File {
                step,
                path: path.clone(),
            },
            text,
            Some(path),
            1,
            dark(window),
        ));
        self.start_editing(window, cx);
    }

    fn start_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.plan_focus.focus(window);
        self.caret_on = true;
        self.blink(cx);
        cx.notify();
    }

    /// Keys typed into the middle's editor: into the plan, which is worked
    /// out again once typing pauses. Esc stops editing.
    pub(super) fn plan_editor_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.plan.as_mut() else {
            return;
        };
        let Some(editor) = view.editor.as_mut() else {
            return;
        };
        if event.keystroke.key == "escape" {
            view.editor = None;
            self.focus.focus(window);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if !editor.field.key(&event.keystroke, cx) {
            return;
        }
        editor.recolor(dark(window));
        let text = editor.field.text().to_string();
        match editor.target.clone() {
            EditTarget::Part { step, path, part } => {
                if let Some((sides, regions)) = view.regions.as_ref() {
                    let count = plan::conflicts(regions);
                    let resolution = view.plan.steps[step]
                        .resolutions
                        .entry(path)
                        .or_insert_with(|| Resolution {
                            sides: sides.clone(),
                            choices: Vec::new(),
                        });
                    if resolution.sides != *sides {
                        resolution.sides = sides.clone();
                        resolution.choices.clear();
                    }
                    resolution.choices.resize(count, None);
                    resolution.choices[part] = Some(Choice::Text(text));
                }
            }
            EditTarget::Message { step } => view.plan.steps[step].message = Some(text),
            EditTarget::File { step, path } => {
                view.plan.steps[step].edits.insert(path, Some(text));
            }
        }
        self.caret_on = true;
        self.blink_task = None;
        self.blink(cx);
        self.work_out_soon(cx);
        cx.stop_propagation();
        cx.notify();
    }

    /// The editor is in use: its caret blinks.
    pub(super) fn plan_editing(&self) -> bool {
        self.plan.as_ref().is_some_and(|v| v.editor.is_some())
    }

    /// The editor drawn for `target`, or none when it edits another.
    fn editor_view(
        &self,
        target: &EditTarget,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let editor = self.plan.as_ref()?.editor.as_ref()?;
        if editor.target != *target {
            return None;
        }
        let focused = self.plan_focus.is_focused(window);
        let styles = editor
            .colors
            .iter()
            .map(|(range, color)| {
                let style = gpui::HighlightStyle {
                    color: Some(*color),
                    ..Default::default()
                };
                (range.clone(), style)
            })
            .collect();
        let field = editor.field.render(
            "plan-editor-text",
            "",
            focused,
            self.caret_on,
            styles,
            &self.plan_focus,
            theme,
            |this: &mut Self| {
                this.plan
                    .as_mut()
                    .and_then(|v| v.editor.as_mut())
                    .map(|e| &mut e.field)
            },
            cx,
        );
        // Code: its lines numbered as in the file, and long lines running
        // on rather than wrapping, so the numbers stay by their lines.
        let gutter = editor.code.as_ref().map(|_| {
            let count = editor.field.text().split('\n').count();
            div()
                .flex()
                .flex_col()
                .flex_none()
                .pr(px(8.))
                .text_right()
                .text_color(theme.secondary)
                .children((0..count).map(|k| div().child(format!("{}", editor.first_line + k))))
        });
        Some(
            div()
                .id("plan-editor")
                .track_focus(&self.plan_focus)
                .on_key_down(cx.listener(Self::plan_editor_key))
                .flex()
                .px(px(4.))
                .py(px(2.))
                .border_1()
                .border_color(match focused {
                    true => theme.accent,
                    false => theme.separator,
                })
                .bg(theme.background)
                .children(gutter)
                .child(
                    div()
                        .id("plan-editor-code")
                        .flex_1()
                        .min_w_0()
                        .when(editor.code.is_some(), |code| {
                            code.whitespace_nowrap().overflow_x_scroll()
                        })
                        .child(field),
                )
                .into_any_element(),
        )
    }

    /// Changes a step's action, then works the plan out again.
    fn set_step_action(&mut self, step: usize, action: Action, cx: &mut Context<Self>) {
        if let Some(view) = self.plan.as_mut() {
            view.plan.steps[step].action = action;
            view.choosing = None;
        }
        self.work_out_plan(cx);
        self.select_plan_step(step, cx);
    }

    /// Moves a step earlier, or later.
    fn move_step(&mut self, step: usize, later: bool, cx: &mut Context<Self>) {
        let Some(view) = self.plan.as_mut() else {
            return;
        };
        let other = match later {
            true => step + 1,
            false => step.wrapping_sub(1),
        };
        if other >= view.plan.steps.len() {
            return;
        }
        view.plan.steps.swap(step, other);
        view.selected = Some(other);
        view.editor = None;
        self.work_out_plan(cx);
    }

    /// Moves a step dropped on another into its place: the steps between
    /// shift one along.
    fn drop_step(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let Some(view) = self.plan.as_mut() else {
            return;
        };
        if from == to || from >= view.plan.steps.len() || to >= view.plan.steps.len() {
            return;
        }
        let step = view.plan.steps.remove(from);
        view.plan.steps.insert(to, step);
        view.selected = Some(to);
        view.choosing = None;
        view.editor = None;
        self.work_out_plan(cx);
    }

    /// Takes a step out of a plan of commits put on top; others drop it.
    fn remove_step(&mut self, step: usize, cx: &mut Context<Self>) {
        let Some(view) = self.plan.as_mut() else {
            return;
        };
        match view.plan.base {
            Base::Tip => {
                view.plan.steps.remove(step);
            }
            Base::Commit { .. } => view.plan.steps[step].action = Action::Drop,
        }
        view.selected = None;
        view.middle = false;
        view.editor = None;
        self.work_out_plan(cx);
    }

    fn apply_plan(&mut self, cx: &mut Context<Self>) {
        let (Some(repo), Some(view)) = (self.repo_path.clone(), self.plan.as_mut()) else {
            return;
        };
        let Some(worked) = view.worked.clone().filter(WorkedOut::ready) else {
            return;
        };
        let onto = view.plan.onto.clone();
        let wip = view.wip.clone();
        view.working = true;
        let task = cx.background_executor().spawn(async move {
            let undo = plan::apply(&repo, &onto, &worked)?;
            plan::mark_wip(&repo, &wip, None)?;
            Ok::<_, CoreError>(undo)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(view) = this.plan.as_mut() else {
                    return;
                };
                view.working = false;
                match result {
                    Ok(undo) => {
                        view.applied = Some(undo);
                        view.middle = false;
                        view.editor = None;
                        view.error = None;
                        this.start_refresh(cx);
                    }
                    Err(error) => {
                        view.error = Some(message(error));
                        this.work_out_plan(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn undo_plan(&mut self, cx: &mut Context<Self>) {
        let (Some(repo), Some(view)) = (self.repo_path.clone(), self.plan.as_mut()) else {
            return;
        };
        let Some(undo) = view.applied.clone() else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { plan::undo(&repo, &undo) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        // Back to the plan as it was before Apply.
                        if let Some(view) = this.plan.as_mut() {
                            view.applied = None;
                        }
                        this.work_out_plan(cx);
                        this.start_refresh(cx);
                    }
                    Err(error) => {
                        if let Some(view) = this.plan.as_mut() {
                            view.error = Some(message(error));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Closes the plan: before Apply, nothing has changed. Its progress
    /// branch goes.
    fn close_plan(&mut self, cx: &mut Context<Self>) {
        if let (Some(repo), Some(view)) = (self.repo_path.clone(), self.plan.take()) {
            let wip = view.wip;
            cx.background_executor()
                .spawn(async move { plan::mark_wip(&repo, &wip, None) })
                .detach();
        }
        self.start_refresh(cx);
        self.update_menus(cx);
        cx.notify();
    }

    /// The Rewrite tab.
    pub(super) fn plan_panel_view(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let panel = div()
            .id("plan-panel")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(self.sidebar_width))
            .h_full()
            .bg(theme.background)
            .text_sm()
            .overflow_y_scroll()
            .gap(px(6.))
            .pb(px(10.));
        let Some(view) = &self.plan else {
            return panel
                .child(
                    div()
                        .px(px(12.))
                        .pt(px(10.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Rewrite"),
                )
                .children(
                    [
                        "Nothing to rewrite yet. To start, right-click in the graph or the \
                         commit history:",
                        "• a commit: Cherry-pick, Drop, Squash or Fixup into Previous, Move Up \
                         or Down, Reword…, Edit…",
                        "• several commits (⌘- or ⇧-click first), or a section's header: \
                         Cherry-pick, Squash Together, Drop",
                        "• a branch or a commit: Rebase onto This, which moves the branch \
                         your worktree has checked out",
                        "While this tab is open, each of them waits here as a step and nothing \
                         changes until Apply. Then drag steps to reorder them, click a step's \
                         chip to change what it does, and resolve conflicts in the middle.",
                    ]
                    .map(|line| {
                        div()
                            .px(px(12.))
                            .text_xs()
                            .text_color(theme.secondary)
                            .child(line)
                    }),
                )
                .into_any_element();
        };
        let outcomes = view.worked.as_ref().map(|w| &w.outcomes);
        let waiting = outcomes.map_or(0, |o| {
            o.iter().filter(|o| o.state == State::Conflict).count()
        });
        let (title, note) = match &view.applied {
            Some(_) => (format!("Applied to {}", view.place), "done".to_string()),
            None => (
                format!("Rewrite {}", view.place),
                match (view.worked.is_some(), waiting, view.plan.steps.is_empty()) {
                    (_, _, true) => "no steps yet: actions from the menus wait here".to_string(),
                    (false, _, _) => "working out…".to_string(),
                    (true, 0, _) => "ready: nothing has changed yet".to_string(),
                    (true, 1, _) => "1 step needs you".to_string(),
                    (true, n, _) => format!("{n} steps need you"),
                },
            ),
        };
        let button = |id: &'static str, primary: bool, on: bool| {
            div()
                .id(id)
                .px(px(10.))
                .py(px(2.))
                .rounded(px(4.))
                .border_1()
                .map(|b| match primary && on {
                    true => b
                        .bg(theme.accent)
                        .border_color(theme.accent)
                        .text_color(gpui::white()),
                    false => b.border_color(theme.separator),
                })
                .when(!on, |b| b.opacity(0.4))
                .when(on, |b| b.cursor_pointer())
        };
        let count = view.plan.steps.len();
        let editable = view.applied.is_none();
        // Newest first, as the commit history lists commits; they are
        // applied from the foot up.
        // Steps whose commit the commit history shows now: marked apart from
        // the selected one.
        let viewed: HashSet<&str> = self.commits.iter().map(|c| c.id.as_str()).collect();
        let steps = view.plan.steps.iter().enumerate().rev().map(|(k, step)| {
            let outcome = outcomes.and_then(|o| o.get(k));
            let (mark, color, note) = step_note(outcome, view.applied.is_some(), theme);
            let selected = view.selected == Some(k);
            let related = viewed.contains(step.commit.as_str())
                || outcome
                    .and_then(|o| o.shown())
                    .is_some_and(|c| viewed.contains(c));
            // As it will read: a message written for it, or its own.
            let subject = step
                .message
                .as_deref()
                .and_then(|m| m.lines().next())
                .map(str::to_string)
                .or_else(|| view.subjects.get(&step.commit).cloned())
                .unwrap_or_else(|| popups::short_hash(&step.commit));
            // The commit it was, and the one it becomes.
            let ids = match outcome.and_then(|o| o.commit.as_deref()) {
                Some(new) => format!(
                    "{} → {}",
                    popups::short_hash(&step.commit),
                    popups::short_hash(new)
                ),
                None => popups::short_hash(&step.commit),
            };
            let dropped = step.action == Action::Drop;
            let arrow = |id: &'static str, label: &'static str, later: bool, on: bool| {
                div()
                    .id((id, k))
                    .px(px(3.))
                    .text_xs()
                    .text_color(theme.secondary)
                    .when(!on, |a| a.opacity(0.3))
                    .when(on, |a| {
                        a.cursor_pointer()
                            .hover(|a| a.text_color(theme.foreground))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.move_step(k, later, cx)
                            }))
                    })
                    .child(label)
            };
            let highlight = theme.selection;
            let dragged = DraggedStep {
                step: k,
                subject: subject.clone(),
            };
            let row = div()
                .id(("plan-step", k))
                .relative()
                // Dragged onto another step, it takes that one's place.
                .when(editable, |row| {
                    row.on_drag(dragged, |step, _, _, cx| {
                        cx.new(|_| crate::workspace::TabPreview(step.subject.clone()))
                    })
                    .drag_over::<DraggedStep>(move |style, _, _, _| style.bg(highlight))
                    .on_drop(cx.listener(
                        move |this, dragged: &DraggedStep, _, cx| {
                            this.drop_step(dragged.step, k, cx)
                        },
                    ))
                })
                .flex()
                .items_start()
                .gap(px(6.))
                .mx(px(6.))
                .px(px(6.))
                .py(px(4.))
                .rounded(px(4.))
                .border_1()
                .border_color(match selected {
                    true => theme.accent,
                    false => theme.separator,
                })
                .cursor_pointer()
                .hover(|row| row.bg(theme.selection))
                .on_click(cx.listener(move |this, _, _, cx| this.select_plan_step(k, cx)))
                .when(related, |row| {
                    row.child(
                        div()
                            .absolute()
                            .left(px(-1.))
                            .top(px(4.))
                            .bottom(px(4.))
                            .w(px(3.))
                            .rounded(px(1.5))
                            .bg(theme.lanes[0]),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .w(px(12.))
                        .text_color(color)
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(mark),
                )
                .child(
                    div()
                        .id(("plan-action", k))
                        .flex_none()
                        .px(px(4.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(theme.separator)
                        .text_xs()
                        .text_color(theme.secondary)
                        .when(editable, |chip| {
                            chip.cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(view) = this.plan.as_mut() {
                                        view.choosing = match view.choosing == Some(k) {
                                            true => None,
                                            false => Some(k),
                                        };
                                    }
                                    cx.notify();
                                }))
                        })
                        .child(step.action.name()),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_color(match dropped {
                                    true => theme.secondary,
                                    false => theme.foreground,
                                })
                                .when(dropped, |t| t.line_through())
                                .child(subject),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(color)
                                .child(format!("{ids} · {note}")),
                        ),
                )
                .when(editable, |row| {
                    row.child(
                        div()
                            .flex()
                            .flex_none()
                            // Up the list is later in history.
                            .child(arrow("plan-up", "↑", true, k + 1 < count))
                            .child(arrow("plan-down", "↓", false, k > 0))
                            .child(
                                div()
                                    .id(("plan-remove", k))
                                    .px(px(3.))
                                    .text_xs()
                                    .text_color(theme.secondary)
                                    .cursor_pointer()
                                    .hover(|a| a.text_color(theme.error))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.remove_step(k, cx)
                                    }))
                                    .child("×"),
                            ),
                    )
                });
            let choices =
                (view.choosing == Some(k)).then(|| {
                    div().flex().flex_wrap().gap(px(4.)).mx(px(26.)).children(
                        Action::ALL.iter().map(|&action| {
                            let on = step.action == action;
                            div()
                                .id(SharedString::from(format!(
                                    "plan-set-{k}-{}",
                                    action.name()
                                )))
                                .px(px(6.))
                                .rounded(px(3.))
                                .border_1()
                                .text_xs()
                                .cursor_pointer()
                                .map(|b| match on {
                                    true => b
                                        .bg(theme.accent)
                                        .border_color(theme.accent)
                                        .text_color(gpui::white()),
                                    false => b.border_color(theme.separator),
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_step_action(k, action, cx)
                                }))
                                .child(action.name())
                        }),
                    )
                });
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(row)
                .children(choices)
        });
        let ready = view.worked.as_ref().is_some_and(WorkedOut::ready)
            && !view.working
            && !view.plan.steps.is_empty();
        let actions = match view.applied {
            Some(_) => div()
                .flex()
                .gap(px(8.))
                .child(
                    button("plan-undo", false, true)
                        .child("Undo")
                        .on_click(cx.listener(|this, _, _, cx| this.undo_plan(cx))),
                )
                .child(
                    button("plan-close", false, true)
                        .child("Close")
                        .on_click(cx.listener(|this, _, _, cx| this.close_plan(cx))),
                ),
            None => div()
                .flex()
                .flex_wrap()
                .gap(px(8.))
                .child(
                    button("plan-apply", true, ready)
                        .child("Apply")
                        .when(ready, |b| {
                            b.on_click(cx.listener(|this, _, _, cx| this.apply_plan(cx)))
                        }),
                )
                .child(
                    button("plan-cancel", false, true)
                        .child("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| this.close_plan(cx))),
                ),
        };
        panel
            .child(
                div()
                    .px(px(12.))
                    .pt(px(10.))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(title),
                    )
                    .child(div().text_xs().text_color(theme.secondary).child(note)),
            )
            .child(div().px(px(12.)).child(actions))
            // What is settled so far, on the progress branch, to look at in
            // the commit history as any branch.
            .when(view.applied.is_none() && count > 0, |panel| {
                panel.child(
                    div().px(px(12.)).flex().child(
                        div()
                            .id("plan-progress")
                            .text_xs()
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .hover(|b| b.underline())
                            .on_click(cx.listener(|this, _, _, cx| this.show_progress(cx)))
                            .child(format!("Show done so far: {}", view.wip)),
                    ),
                )
            })
            .children(view.error.clone().map(|error| {
                div()
                    .px(px(12.))
                    .text_xs()
                    .text_color(theme.error)
                    .child(error)
            }))
            .children(steps)
            .into_any_element()
    }

    /// The selected step in the middle: its message to write, its files to
    /// edit, its conflicts in three columns.
    pub(super) fn plan_middle_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self
            .plan
            .as_ref()
            .filter(|v| v.middle && v.applied.is_none())?;
        let step = view.selected?;
        let entry = view.plan.steps.get(step)?;
        let outcome = self.plan_outcome(step);
        let subject = view
            .subjects
            .get(&entry.commit)
            .cloned()
            .unwrap_or_default();
        let header = div()
            .flex()
            .justify_between()
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(theme.separator)
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!("{} · {subject}", entry.action.name())),
            )
            .child(div().text_xs().text_color(theme.secondary).child(format!(
                "{} in {}",
                popups::short_hash(&entry.commit),
                view.place
            )));
        let message = matches!(entry.action, Action::Reword | Action::Squash).then(|| {
            let target = EditTarget::Message { step };
            let shown = entry.message.clone().unwrap_or_else(|| match entry.action {
                Action::Squash => "The two messages, joined — click to write one".to_string(),
                _ => "The commit's message — click to change it".to_string(),
            });
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .px(px(12.))
                .py(px(8.))
                .border_b_1()
                .border_color(theme.separator)
                .child(div().text_xs().text_color(theme.secondary).child("Message"))
                .child(
                    self.editor_view(&target, theme, window, cx)
                        .unwrap_or_else(|| {
                            div()
                                .id("plan-message")
                                .px(px(8.))
                                .py(px(2.))
                                .border_1()
                                .border_color(theme.separator)
                                .cursor_text()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.edit_message(step, window, cx)
                                }))
                                .child(shown)
                                .into_any_element()
                        }),
                )
        });
        let conflicted = outcome.is_some_and(|o| !o.files.is_empty());
        let body = if conflicted {
            self.merge_view(step, outcome?, theme, window, cx)
        } else if entry.action == Action::Edit {
            Some(self.file_editor_view(step, theme, window, cx))
        } else if matches!(entry.action, Action::Reword | Action::Squash) {
            None
        } else {
            Some(self.step_card(step, theme, cx))
        };
        Some(
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(header)
                .children(message)
                .child(div().flex_1().min_h_0().children(body))
                .into_any_element(),
        )
    }

    /// A step with nothing to settle: what it is, the files its commit
    /// changes, and the commit itself to look at.
    fn step_card(&self, step: usize, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = &self.plan else {
            return placeholder("No rewrite", None, theme);
        };
        let entry = &view.plan.steps[step];
        let outcome = self.plan_outcome(step);
        let what = match outcome.map(|o| o.state) {
            Some(State::Dropped) => "Left out of the branch.".to_string(),
            Some(State::Empty) => "Changes nothing there: left out.".to_string(),
            Some(_) => format!(
                "Applies cleanly: {} becomes {}.",
                popups::short_hash(&entry.commit),
                outcome
                    .and_then(|o| o.shown())
                    .map(popups::short_hash)
                    .unwrap_or_default()
            ),
            None => "Working out…".to_string(),
        };
        let shown = outcome
            .and_then(|o| o.shown())
            .unwrap_or(&entry.commit)
            .to_string();
        div()
            .id("step-card")
            .flex()
            .flex_col()
            .gap(px(8.))
            .size_full()
            .overflow_y_scroll()
            .px(px(12.))
            .py(px(8.))
            .child(div().child(what))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .font_family(preferences::code_font())
                    .text_size(px(preferences::code_size()))
                    .children(view.changed.iter().map(|path| div().child(path.clone()))),
            )
            .child(
                div().flex().child(
                    div()
                        .id("step-show-commit")
                        .px(px(10.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(theme.separator)
                        .cursor_pointer()
                        .hover(|b| b.bg(theme.selection))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show_commit_off_graph(shown.clone(), cx)
                        }))
                        .child("Show this commit"),
                ),
            )
            .into_any_element()
    }

    /// Selects the progress branch in the graph: the steps settled so far,
    /// in the commit history as any branch's commits are.
    fn show_progress(&mut self, cx: &mut Context<Self>) {
        let Some(wip) = self.plan.as_ref().map(|v| v.wip.clone()) else {
            return;
        };
        let node = self
            .graph
            .nodes
            .iter()
            .position(|node| node.refs.contains(&wip) || node.branch.as_ref() == Some(&wip));
        match node {
            Some(node) => self.select_and_reveal(node, cx),
            // Not in the graph yet: it comes with the next refresh.
            None => self.start_refresh(cx),
        }
    }

    /// An edit step's files: one picked, its content to type.
    fn file_editor_view(
        &self,
        step: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(view) = &self.plan else {
            return placeholder("No rewrite", None, theme);
        };
        let chips = div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .px(px(12.))
            .py(px(6.))
            .children(view.files.iter().map(|path| {
                let shown = view.file.as_ref() == Some(path);
                let edited = view.plan.steps[step].edits.contains_key(path);
                let target = path.clone();
                div()
                    .id(SharedString::from(format!("plan-edit-{path}")))
                    .px(px(6.))
                    .rounded(px(4.))
                    .text_xs()
                    .cursor_pointer()
                    .when(shown, |chip| chip.bg(theme.selection))
                    .hover(|chip| chip.bg(theme.selection))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.edit_file(target.clone(), window, cx)
                    }))
                    .child(format!("{}{path}", if edited { "✎ " } else { "" }))
            }));
        let editor = view.file.as_ref().map(|path| {
            let target = EditTarget::File {
                step,
                path: path.clone(),
            };
            self.editor_view(&target, theme, window, cx)
                .unwrap_or_else(|| placeholder("Click the file above to edit it", None, theme))
        });
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(chips)
            .child(
                div()
                    .id("plan-file-editor")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(8.))
                    .font_family(preferences::code_font())
                    .font_weight(preferences::code_weight())
                    .text_size(px(preferences::code_size()))
                    .children(editor),
            )
            .into_any_element()
    }

    /// A step's conflicts: the file strip, then the file's parts in three
    /// columns, or three sections each when the diff is unified.
    fn merge_view(
        &self,
        step: usize,
        outcome: &Outcome,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let view = self.plan.as_ref()?;
        let path = view.file.clone()?;
        let file = outcome.files.iter().find(|f| f.path == path)?;
        let strip =
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .px(px(12.))
                .py(px(6.))
                .children(outcome.files.iter().map(|f| {
                    let shown = f.path == path;
                    let (mark, color) = match f.resolved {
                        true => ("✓", theme.foreground),
                        false => ("!", theme.error),
                    };
                    let target = f.path.clone();
                    div()
                        .id(SharedString::from(format!("plan-file-{}", f.path)))
                        .px(px(6.))
                        .rounded(px(4.))
                        .text_xs()
                        .cursor_pointer()
                        .when(shown, |chip| chip.bg(theme.selection))
                        .hover(|chip| chip.bg(theme.selection))
                        .text_color(color)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_plan_file(target.clone(), cx)
                        }))
                        .child(format!("{mark} {}", f.path))
                }));
        let body = match (&view.regions, file.by_lines()) {
            (None, _) => placeholder("Reading the file…", None, theme),
            (Some((_, regions)), false) => self.whole_file_view(regions, file, view, theme, cx),
            (Some((_, regions)), true) => {
                self.merge_columns(regions, file, view, step, theme, window, cx)
            }
        };
        Some(
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(strip)
                .child(div().flex_1().min_h_0().child(body))
                .into_any_element(),
        )
    }

    /// Takes one side whole for the shown file, or drops the choice when it
    /// was the one taken.
    fn choose_whole(&mut self, choice: Choice, cx: &mut Context<Self>) {
        let Some(view) = self.plan.as_mut() else {
            return;
        };
        let (Some(step), Some(path), Some((sides, _))) =
            (view.selected, view.file.clone(), view.regions.as_ref())
        else {
            return;
        };
        let taken = Some(choice);
        let resolutions = &mut view.plan.steps[step].resolutions;
        let same = resolutions
            .get(&path)
            .is_some_and(|r| r.sides == *sides && r.choices.first() == Some(&taken));
        match same {
            true => {
                resolutions.remove(&path);
            }
            false => {
                let resolution = Resolution {
                    sides: sides.clone(),
                    choices: vec![taken],
                };
                resolutions.insert(path, resolution);
            }
        }
        self.work_out_plan(cx);
    }

    /// A file one side lacks, or a submodule both moved: what each side has,
    /// and a button for each, the file's content shown from the side that
    /// has it.
    fn whole_file_view(
        &self,
        regions: &[Region],
        file: &FileConflict,
        view: &PlanView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let place = match &view.plan.onto {
            Onto::Worktree(_) => view.place.clone(),
            Onto::Branch(branch) => branch.clone(),
        };
        let path = &file.path;
        let (what, ours, theirs) = match (&file.sides.ours, &file.sides.theirs) {
            (None, _) => (
                format!("{path} isn't on {place}, and the commit changes it."),
                "Leave it out".to_string(),
                "Add it as the commit has it".to_string(),
            ),
            (_, None) => (
                format!("The commit deletes {path}, which {place} has changed."),
                format!("Keep it as {place} has it"),
                "Delete it, as the commit does".to_string(),
            ),
            _ => (
                format!("The submodule {path} moved on both sides."),
                format!("Keep {place}'s commit"),
                "Take the commit's".to_string(),
            ),
        };
        let chosen = self.shown_choices().first().cloned().flatten();
        let pick = |id: &'static str, label: String, choice: Choice| {
            let on = chosen.as_ref() == Some(&choice);
            div()
                .id(id)
                .px(px(10.))
                .py(px(2.))
                .rounded(px(4.))
                .border_1()
                .cursor_pointer()
                .map(|b| match on {
                    true => b
                        .bg(theme.accent)
                        .border_color(theme.accent)
                        .text_color(gpui::white()),
                    false => b.border_color(theme.separator).bg(theme.background),
                })
                .on_click(cx.listener(move |this, _, _, cx| this.choose_whole(choice.clone(), cx)))
                .child(label)
        };
        // The content of the side that has the file: every part of the
        // merge taken from that side.
        let side = match file.sides.ours {
            None => Choice::Theirs,
            Some(_) => Choice::Ours,
        };
        let content = (file.mode != "160000")
            .then(|| {
                let parts = vec![Some(side); plan::conflicts(regions)];
                plan::resolve(regions, &parts)
            })
            .flatten()
            .unwrap_or_default();
        let colors = crate::syntax::text_colors(path, &content, theme.dark);
        let mut at = 0;
        let lines = content.split_inclusive('\n').map(|line| {
            let start = at;
            at += line.len();
            let text = line.trim_end_matches(['\n', '\r']).to_string();
            let end = start + text.len();
            let highlights: Vec<_> = colors
                .iter()
                .filter(|(r, _)| r.start >= start && r.end <= end)
                .map(|(r, color)| {
                    let style = gpui::HighlightStyle {
                        color: Some(*color),
                        ..Default::default()
                    };
                    (r.start - start..r.end - start, style)
                })
                .collect();
            div()
                .px(px(8.))
                .whitespace_nowrap()
                .child(StyledText::new(text).with_highlights(highlights))
        });
        let shown = match file.sides.ours {
            None => "The commit's version",
            Some(_) => "The version there",
        };
        div()
            .id("whole-file")
            .flex()
            .flex_col()
            .gap(px(8.))
            .size_full()
            .overflow_y_scroll()
            .px(px(12.))
            .py(px(8.))
            .child(div().child(what))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(pick("whole-ours", ours, Choice::Ours))
                    .child(pick("whole-theirs", theirs, Choice::Theirs)),
            )
            .when(!content.is_empty(), |pane| {
                pane.child(div().text_xs().text_color(theme.secondary).child(shown))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .font_family(preferences::code_font())
                            .font_weight(preferences::code_weight())
                            .text_size(px(preferences::code_size()))
                            .children(lines),
                    )
            })
            .into_any_element()
    }

    /// A file's conflicts, each one block with the lines around it: where
    /// the commits go, the result, the commit's side, in three columns or,
    /// unified, one under another. The result is typed into wherever it is
    /// clicked; three buttons above take the left side, both, or the right.
    #[allow(clippy::too_many_arguments)]
    fn merge_columns(
        &self,
        regions: &[Region],
        file: &FileConflict,
        view: &PlanView,
        step: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ours_color = theme.lanes[0].opacity(0.14);
        let theirs_color = theme.lanes[1].opacity(0.14);
        let open_color = gpui::hsla(0.09, 0.9, 0.55, 0.16);
        let choices = self.shown_choices();
        let windows = plan::windows(regions);
        let one = self.unified.unwrap_or(self.one_column.get());
        let onto = match &view.plan.onto {
            Onto::Worktree(_) => view.place.clone(),
            Onto::Branch(branch) => branch.clone(),
        };
        let commit = &view.plan.steps[step].commit;
        let subject = view.subjects.get(commit).cloned().unwrap_or_default();
        let titles = [
            (onto, "where it goes".to_string()),
            ("Result".to_string(), file.path.clone()),
            (popups::short_hash(commit), subject),
        ];
        let column_head = |k: usize| {
            let (title, note) = titles[k].clone();
            div()
                .flex_1()
                .min_w_0()
                .px(px(8.))
                .py(px(4.))
                .bg([ours_color, theme.background, theirs_color][k])
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.secondary)
                        .truncate()
                        .child(note),
                )
        };
        // Lines colored as the file's code, together, so strings and comments
        // over several lines keep their color; `tint` behind those of the
        // conflict itself.
        let dark = theme.dark;
        let code = |parts: &[(&[String], Option<gpui::Hsla>)]| {
            let all: String = parts.iter().map(|(lines, _)| lines.concat()).collect();
            let colors = crate::syntax::text_colors(&file.path, &all, dark);
            let mut at = 0;
            let mut rows = Vec::new();
            for (lines, tint) in parts {
                for line in lines.iter() {
                    let start = at;
                    at += line.len();
                    let text = line.trim_end_matches(['\n', '\r']);
                    let shown: AnyElement = match text.contains('\t') {
                        true => text.replace('\t', "    ").into_any_element(),
                        false => {
                            let end = start + text.len();
                            let highlights: Vec<_> = colors
                                .iter()
                                .filter(|(r, _)| r.start >= start && r.end <= end)
                                .map(|(r, color)| {
                                    let style = gpui::HighlightStyle {
                                        color: Some(*color),
                                        ..Default::default()
                                    };
                                    (r.start - start..r.end - start, style)
                                })
                                .collect();
                            StyledText::new(text.to_string())
                                .with_highlights(highlights)
                                .into_any_element()
                        }
                    };
                    rows.push(
                        div()
                            .px(px(8.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .when_some(*tint, |row, tint| row.bg(tint))
                            .child(shown),
                    );
                }
            }
            div().flex().flex_col().children(rows)
        };
        let folded = |count: usize| {
            div()
                .py(px(2.))
                .text_center()
                .text_xs()
                .text_color(theme.secondary)
                .child(format!("⋯ {count} lines the same ⋯"))
        };
        let section = |title: String, color: gpui::Hsla, body: AnyElement| {
            div()
                .flex()
                .flex_col()
                .bg(color)
                .child(
                    div()
                        .px(px(8.))
                        .pt(px(2.))
                        .font_family(preferences::ui_font())
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(title),
                )
                .child(body)
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut part = 0;
        for (k, region) in regions.iter().enumerate() {
            match region {
                Region::Same(same) => {
                    // The lines not going with a conflict's block.
                    let after_prev = match k.checked_sub(1).map(|k| &regions[k]) {
                        Some(Region::Conflict { .. }) => windows[part - 1].1,
                        _ => 0,
                    };
                    let before_next = match regions.get(k + 1) {
                        Some(Region::Conflict { .. }) => windows[part].0,
                        _ => 0,
                    };
                    let between = &same[after_prev..same.len() - before_next];
                    if between.is_empty() {
                        continue;
                    }
                    if between.len() > 4 {
                        rows.push(folded(between.len()).into_any_element());
                        continue;
                    }
                    let columns = if one { 1 } else { 3 };
                    rows.push(
                        div()
                            .flex()
                            .gap(px(4.))
                            .children(
                                (0..columns).map(|_| {
                                    div().flex_1().min_w_0().child(code(&[(between, None)]))
                                }),
                            )
                            .into_any_element(),
                    );
                }
                Region::Conflict { ours, theirs, .. } => {
                    let index = part;
                    part += 1;
                    let (before_n, after_n) = windows[index];
                    let before: &[String] = match k.checked_sub(1).map(|k| &regions[k]) {
                        Some(Region::Same(lines)) => &lines[lines.len() - before_n..],
                        _ => &[],
                    };
                    let after: &[String] = match regions.get(k + 1) {
                        Some(Region::Same(lines)) => &lines[..after_n],
                        _ => &[],
                    };
                    let chosen = choices.get(index).cloned().flatten();
                    // The result block: typed into, as chosen, or waiting.
                    let target = EditTarget::Part {
                        step,
                        path: file.path.clone(),
                        part: index,
                    };
                    let result: AnyElement =
                        match (self.editor_view(&target, theme, window, cx), &chosen) {
                            (Some(editor), _) => editor,
                            (None, Some(Choice::Text(text))) => {
                                let lines: Vec<String> =
                                    text.split_inclusive('\n').map(str::to_string).collect();
                                code(&[(&lines, None)]).into_any_element()
                            }
                            (None, Some(choice)) => {
                                let lines: Vec<String> = plan::chosen(ours, theirs, choice)
                                    .split_inclusive('\n')
                                    .map(str::to_string)
                                    .collect();
                                code(&[
                                    (before, None),
                                    (&lines, Some(theme.selection.opacity(0.5))),
                                    (after, None),
                                ])
                                .into_any_element()
                            }
                            (None, None) => div()
                                .flex()
                                .flex_col()
                                .child(code(&[(before, None)]))
                                .child(
                                    div()
                                        .px(px(8.))
                                        .bg(open_color)
                                        .text_color(theme.secondary)
                                        .child(format!(
                                            "Conflict {} — choose above, or click to type",
                                            index + 1
                                        )),
                                )
                                .child(code(&[(after, None)]))
                                .into_any_element(),
                        };
                    // A click anywhere in it starts typing there.
                    let result =
                        div()
                            .id(("result-block", index))
                            .flex_1()
                            .min_w_0()
                            .cursor_text()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_part(index, window, cx)
                            }))
                            .child(result);
                    let pick = |id: &'static str, label: &'static str, choice: Choice| {
                        let on = chosen.as_ref() == Some(&choice);
                        div()
                            .id((id, index))
                            .px(px(10.))
                            .rounded(px(4.))
                            .border_1()
                            .text_xs()
                            .cursor_pointer()
                            .map(|b| match on {
                                true => b
                                    .bg(theme.accent)
                                    .border_color(theme.accent)
                                    .text_color(gpui::white()),
                                false => b.border_color(theme.separator).bg(theme.background),
                            })
                            .hover(|b| b.opacity(0.85))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.choose(index, choice.clone(), cx)
                            }))
                            .child(label)
                    };
                    let (left, right) = match one {
                        true => ("◂ Where it goes", "The commit ▸"),
                        false => ("◂ Left", "Right ▸"),
                    };
                    let buttons = div()
                        .flex()
                        .justify_center()
                        .gap(px(4.))
                        .py(px(3.))
                        .font_family(preferences::ui_font())
                        .child(pick("take-ours", left, Choice::Ours))
                        .child(pick("take-both", "Both", Choice::OursThenTheirs))
                        .child(pick("take-theirs", right, Choice::Theirs));
                    let side = |lines: &[String], tint: gpui::Hsla| {
                        code(&[(before, None), (lines, Some(tint)), (after, None)])
                    };
                    if one {
                        rows.push(buttons.into_any_element());
                        rows.push(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(section(
                                    format!("{} · where it goes", titles[0].0),
                                    gpui::transparent_black(),
                                    side(ours, ours_color).into_any_element(),
                                ))
                                .child(section(
                                    "Result — click to type".to_string(),
                                    gpui::transparent_black(),
                                    result.into_any_element(),
                                ))
                                .child(section(
                                    format!("{} · {}", titles[2].0, titles[2].1),
                                    gpui::transparent_black(),
                                    side(theirs, theirs_color).into_any_element(),
                                ))
                                .into_any_element(),
                        );
                        continue;
                    }
                    rows.push(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(div().flex_1())
                            .child(div().flex_1().child(buttons))
                            .child(div().flex_1())
                            .into_any_element(),
                    );
                    rows.push(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(div().flex_1().min_w_0().child(side(ours, ours_color)))
                            .child(result)
                            .child(div().flex_1().min_w_0().child(side(theirs, theirs_color)))
                            .into_any_element(),
                    );
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .when(!one, |pane| {
                pane.child(
                    div()
                        .flex()
                        .gap(px(4.))
                        .px(px(4.))
                        .pt(px(4.))
                        .text_sm()
                        .child(column_head(0))
                        .child(column_head(1))
                        .child(column_head(2)),
                )
            })
            .child(
                div()
                    .id("merge-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(4.))
                    .pb(px(12.))
                    .font_family(preferences::code_font())
                    .font_weight(preferences::code_weight())
                    .text_size(px(preferences::code_size()))
                    .children(rows),
            )
            .into_any_element()
    }
}

/// Shapes `plan` by `action`: the steps it adds or changes.
fn shape(repo: &str, plan: &mut Plan, action: &HistoryAction) -> Result<(), String> {
    let reach = |plan: &mut Plan, commit: &str| plan.reach(repo, commit).map_err(message);
    // The commit before, in history, made a step too: for a squash into it,
    // or a move past it. The first commit of all has none.
    let reach_parent = |plan: &mut Plan, commit: &str| -> Result<(), String> {
        if let Ok(parent) = plan::rev(repo, &format!("{commit}^")) {
            reach(plan, &parent)?;
        }
        Ok(())
    };
    match action {
        HistoryAction::Pick { commits, .. } => {
            plan.steps
                .extend(commits.iter().map(|c| plan::Step::new(c)));
        }
        HistoryAction::Rebase { .. } => {}
        HistoryAction::Drop(commits) => {
            for commit in commits {
                let at = reach(plan, commit)?;
                plan.steps[at].action = Action::Drop;
            }
        }
        HistoryAction::Squash(commits) | HistoryAction::Fixup(commits) => {
            let joined = match action {
                HistoryAction::Squash(_) => Action::Squash,
                _ => Action::Fixup,
            };
            if let [commit] = &commits[..] {
                reach_parent(plan, commit)?;
                let at = reach(plan, commit)?;
                if at == 0 {
                    return Err("Nothing comes before this commit to squash into".into());
                }
                plan.steps[at].action = joined;
                return Ok(());
            }
            // Several: into the oldest of them, the others moved right after
            // it, in their order.
            for commit in commits {
                reach(plan, commit)?;
            }
            let mut at: Vec<usize> = commits.iter().filter_map(|c| plan.step_of(c)).collect();
            at.sort_unstable();
            let first = at[0];
            let mut moved: Vec<plan::Step> = at[1..]
                .iter()
                .rev()
                .map(|&k| plan.steps.remove(k))
                .collect();
            moved.reverse();
            for (k, mut step) in moved.into_iter().enumerate() {
                step.action = joined;
                plan.steps.insert(first + 1 + k, step);
            }
        }
        HistoryAction::MoveUp(commit) => {
            let at = reach(plan, commit)?;
            if at + 1 >= plan.steps.len() {
                return Err("This is the newest commit already".into());
            }
            plan.steps.swap(at, at + 1);
        }
        HistoryAction::MoveDown(commit) => {
            reach_parent(plan, commit)?;
            let at = reach(plan, commit)?;
            if at == 0 {
                return Err("Nothing comes before this commit".into());
            }
            plan.steps.swap(at, at - 1);
        }
        HistoryAction::Reword(commit) => {
            let at = reach(plan, commit)?;
            plan.steps[at].action = Action::Reword;
        }
        HistoryAction::Edit(commit) => {
            let at = reach(plan, commit)?;
            plan.steps[at].action = Action::Edit;
        }
    }
    Ok(())
}

/// A step's mark, its colour and a few words for the Rewrite tab.
fn step_note(
    outcome: Option<&Outcome>,
    applied: bool,
    theme: &Theme,
) -> (&'static str, gpui::Hsla, String) {
    let green = header::ADDED;
    let Some(outcome) = outcome else {
        return ("○", theme.secondary, "working out…".to_string());
    };
    let files = |n: usize| match n {
        1 => "1 file".to_string(),
        n => format!("{n} files"),
    };
    let guess = if outcome.guessed { "likely " } else { "" };
    match outcome.state {
        State::Dropped => ("–", theme.secondary, "dropped".to_string()),
        _ if applied && outcome.state != State::Empty => ("✓", green, "applied".to_string()),
        State::Clean => ("✓", green, format!("{guess}clean")),
        State::Empty => ("–", theme.secondary, "already there: left out".to_string()),
        State::Resolved => ("✓", theme.accent, "resolved".to_string()),
        State::Conflict => {
            let left = outcome.files.iter().filter(|f| !f.resolved).count();
            (
                "!",
                theme.error,
                format!("{guess}conflict in {}", files(left)),
            )
        }
    }
}
