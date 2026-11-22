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
            _ if type_key(message, keystroke, false, cx) => {}
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
        let commits = self.header_commits();
        // The header's commits are a run, newest first, as the diff covers.
        let (Some(newest), Some(oldest)) = (commits.first(), commits.last()) else {
            return;
        };
        let (base, tip) = (oldest.parent.clone(), Revision::Commit(newest.id.clone()));
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

    /// The one-file or all-files and structural or text switches, reading
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
            // Changes or a snapshot: what the review shows, first.
            .child(
                group()
                    .child(option("Changes", !self.snapshot_mode).on_click(
                        cx.listener(|this, _, window, cx| {
                            this.set_snapshot_mode(false, window, cx)
                        }),
                    ))
                    .child(option("Snapshot", self.snapshot_mode).on_click(
                        cx.listener(|this, _, window, cx| this.set_snapshot_mode(true, window, cx)),
                    )),
            )
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
                // Who and when first, then the message.
                description = description.child(
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
                                    " · {} · {} ({})",
                                    commit.author,
                                    local_datetime(commit.time),
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
                description = description.child(message);
            }
            several => {
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
                            .child(aggregate(&several.iter().collect::<Vec<_>>(), now))
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
    pub(super) fn under_header(
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
}

/// "5 commits · Ann, Bot · 3 days ago – 2 hours ago": the authors, most
/// commits first, and the time span of several commits.
pub(super) fn aggregate(commits: &[&Commit], now: i64) -> String {
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
