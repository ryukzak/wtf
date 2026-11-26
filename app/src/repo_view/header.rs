//! The commit header above the diff: messages, summaries and rewording.

use super::*;

impl RepoView {
    /// A commit's message as text to select, its subject in bold; a
    /// right-click opens the commit's menu.
    fn selectable_message(
        &self,
        commit: &Commit,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = full_message(commit);
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
        let message = full_message(commit);
        let field = crate::text_field::Field::new(crate::line_edit::LineEdit::multiline(message));
        self.rewording = Some((commit.id.clone(), path, field));
        self.reword_focus.focus(window);
        cx.notify();
    }

    fn reword(&mut self, cx: &mut Context<Self>) {
        let Some((commit, path, message)) = self.rewording.take() else {
            return;
        };
        let message = message.text().trim().to_string();
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
            _ if message.key(keystroke, cx) => {}
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// `content` below the commit header, when one is selected, which stays put.
    pub(super) fn with_header(
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

    /// Diff rows the commit header covers before the first real one.
    pub(super) fn header_rows(&self) -> usize {
        match self.range {
            Some(_) => (self.header_height / diff_view::row_height()).ceil() as usize,
            None => 0,
        }
    }

    /// The list row to scroll to for diff row `row`, so it shows just below
    /// the summary bar.
    pub(super) fn below_bar(&self, row: usize) -> usize {
        let bar = (summary_bar_height() / diff_view::row_height()).ceil() as usize;
        (row + self.header_rows()).saturating_sub(bar)
    }

    /// The key a selection of several commits has its summary by.
    fn summary_key(&self) -> Option<String> {
        // A whole bar shares the summary its header in the commit list has.
        if let Some(node) = self.whole_bar() {
            return Some(super::lists::bar_summary_key(node));
        }
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
    pub(super) fn request_summary(&mut self, cx: &mut Context<Self>) {
        let (Some(key), Some(repo)) = (self.summary_key(), self.repo_path.clone()) else {
            return;
        };
        if !preferences::summaries() || self.summaries.contains_key(&key) {
            return;
        }
        if let Some(index) = self.whole_bar().and(self.selected_nodes.first().copied()) {
            self.request_bar_summary(index, cx);
            return;
        }
        let commits = self.header_commits();
        // The header's commits are a run, newest first, as the diff covers.
        let (Some(newest), Some(oldest)) = (commits.first(), commits.last()) else {
            return;
        };
        let (base, tip) = (oldest.parent.clone(), Revision::Commit(newest.id.clone()));
        self.summaries.insert(key.clone(), None);
        let prompt = preferences::summary_template(&repo);
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

    /// The selected bar, when the diff covers it whole: one node of several
    /// commits, none picked.
    fn whole_bar(&self) -> Option<&wtf_core::GraphNode> {
        let [index] = self.selected_nodes[..] else {
            return None;
        };
        let node = self.graph.nodes.get(index)?;
        (self.narrowed.is_empty()
            && !self.snapshot_mode
            && node.kind != wtf_core::NodeKind::Uncommitted
            && node.commits.len() >= 2)
            .then_some(node)
    }

    /// The summary of the selected commits, once the model has written it.
    pub(super) fn summary(&self) -> Option<&wtf_core::Summary> {
        match self.summaries.get(&self.summary_key()?)? {
            Some(Ok(summary)) if preferences::summaries() => Some(summary),
            _ => None,
        }
    }

    /// The commits the diff covers: all listed ones, or every one from the
    /// newest picked to the oldest, as `commit_range` diffs them.
    fn header_commits(&self) -> Vec<Commit> {
        // A snapshot keeps the last header while the next commits load or a
        // step waits to narrow to its commit, so the message changes once.
        if self.snapshot_mode && (self.commits_task.is_some() || self.pending_commit.is_some()) {
            return self.snapshot.header.borrow().clone();
        }
        let commits: Vec<Commit> = match (self.narrowed.iter().min(), self.narrowed.iter().max()) {
            (Some(&newest), Some(&oldest)) => self
                .commits
                .get(newest..=oldest)
                .into_iter()
                .flatten()
                .cloned()
                .collect(),
            _ => self.commits.clone(),
        };
        if self.snapshot_mode {
            self.snapshot.header.replace(commits.clone());
        }
        commits
    }

    /// The changes or snapshot and structural or text switches, reading
    /// down the strip.
    pub(super) fn view_toggles(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let option = |label: &'static str, selected: bool| {
            strip_option(label, selected, theme).child(vertical_label(
                label,
                theme.foreground,
                window,
            ))
        };
        let group = || strip_group(theme);
        let mode = |label: &'static str, mode: DiffMode| {
            option(label, self.diff_mode == mode)
                .on_click(cx.listener(move |this, _, _, cx| this.set_diff_mode(mode, cx)))
        };
        // As many of the pairs as the strip has room for, in order; the
        // rest in a menu under "…".
        let shown = self.strip_pairs_shown(window);
        let pairs: [AnyElement; 4] = [
            // Changes or a snapshot: what the review shows, first.
            group()
                .child(option("Diff", !self.snapshot_mode).on_click(
                    cx.listener(|this, _, window, cx| this.set_snapshot_mode(false, window, cx)),
                ))
                .child(option("Tree", self.snapshot_mode).on_click(
                    cx.listener(|this, _, window, cx| this.set_snapshot_mode(true, window, cx)),
                ))
                .into_any_element(),
            // Old and new side by side, or in one column; a split diff with
            // too little room shows in one column anyway.
            group()
                .child(option("Split", !self.one_column.get()).on_click(
                    cx.listener(|this, _, window, cx| this.set_unified(false, window, cx)),
                ))
                .child(option("Unified", self.one_column.get()).on_click(
                    cx.listener(|this, _, window, cx| this.set_unified(true, window, cx)),
                ))
                .into_any_element(),
            group()
                .child(mode("Structural", DiffMode::Structural))
                .child(mode("Text", DiffMode::Text))
                .into_any_element(),
            // Long lines wrap, run on and scroll sideways, or wrap in the
            // files Settings always wrap.
            group()
                .children(
                    [
                        ("Wrap", WrapMode::Wrap),
                        ("Auto", WrapMode::Auto),
                        ("No Wrap", WrapMode::NoWrap),
                    ]
                    .map(|(label, wrap)| {
                        option(label, self.wrap == wrap).on_click(
                            cx.listener(move |this, _, window, cx| this.set_wrap(wrap, window, cx)),
                        )
                    }),
                )
                .into_any_element(),
        ];
        let more = (shown < STRIP_PAIRS.len()).then(|| {
            group().child(
                strip_option("strip-more", false, theme)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.dropdown = Some((Dropdown::Modes(shown), event.position));
                            cx.notify();
                        }),
                    )
                    .child(vertical_label("…", theme.foreground, window)),
            )
        });
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(STRIP_GAP))
            .children(pairs.into_iter().take(shown))
            .children(more)
    }

    /// How many of the strip's pairs of modes fit in it, with room for the
    /// "…" that holds the rest when not all do.
    fn strip_pairs_shown(&self, window: &Window) -> usize {
        let height = self.strip_height.get();
        if height <= 0. {
            return STRIP_PAIRS.len();
        }
        let tab = |label: &str| strip_group_height(&[label], 12., window);
        let fixed = STRIP_GAP
            + tab("Attention")
            + STRIP_GAP
            + tab("Files")
            + STRIP_GAP
            + 1.
            + STRIP_GAP
            + STRIP_GAP
            + tab("Terminal")
            + STRIP_GAP;
        let more = STRIP_GAP + strip_group_height(&["…"], 0., window);
        let mut used = fixed;
        for (k, pair) in STRIP_PAIRS.iter().enumerate() {
            let next =
                used + strip_group_height(pair, 0., window) + if k > 0 { STRIP_GAP } else { 0. };
            let rest = if k + 1 < STRIP_PAIRS.len() { more } else { 0. };
            if next + rest > height {
                return k;
            }
            used = next;
        }
        STRIP_PAIRS.len()
    }

    /// The commit header's first line, kept at the top once the rest of it
    /// scrolls away: its text selects and copies as the header's does, and
    /// a right-click on one commit's opens its menu.
    fn summary_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        // The newest commit's first line: in bold when it is the only one.
        let commits = self.header_commits();
        let (summary, bold) = match &commits[..] {
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
            // Presses are the bar's, not the diff's under it.
            .block_mouse_except_scroll()
            .px(px(12.))
            .py(px(HEADER_PADDING))
            .bg(theme.background)
            .border_b_1()
            .border_color(theme.separator)
            .font_family(preferences::ui_font())
            .text_sm()
            .line_height(rems(HEADER_LINE))
            .flex()
            .items_center()
            .child({
                let styles = match bold {
                    true => vec![(
                        0..summary.len(),
                        HighlightStyle {
                            font_weight: Some(gpui::FontWeight::SEMIBOLD),
                            ..Default::default()
                        },
                    )],
                    false => Vec::new(),
                };
                let menu = match &commits[..] {
                    [commit] => Some(Target::Commit {
                        commit: commit.id.clone(),
                        message: full_message(commit),
                    }),
                    _ => None,
                };
                let key = match commits.first() {
                    Some(commit) => format!("bar-{}", commit.id),
                    None => "bar".to_string(),
                };
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(self.selectable_text(key, summary, styles, menu, theme, cx))
            })
            .children(
                who_when(&commits, now_seconds()).map(|WhoWhen { who, when, .. }| {
                    div()
                        .flex_none()
                        .max_w(gpui::relative(0.4))
                        .truncate()
                        .pl(px(12.))
                        .text_xs()
                        .text_color(theme.secondary)
                        .on_mouse_down(MouseButton::Right, self.facts_menu(&commits, cx))
                        .child(format!("{who} · {when}"))
                }),
            )
    }

    /// What the diff shows, above it: the selected commit's message, or the
    /// subjects of several, next to the layout and diff mode toggles.
    pub(super) fn commit_header(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let commits = self.header_commits();
        let uncommitted =
            self.narrowed.is_empty() && matches!(self.range, Some((_, Revision::Worktree(_))));
        let now = now_seconds();

        // Wide, the facts stand on the right of the message or summary.
        let wide = self.header_width >= WIDE_HEADER && !commits.is_empty();
        let mut description = div().flex().flex_col().gap(px(4.)).min_w_0();
        match &commits[..] {
            [] => {}
            [commit] => {
                let editable = self.reword_worktree(&commit.id).is_some();
                let edited = (*commit).clone();
                let editing = self
                    .rewording
                    .as_ref()
                    .filter(|(id, _, _)| *id == commit.id)
                    .map(|(_, _, message)| message);
                let link = |id: &'static str, label: &'static str| {
                    div()
                        .id(id)
                        .text_color(theme.accent)
                        .cursor_pointer()
                        .child(label)
                };
                // Editing keeps the message's look: the first line in bold,
                // the rest muted.
                let message = match editing {
                    None => self.selectable_message(commit, theme, cx),
                    Some(field) => {
                        let focused = self.reword_focus.is_focused(window);
                        let text = field.text();
                        let first = text.find('\n').unwrap_or(text.len());
                        let styles = vec![
                            (
                                0..first,
                                gpui::HighlightStyle {
                                    font_weight: Some(gpui::FontWeight::SEMIBOLD),
                                    ..Default::default()
                                },
                            ),
                            (
                                first..text.len(),
                                gpui::HighlightStyle {
                                    color: Some(theme.secondary),
                                    ..Default::default()
                                },
                            ),
                        ];
                        div()
                            .id("reword-message")
                            .track_focus(&self.reword_focus)
                            .on_key_down(cx.listener(Self::reword_key))
                            .mx(px(-5.))
                            .px(px(4.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(theme.accent)
                            .child(field.render(
                                "reword-text",
                                "Commit message",
                                focused,
                                true,
                                styles,
                                &self.reword_focus,
                                theme,
                                |this: &mut Self| {
                                    this.rewording.as_mut().map(|(_, _, field)| field)
                                },
                                cx,
                            ))
                            .into_any_element()
                    }
                };
                // The links under the message; narrow, after the facts on
                // one line, which stand on the right when wide.
                let info = div()
                    .flex()
                    .gap(px(8.))
                    .text_xs()
                    .text_color(theme.secondary)
                    .when(!wide, |line| {
                        line.child(self.facts_line(&commits, now, theme, cx))
                    })
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
                                move |this, _, window, cx| this.start_reword(&edited, window, cx),
                            )))
                        }
                        None => line,
                    });
                description = description.child(message).child(info);
            }
            several => {
                // A summary from the model leads, when summaries are on, its
                // mark beside it: a click there asks for another. The
                // commits' own subjects show while it is written, when it
                // failed, or when asked for.
                let key = self.summary_key();
                let state = key
                    .as_ref()
                    .and_then(|key| self.summaries.get(key).cloned());
                let on = preferences::summaries();
                let summarizing = on && matches!(state, Some(None) | None);
                let summary = self.summary().cloned().or_else(|| {
                    summarizing
                        .then(|| {
                            key.as_ref()
                                .and_then(|key| self.replaced_summaries.get(key))
                        })
                        .flatten()
                        .cloned()
                });
                let failed = match &state {
                    Some(Some(Err(error))) if on => Some(error.clone()),
                    _ => None,
                };
                let size = (preferences::ui_size() * 0.95).round();
                let mark = div()
                    .id("summary-mark")
                    .flex_none()
                    .p(px(2.))
                    .cursor_pointer()
                    .child(match summarizing {
                        true => {
                            working_mark("summary-mark-working", size, theme).into_any_element()
                        }
                        false => ai_mark(size, None, theme).into_any_element(),
                    })
                    .tooltip(move |_, cx| {
                        let text = match summarizing {
                            true => "Apple Intelligence is summarizing",
                            false => "Summary by Apple Intelligence · click for another",
                        };
                        cx.new(|_| crate::workspace::TabPreview(text.to_string()))
                            .into()
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.regenerate_summary(cx)));
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
                    let id = format!("summary-{}", key.clone().unwrap_or_default());
                    description = description.child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(self.selectable_text(id, text, styles, None, theme, cx)),
                            )
                            .child(mark),
                    );
                } else if summarizing {
                    description = description.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_color(theme.secondary)
                            .child(shimmer(
                                "summarizing-text",
                                format!("Summarizing {} commits…", several.len()),
                                theme.secondary,
                                theme.foreground,
                            ))
                            .child(div().flex_1())
                            .child(mark),
                    );
                }
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
                let ids = several
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
                                            ids,
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
                    // Narrow, who and when under it; wide, they are facts.
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .text_xs()
                            .text_color(theme.secondary)
                            .when(!wide, |line| {
                                line.child(self.facts_line(&commits, now, theme, cx))
                            })
                            .children(failed.map(|error| {
                                div().truncate().child(format!("no summary: {error}"))
                            }))
                            .when(summary.is_some(), |line| {
                                line.child(
                                    div()
                                        .id("show-subjects")
                                        .text_color(theme.accent)
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.show_subjects = !this.show_subjects;
                                            cx.notify();
                                        }))
                                        .child(match self.show_subjects {
                                            true => "Hide commits".to_string(),
                                            false => format!("Show {} commits", several.len()),
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
        // Submodules changed only inside: nothing to commit here, but work
        // to see, in the submodule's own tab.
        for (k, path) in self.submodules_inside.iter().enumerate() {
            let open = path.clone();
            description = description.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_xs()
                    .text_color(theme.secondary)
                    .child(format!("⊙ {path} has changes inside"))
                    .child(
                        div()
                            .id(("open-inside", k))
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_submodule(&open, cx)),
                            )
                            .child("Open"),
                    ),
            );
        }

        // The diff leaves room for the header as it lays out.
        let view = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let height = f32::from(bounds.size.height);
                let width = f32::from(bounds.size.width);
                view.update(cx, |this, cx| {
                    if (this.header_height - height).abs() > 0.5
                        || (this.header_width - width).abs() > 0.5
                    {
                        this.header_height = height;
                        this.header_width = width;
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
            .when(wide, |header| {
                header.child(self.facts(&commits, now, theme, cx))
            })
    }

    /// Asks the model again for the summary shown.
    fn regenerate_summary(&mut self, cx: &mut Context<Self>) {
        if let Some(key) = self.summary_key() {
            // One being written is left to finish.
            if matches!(self.summaries.get(&key), Some(Some(_))) {
                // The old one stays in view while the new one is written,
                // so the header keeps its size.
                if let Some(Some(Ok(old))) = self.summaries.remove(&key) {
                    self.replaced_summaries.insert(key, old);
                }
                self.request_summary(cx);
                cx.notify();
            }
        }
    }

    /// The facts of the commits shown, by name: the commit or how many,
    /// what the diff changes, who and when. Hovering shows full names and
    /// exact times; a commit id copies on a click.
    fn fact_parts(
        &self,
        commits: &[Commit],
        now: i64,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<(&'static str, AnyElement)> {
        let tip = |text: String| {
            move |_: &mut Window, cx: &mut gpui::App| -> gpui::AnyView {
                cx.new(|_| crate::workspace::TabPreview(text.clone()))
                    .into()
            }
        };
        let sha = |id: &str, short: &str, key: &'static str| {
            let copied = self.copied.as_deref() == Some(id);
            let id = id.to_string();
            div()
                .id(SharedString::from(format!("facts-{key}")))
                .cursor_pointer()
                .hover(|sha| sha.text_color(theme.accent))
                .on_click(cx.listener(move |this, _, _, cx| this.copy_sha(id.clone(), cx)))
                .child(match copied {
                    true => "Copied".to_string(),
                    false => short.to_string(),
                })
        };
        let range = commit_range(&self.commits, &self.narrowed).or_else(|| self.range.clone());
        let changes = self
            .totals
            .as_ref()
            .filter(|(of, _)| Some(of) == range.as_ref())
            .map(|(_, (files, added, removed))| {
                div()
                    .flex()
                    .gap(px(6.))
                    .child(format!(
                        "{files} {}",
                        if *files == 1 { "file" } else { "files" }
                    ))
                    .child(div().text_color(ADDED).child(format!("+{added}")))
                    .child(div().text_color(REMOVED).child(format!("−{removed}")))
            });
        let Some(WhoWhen {
            who,
            signatures,
            when,
            exact,
        }) = who_when(commits, now)
        else {
            return Vec::new();
        };
        let (Some(newest), Some(oldest)) = (commits.first(), commits.last()) else {
            return Vec::new();
        };
        let first = match commits {
            [commit] => div().child(sha(&commit.id, &commit.short_id, "sha")),
            several => div()
                .flex()
                .gap(px(4.))
                .child(format!("{} commits ·", several.len()))
                .child(sha(&newest.id, &newest.short_id, "newest"))
                .child("…")
                .child(sha(&oldest.id, &oldest.short_id, "oldest")),
        };
        let (label, who_label) = match commits.len() {
            1 => ("Commit", "Author"),
            _ => ("Commits", "Authors"),
        };
        let mut parts = vec![(
            who_label,
            div()
                .id("facts-who")
                .truncate()
                .child(who)
                .tooltip(tip(signatures.join("\n")))
                .into_any_element(),
        )];
        parts.push((
            "When",
            div()
                .id("facts-when")
                .child(when)
                .tooltip(tip(exact))
                .into_any_element(),
        ));
        parts.push((label, first.into_any_element()));
        parts.extend(changes.map(|changes| ("Changes", changes.into_any_element())));
        parts
    }

    /// The facts as text to copy, each by its name, for their menu.
    fn fact_copies(&self, commits: &[Commit]) -> Vec<(String, String)> {
        let (Some(newest), Some(oldest)) = (commits.first(), commits.last()) else {
            return Vec::new();
        };
        let fact = |name: &str, value: String| (name.to_string(), value);
        let mut signatures: Vec<String> = Vec::new();
        for commit in commits {
            let signature = match commit.email.is_empty() {
                true => commit.author.clone(),
                false => format!("{} <{}>", commit.author, commit.email),
            };
            if !signatures.contains(&signature) {
                signatures.push(signature);
            }
        }
        let hashes = match commits {
            [commit] => vec![
                fact("Hash", commit.id.clone()),
                fact("Short Hash", commit.short_id.clone()),
            ],
            several => vec![fact(
                "Hashes",
                several
                    .iter()
                    .map(|commit| commit.id.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )],
        };
        let mut facts = Vec::new();
        match commits {
            [commit] => {
                facts.push(fact("Author", commit.author.clone()));
                if !commit.email.is_empty() {
                    facts.push(fact("Author and Email", signatures[0].clone()));
                }
            }
            _ => facts.push(fact("Authors", signatures.join("\n"))),
        }
        facts.push(fact(
            "Date",
            match commits.len() {
                1 => local_datetime(newest.time),
                _ => format!(
                    "{} – {}",
                    local_datetime(oldest.time),
                    local_datetime(newest.time)
                ),
            },
        ));
        facts.extend(hashes);
        facts
    }

    /// A right-click on the facts opens their menu, to copy them.
    fn facts_menu(
        &self,
        commits: &[Commit],
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static {
        let facts = self.fact_copies(commits);
        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
            let target = Target::Facts {
                facts: facts.clone(),
            };
            this.open_context_menu(target, event.position, cx);
        })
    }

    /// The facts beside a wide header, a line each under its name.
    fn facts(
        &self,
        commits: &[Commit],
        now: i64,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .flex_none()
            .w(px(FACTS_WIDTH))
            .ml(px(16.))
            .pl(px(12.))
            .border_l_1()
            .border_color(theme.separator)
            .text_xs()
            .text_color(theme.secondary)
            .on_mouse_down(MouseButton::Right, self.facts_menu(commits, cx))
            .children(
                self.fact_parts(commits, now, theme, cx)
                    .into_iter()
                    .map(|(name, value)| {
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(FACT_NAME_WIDTH))
                                    .text_color(theme.secondary.opacity(0.7))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(theme.foreground)
                                    .child(value),
                            )
                    }),
            )
    }

    /// The facts on a line, for a narrow header, or more when they do not
    /// fit.
    fn facts_line(
        &self,
        commits: &[Commit],
        now: i64,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let parts = self.fact_parts(commits, now, theme, cx);
        let count = parts.len();
        // What does not fit goes on to the next line, a fact whole, its
        // dot kept with it.
        div()
            .flex()
            .flex_wrap()
            .gap_x(px(6.))
            .min_w_0()
            .on_mouse_down(MouseButton::Right, self.facts_menu(commits, cx))
            .children(parts.into_iter().enumerate().map(move |(k, (_, value))| {
                div()
                    .flex()
                    .gap(px(6.))
                    .whitespace_nowrap()
                    .child(value)
                    .when(k + 1 < count, |part| part.child("·"))
            }))
    }

    /// The diff `list` with the commit header scrolling above its first rows,
    /// and the summary bar and a `pinned` file header kept on top, pushed
    /// up by as much as the next one needs.
    pub(super) fn under_header(
        &self,
        list: AnyElement,
        scroll_y: f32,
        pinned: Option<(AnyElement, f32)>,
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
            .children(pinned.map(|(pinned, push)| {
                let bar = if scrolled { summary_bar_height() } else { 0. };
                div()
                    .absolute()
                    .top(px(bar + push))
                    .left_0()
                    .right_0()
                    .child(pinned)
            }))
            .when(scrolled, |pane| pane.child(self.summary_bar(theme, cx)))
    }
}

