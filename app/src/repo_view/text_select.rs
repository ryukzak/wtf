//! Selecting part of the diff's code, as text, to copy: dragged over the
//! code, a word double-clicked, a line triple-clicked. Lines are picked from
//! their numbers instead.

use gpui::{
    ClipboardItem, Context, Modifiers, MouseDownEvent, MouseMoveEvent, Pixels, Point, Window,
};

use super::*;
use crate::diff_view::{self, Pane};

/// A place in the code: a file's diff row and a byte of its line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct TextAt {
    pub row: usize,
    pub byte: usize,
}

/// Text selected in one file's diff, in one pane, from `anchor` to `head`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TextSelection {
    pub file: usize,
    pub pane: Pane,
    pub anchor: TextAt,
    pub head: TextAt,
}

impl TextSelection {
    fn ordered(&self) -> (TextAt, TextAt) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }
}

impl RepoView {
    /// The pane and the character column, scrolled, the mouse at `position`
    /// is over on a row; columns count from the start of the row's part.
    /// In `pane`, when given, wherever the mouse is.
    fn column_at(
        &self,
        file: usize,
        position: Point<Pixels>,
        pane: Option<Pane>,
    ) -> Option<(Pane, f32)> {
        let bounds = self.stacked_scroll.0.borrow().base_handle.bounds();
        let x = f32::from(position.x - bounds.origin.x);
        let width = f32::from(bounds.size.width);
        let gutter = diff_view::gutter_width();
        let half = (width - 1.) / 2.;
        let (pane, left) = match (self.one_column.get(), pane) {
            (true, _) => (Pane::Unified, 2. * gutter),
            (false, Some(Pane::Left)) => (Pane::Left, gutter),
            (false, Some(_)) => (Pane::Right, half + 1. + gutter),
            (false, None) if x < half => (Pane::Left, gutter),
            (false, None) => (Pane::Right, half + 1. + gutter),
        };
        if self.scroll_room.get().1 <= 0. {
            return None;
        }
        Some((pane, (x - left + self.shift(file)) / self.char_width()))
    }

    /// The byte of `row`'s line in `pane` nearest column `column` of
    /// wrapped part `part`.
    fn byte_at(&self, file: usize, row: usize, part: usize, pane: Pane, column: f32) -> usize {
        let Some(text) = self.row_text(file, row, pane) else {
            return 0;
        };
        let start = match self.file_cols(file) {
            Some(cols) => diff_view::wrap_starts(text, cols)
                .get(part)
                .copied()
                .unwrap_or(0),
            None => 0,
        };
        let column = start + column.round().max(0.) as usize;
        text.char_indices().nth(column).map_or(text.len(), |(i, _)| i)
    }

    /// The line of a file's diff row in `pane`.
    pub(super) fn row_text(&self, file: usize, row: usize, pane: Pane) -> Option<&str> {
        let diff = self.stacked_diffs.get(file)?.as_ref().ok()?;
        let row = self.shown_rows(diff).get(row)?;
        diff_view::pane_side(row, pane).map(|side| side.text.as_str())
    }

    /// Whether a press at `position` on a row lands on its code, not on
    /// the line numbers, where lines are picked.
    pub(super) fn on_code(&self, file: usize, position: Point<Pixels>) -> bool {
        self.column_at(file, position, None)
            .is_some_and(|(_, column)| column + self.shift(file) / self.char_width() >= 0.)
    }

    fn shift(&self, file: usize) -> f32 {
        match self.file_cols(file) {
            Some(_) => 0.,
            None => self.diff_x.get(),
        }
    }

    fn char_width(&self) -> f32 {
        self.scroll_room.get().1.max(1.)
    }

    /// A press on the code: starts a selection there, a word's on a double
    /// click, the line's on a triple; ⇧ extends the one there is.
    pub(super) fn press_text(
        &mut self,
        file: usize,
        row: usize,
        part: usize,
        position: Point<Pixels>,
        clicks: usize,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let Some((pane, column)) = self.column_at(file, position, None) else {
            return;
        };
        let byte = self.byte_at(file, row, part, pane, column);
        let at = TextAt { row, byte };
        self.line_selection = None;
        self.line_drag = None;
        self.selecting_text = true;
        match &mut self.text_selection {
            Some(selection) if modifiers.shift && selection.file == file => {
                selection.head = at;
            }
            _ => {
                let text = self.row_text(file, row, pane).unwrap_or_default();
                let (start, end) = match clicks {
                    1 => (byte, byte),
                    2 => word_around(text, byte),
                    _ => (0, text.len()),
                };
                self.text_selection = Some(TextSelection {
                    file,
                    pane,
                    anchor: TextAt { row, byte: start },
                    head: TextAt { row, byte: end },
                });
            }
        }
        cx.notify();
    }

