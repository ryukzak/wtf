//! The attention sidebar: which branches the graph shows.

use super::*;

/// How far each folder level of the attention lists steps in.
const ATTENTION_INDENT: f32 = 5.;

impl RepoView {
    /// The attention sidebar on the left of the graph, or a strip that opens it:
    /// a search field, the filters, and the branches by group.
    pub(super) fn attention_panel_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let attention = &self.attention;
        let now = now_seconds();
        let search = self.attention_search.text().to_lowercase();
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
            let branches = self.section_branches(section);
            let mut tags: Vec<&str> = Vec::new();
            // Every tag, so they can be looked over before the graph shows
            // them.
            if section == Section::Tags {
                tags.extend(self.graph.tags.iter().map(String::as_str));
            }
            // A group's checkbox turns it on or off; worktrees are always on.
            // A double click on it, or its menu, shows or hides all its
            // branches. An open group shows its count and folds with the
            // chevron.
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
                checkbox.on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    // The second click of a double one turns the group back
                    // as it was before showing or hiding all.
                    this.toggle_group(section, cx);
                    if event.click_count() == 2 {
                        let names = this.section_branches(section);
                        let all = names.iter().all(|branch| branch.shown);
                        let command = if all { Command::Hide } else { Command::Pin };
                        this.attention_group(section, command, cx);
                    }
                }))
            });
            let menu = match section {
                Section::Local => Some(Target::Group { remote: false }),
                Section::Remote => Some(Target::Group { remote: true }),
                _ => None,
            };
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
                    .when_some(menu, |header, menu| {
                        header.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                this.open_context_menu(menu.clone(), event.position, cx);
                            }),
                        )
                    })
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
                    .collapsed_branch_dirs
                    .iter()
                    .filter_map(|key| key.strip_prefix(prefix).map(str::to_string))
                    .collect(),
                false => HashSet::new(),
            };
            for row in name_tree(&names, &folded, false) {
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
                                .pl(px(8. + (depth + 1) as f32 * ATTENTION_INDENT))
                                .pr(px(8.))
                                .cursor_pointer()
                                .hover(|row| row.bg(theme.selection.opacity(0.5)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.collapsed_branch_dirs.remove(&key) {
                                        this.collapsed_branch_dirs.insert(key.clone());
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
                    .child(self.attention_search.render(
                        "attention-search-text",
                        "Filter worktrees, branches, tags",
                        focused,
                        true,
                        Vec::new(),
                        &self.attention_search_focus,
                        theme,
                        |this: &mut Self| Some(&mut this.attention_search),
                        cx,
                    )),
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

    /// A branch in the sidebar tree, `name` short for it under its folder,
    /// checked when shown and dimmed when not, with its checkbox at the
    /// right. Clicking hides a shown branch or pins a left-out one; a pin or
    /// hide shows as a note whose × clears it.
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
            .pl(px(8. + depth as f32 * ATTENTION_INDENT + 16.))
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

    /// The branches a group of the attention list holds.
    fn section_branches(&self, section: Section) -> Vec<&wtf_core::Branch> {
        self.graph
            .branches
            .iter()
            .filter(|branch| match section {
                Section::Worktrees => branch.worktree,
                Section::Local => !branch.worktree && !branch.remote,
                Section::Remote => branch.remote,
                Section::Tags | Section::Stashes => false,
            })
            .collect()
    }

    /// Shows all of a group's branches with [`Command::Pin`], hides them
    /// with [`Command::Hide`], or leaves them to the filters.
    pub(super) fn attention_group(
        &mut self,
        section: Section,
        command: Command,
        cx: &mut Context<Self>,
    ) {
        let names: Vec<String> = self
            .section_branches(section)
            .into_iter()
            .map(|branch| branch.name.clone())
            .collect();
        match command {
            Command::Pin => self.toggle_branches(&names, false, cx),
            Command::Hide => self.toggle_branches(&names, true, cx),
            Command::FollowFilters => self.change_attention(
                |attention| {
                    attention.pinned.retain(|pinned| !names.contains(pinned));
                    attention.hidden.retain(|hidden| !names.contains(hidden));
                },
                cx,
            ),
            _ => {}
        }
    }

    /// Drops a pin or hide, so the filters decide on the branch again.
    pub(super) fn follow_filters(&mut self, name: &str, cx: &mut Context<Self>) {
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
            self.select_and_reveal(node, cx);
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
        match keystroke.key.as_str() {
            "escape" if self.attention_search.text().is_empty() => self.focus.focus(window),
            "escape" => self.attention_search.edit.set_text(""),
            _ if self.attention_search.key(keystroke, cx) => {}
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The time window and authors dropdowns of the sidebar.
    pub(super) fn dropdown_view(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
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
            .map(|panel| floating(panel, theme))
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
            Dropdown::Modes(shown) => {
                let checks = [
                    !self.snapshot_mode,
                    self.snapshot_mode,
                    !self.one_column.get(),
                    self.one_column.get(),
                    self.diff_mode == DiffMode::Structural,
                    self.diff_mode == DiffMode::Text,
                    self.wrap_lines,
                    !self.wrap_lines,
                ];
                for (pair, names) in header::STRIP_PAIRS.iter().enumerate().skip(*shown) {
                    if pair > *shown {
                        menu = menu.child(div().my(px(4.)).h(px(1.)).bg(theme.separator));
                    }
                    for (k, name) in names.iter().enumerate() {
                        let at = pair * 2 + k;
                        menu =
                            menu.child(item(("mode", at), checks[at], name.to_string()).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.dropdown = None;
                                    match at {
                                        0 | 1 => this.set_snapshot_mode(at == 1, window, cx),
                                        2 | 3 => this.set_unified(at == 3, window, cx),
                                        4 => this.set_diff_mode(DiffMode::Structural, cx),
                                        5 => this.set_diff_mode(DiffMode::Text, cx),
                                        _ => this.set_wrap_lines(at == 6, window, cx),
                                    }
                                    cx.notify();
                                }),
                            ));
                    }
                }
            }
        }
        Some(
            deferred(anchored().position(*position).snap_to_window().child(menu))
                .with_priority(1)
                .into_any_element(),
        )
    }
}

/// An attention checkbox: on, off, or `None` for a folder partly on.
pub(super) fn attention_checkbox(
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