/// Who wrote commits and when, as the facts and the pinned bar say it.
struct WhoWhen {
    /// Up to three names, then how many more.
    who: String,
    /// Each author once, with their address as git gives it.
    signatures: Vec<String>,
    /// How long ago, or from when to when.
    when: String,
    /// The exact times, for a tooltip.
    exact: String,
}

fn who_when(commits: &[Commit], now: i64) -> Option<WhoWhen> {
    let (newest, oldest) = (commits.first()?, commits.last()?);
    let mut authors: Vec<&str> = Vec::new();
    let mut signatures: Vec<String> = Vec::new();
    for commit in commits {
        if !authors.contains(&commit.author.as_str()) {
            authors.push(&commit.author);
            signatures.push(match commit.email.is_empty() {
                true => commit.author.clone(),
                false => format!("{} <{}>", commit.author, commit.email),
            });
        }
    }
    let names = authors[..authors.len().min(3)].join(", ");
    let who = match authors.len().saturating_sub(3) {
        0 => names,
        n => format!("{names}, +{n}"),
    };
    let (from, to) = (
        relative_time(oldest.time, now),
        relative_time(newest.time, now),
    );
    let when = match from == to {
        true => from,
        false => format!("{from} – {to}"),
    };
    let exact = match commits.len() {
        1 => local_datetime(newest.time),
        _ => format!(
            "{} – {}",
            local_datetime(oldest.time),
            local_datetime(newest.time)
        ),
    };
    Some(WhoWhen {
        who,
        signatures,
        when,
        exact,
    })
}

