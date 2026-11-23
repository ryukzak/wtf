//! The commit box: typing a message, the model writing one, and committing.

use super::*;

impl RepoView {
    pub(super) fn commit(&mut self, cx: &mut Context<Self>) {
        let message = self.draft.message.trim().to_string();
        let staged = self
            .worktree_split
            .is_some_and(|split| split < self.files.len());
        let amend = self.draft.amend;
        // With no message typed, the commit waits for the model to write one
        // from what is staged.
        if message.is_empty() && staged && !amend && preferences::summaries() {
            self.draft.commit_when_written = true;
            if !self.draft.writing_message {
                self.draft.suggest_key = None;
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
        let worktree = path.clone();
        let run = cx
            .background_executor()
            .spawn(async move { wtf_core::commit(&path, &message, amend) });
        cx.spawn(async move |this, cx| {
            let result = run.await;
            this.update(cx, |this, cx| {
                // The committed worktree's box empties, shown or put away.
                if result.is_ok() {
                    match this.draft.worktree.as_ref() == Some(&worktree) {
                        true => {
                            this.draft = CommitDraft {
                                worktree: Some(worktree),
                                ..CommitDraft::default()
                            }
                        }
                        false => drop(this.drafts.remove(&worktree)),
                    }
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
    pub(super) fn suggest_message(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.staging_worktree() else {
            return;
        };
        let own = !self.draft.message.trim().is_empty()
            && self.draft.suggested.as_ref() != Some(&self.draft.message);
        if !preferences::summaries() || own || self.draft.amend {
            return;
        }
        let split = self.worktree_split.unwrap_or(self.files.len());
        let key: Vec<String> = self.files[split.min(self.files.len())..]
            .iter()
            .map(|file| format!("{}:{}", file.path(), file.new_id))
            .collect();
        if key.is_empty() {
            if self.draft.suggested.take().as_ref() == Some(&self.draft.message) {
                self.draft.message.clear();
            }
            (self.draft.suggest_key, self.draft.suggest_task) = (None, None);
            self.draft.writing_message = false;
            return;
        }
        let key = key.join(",");
        if self.draft.suggest_key.as_ref() == Some(&key) {
            return;
        }
        self.draft.suggest_key = Some(key);
        self.draft.writing_message = true;
        self.draft.suggest_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SUGGEST_DELAY).await;
            let prompt = preferences::message_prompt();
            let write = cx
                .background_executor()
                .spawn(async move { wtf_core::write_commit_message(&path, &prompt) });
            let written = write.await;
            this.update(cx, |this, cx| {
                this.draft.writing_message = false;
                let own = !this.draft.message.trim().is_empty()
                    && this.draft.suggested.as_ref() != Some(&this.draft.message);
                match (written, own) {
                    // Amend turned on meanwhile keeps the latest commit's message.
                    (Ok(_), false) if this.draft.amend => {}
                    (Ok(message), false) => {
                        this.set_message(message.clone(), false);
                        this.draft.suggested = Some(message);
                        // A commit asked for with the box empty goes now.
                        if std::mem::take(&mut this.draft.commit_when_written) {
                            this.commit(cx);
                        }
                    }
                    (Err(error), _) if this.draft.commit_when_written => {
                        this.draft.commit_when_written = false;
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
            "a" if modifiers.platform => self.draft.message_selected = true,
            "c" if modifiers.platform => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.draft.message.clone()))
            }
            "x" if modifiers.platform => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.draft.message.clone()));
                self.set_message(String::new(), false);
                self.draft.message_selected = false;
            }
            "up" | "down" => {
                self.draft.message_selected = false;
                self.cursor_line(keystroke.key == "down");
            }
            _ => {
                // Typing, pasting or deleting over a selection replaces it;
                // moving the cursor drops the selection.
                let moving = matches!(keystroke.key.as_str(), "left" | "right" | "home" | "end");
                let replace = self.draft.message_selected && !moving;
                let (mut text, mut cursor) = match replace {
                    true => (String::new(), 0),
                    false => (self.draft.message.clone(), self.cursor()),
                };
                let paste = || cx.read_from_clipboard().and_then(|item| item.text());
                let deleting = matches!(keystroke.key.as_str(), "backspace" | "delete");
                if !(edit_key(&mut text, &mut cursor, keystroke, paste) || deleting && replace) {
                    return;
                }
                self.draft.message_selected = false;
                if text == self.draft.message {
                    self.draft.message_cursor = Some(cursor);
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                // The user writes: a message the model is still writing is
                // dropped.
                if self.draft.writing_message {
                    (
                        self.draft.suggest_task,
                        self.draft.writing_message,
                        self.draft.commit_when_written,
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
                self.draft.message_cursor = Some(cursor);
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
        if text == self.draft.message {
            return;
        }
        if !(letter && self.draft.typing_run) {
            let before = std::mem::take(&mut self.draft.message);
            self.draft.message_undo.push(before);
        }
        self.draft.typing_run = letter;
        self.draft.message_redo.clear();
        self.draft.message = text;
        self.draft.message_cursor = None;
    }

    /// The commit message's cursor, on a character of the text.
    pub(super) fn cursor(&self) -> usize {
        let text = &self.draft.message;
        let mut at = self
            .draft
            .message_cursor
            .unwrap_or(text.len())
            .min(text.len());
        while !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    /// Moves the cursor a line up or down, by where the text lies on screen.
    fn cursor_line(&mut self, down: bool) {
        let Some(layout) = self.draft.message_layout.borrow().clone() else {
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
        self.draft.message_cursor = Some(from_shown(shown, cursor));
    }

    fn undo_message(&mut self) {
        if let Some(before) = self.draft.message_undo.pop() {
            let now = std::mem::replace(&mut self.draft.message, before);
            self.draft.message_redo.push(now);
            self.draft.typing_run = false;
            self.draft.message_cursor = None;
        }
    }

    fn redo_message(&mut self) {
        if let Some(after) = self.draft.message_redo.pop() {
            let now = std::mem::replace(&mut self.draft.message, after);
            self.draft.message_undo.push(now);
            self.draft.typing_run = false;
            self.draft.message_cursor = None;
        }
    }

    /// The commit message with its cursor where it stands; a click places
    /// the cursor.
    fn message_text(&self, focused: bool, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if self.draft.message.is_empty() {
            self.draft.message_layout.replace(None);
            let placeholder = match self.draft.writing_message {
                true => "",
                false => "Commit message",
            };
            return field_text("", placeholder, focused, theme);
        }
        let cursor = self.cursor();
        let mut shown = self.draft.message.clone();
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
        self.draft
            .message_layout
            .replace(Some(text.layout().clone()));
        div()
            .id("commit-message-text")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    let Some(layout) = this.draft.message_layout.borrow().clone() else {
                        return;
                    };
                    let shown = layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|i| i);
                    let cursor = this.cursor();
                    let focused = this.commit_focus.is_focused(window);
                    this.draft.message_cursor = Some(match focused {
                        true => from_shown(shown, cursor),
                        false => shown,
                    });
                    this.draft.message_selected = false;
                    this.commit_focus.focus(window);
                    cx.notify();
                }),
            )
            .child(text)
            .into_any_element()
    }

    /// The commit box holds a message the model wrote, untouched.
    fn suggesting(&self) -> bool {
        !self.draft.message.is_empty() && self.draft.suggested.as_ref() == Some(&self.draft.message)
    }

    /// Turns amending on, with the latest commit's message to edit when none
    /// is typed, or off, dropping that message if it is unchanged.
    fn toggle_amend(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.staging_worktree() else {
            return;
        };
        self.draft.amend = !self.draft.amend;
        let amend = self.draft.amend;
        let load = cx.background_executor().spawn({
            let path = path.clone();
            async move { wtf_core::head_message(&path) }
        });
        cx.spawn(async move |this, cx| {
            let head = load.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                // Amend toggled again, or another worktree picked, meanwhile.
                if this.draft.amend != amend || this.draft.worktree.as_ref() != Some(&path) {
                    return;
                }
                if amend && this.draft.message.trim().is_empty() {
                    this.set_message(head, false);
                } else if !amend && this.draft.message.trim() == head.trim() {
                    this.set_message(String::new(), false);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The commit message box above a worktree's changes.
    pub(super) fn commit_box(
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
        let generate = self.draft.message.trim().is_empty()
            && staged > 0
            && !self.draft.amend
            && preferences::summaries();
        let ready =
            (staged > 0 || self.draft.amend) && (!self.draft.message.trim().is_empty() || generate);
        let label = match (self.draft.amend, staged) {
            (true, 0) => "Amend message".to_string(),
            (true, _) => format!(
                "Amend with {staged} file{}",
                if staged == 1 { "" } else { "s" }
            ),
            (false, 0) => "Nothing staged".to_string(),
            (false, _) => format!("Commit {staged} file{}", if staged == 1 { "" } else { "s" }),
        };
        let has_changes = !self.files.is_empty();
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
                                this.draft.message_selected = false;
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
                                    .when(self.draft.writing_message, |text| text.opacity(0.3))
                                    // All of it selected, by ⌘A.
                                    .when(self.draft.message_selected && focused, |text| {
                                        text.bg(theme.selection).rounded(px(2.))
                                    })
                                    .child(self.message_text(focused, theme, cx)),
                            ),
                    )
                    // While the model writes, into an empty box or over its own
                    // message as staging changes: grey words and a pulsing mark
                    // in the middle.
                    .when(
                        self.draft.writing_message
                            && (self.draft.message.is_empty() || self.suggesting()),
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
                                    .map(|panel| floating(panel, theme))
                                    .text_xs()
                                    .invisible()
                                    .group_hover("ai-mark", |note| note.visible())
                                    .child(note),
                            )
                    })),
            )
            // The button over the column's width, its shortcut in it; under
            // it, Amend and Stash on one line.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(gpui::rems(COMMIT_COLUMN))
                    .gap(px(8.))
                    .child(
                        div()
                            .id("commit-button")
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .w_full()
                            .px(px(10.))
                            .py(px(5.))
                            .rounded(px(6.))
                            .border_1()
                            .map(|button| match ready {
                                true => button
                                    .bg(theme.accent)
                                    .border_color(theme.accent)
                                    .text_color(theme.tag_head_text)
                                    .cursor_pointer()
                                    .hover(|button| button.opacity(0.85)),
                                false => button
                                    .justify_center()
                                    .border_color(theme.separator)
                                    .text_color(theme.secondary),
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.commit(cx)))
                            .child(div().truncate().child(label))
                            .when(ready, |button| {
                                button.child(div().flex_none().opacity(0.75).child("⌘↩"))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_xs()
                            .child(
                                div()
                                    .id("amend")
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
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
                                            .when(self.draft.amend, |check| {
                                                check
                                                    .bg(theme.accent)
                                                    .border_color(theme.accent)
                                                    .text_color(theme.tag_head_text)
                                                    .child("✓")
                                            }),
                                    )
                                    .child("Amend"),
                            )
                            // Puts all the changes, untracked files too,
                            // aside in a stash, named by the message box when
                            // it says something.
                            .when(has_changes, |row| {
                                row.child(
                                    div()
                                        .id("stash-changes")
                                        .px(px(8.))
                                        .py(px(1.))
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(theme.separator)
                                        .cursor_pointer()
                                        .hover(|button| button.bg(theme.selection))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            if let Some(path) = this.staging_worktree() {
                                                let line = popups::stash_push(
                                                    &path,
                                                    &this.draft.message,
                                                    &[],
                                                );
                                                this.run_in_terminal(line, window, cx);
                                            }
                                        }))
                                        .child("Stash"),
                                )
                            }),
                    ),
            )
    }
}

/// The commit box's column of buttons, in rems so it grows with the text:
/// wide enough for "Amend with 12 files ⌘↩".
const COMMIT_COLUMN: f32 = 12.5;

/// A text field's content: the text, or a placeholder, with a caret at the
/// end while it has focus. Over a placeholder the caret is laid on top, so
/// the placeholder starts where typed text will.
pub(super) fn field_text(
    text: &str,
    placeholder: &str,
    focused: bool,
    theme: &Theme,
) -> AnyElement {
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

/// A place in text shown with the caret at `cursor`, as a place in the text.
pub(super) fn from_shown(shown: usize, cursor: usize) -> usize {
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
pub(super) fn edit_key(
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
