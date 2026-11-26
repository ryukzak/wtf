//! The commit list and file list beside the diff.

use super::*;

impl RepoView {
    /// One line of history, newest first, straight from the graph: a
    /// worktree's changes, empty or not, then every commit down the line.
    /// A bar on the left marks what the diff covers; a click inside it
    /// picks, a click outside selects the commit, and the line stays as it
    /// is to go forward again.
    pub(super) fn commit_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.line_rows();
        let changed = *self.commit_rows.borrow() != rows;
        if changed {
            // Only the rows that changed are measured again, and the view
            // stays where it was: a splice over the top row would scroll
            // the list back to the splice's start.
            let old = self.commit_rows.borrow();
            let same_start = old.iter().zip(&rows).take_while(|(a, b)| a == b).count();
            let same_end = old[same_start..]
                .iter()
                .rev()
                .zip(rows[same_start..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let old_len = old.len();
            drop(old);
            let top = self.commit_list_state.logical_scroll_top();
            self.commit_list_state.splice(
                same_start..old_len - same_end,
                rows.len() - same_start - same_end,
            );
            if (same_start..old_len - same_end).contains(&top.item_ix) {
                self.commit_list_state.scroll_to(gpui::ListOffset {
                    item_ix: top.item_ix.min(rows.len().saturating_sub(1)),
                    offset_in_item: top.offset_in_item,
                });
            }
            *self.commit_rows.borrow_mut() = rows;
        }
        // A new selection starts at the top of the panel, with the heading
        // over it.
        if self.commit_list_selection.borrow().as_slice() != self.selected_nodes.as_slice() {
            *self.commit_list_selection.borrow_mut() = self.selected_nodes.clone();
            let first = self.commit_rows.borrow().iter().position(|row| match row {
                LineRow::Uncommitted { covered, .. }
                | LineRow::Bar { covered, .. }
                | LineRow::Commit { covered, .. } => *covered,
                LineRow::Detached => false,
                LineRow::Loose(_) => true,
                LineRow::Heading(..) => false,
            });
            if let Some(first) = first {
                let rows = self.commit_rows.borrow();
                let heading = matches!(rows.get(first.wrapping_sub(1)), Some(LineRow::Heading(..)));
                self.commit_list_state.scroll_to(gpui::ListOffset {
                    item_ix: first - usize::from(heading),
                    offset_in_item: px(0.),
                });
            }
        }
        if let Some(index) = self.scroll_to_commit.take() {
            let id = self.commits.get(index).map(|commit| commit.id.as_str());
            let row = self.commit_rows.borrow().iter().position(|row| match row {
                LineRow::Commit { node, at, .. } => {
                    Some(self.graph.nodes[*node].commits[*at].id.as_str()) == id
                }
                LineRow::Loose(loose) => *loose == index,
                _ => false,
            });
            if let Some(row) = row {
                self.commit_list_state.scroll_to_reveal_item(row);
            }
        }
        // Bars from the top of the view down, and a few past its bottom,
        // have their summaries made in that order.
        let top = self.commit_list_state.logical_scroll_top().item_ix;
        *self.summary_wanted.borrow_mut() = self
            .commit_rows
            .borrow()
            .iter()
            .enumerate()
            .skip(top)
            .filter_map(|(at, row)| match row {
                LineRow::Bar { node, .. } => Some((at, *node)),
                _ => None,
            })
            .scan(0, |past, (at, node)| {
                *past += usize::from(at > top + VIEW_ROWS);
                (*past <= SUMMARY_AHEAD).then_some(node)
            })
            .collect();
        let list = gpui::list(
            self.commit_list_state.clone(),
            cx.processor(|this, row: usize, window, cx| {
                let theme = Theme::for_appearance(window.appearance());
                this.next_bar_summary(cx);
                this.line_row(row, &theme, cx)
            }),
        );
        // The wheel, taken before the list sees it: GPUI's list adds up the
        // events of a frame and moves by the sum at each, so a wheel's quick
        // run of them, a trackball's, overshoots. Here each moves once.
        let state = self.commit_list_state.clone();
        let view = cx.entity().downgrade();
        let wheel = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                window.on_mouse_event(move |event: &gpui::ScrollWheelEvent, phase, _, cx| {
                    if phase != gpui::DispatchPhase::Capture || !bounds.contains(&event.position) {
                        return;
                    }
                    let delta = event.delta.pixel_delta(px(20.)).y;
                    let max = state.max_offset_for_scrollbar().height;
                    let now = -state.scroll_px_offset_for_scrollbar().y;
                    let next = (now - delta).clamp(px(0.), max);
                    state.scroll_by(next - now);
                    cx.stop_propagation();
                    view.update(cx, |_, cx| cx.notify()).ok();
                });
            },
        )
        .absolute()
        .size_full();
        // A press anywhere in it gives the list the keys: ↑ and ↓ go through
        // its commits.
        div()
            .id("commit-history")
            .relative()
            .size_full()
            .track_focus(&self.commit_list_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| this.commit_list_focus.focus(window)),
            )
            .child(list.size_full())
            .child(wheel)
    }

    /// ↑ and ↓ in the list: the commit above or below the one picked, or
    /// from the selection's first row, picked as a click would; headers
    /// are passed over.
    pub(super) fn step_list(&mut self, down: bool, cx: &mut Context<Self>) {
        let rows = self.commit_rows.borrow().clone();
        let picked = self
            .pending_commit
            .clone()
            .or_else(|| match self.narrowed[..] {
                [i] => self.commits.get(i).map(|commit| commit.id.clone()),
                _ => None,
            });
        let id_of = |row: &LineRow| match row {
            LineRow::Commit { node, at, .. } => {
                Some(self.graph.nodes[*node].commits[*at].id.clone())
            }
            LineRow::Loose(index) => self.commits.get(*index).map(|c| c.id.clone()),
            _ => None,
        };
        let steppable = |row: &LineRow| {
            matches!(
                row,
                LineRow::Commit { .. } | LineRow::Loose(_) | LineRow::Uncommitted { .. }
            )
        };
        let current = match &picked {
            Some(id) => rows.iter().position(|row| id_of(row).as_ref() == Some(id)),
            // A selected worktree's changes are where it starts.
            None => rows.iter().position(|row| match row {
                LineRow::Uncommitted { covered, .. } => *covered,
                _ => false,
            }),
        };
        let next = match current {
            Some(at) if down => (at + 1..rows.len()).find(|&k| steppable(&rows[k])),
            Some(at) => (0..at).rev().find(|&k| steppable(&rows[k])),
            // Nothing picked: the selection's first commit.
            None => rows.iter().position(|row| match row {
                LineRow::Commit { covered, .. } => *covered,
                LineRow::Loose(_) => true,
                _ => false,
            }),
        };
        let Some(next) = next else {
            return;
        };
        match rows[next].clone() {
            LineRow::Uncommitted { node, .. } => self.select_and_reveal(node, cx),
            LineRow::Loose(index) => self.click_commit(index, Modifiers::default(), cx),
            LineRow::Commit {
                node, at, covered, ..
            } => {
                let id = self.graph.nodes[node].commits[at].id.clone();
                match covered
                    .then(|| self.commits.iter().position(|c| c.id == id))
                    .flatten()
                {
                    Some(index) => self.click_commit(index, Modifiers::default(), cx),
                    None => self.select_commit(node, id, cx),
                }
            }
            _ => return,
        }
        self.keep_list_scroll();
        self.commit_list_state.scroll_to_reveal_item(next);
        cx.notify();
    }

    /// The rows of the list's line.
    fn line_rows(&self) -> Vec<LineRow> {
        let nodes = &self.graph.nodes;
        let chain = self.thread_chain();
        let sides = self.sides(&chain);
        let position = |node: Option<usize>| node.and_then(|n| chain.iter().position(|&c| c == n));
        let span = position(self.selected_nodes.iter().max().copied())
            .zip(position(self.selected_nodes.iter().min().copied()));
        // A selection on a branch merged into the line shows under its merge.
        let on_side = !self.selected_nodes.is_empty()
            && self
                .selected_nodes
                .iter()
                .all(|node| sides.values().any(|side| side.contains(node)));
        let mut rows = Vec::new();
        let Some((from, to)) = span.or(on_side.then_some((usize::MAX, usize::MAX))) else {
            // Off any line, such as a branch the graph cannot draw.
            if !self.commits.is_empty() {
                rows.push(LineRow::Heading("Commits".to_string(), String::new()));
                rows.extend((0..self.commits.len()).map(LineRow::Loose));
            }
            return rows;
        };
        let shared = |index: usize| {
            self.graph.on_main.get(index).copied().unwrap_or(false) || nodes[index].channel
        };
        let now = now_seconds();
        // A line no worktree is on says so where the changes would be.
        if chain
            .first()
            .is_some_and(|&top| nodes[top].kind != wtf_core::NodeKind::Uncommitted)
        {
            rows.push(LineRow::Heading("Uncommitted".to_string(), String::new()));
            rows.push(LineRow::Detached);
        }
        let (mut own_seen, mut shared_seen) = (false, false);
        for (at, &index) in chain.iter().enumerate() {
            let node = &nodes[index];
            let covered = (from..=to).contains(&at);
            if node.kind == wtf_core::NodeKind::Uncommitted {
                rows.push(LineRow::Heading("Uncommitted".to_string(), String::new()));
                rows.push(LineRow::Uncommitted {
                    node: index,
                    covered,
                });
                continue;
            }
            let is_shared = shared(index);
            if is_shared && !shared_seen {
                shared_seen = true;
                let name = match self.graph.on_main.get(index) {
                    Some(true) => self.graph.main.clone(),
                    _ => node
                        .refs
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "Earlier".into()),
                };
                let note = match (own_seen, node.commits.last()) {
                    (true, Some(fork)) => format!("forked {} ago", short_age(fork.time, now)),
                    _ => String::new(),
                };
                rows.push(LineRow::Heading(name, note));
            } else if !is_shared && !own_seen {
                own_seen = true;
                let name = chain
                    .first()
                    .and_then(|&top| nodes[top].branch.clone())
                    .or_else(|| node.refs.first().cloned())
                    .unwrap_or_else(|| "Commits".into());
                rows.push(LineRow::Heading(name, String::new()));
            }
            // A bar of several commits has a header: a click selects them
            // all, and folded, its summary stands for them.
            if node.commits.len() >= 2 {
                let folded = self.folds.get(&node.key).copied().unwrap_or(false);
                rows.push(LineRow::Bar {
                    node: index,
                    covered,
                    folded,
                });
                if folded {
                    continue;
                }
            }
            // A merge folds what it brought in under it, open at first.
            let side = sides.get(&index);
            let side_folded = self.folds.get(&side_key(node)).copied().unwrap_or(false);
            rows.extend((0..node.commits.len()).rev().map(|at| LineRow::Commit {
                node: index,
                at,
                covered,
                shared: is_shared,
                side: false,
                merged: (at == 0 && side.is_some()).then_some(side_folded),
            }));
            if let Some(side) = side.filter(|_| !side_folded) {
                // A selected merge's diff holds what it merged.
                for &branch in side {
                    let covered = covered || self.selected_nodes.contains(&branch);
                    rows.extend(
                        (0..nodes[branch].commits.len())
                            .rev()
                            .map(|at| LineRow::Commit {
                                node: branch,
                                at,
                                covered,
                                shared: is_shared,
                                side: true,
                                merged: None,
                            }),
                    );
                }
            }
        }
        rows
    }

    fn line_row(&self, row: usize, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(line_row) = self.commit_rows.borrow().get(row).cloned() else {
            return div().into_any_element();
        };
        let now = now_seconds();
        match line_row {
            LineRow::Heading(text, note) => heading_with(text, note, theme).into_any_element(),
            LineRow::Uncommitted {
                node: index,
                covered,
            } => {
                let node = &self.graph.nodes[index];
                let label = graph_view::worktree_label(node, self.worktree_names);
                let detail = match (node.added, node.removed) {
                    (0, 0) => format!("{label} · no changes"),
                    (added, removed) => format!("{label} · +{added} −{removed}"),
                };
                let item = list_item(("uncommitted", index), covered, theme);
                let item = match covered {
                    true => bar(item, theme)
                        .on_click(cx.listener(|this, _, _, cx| this.show_all_commits(cx))),
                    false => hoverable(item, theme).on_click(cx.listener(move |this, _, _, cx| {
                        this.select_and_reveal(index, cx);
                        this.keep_list_scroll();
                    })),
                };
                item.child(div().truncate().child("Uncommitted changes"))
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.secondary)
                            .child(detail),
                    )
                    .into_any_element()
            }
            LineRow::Bar {
                node: index,
                covered,
                folded,
            } => self.bar_header(index, covered, folded, theme, cx),
            LineRow::Detached => {
                let current = self.current_worktree_node();
                let label = current
                    .map(|node| {
                        graph_view::worktree_label(&self.graph.nodes[node], self.worktree_names)
                    })
                    .map(|label| format!("{label} is elsewhere · click to go back"))
                    .unwrap_or_default();
                hoverable(list_item("detached", false, theme), theme)
                    .text_color(theme.secondary)
                    .when_some(current, |item, node| {
                        item.on_click(
                            cx.listener(move |this, _, _, cx| this.select_and_reveal(node, cx)),
                        )
                    })
                    .child(div().truncate().child("Detached from worktree"))
                    .child(div().truncate().text_xs().child(label))
                    .into_any_element()
            }
            LineRow::Commit {
                node,
                at,
                covered,
                shared,
                side,
                merged,
            } => {
                let commit = &self.graph.nodes[node].commits[at];
                // Where the diff's commits, once loaded, hold it.
                let loaded = covered
                    .then(|| self.commits.iter().position(|c| c.id == commit.id))
                    .flatten();
                let picked = covered
                    && (self.narrowed.is_empty()
                        || loaded.is_some_and(|i| self.narrowed.contains(&i)));
                let item = list_item(("line", row), picked, theme);
                let item = match (covered, shared) {
                    (true, _) => bar(item, theme),
                    (false, true) => dim_list_item(("line", row), theme),
                    (false, false) => hoverable(item, theme),
                };
                let id = commit.id.clone();
                let item = match (covered, loaded) {
                    (true, Some(index)) => {
                        item.on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            // Focus moves to the list, for its ⌘A.
                            this.commit_list_focus.focus(window);
                            this.click_commit(index, event.modifiers(), cx)
                        }))
                    }
                    _ => item.on_click(cx.listener(move |this, _, _, cx| {
                        this.select_commit(node, id.clone(), cx);
                        this.keep_list_scroll();
                    })),
                };
                // Under a bar's header, a guide down to its last commit.
                let item = match (side, self.graph.nodes[node].commits.len() >= 2) {
                    (true, _) => {
                        item.pl(px(LIST_INDENT + SIDE_INDENT - if covered { 4. } else { 0. }))
                    }
                    (false, false) if merged.is_some() => indented(item, covered),
                    (false, true) => guide(
                        indented(item, covered),
                        covered,
                        false,
                        // A merge's own chevron ends the line above it.
                        match (at == 0, merged.is_some()) {
                            (true, true) => Some(LIST_ITEM_HEIGHT - 6.),
                            (true, false) => Some(8.),
                            _ => None,
                        },
                        theme,
                    ),
                    (false, false) => item,
                };
                let fold = merged.map(|folded| {
                    let merge = &self.graph.nodes[node];
                    let chain = self.thread_chain();
                    let count: usize = self
                        .sides(&chain)
                        .get(&node)
                        .map(|side| {
                            side.iter()
                                .map(|&n| self.graph.nodes[n].commits.len())
                                .sum()
                        })
                        .unwrap_or_default();
                    (side_key(merge), folded, count)
                });
                self.commit_row(item, commit, now, fold, theme, cx)
            }
            LineRow::Loose(index) => {
                let commit = &self.commits[index];
                let picked = self.narrowed.is_empty() || self.narrowed.contains(&index);
                let item = bar(list_item(("loose", index), picked, theme), theme).on_click(
                    cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.commit_list_focus.focus(window);
                        this.click_commit(index, event.modifiers(), cx)
                    }),
                );
                self.commit_row(item, commit, now, None, theme, cx)
            }
        }
    }

    /// A bar's header: a chevron that folds it, then its summary, or what
    /// stands for one, over its count and age.
    fn bar_header(
        &self,
        index: usize,
        covered: bool,
        folded: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let node = &self.graph.nodes[index];
        let key = node.key.clone();
        let count = node.commits.len();
        let age = short_age(node.last_time, now_seconds());
        let (title, waiting) = match self.summaries.get(&bar_summary_key(node)) {
            Some(Some(Ok(summary))) if preferences::summaries() => (summary.header.clone(), false),
            // Asked for again: the old one until the new one comes.
            Some(None) => (
                self.replaced_summaries
                    .get(&bar_summary_key(node))
                    .filter(|_| preferences::summaries())
                    .map(|old| old.header.clone())
                    .unwrap_or_default(),
                true,
            ),
            _ => (String::new(), false),
        };
        let title = match title.is_empty() {
            false => title,
            true => node
                .refs
                .first()
                .cloned()
                .unwrap_or_else(|| format!("{count} commits")),
        };
        let detail = format!("{count} commits · {age}");
        let summarized = !waiting
            && self
                .summaries
                .get(&bar_summary_key(node))
                .is_some_and(|summary| matches!(summary, Some(Ok(_))) && preferences::summaries());
        let item = list_item(("bar", index), covered && self.narrowed.is_empty(), theme);
        let item = indented(
            match covered {
                true => bar(item, theme),
                false => hoverable(item, theme),
            },
            covered,
        );
        // The chevron hangs in the row's padding, so the title lines up
        // with the commits' summaries.
        let size = (preferences::ui_size() * 0.7).round();
        let mark = match (waiting, summarized) {
            (true, _) => Some(working_mark(("summarizing", index), size, theme).into_any_element()),
            (false, true) => Some(ai_mark(size, None, theme).into_any_element()),
            _ => None,
        };
        let item = match folded {
            true => item,
            false => guide(item, covered, true, None, theme),
        };
        // Its menu is its newest commit's: check it out, branch from it.
        let menu = node.commits.last().map(|newest| Target::Commit {
            commit: newest.id.clone(),
            message: full_message(newest),
        });
        let item = match menu {
            Some(target) => item.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_context_menu(target.clone(), event.position, cx);
                    cx.stop_propagation();
                }),
            ),
            None => item,
        };
        item.on_click(cx.listener(move |this, _, _, cx| {
            match this.selected_nodes == [index] {
                true => this.show_all_commits(cx),
                false => this.select_and_reveal(index, cx),
            }
            this.keep_list_scroll();
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                // The chevron hangs in the padding, on the title's line.
                .child(
                    div()
                        .id(("fold", index))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(12.))
                        .ml(px(-15.))
                        .mr(px(-3.))
                        .cursor_pointer()
                        .child(chevron(!folded, theme.secondary, 10.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.folds.insert(key.clone(), !folded);
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .children(mark),
        )
        .child(
            div()
                .truncate()
                .text_xs()
                .text_color(theme.secondary)
                .child(detail),
        )
        .into_any_element()
    }

    /// Leaves the commit list where it is for a selection made in it.
    fn keep_list_scroll(&self) {
        *self.commit_list_selection.borrow_mut() = self.selected_nodes.clone();
    }

    /// Asks the model to sum up a bar's commits, as the commit panel does a
    /// selection's, when summaries are on and none is made or on its way.
    /// Starts the next summary wanted, top down, when none is being made.
    fn next_bar_summary(&mut self, cx: &mut Context<Self>) {
        if self.bar_summary_running || !preferences::summaries() {
            return;
        }
        let next = self.summary_wanted.borrow().iter().copied().find(|&node| {
            self.graph
                .nodes
                .get(node)
                .is_some_and(|node| !self.summaries.contains_key(&bar_summary_key(node)))
        });
        if let Some(node) = next {
            self.request_bar_summary(node, cx);
        }
    }

    pub(super) fn request_bar_summary(&mut self, index: usize, cx: &mut Context<Self>) {
        let (Some(node), Some(repo)) = (self.graph.nodes.get(index), self.repo_path.clone()) else {
            return;
        };
        let key = bar_summary_key(node);
        if !preferences::summaries() || self.summaries.contains_key(&key) {
            return;
        }
        let commits: Vec<Commit> = node.commits.iter().rev().cloned().collect();
        let (base, tip) = (node.base.clone(), node.tip.clone());
        self.summaries.insert(key.clone(), None);
        self.bar_summary_running = true;
        let prompt = preferences::summary_template(&repo);
        let run = cx.background_executor().spawn(async move {
            wtf_core::summarize_commits(&repo, &commits, base.as_deref(), &tip, &prompt)
                .map_err(message)
        });
        cx.spawn(async move |this, cx| {
            let summary = run.await;
            this.update(cx, |this, cx| {
                this.summaries.insert(key, Some(summary));
                this.bar_summary_running = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A commit's summary over its detail, with its menu.
    fn commit_row(
        &self,
        item: Stateful<gpui::Div>,
        commit: &Commit,
        now: i64,
        fold: Option<(String, bool, usize)>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let merge = fold.is_some();
        let detail = match &fold {
            Some((_, _, count)) => format!("{} · +{count} merged", commit_detail(commit, now)),
            None => commit_detail(commit, now),
        };
        // A merge's chevron hangs in the padding, on the summary's line.
        let chevron_mark = fold.map(|(key, folded, _)| {
            div()
                .id(SharedString::from(key.clone()))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(12.))
                .ml(px(-15.))
                .mr(px(3.))
                .cursor_pointer()
                .child(chevron(!folded, theme.secondary, 10.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.folds.insert(key.clone(), !folded);
                    cx.stop_propagation();
                    cx.notify();
                }))
        });
        let target = Target::Commit {
            commit: commit.id.clone(),
            message: full_message(commit),
        };
        item
            // The menu leaves the selection as it is.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_context_menu(target.clone(), event.position, cx);
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .children(chevron_mark)
                    .when(merge, |line| {
                        line.child(
                            div()
                                .flex_none()
                                .mr(px(4.))
                                .text_color(theme.secondary)
                                .child("⑂"),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(commit.summary.clone()),
                    ),
            )
            .child(
                div()
                    .truncate()
                    .text_xs()
                    .text_color(theme.secondary)
                    .child(detail),
            )
            .into_any_element()
    }

    /// The line the list shows, newest node first, down first parents.
    fn thread_chain(&self) -> Vec<usize> {
        let top = self
            .thread
            .as_deref()
            .and_then(|key| self.graph.nodes.iter().position(|node| node.key == key))
            .or_else(|| self.selected_nodes.iter().max().copied());
        self.chain_from(top)
    }

    fn chain_from(&self, top: Option<usize>) -> Vec<usize> {
        let mut chain = Vec::new();
        let mut at = top.filter(|&top| top < self.graph.nodes.len());
        while let Some(index) = at {
            chain.push(index);
            at = self.graph.nodes[index].parents.first().copied();
        }
        chain
    }

    /// What each merge on the line brought in: the nodes of the branch it
    /// merged, newest first, back to where that branch left the line.
    fn sides(&self, chain: &[usize]) -> HashMap<usize, Vec<usize>> {
        let nodes = &self.graph.nodes;
        let on_line: HashSet<usize> = chain.iter().copied().collect();
        let mut sides = HashMap::new();
        for &merge in chain {
            let mut side = Vec::new();
            for &parent in nodes[merge].parents.iter().skip(1) {
                let mut at = Some(parent);
                while let Some(index) = at.filter(|index| {
                    !on_line.contains(index) && !side.contains(index) && side.len() < MAX_SIDE
                }) {
                    side.push(index);
                    at = nodes[index].parents.first().copied();
                }
            }
            if !side.is_empty() {
                sides.insert(merge, side);
            }
        }
        sides
    }

    /// The current worktree's node: the one last selected, or the tab's own.
    fn current_worktree_node(&self) -> Option<usize> {
        let current = self
            .current_worktree_path()
            .map(str::to_string)
            .or_else(|| self.repo_path.clone())?;
        let current = current.trim_end_matches('/');
        self.graph.nodes.iter().position(|node| {
            matches!(&node.tip, Revision::Worktree(path) if path.trim_end_matches('/') == current)
        })
    }

    /// Whether the line starting at `top` holds `node`, on it or merged in.
    fn line_holds(&self, top: usize, node: usize) -> bool {
        let chain = self.chain_from(Some(top));
        chain.contains(&node) || self.sides(&chain).values().any(|side| side.contains(&node))
    }

    /// Keeps the list's line while the selection stays on it; else takes the
    /// line of a worktree holding the selection, the current one first, or
    /// starts at the selection.
    pub(super) fn follow_thread(&mut self) {
        let Some(&top) = self.selected_nodes.iter().max() else {
            return;
        };
        // The current worktree's line first, when it holds the selection;
        // then the line shown, to go back and forth along it; else the
        // selection's own, detached from any worktree.
        if let Some(worktree) = self
            .current_worktree_node()
            .filter(|&worktree| self.line_holds(worktree, top))
        {
            self.thread = Some(self.graph.nodes[worktree].key.clone());
            return;
        }
        let line = self
            .thread
            .as_deref()
            .and_then(|key| self.graph.nodes.iter().position(|node| node.key == key));
        if line.is_some_and(|line| self.line_holds(line, top)) {
            return;
        }
        self.thread = Some(self.graph.nodes[top].key.clone());
    }

    /// The selected node when it is the only one.
    pub(super) fn single_node(&self) -> Option<(usize, &wtf_core::GraphNode)> {
        let [selected] = self.selected_nodes[..] else {
            return None;
        };
        Some((selected, self.graph.nodes.get(selected)?))
    }

    /// Files in the order the file tree lists them, folded folders included.
    pub(super) fn file_order(&self) -> Vec<usize> {
        file_tree(&self.files, self.worktree_split, &HashSet::new())
            .into_iter()
            .filter_map(|row| match row {
                TreeRow::File { index, .. } => Some(index),
                _ => None,
            })
            .collect()
    }

    /// The changed files as a tree of folders, under the unstaged and staged
    /// sections for a worktree.
    pub(super) fn file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                                let path = match this.worktree_split {
                                    Some(_) => key
                                        .strip_prefix("staged:")
                                        .or_else(|| key.strip_prefix("unstaged:"))
                                        .unwrap_or(key),
                                    None => key,
                                };
                                let menu = super::this_path_menu(path.to_string(), cx);
                                let key = key.clone();
                                item.pl(indent(*depth))
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Right, menu)
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
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        super::this_file_menu(index, cx),
                                    )
                                    .child(
                                        div()
                                            .w(px(10.))
                                            .font_family(preferences::code_font())
                                            .font_weight(preferences::code_weight())
                                            .text_xs()
                                            .text_color(theme.secondary)
                                            .child(match file.submodule {
                                                true => "⊞".to_string(),
                                                false => file.status.clone(),
                                            }),
                                    )
                                    .child(div().truncate().child(name.clone()))
                                    .children(old_name(file).map(|old| {
                                        div()
                                            .truncate()
                                            .text_color(theme.secondary)
                                            .child(format!("← {old}"))
                                    }))
                            }
                        }
                    })
                    .collect()
            }),
        )
        .size_full()
        .py(px(4.))
    }
}

