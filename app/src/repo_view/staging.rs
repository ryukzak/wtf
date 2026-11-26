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

    /// The rows of the hunk a row is in, as its header spans them; the
    /// whole file where the diff has no hunk headers.
    pub(super) fn hunk_of(&self, file: usize, row: usize) -> std::ops::Range<usize> {
        self.stacked
            .iter()
            .find_map(|item| match *item {
                StackItem::Hunk(f, start, end) if f == file && (start..end).contains(&row) => {
                    Some(start..end)
                }
                _ => None,
            })
            .unwrap_or(0..usize::MAX)
    }

    /// A click picks one line, Shift-click a range and Cmd-click toggles one;
    /// picking stays inside one hunk: in another file or hunk it starts
    /// over, and a range ends at the hunk's edge.
    pub(super) fn select_line(
        &mut self,
        file: usize,
        row: usize,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let hunk = self.hunk_of(file, row);
        let rows = match &mut self.line_selection {
            Some((f, rows)) if *f == file && rows.iter().all(|r| hunk.contains(r)) => rows,
            // Shift-click in another hunk: from the edge of the anchor's.
            Some((f, _)) if *f == file && modifiers.shift => {
                let anchor = self.line_anchor.unwrap_or(row);
                let theirs = self.hunk_of(file, anchor);
                let row = row.clamp(theirs.start, theirs.end.saturating_sub(1));
                let picked = (anchor.min(row)..=anchor.max(row)).collect();
                self.line_selection = Some((file, picked));
                cx.notify();
                return;
            }
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

    /// A press on a line picks it as a click does, and starts a drag that
    /// picks the lines from it to wherever the mouse goes; with ⌘ the lines
    /// picked before stay picked, with ⇧ the drag runs from the last line
    /// picked.
    pub(super) fn press_line(
        &mut self,
        file: usize,
        row: usize,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        // With ⌘, the lines picked before stay: those in this hunk.
        let hunk = self.hunk_of(file, row);
        let before = match &self.line_selection {
            Some((f, rows)) if *f == file && modifiers.platform => {
                rows.iter().copied().filter(|r| hunk.contains(r)).collect()
            }
            _ => Vec::new(),
        };
        let anchor = match self.line_anchor {
            Some(anchor) if modifiers.shift => anchor,
            _ => row,
        };
        self.select_line(file, row, modifiers, cx);
        self.line_drag = Some((file, anchor, before, false));
    }

    /// The mouse over a line while a drag picks lines: the lines from where
    /// it started to this one. Released, the drag is over.
    pub(super) fn drag_line(
        &mut self,
        file: usize,
        row: usize,
        pressed: bool,
        cx: &mut Context<Self>,
    ) {
        let Some((from_file, anchor, _, _)) = &self.line_drag else {
            return;
        };
        if !pressed {
            self.line_drag = None;
            return;
        }
        // Dragged out of its hunk, it stops at the hunk's edge.
        let hunk = self.hunk_of(*from_file, *anchor);
        let row = row.clamp(hunk.start, hunk.end.saturating_sub(1));
        let Some((from_file, anchor, before, moved)) = &mut self.line_drag else {
            return;
        };
        // Over its first line still, it is a click so far.
        if *from_file != file || (!*moved && row == *anchor) {
            return;
        }
        *moved = true;
        let mut rows = before.clone();
        for line in (*anchor).min(row)..=(*anchor).max(row) {
            if !rows.contains(&line) {
                rows.push(line);
            }
        }
        if self.line_selection.as_ref().map(|(_, r)| r) != Some(&rows) {
            self.line_selection = Some((file, rows));
            cx.notify();
        }
    }

    /// Copies the picked lines' code, one a line: the new side's when any
    /// picked line has one, else the removed lines'. False with none picked.
    pub(super) fn copy_picked_lines(&self, cx: &mut Context<Self>) -> bool {
        let Some((file, rows)) = &self.line_selection else {
            return false;
        };
        let Some(Ok(diff)) = self.stacked_diffs.get(*file) else {
            return false;
        };
        let Some(text) = picked_text(self.shown_rows(diff), rows) else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
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

/// The code of picked diff rows, one a line, in order: the new side's when
/// any picked row has one, else the removed lines'; none with nothing.
fn picked_text(shown: &[wtf_core::DiffRow], rows: &[usize]) -> Option<String> {
    let mut rows = rows.to_vec();
    rows.sort_unstable();
    let picked: Vec<&wtf_core::DiffRow> = rows.iter().filter_map(|&r| shown.get(r)).collect();
    let new = picked.iter().any(|row| row.right.is_some());
    let lines: Vec<&str> = picked
        .iter()
        .filter_map(|row| match new {
            true => row.right.as_ref(),
            false => row.left.as_ref(),
        })
        .map(|side| side.text.as_str())
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wtf_core::{DiffRow, DiffSide};

    fn side(text: &str) -> Option<DiffSide> {
        Some(DiffSide {
            line_number: 1,
            text: text.to_string(),
            changes: Vec::new(),
        })
    }

    #[test]
    fn picked_lines_copy_the_new_code_or_else_the_removed() {
        let rows = vec![
            DiffRow {
                left: side("a"),
                right: side("a"),
            },
            DiffRow {
                left: side("old"),
                right: None,
            },
            DiffRow {
                left: side("b"),
                right: side("B"),
            },
            DiffRow {
                left: None,
                right: side("added"),
            },
        ];
        // Picked in any order, copied in the diff's.
        assert_eq!(picked_text(&rows, &[3, 0, 1, 2]).unwrap(), "a\nB\nadded\n");
        // Removed lines alone: theirs.
        assert_eq!(picked_text(&rows, &[1]).unwrap(), "old\n");
        assert_eq!(picked_text(&rows, &[]), None);
    }
}
