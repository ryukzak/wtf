//! The commit list and file list beside the diff.

use super::*;

impl RepoView {
    /// Sections of rows alike: uncommitted changes, the selection's commits,
    /// then earlier ones; each row a title over a dimmed detail line.
    pub(super) fn commit_list(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
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
                        let now = now_seconds();
                        range
                            .map(|index| {
                                let commit = &this.commits[index];
                                let selected =
                                    this.narrowed.is_empty() || this.narrowed.contains(&index);
                                let target = Target::Commit {
                                    commit: commit.id.clone(),
                                    message: full_message(commit),
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
    pub(super) fn single_node(&self) -> Option<(usize, &wtf_core::GraphNode)> {
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
                dim_list_item(("later", w), theme)
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
                    dim_list_item(("earlier-node", index), theme)
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
            dim_list_item(("earlier", k), theme)
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
                                            .font_weight(preferences::code_weight())
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
}