/// The strip's pairs of modes, top down, by their buttons' names.
pub(super) const STRIP_PAIRS: [&[&str]; 4] = [
    &["Diff", "Tree"],
    &["Split", "Unified"],
    &["Structural", "Text"],
    &["Wrap", "Auto", "No Wrap"],
];

/// How wide the header is before the facts stand on its right, and how
/// wide they are.
const WIDE_HEADER: f32 = 900.;
const FACTS_WIDTH: f32 = 290.;
/// How wide the facts' names are.
const FACT_NAME_WIDTH: f32 = 52.;

/// Lines added and removed, as the facts count them.
const ADDED: gpui::Hsla = gpui::Hsla {
    h: 0.38,
    s: 0.55,
    l: 0.4,
    a: 1.,
};
const REMOVED: gpui::Hsla = gpui::Hsla {
    h: 0.01,
    s: 0.6,
    l: 0.5,
    a: 1.,
};

/// The summary bar's height: the header's first line with its padding.
pub(super) fn summary_bar_height() -> f32 {
    2. * HEADER_PADDING + HEADER_LINE * preferences::ui_size() + 1.
}

/// The word around byte `at` of `text`, for a double click.
pub(super) fn word_around(text: &str, at: usize) -> (usize, usize) {
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

/// A commit's message: its summary, then its body after a blank line.
pub(super) fn full_message(commit: &Commit) -> String {
    match commit.body.is_empty() {
        true => commit.summary.clone(),
        false => format!("{}\n\n{}", commit.summary, commit.body),
    }
}