/// How many bars past the view have their summaries made ahead of being
/// seen, and about how many rows the view shows.
const SUMMARY_AHEAD: usize = 3;
const VIEW_ROWS: usize = 30;

/// A row of the commit list.
#[derive(Clone, PartialEq)]
pub(super) enum LineRow {
    /// A section title and a note on its right.
    Heading(String, String),
    /// A worktree's changes; `covered` when the diff shows them.
    Uncommitted { node: usize, covered: bool },
    /// A graph node's commit, by its place in the node.
    Commit {
        node: usize,
        at: usize,
        covered: bool,
        shared: bool,
        /// On a branch merged into the line, under its merge.
        side: bool,
        /// On a merge that brought commits in: whether they are folded.
        merged: Option<bool>,
    },
    /// In place of a worktree's changes on a line no worktree is on.
    Detached,
    /// The header of a bar of several commits.
    Bar {
        node: usize,
        covered: bool,
        folded: bool,
    },
    /// One of the diff's commits, for a selection off the graph's lines.
    Loose(usize),
}

/// A short id, the author's first name and an age.
fn commit_detail(commit: &Commit, now: i64) -> String {
    let author = commit.author.split_whitespace().next().unwrap_or_default();
    format!(
        "{} · {author} · {}",
        commit.short_id,
        short_age(commit.time, now)
    )
}

