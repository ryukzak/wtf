//! Staging, unstaging and discarding files and picked lines.

use super::*;

impl RepoView {
    /// Stages or unstages files, all of them when `files` is empty, then
    /// shows the result.
    /// Gives the view the keys after a click on a button, when nothing in it
    /// has them, so ⌘↩ commits what was just staged.
    pub(super) fn keep_keys(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.contains_focused(window, cx) {
            self.focus.focus(window);
        }
    }

    pub(super) fn stage(&mut self, staged: bool, files: Vec<String>, cx: &mut Context<Self>) {
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

    /// A click picks one line, Shift-click a range and Cmd-click toggles one;
    /// picking in another file starts over.
    pub(super) fn select_line(
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
    pub(super) fn stage_selected_lines(&mut self, staged: bool, cx: &mut Context<Self>) {
        // Picked lines stay picked while a refresh leaves them nothing to stage.
        if let Some((index, rows)) = self.line_selection.clone()
            && self.picked_lines(index, &rows).is_some()
        {
            self.line_selection = None;
            self.stage_rows(index, rows, staged, cx);
        }
    }

    /// Stages, or unstages, the changes on some diff rows of a file.
    pub(super) fn stage_rows(
        &mut self,
        index: usize,
        rows: Vec<usize>,
        staged: bool,
        cx: &mut Context<Self>,
    ) {
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
        // While a refresh is under way the diff may be another file's.
        if self.stacked_files.get(index) != Some(file) {
            return None;
        }
        let (mut removed, mut added) = (HashSet::new(), HashSet::new());
        for row in rows.iter().filter_map(|&r| self.shown_rows(diff).get(r)) {
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
    pub(super) fn discard_rows(
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
            file.path()
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
    pub(super) fn discard_file(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(worktree), Some(file)) =
            (self.staging_worktree(), self.files.get(index).cloned())
        else {
            return;
        };
        let question = format!("Discard the changes to {}?", file.path());
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
    pub(super) fn after_change(&mut self, result: Result<(), CoreError>, cx: &mut Context<Self>) {
        self.line_selection = None;
        if let Err(error) = result {
            self.error_message = Some(message(error));
        }
        let keep = self
            .selected_file
            .and_then(|i| self.files.get(i))
            .map(|f| f.path().to_string());
        self.load_files(keep, cx);
        self.start_refresh(cx);
        cx.notify();
    }
}