    /// The mouse over a row while text is being selected: the selection
    /// runs to it, in its own file and pane. Released, it is over.
    pub(super) fn drag_text(
        &mut self,
        file: usize,
        row: usize,
        part: usize,
        position: Point<Pixels>,
        pressed: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting_text {
            return;
        }
        if !pressed {
            self.selecting_text = false;
            return;
        }
        let Some(selection) = &self.text_selection else {
            return;
        };
        if selection.file != file {
            return;
        }
        let pane = selection.pane;
        let Some((_, column)) = self.column_at(file, position, Some(pane)) else {
            return;
        };
        let byte = self.byte_at(file, row, part, pane, column);
        let head = TextAt { row, byte };
        if let Some(selection) = &mut self.text_selection
            && selection.head != head
        {
            selection.head = head;
            cx.notify();
        }
    }

    /// The bytes of `row`'s line selected, and in which pane.
    pub(super) fn selected_in(&self, file: usize, row: usize) -> diff_view::Selected {
        let selection = self.text_selection.as_ref().filter(|s| s.file == file)?;
        let (from, to) = selection.ordered();
        if row < from.row || row > to.row {
            return None;
        }
        let len = self
            .row_text(file, row, selection.pane)
            .map_or(0, str::len);
        let start = if row == from.row { from.byte } else { 0 };
        let end = if row == to.row { to.byte } else { len };
        Some((selection.pane, start.min(len)..end.min(len)))
    }

    /// Copies the selected text, its lines joined by newlines; a pane with
    /// no line on a row adds none. False with nothing selected.
    pub(super) fn copy_selected_text(&self, cx: &mut Context<Self>) -> bool {
        let Some(selection) = &self.text_selection else {
            return false;
        };
        let (from, to) = selection.ordered();
        if from == to {
            return false;
        }
        let lines: Vec<&str> = (from.row..=to.row)
            .filter_map(|row| {
                let text = self.row_text(selection.file, row, selection.pane)?;
                let start = if row == from.row { from.byte } else { 0 };
                let end = if row == to.row { to.byte } else { text.len() };
                text.get(start.min(text.len())..end.min(text.len()))
            })
            .collect();
        cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
        true
    }

    /// The Snapshot file's line `row`, as shown.
    fn snapshot_line(&self, row: usize) -> Option<&str> {
        let (_, _, text) = self.snapshot.text.as_ref()?;
        text.as_ref().ok()?.lines().nth(row)
    }

    /// The byte of the Snapshot file's line `row` the mouse at `position`
    /// is nearest.
    fn snapshot_byte(&self, row: usize, position: Point<Pixels>, window: &Window) -> usize {
        let bounds = self.snapshot_scroll.0.borrow().base_handle.bounds();
        let x = f32::from(position.x - bounds.origin.x) - diff_view::gutter_width();
        let width = stack::code_char_width(window).max(1.);
        let column = (x / width).round().max(0.) as usize;
        let text = self.snapshot_line(row).unwrap_or_default();
        text.char_indices().nth(column).map_or(text.len(), |(i, _)| i)
    }

    /// A press on the Snapshot file: starts a selection there, a word's on
    /// a double click, the line's on a triple; ⇧ extends the one there is.
    pub(super) fn press_snapshot(
        &mut self,
        row: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.snapshot.path.clone() else {
            return;
        };
        let byte = self.snapshot_byte(row, event.position, window);
        self.selecting_snapshot = true;
        match &mut self.snapshot_selection {
            Some((shown, _, head)) if event.modifiers.shift && *shown == path => {
                *head = TextAt { row, byte };
            }
            _ => {
                let text = self.snapshot_line(row).unwrap_or_default();
                let (start, end) = match event.click_count {
                    0 | 1 => (byte, byte),
                    2 => word_around(text, byte),
                    _ => (0, text.len()),
                };
                self.snapshot_selection = Some((
                    path,
                    TextAt { row, byte: start },
                    TextAt { row, byte: end },
                ));
            }
        }
        cx.notify();
    }

    /// The mouse over the Snapshot file's line `row` while selecting: the
    /// selection runs to it. Released, it is over.
    pub(super) fn drag_snapshot(
        &mut self,
        row: usize,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting_snapshot {
            return;
        }
        if event.pressed_button != Some(gpui::MouseButton::Left) {
            self.selecting_snapshot = false;
            return;
        }
        let head = TextAt {
            row,
            byte: self.snapshot_byte(row, event.position, window),
        };
        if let Some((_, _, at)) = &mut self.snapshot_selection
            && *at != head
        {
            *at = head;
            cx.notify();
        }
    }

    /// Copies the text selected in the Snapshot file; false with none.
    pub(super) fn copy_snapshot_text(&self, cx: &mut Context<Self>) -> bool {
        let Some((path, anchor, head)) = &self.snapshot_selection else {
            return false;
        };
        if self.snapshot.path.as_ref() != Some(path) || anchor == head {
            return false;
        }
        let (from, to) = (*anchor.min(head), *anchor.max(head));
        let lines: Vec<&str> = (from.row..=to.row)
            .filter_map(|row| {
                let text = self.snapshot_line(row)?;
                let start = if row == from.row { from.byte } else { 0 };
                let end = if row == to.row { to.byte } else { text.len() };
                text.get(start.min(text.len())..end.min(text.len()))
            })
            .collect();
        cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
        true
    }
}