/// A row the diff covers: a bar along its left edge.
fn bar(item: Stateful<gpui::Div>, theme: &Theme) -> Stateful<gpui::Div> {
    item.border_l(px(4.))
        .border_color(theme.accent)
        .pl(px(LIST_PADDING - 4.))
}

/// A bar's header or one of its commits, set in under its chevron.
fn indented(item: Stateful<gpui::Div>, covered: bool) -> Stateful<gpui::Div> {
    item.pl(px(LIST_INDENT - if covered { 4. } else { 0. }))
}

/// A section title with a note on the right.
fn heading_with(text: String, note: String, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .pl(px(LIST_PADDING))
        .pr(px(10.))
        .pt(px(8.))
        .pb(px(2.))
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.secondary)
        .flex()
        .justify_between()
        .gap(px(8.))
        .child(div().truncate().child(text))
        .child(
            div()
                .flex_none()
                .font_weight(gpui::FontWeight::NORMAL)
                .child(note),
        )
}

/// A line down the gutter under a bar's chevron, through its commits: from
/// under the chevron on the header, and ending short on the last commit.
/// Not on the selected bar, which its blue edge marks.
fn guide(
    item: Stateful<gpui::Div>,
    covered: bool,
    header: bool,
    end: Option<f32>,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    // A selected bar has its blue edge to show where it ends.
    if covered {
        return item;
    }
    let left = LIST_INDENT - 9.;
    let top = if header { 26. } else { 0. };
    // On the last row, how far short of its bottom the line stops.
    let bottom = end.unwrap_or(0.);
    item.relative().child(
        div()
            .absolute()
            .left(px(left))
            .top(px(top))
            .h(px(LIST_ITEM_HEIGHT - top - bottom))
            .w(px(1.))
            .bg(theme.secondary.opacity(0.5)),
    )
}

/// A row outside the diff on the branch's own line: a click selects it.
fn hoverable(item: Stateful<gpui::Div>, theme: &Theme) -> Stateful<gpui::Div> {
    item.cursor_pointer()
        .hover(|item| item.bg(theme.selection.opacity(0.5)))
}

/// The key a bar's summary is kept by: its commits, newest first, as the
/// commit panel keys a selection's.
pub(super) fn bar_summary_key(node: &wtf_core::GraphNode) -> String {
    node.commits
        .iter()
        .rev()
        .map(|commit| commit.id.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// The key a merge's side branch is folded by.
fn side_key(merge: &wtf_core::GraphNode) -> String {
    format!("side:{}", merge.key)
}

/// How much further in a merge's commits sit than the merge.
const SIDE_INDENT: f32 = 12.;

/// The most nodes a merge's side branch lists.
const MAX_SIDE: usize = 200;
