//! Menus and dialogs over the window: context menu, Go To, prompts, prune and stash.

use super::*;

impl RepoView {
    pub fn open_context_menu(
        &mut self,
        target: Target,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some((target, position));
        cx.notify();
    }

    fn run_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, _)) = self.context_menu.take() else {
            return;
        };
        if let Target::Group { remote } = target {
            let section = if remote {
                Section::Remote
            } else {
                Section::Local
            };
            self.attention_group(section, command, cx);
            return;
        }
        let repo = self.repo_path.clone().unwrap_or_default();
        let (name, path, commit) = match &target {
            Target::Branch { name, commit, .. } => (Some(name.clone()), None, commit.clone()),
            Target::Commit { commit, .. } => (None, None, commit.clone()),
            Target::Remote {
                remote,
                branch,
                commit,
            } => (Some(format!("{remote}/{branch}")), None, commit.clone()),
            Target::Tag { name, commit } => (Some(name.clone()), None, commit.clone()),
            Target::Stash { name, id } => (Some(name.clone()), None, id.clone()),
            Target::Worktree {
                path,
                branch,
                commit,
                ..
            } => (branch.clone(), Some(path.clone()), commit.clone()),
            Target::Group { .. } => return,
        };
        let copy = |text: String, cx: &mut Context<Self>| {
            cx.write_to_clipboard(ClipboardItem::new_string(text))
        };
        match command {
            Command::CopyName => copy(name.unwrap_or_default(), cx),
            Command::CopyHash => copy(commit, cx),
            Command::CopyShortHash => copy(popups::short_hash(&commit), cx),
            Command::CopyMessage => {
                if let Target::Commit { message, .. } = &target {
                    copy(message.clone(), cx)
                }
            }
            Command::CopyPath => copy(path.unwrap_or_default(), cx),
            Command::OpenWithRecent(k) => {
                if let (Some(path), Some(command)) = (path, self.open_with.get(k).cloned()) {
                    self.run_in_terminal(popups::open_with(&command, &path), window, cx);
                    self.remember_open_with(&command);
                }
            }
            Command::RevealInFinder => {
                let opened = std::process::Command::new("open")
                    .arg("-R")
                    .arg(path.unwrap_or_default())
                    .spawn();
                if let Err(error) = opened {
                    self.error_message = Some(format!("Cannot open Finder: {error}"));
                }
            }
            Command::OpenWith => {
                if let Some(path) = path {
                    let last = self.open_with.first().cloned().unwrap_or_default();
                    self.show_prompt(PromptKind::OpenWith(path), last, window);
                }
            }
            Command::Rename => {
                if let Some(name) = name {
                    self.show_prompt(PromptKind::Rename(name.clone()), name, window);
                }
            }
            Command::Delete => {
                if let Some(name) = name {
                    let question = format!("Delete branch {name}?");
                    let detail = "git refuses to delete a branch that is not merged.";
                    self.confirm(
                        question,
                        detail,
                        "Delete",
                        popups::delete_branch(&repo, &name),
                        window,
                        cx,
                    );
                }
            }
            Command::NewWorktree => match &target {
                Target::Branch { name, .. } => {
                    let path = popups::worktree_path(&repo, name);
                    self.show_prompt(PromptKind::WorktreeFor(name.clone()), path, window);
                }
                Target::Commit { .. } => {
                    let kind = PromptKind::NewBranch {
                        commit: commit.clone(),
                        worktree: true,
                    };
                    self.show_prompt(kind, name.unwrap_or_default(), window);
                }
                _ => {}
            },
            // A commit asks for a name.
            Command::NewBranch => {
                if let Target::Commit { .. } = &target {
                    let kind = PromptKind::NewBranch {
                        commit: commit.clone(),
                        worktree: false,
                    };
                    self.show_prompt(kind, name.unwrap_or_default(), window);
                }
            }
            Command::StashApply(k) | Command::StashPop(k) => {
                let pop = matches!(command, Command::StashPop(_));
                if let (Some(name), Some((path, _))) =
                    (name, self.checkout_worktrees(&target).get(k).cloned())
                {
                    self.run_in_terminal(popups::stash_apply(&path, &name, pop), window, cx);
                }
            }
            Command::StashWorktree => {
                if let Target::Stash { name, .. } = &target
                    && let Some(stash) = self.graph.stashes.iter().find(|s| s.name == *name)
                {
                    let kind = PromptKind::StashWorktree {
                        stash: stash.name.clone(),
                        base: stash.base.clone(),
                    };
                    self.show_prompt(kind, String::new(), window);
                }
            }
            Command::StashChanges => {
                if let Target::Worktree { path, .. } = &target {
                    self.open_stash_picker(path.clone(), window, cx);
                }
            }
            Command::DropStash => {
                if let Some(name) = name {
                    self.confirm(
                        format!("Drop {name}?"),
                        "Its changes are gone once dropped.",
                        "Drop",
                        popups::stash_drop(&repo, &name),
                        window,
                        cx,
                    );
                }
            }
            Command::CheckoutIn(k) => {
                if let Some((path, _)) = self.checkout_worktrees(&target).get(k).cloned() {
                    self.check_out(&target, &path, window, cx);
                }
            }
            Command::DeleteWithCommits => {
                if let Some(name) = name {
                    self.confirm(
                        format!("Delete branch {name} and its commits?"),
                        "Nothing else grows from it and the main branch has not merged it; \
                         its commits are lost unless they were pushed.",
                        "Delete",
                        popups::delete_branch_with_commits(&repo, &name),
                        window,
                        cx,
                    );
                }
            }
            Command::DeleteTag => {
                if let Some(name) = name {
                    self.confirm(
                        format!("Delete tag {name}?"),
                        "Only this repository's tag goes; a pushed tag stays on its remote.",
                        "Delete",
                        popups::delete_tag(&repo, &name),
                        window,
                        cx,
                    );
                }
            }
            Command::DeleteRemote => {
                if let Target::Remote { remote, branch, .. } = &target {
                    self.confirm(
                        format!("Delete {branch} on {remote}?"),
                        "This pushes the deletion: the branch goes from the shared repository for everyone.",
                        "Delete on Remote",
                        popups::delete_remote_branch(&repo, remote, branch),
                        window,
                        cx,
                    );
                }
            }
            Command::Hide => self.hide_branch(&popups::attention_name(&target), cx),
            Command::FollowFilters => self.follow_filters(&popups::attention_name(&target), cx),
            Command::ToggleChannel => {
                if let Some(name) = self.channel_name(&target) {
                    match self.graph.channels.contains(&name) {
                        true => self.remove_channel(&name, cx),
                        false => self.add_channel(&name, cx),
                    }
                }
            }
            Command::Pin => {
                let name = popups::attention_name(&target);
                self.change_attention(
                    |attention| {
                        attention.hidden.retain(|hidden| *hidden != name);
                        if !attention.pinned.contains(&name) {
                            attention.pinned.push(name.clone());
                        }
                    },
                    cx,
                );
            }
            Command::RemoveWorktreeAndBranch => {
                let merged = matches!(
                    target,
                    Target::Worktree {
                        leaf: Some(true),
                        ..
                    }
                );
                if let (Some(path), Some(name)) = (path, name) {
                    let detail = match merged {
                        true => {
                            "The main branch has merged it, so no work is lost. git refuses to \
                             remove a worktree with uncommitted changes."
                        }
                        false => {
                            "Nothing else grows from this branch; its commits are lost unless \
                             they were pushed. git refuses to remove a worktree with uncommitted \
                             changes."
                        }
                    };
                    self.confirm(
                        format!("Remove worktree {path} and delete branch {name}?"),
                        detail,
                        "Remove Both",
                        popups::remove_worktree_and_branch(&repo, &path, &name, merged),
                        window,
                        cx,
                    );
                }
            }
            Command::RemoveWorktree => {
                if let Some(path) = path {
                    let question = format!("Remove worktree {path}?");
                    let detail = "git refuses to remove a worktree with uncommitted changes.";
                    self.confirm(
                        question,
                        detail,
                        "Remove",
                        popups::remove_worktree(&repo, &path),
                        window,
                        cx,
                    );
                }
            }
        }
        cx.notify();
    }

    /// Asks before running a command that changes the repository.
    fn confirm(
        &mut self,
        question: String,
        detail: &str,
        action: &str,
        line: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = window.prompt(
            PromptLevel::Warning,
            &question,
            Some(detail),
            &[action, "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                this.update_in(cx, |this, window, cx| {
                    this.run_in_terminal(line, window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    /// Types a command into the terminal, opening the dock to show it.
    pub(super) fn run_in_terminal(
        &mut self,
        line: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.console_open {
            self.toggle_console(window, cx);
        }
        if let Some(terminal) = &self.terminal {
            terminal.update(cx, |terminal, cx| terminal.run(&line, cx));
        }
    }

    /// Lists the stale branches and worktrees, all ticked to go.
    pub(super) fn open_prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prune = Some(self.stale_branches());
        self.prune_focus.focus(window);
        cx.notify();
    }

    /// Branches the prune dialog offers, each with whether it starts ticked.
    pub(super) fn stale_branches(&self) -> Vec<(String, bool)> {
        self.graph
            .branches
            .iter()
            .filter(|branch| !branch.remote && !branch.unrelated && branch.name != self.graph.main)
            .filter(|branch| graph_view::stale_note(&self.graph, &branch.name).is_some())
            // Merged ones start ticked; the rest, idle or with a gone remote,
            // may still hold work, so they wait to be ticked.
            .map(|branch| {
                let merged = branch.stale.iter().any(|reason| reason == "merged");
                (branch.name.clone(), merged)
            })
            .collect()
    }

    fn close_prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prune = None;
        self.focus.focus(window);
        cx.notify();
    }

    /// Deletes the ticked branches, each worktree first, in the terminal: a
    /// merged branch as git checks it, others with their commits. One that
    /// fails leaves the rest to go on.
    fn run_prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(list), Some(repo)) = (self.prune.take(), self.repo_path.clone()) else {
            return;
        };
        let lines: Vec<String> = list
            .into_iter()
            .filter(|(_, ticked)| *ticked)
            .filter_map(|(name, _)| {
                let branch = self.graph.branches.iter().find(|b| b.name == name)?;
                let merged = branch.stale.iter().any(|reason| reason == "merged");
                let delete = match merged {
                    true => popups::delete_branch(&repo, &name),
                    false => popups::delete_branch_with_commits(&repo, &name),
                };
                let worktree = self.graph.nodes.iter().find_map(|node| match &node.tip {
                    Revision::Worktree(path) if node.branch.as_ref() == Some(&name) => {
                        Some(path.clone())
                    }
                    _ => None,
                });
                Some(match worktree {
                    Some(path) => format!("{} && {delete}", popups::remove_worktree(&repo, &path)),
                    None => delete,
                })
            })
            .collect();
        self.focus.focus(window);
        if !lines.is_empty() {
            self.run_in_terminal(lines.join(" ; "), window, cx);
        }
        cx.notify();
    }

    fn prune_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close_prune(window, cx),
            "enter" => self.run_prune(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The prune list: each stale branch with why, ticked to go, and a button
    /// that deletes the ticked ones.
    pub(super) fn prune_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let list = self.prune.as_ref()?;
        let ticked = list.iter().filter(|(_, ticked)| *ticked).count();
        let rows = list.iter().enumerate().map(|(k, (name, on))| {
            let worktree = self.graph.nodes.iter().find(|node| {
                node.kind == wtf_core::NodeKind::Uncommitted && node.branch.as_ref() == Some(name)
            });
            let note = graph_view::stale_note(&self.graph, name).unwrap_or_default();
            div()
                .id(("prune", k))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(26.))
                .px(px(8.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|row| row.bg(theme.selection.opacity(0.5)))
                .when(!on, |row| row.text_color(theme.secondary))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some((_, on)) = this.prune.as_mut().and_then(|list| list.get_mut(k)) {
                        *on = !*on;
                        cx.notify();
                    }
                }))
                .child(attention_checkbox(
                    ("prune-check", k),
                    Some(*on),
                    true,
                    theme,
                ))
                .child(div().min_w_0().truncate().child(name.clone()))
                .children(worktree.map(|node| {
                    div()
                        .flex_none()
                        .px(px(5.))
                        .rounded(px(3.))
                        .bg(theme.tag_background)
                        .text_xs()
                        .child(format!(
                            "▣ {}",
                            graph_view::worktree_label(node, WorktreeNames::Folder)
                        ))
                }))
                .child(div().flex_1())
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.secondary)
                        .italic()
                        .child(note),
                )
        });
        let button = |id: &'static str, label: String| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(4.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.separator)
                .cursor_pointer()
                .hover(|button| button.bg(theme.selection))
                .child(label)
        };
        let panel =
            div()
                .id("prune-panel")
                .track_focus(&self.prune_focus)
                .on_key_down(cx.listener(Self::prune_key))
                .occlude()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(12.))
                .map(|panel| dialog_chrome(panel, GOTO_WIDTH, theme, window))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_prune(window, cx)))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Prune stale branches and worktrees"),
                )
                .child(div().text_xs().text_color(theme.secondary).child(
                    "Untick what to keep. Worktrees go first, then their branches; merged ones \
                 as git checks them, others with their commits.",
                ))
                // Everything at once, then untick or tick from there.
                .when(!list.is_empty(), |panel| {
                    let all = |id: &'static str, label: &'static str, on: bool| {
                        div()
                            .id(id)
                            .text_xs()
                            .text_color(theme.accent)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                for (_, ticked) in this.prune.iter_mut().flatten() {
                                    *ticked = on;
                                }
                                cx.notify();
                            }))
                            .child(label)
                    };
                    panel.child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(all("prune-all", "Select all", true))
                            .child(all("prune-none", "Deselect all", false)),
                    )
                })
                .child(
                    div()
                        .id("prune-list")
                        .flex()
                        .flex_col()
                        .max_h(px(360.))
                        .overflow_y_scroll()
                        .children(rows),
                )
                .when(list.is_empty(), |panel| {
                    panel.child(
                        div()
                            .text_color(theme.secondary)
                            .child("Nothing looks stale."),
                    )
                })
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(button("prune-cancel", "Cancel".to_string()).on_click(
                            cx.listener(|this, _, window, cx| this.close_prune(window, cx)),
                        ))
                        .when(ticked > 0, |buttons| {
                            buttons.child(
                                button("prune-run", format!("Delete {ticked}"))
                                    .bg(theme.error)
                                    .border_color(theme.error)
                                    .text_color(theme.tag_head_text)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.run_prune(window, cx)
                                    })),
                            )
                        }),
                );
        Some(dialog_overlay(panel, GOTO_WIDTH, 60., window))
    }

    /// Asks for a command to open a worktree with, the last one offered:
    /// worktree node `node`, or the one selected, or the repository's.
    pub(super) fn open_worktree_with(
        &mut self,
        node: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let node = node.or_else(|| match self.selected_nodes[..] {
            [selected] => Some(selected),
            _ => None,
        });
        let path = node
            .and_then(|node| match &self.graph.nodes.get(node)?.tip {
                Revision::Worktree(path) => Some(path.clone()),
                Revision::Commit(_) => None,
            })
            .or_else(|| self.repo_path.clone());
        if let Some(path) = path {
            let last = self.open_with.first().cloned().unwrap_or_default();
            self.show_prompt(PromptKind::OpenWith(path), last, window);
            cx.notify();
        }
    }

    fn remember_open_with(&mut self, command: &str) {
        popups::remember(&mut self.open_with, command);
        recent::save_list("open-with", &self.open_with);
    }

    /// The worktree the palette runs commands on: the selected one, or
    /// the repository's.
    pub(super) fn command_path(&self) -> Option<String> {
        self.current_worktree_path()
            .map(str::to_string)
            .or_else(|| self.repo_path.clone())
    }

    /// What the picker lists for its query. Go To: the places it matches.
    /// The palette: the commands it matches and those run before on the
    /// worktree; with ⌥ held, the query itself run on the worktree.
    fn goto_found(&self, window: &Window) -> Vec<goto::Entry> {
        let Some((query, _, palette)) = &self.goto else {
            return Vec::new();
        };
        let query = query.text();
        if !*palette {
            let entries = goto::entries(&self.graph, self.worktree_names);
            return goto::filter(&entries, query).into_iter().cloned().collect();
        }
        let path = self.command_path();
        if window.modifiers().alt && !query.trim().is_empty() {
            return path
                .map(|path| goto::shell_entry(query, &path))
                .into_iter()
                .collect();
        }
        // The last commands run come first, then the app's.
        let entries: Vec<goto::Entry> = path
            .iter()
            .flat_map(|path| {
                self.shell_commands
                    .iter()
                    .map(move |command| goto::shell_entry(command, path))
            })
            .chain(palette::entries(window))
            .collect();
        goto::filter(&entries, query).into_iter().cloned().collect()
    }

    pub(super) fn open_goto(&mut self, palette: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.goto = Some((Default::default(), 0, palette));
        self.goto_focus.focus(window);
        cx.notify();
    }

    fn close_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.goto = None;
        self.focus.focus(window);
        cx.notify();
    }

    /// Jumps to a picked place: a worktree's or branch's node, or a commit.
    fn pick_goto(&mut self, entry: goto::Entry, window: &mut Window, cx: &mut Context<Self>) {
        self.close_goto(window, cx);
        if entry.kind == goto::Kind::Command {
            if let Some((_, action)) = palette::commands().into_iter().nth(entry.node) {
                window.dispatch_action(action, cx);
            }
            return;
        }
        if let Some((command, path)) = &entry.run {
            // Run in the terminal without opening the dock over the review.
            let line = popups::run_in(command, path);
            match &self.terminal {
                Some(terminal) => terminal.update(cx, |terminal, cx| terminal.run(&line, cx)),
                None => self.run_in_terminal(line, window, cx),
            }
            popups::remember(&mut self.shell_commands, command);
            recent::save_list("commands", &self.shell_commands);
            return;
        }
        match entry.commit {
            Some(commit) => self.select_commit(entry.node, commit, cx),
            None => self.select_and_reveal(entry.node, cx),
        }
    }

    fn goto_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.goto_found(window);
        let path = self.command_path();
        let completions = std::mem::take(&mut self.completions);
        let Some((query, highlighted, palette)) = &mut self.goto else {
            return;
        };
        let keystroke = &event.keystroke;
        let found = entries.len();
        match keystroke.key.as_str() {
            "escape" => return self.close_goto(window, cx),
            // In the palette, Tab completes a file name in the worktree.
            "tab" if *palette && !query.text().trim().is_empty() => {
                let Some(path) = path else { return };
                let (completed, matches) = goto::complete(query.text(), |folder| {
                    std::fs::read_dir(Path::new(&path).join(folder))
                        .into_iter()
                        .flatten()
                        .flatten()
                        .map(|entry| {
                            let folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
                            (entry.file_name().to_string_lossy().into_owned(), folder)
                        })
                        .collect()
                });
                // A second Tab with nothing more to add keeps the choices shown.
                self.completions = match matches.is_empty() && completed == query.text() {
                    true => completions,
                    false => matches,
                };
                if completed != query.text() {
                    *highlighted = 0;
                    query.edit.set_text(completed);
                }
            }
            "enter" => {
                // Holding ⌥ can shorten the list under the highlight.
                let picked = entries
                    .get((*highlighted).min(found.saturating_sub(1)))
                    .cloned();
                // ⌘↩ on a worktree opens it with a command instead.
                let open_with = keystroke.modifiers.platform;
                match picked {
                    Some(entry) if open_with && entry.kind == goto::Kind::Worktree => {
                        self.close_goto(window, cx);
                        self.open_worktree_with(Some(entry.node), window, cx);
                    }
                    Some(entry) => self.pick_goto(entry, window, cx),
                    None => {}
                }
                return;
            }
            // Only the rows shown can be highlighted.
            "down" => *highlighted = (*highlighted + 1).min(found.min(GOTO_ROWS).saturating_sub(1)),
            "up" => {
                let last = found.min(GOTO_ROWS).saturating_sub(1);
                *highlighted = (*highlighted).min(last).saturating_sub(1);
            }
            // A changed query lists from its best match again.
            _ => {
                let before = query.text().to_string();
                if !query.key(keystroke, cx) {
                    return;
                }
                if query.text() != before {
                    *highlighted = 0;
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The Go To picker: a query above the worktrees, branches and commits
    /// it matches.
    pub(super) fn goto_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (query, highlighted, commands) = self.goto.as_ref()?;
        let found = self.goto_found(window);
        let highlighted = &(*highlighted).min(found.len().saturating_sub(1));
        let running = *commands && window.modifiers().alt && !query.text().trim().is_empty();
        let now = now_seconds();
        let input = query.render(
            "goto-text",
            if *commands {
                "Run a command"
            } else {
                "Go to a worktree, branch or commit"
            },
            true,
            true,
            Vec::new(),
            &self.goto_focus,
            theme,
            |this: &mut Self| this.goto.as_mut().map(|(query, _, _)| query),
            cx,
        );
        let rows = found.iter().take(GOTO_ROWS).enumerate().map(|(k, entry)| {
            let (icon, kind) = match entry.kind {
                goto::Kind::Worktree => ("▣", "worktree · ⌘↩ open with"),
                goto::Kind::Shell => ("$", "run in terminal"),
                goto::Kind::Branch => ("⎇", "branch"),
                goto::Kind::Commit => ("●", "commit"),
                goto::Kind::Command => ("›", ""),
            };
            let picked = entry.clone();
            div()
                .id(("goto", k))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(28.))
                .px(px(10.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(k == *highlighted, |row| row.bg(theme.selection))
                .hover(|row| row.bg(theme.selection.opacity(0.5)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.pick_goto(picked.clone(), window, cx)
                }))
                .child(div().w(px(14.)).text_color(theme.secondary).child(icon))
                .child(div().min_w_0().truncate().child(entry.label.clone()))
                .child(match entry.kind {
                    // A command's folder keeps its end in view: one too long
                    // is cut from the start, against the right edge.
                    goto::Kind::Shell => div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .justify_end()
                        .overflow_hidden()
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(
                            div()
                                .flex_none()
                                .whitespace_nowrap()
                                .child(entry.detail.clone()),
                        ),
                    _ => div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(entry.detail.clone()),
                })
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(match entry.time {
                            0 => kind.to_string(),
                            time => format!("{kind} · {}", short_age(time, now)),
                        }),
                )
        });
        let panel = div()
            .id("goto-panel")
            .occlude()
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(8.))
            .map(|panel| dialog_chrome(panel, GOTO_WIDTH, theme, window))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_goto(window, cx)))
            .child(
                div()
                    .id("goto-input")
                    .track_focus(&self.goto_focus)
                    .on_key_down(cx.listener(Self::goto_key))
                    // Holding ⌥ turns the query into a command to run.
                    .on_modifiers_changed(cx.listener(|_, _, _, cx| cx.notify()))
                    .flex()
                    .items_center()
                    .h(px(30.))
                    .px(px(10.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme.accent)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(input),
            )
            .children(rows)
            .when(found.is_empty(), |panel| {
                panel.child(
                    div()
                        .px(px(10.))
                        .py(px(6.))
                        .text_color(theme.secondary)
                        .child("Nothing matches"),
                )
            })
            .when(*commands, |panel| {
                let path = self.command_path().unwrap_or_default();
                let worktree = path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or_default();
                let hint = match (running, query.text().trim()) {
                    _ if !self.completions.is_empty() => self.completions.join("   "),
                    (true, _) => format!(
                        "↩ runs it in {} · ⇥ completes file names",
                        popups::tilde(&path)
                    ),
                    (false, "") => {
                        format!("Type a shell command and hold ⌥ to run it in {worktree}")
                    }
                    (false, query) => {
                        format!("Hold ⌥ to run “{query}” in {worktree} · ⇥ completes file names")
                    }
                };
                panel.child(
                    div()
                        .mt(px(2.))
                        .pt(px(6.))
                        .px(px(10.))
                        .border_t_1()
                        .border_color(theme.separator)
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(hint),
                )
            });
        Some(dialog_overlay(panel, GOTO_WIDTH, 60., window))
    }

    pub(super) fn show_prompt(&mut self, kind: PromptKind, input: String, window: &mut Window) {
        self.prompt = Some(Prompt {
            kind,
            field: crate::text_field::Field::new(crate::line_edit::LineEdit::new(input)),
        });
        self.prompt_focus.focus(window);
        self.caret_on = true;
        self.blink_task = None;
    }

    /// Blinks the prompt's caret, as text fields' carets do, while the
    /// prompt is open; typing shows it again at once.
    pub(super) fn blink(&mut self, cx: &mut Context<Self>) {
        if self.blink_task.is_some() {
            return;
        }
        self.blink_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CARET_BLINK).await;
                let open = this.update(cx, |this, cx| {
                    this.caret_on = !this.caret_on;
                    cx.notify();
                    this.prompt.is_some()
                });
                if !open.unwrap_or(false) {
                    break;
                }
            }
        }));
    }

    fn close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt = None;
        self.blink_task = None;
        self.focus.focus(window);
        cx.notify();
    }

    fn prompt_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = &mut self.prompt else {
            return;
        };
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "enter" => self.submit_prompt(window, cx),
            "escape" => self.close_prompt(window, cx),
            _ if prompt.field.key(keystroke, cx) => {
                // The caret stays lit while typing, and blinks from there.
                self.caret_on = true;
                self.blink_task = None;
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn submit_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        self.focus.focus(window);
        self.blink_task = None;
        let input = prompt.field.text().trim().to_string();
        if input.is_empty() {
            return;
        }
        let repo = self.repo_path.clone().unwrap_or_default();
        match prompt.kind {
            PromptKind::Rename(old) if old != input => {
                self.run_in_terminal(popups::rename_branch(&repo, &old, &input), window, cx);
                // Pins and hides follow the branch to its new name.
                self.change_attention(
                    |attention| {
                        for name in attention.pinned.iter_mut().chain(&mut attention.hidden) {
                            if *name == old {
                                *name = input.clone();
                            }
                        }
                    },
                    cx,
                );
            }
            PromptKind::Rename(_) => {}
            PromptKind::OpenWith(path) => {
                self.run_in_terminal(popups::open_with(&input, &path), window, cx);
                self.remember_open_with(&input);
            }
            PromptKind::StashWorktree { stash, base } => {
                let path = popups::worktree_path(&repo, &input);
                let line = popups::stash_worktree(&repo, &path, &input, &base, &stash);
                self.run_in_terminal(line, window, cx);
            }
            PromptKind::WorktreeFor(branch) => {
                self.run_in_terminal(popups::add_worktree(&repo, &input, &branch), window, cx);
            }
            PromptKind::AddChannel => self.add_channel(&input, cx),
            PromptKind::NewBranch {
                commit,
                worktree: false,
            } => self.run_in_terminal(popups::new_branch(&repo, &input, &commit), window, cx),
            PromptKind::NewBranch {
                commit,
                worktree: true,
            } => {
                let path = popups::worktree_path(&repo, &input);
                let line = popups::add_worktree_at(&repo, &path, &input, &commit);
                self.run_in_terminal(line, window, cx);
            }
        }
    }

    /// The name a branch would be a channel by: a local branch's, or a remote
    /// one's without its remote. Never the main branch, always one.
    fn channel_name(&self, target: &Target) -> Option<String> {
        let name = match target {
            Target::Branch { name, .. } => name,
            Target::Remote { branch, .. } => branch,
            _ => return None,
        };
        (*name != self.graph.main).then(|| name.clone())
    }

    /// The menu's channel item for a branch, if it offers one.
    fn channel_label(&self, target: &Target) -> Option<&'static str> {
        let name = self.channel_name(target)?;
        let patterns = &self.graph.channels;
        if patterns.contains(&name) {
            Some("Stop Treating as Channel")
        } else if patterns
            .iter()
            .any(|pattern| wtf_core::glob(pattern, &name))
        {
            None
        } else {
            Some("Treat as Channel")
        }
    }

    /// Checks `target` out in the worktree at `path`: a commit detached, a
    /// branch by its name.
    fn check_out(
        &mut self,
        target: &Target,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line = match target {
            Target::Commit { commit, .. } => popups::switch_detached(path, commit),
            // By its name alone, git makes a local branch tracking it.
            Target::Remote { branch, .. } => popups::switch(path, branch),
            Target::Branch { name, .. } => popups::switch(path, name),
            _ => return,
        };
        self.run_in_terminal(line, window, cx);
    }

    /// Checks `target` out in the current worktree, as a double click on a
    /// label or a commit does; nothing when it is already there.
    pub fn check_out_here(&mut self, target: &Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some(here) = self.command_path() else {
            return;
        };
        let here = here.trim_end_matches('/');
        let found = self
            .checkout_worktrees(target)
            .into_iter()
            .find(|(path, _)| path.trim_end_matches('/') == here);
        if let Some((path, _)) = found {
            self.check_out(target, &path, window, cx);
        }
    }

    /// The worktrees a branch can be checked out in: all but one that has it.
    fn checkout_worktrees(&self, target: &Target) -> Vec<(String, String)> {
        let name = match target {
            Target::Branch { name, .. } => Some(name),
            // A stash, a commit or a remote branch goes into any worktree.
            Target::Stash { .. } | Target::Commit { .. } | Target::Remote { .. } => None,
            _ => return Vec::new(),
        };
        self.graph
            .nodes
            .iter()
            .filter(|node| node.kind == wtf_core::NodeKind::Uncommitted)
            .filter(|node| name.is_none() || node.branch.as_ref() != name)
            .filter_map(|node| match &node.tip {
                Revision::Worktree(path) => Some((
                    path.clone(),
                    graph_view::worktree_label(node, self.worktree_names),
                )),
                Revision::Commit(_) => None,
            })
            .collect()
    }

    pub(super) fn context_menu_view(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (target, position) = self.context_menu.as_ref()?;
        let main = self.graph.main.as_str();
        let mut menu = div()
            .id("context-menu")
            .flex()
            .flex_col()
            .min_w(px(200.))
            .py(px(4.))
            .rounded(px(6.))
            .map(|panel| floating(panel, theme))
            .text_sm()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.context_menu = None;
                cx.notify();
            }));
        let name = popups::attention_name(target);
        let shown = self
            .graph
            .branches
            .iter()
            .find(|branch| branch.name == name)
            .is_none_or(|branch| branch.shown);
        let worktrees: Vec<String> = self
            .checkout_worktrees(target)
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        let overridden =
            self.attention.pinned.contains(&name) || self.attention.hidden.contains(&name);
        let mut items = popups::commands(
            target,
            main,
            self.graph.main_worktree.as_deref(),
            &self.open_with,
            shown,
            overridden,
            &worktrees,
        );
        // A branch becomes a channel by its name, next to what attention
        // makes of it; one a wider pattern names already offers nothing.
        if let Some(label) = self.channel_label(target) {
            let at = items
                .iter()
                .position(|item| {
                    matches!(
                        item,
                        Some((Command::Hide | Command::Pin | Command::FollowFilters, _))
                    )
                })
                .map_or(items.len(), |at| at + 1);
            items.insert(at, Some((Command::ToggleChannel, label.to_string())));
        }
        for (k, item) in items.into_iter().enumerate() {
            menu = match item {
                None => menu.child(div().h(px(1.)).my(px(4.)).bg(theme.separator)),
                Some((command, label)) => menu.child(
                    div()
                        .id(("context-item", k))
                        .px(px(12.))
                        .py(px(3.))
                        .hover(|item| item.bg(theme.selection))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run_command(command, window, cx)
                        }))
                        .child(label),
                ),
            };
        }
        Some(
            deferred(anchored().position(*position).snap_to_window().child(menu))
                .with_priority(1)
                .into_any_element(),
        )
    }

    pub(super) fn prompt_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let prompt = self.prompt.as_ref()?;
        let title = match &prompt.kind {
            PromptKind::Rename(name) => format!("Rename branch {name}"),
            PromptKind::OpenWith(_) => {
                "Open worktree with a command; its path is appended".to_string()
            }
            PromptKind::WorktreeFor(branch) => format!("New worktree for {branch} at"),
            PromptKind::StashWorktree { stash, .. } => {
                format!("New worktree with {stash}, on a new branch named")
            }
            PromptKind::NewBranch {
                commit,
                worktree: false,
            } => format!("New branch at {}", &commit[..commit.len().min(7)]),
            PromptKind::NewBranch {
                commit,
                worktree: true,
            } => format!(
                "New worktree at {} on a new branch named",
                popups::short_hash(commit)
            ),
            PromptKind::AddChannel => {
                "Channel branches: a name, or a pattern such as v[0-9][0-9][0-9][0-9]".to_string()
            }
        };
        // A pattern shows what it would name before it is added, so one too
        // loose shows at once.
        let hint = match &prompt.kind {
            PromptKind::AddChannel => {
                let input = prompt.field.text().trim();
                let names = self.channel_matches(input);
                let shown: Vec<&str> = names.iter().take(8).map(String::as_str).collect();
                match (input.is_empty(), names.len()) {
                    (true, _) => "* matches any run of characters, ? one, [0-9] one of a set. \
                                  Enter to add · Esc to cancel"
                        .to_string(),
                    (false, 0) => {
                        "Matches no branch yet · Enter to add · Esc to cancel".to_string()
                    }
                    (false, n) => format!(
                        "Matches {n} branch{}: {}{} · Enter to add",
                        if n == 1 { "" } else { "es" },
                        shown.join(", "),
                        if n > shown.len() { ", …" } else { "" }
                    ),
                }
            }
            _ => "Enter to run in the terminal · Esc to cancel".to_string(),
        };
        let panel = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .map(|panel| dialog_chrome(panel, PROMPT_WIDTH, theme, window))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
            .child(
                div()
                    .id("prompt-input")
                    .track_focus(&self.prompt_focus)
                    .on_key_down(cx.listener(Self::prompt_key))
                    .flex()
                    .items_center()
                    .h(px(26.))
                    .px(px(8.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme.accent)
                    .font_family(preferences::code_font())
                    .font_weight(preferences::code_weight())
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(prompt.field.render(
                        "prompt-text",
                        "",
                        true,
                        self.caret_on,
                        Vec::new(),
                        &self.prompt_focus,
                        theme,
                        |this: &mut Self| this.prompt.as_mut().map(|prompt| &mut prompt.field),
                        cx,
                    )),
            )
            .child(div().text_xs().text_color(theme.secondary).child(hint))
            // A click anywhere else closes it, as Esc does.
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_prompt(window, cx)));
        Some(dialog_overlay(panel, PROMPT_WIDTH, 80., window))
    }

    pub(super) fn open_stash_picker(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stash_picker = Some((path, 0));
        self.stash_focus.focus(window);
        cx.notify();
    }

    fn close_stash_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stash_picker = None;
        self.focus.focus(window);
        cx.notify();
    }

    /// A row of the Stash picker taken: the first runs the stash with the
    /// flags on, the others turn their flag on or off.
    fn pick_stash_row(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, _)) = self.stash_picker.clone() else {
            return;
        };
        if row == 0 {
            let flags: Vec<&str> = popups::STASH_FLAGS
                .iter()
                .zip(self.stash_flags)
                .filter(|(_, on)| *on)
                .map(|((flag, _), _)| *flag)
                .collect();
            self.close_stash_picker(window, cx);
            self.expect_stash();
            self.run_in_terminal(popups::stash_with(&path, &flags), window, cx);
            return;
        }
        if let Some(flag) = self.stash_flags.get_mut(row - 1) {
            *flag = !*flag;
        }
        if let Some((_, highlighted)) = &mut self.stash_picker {
            *highlighted = row;
        }
        cx.notify();
    }

    fn stash_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, highlighted)) = &mut self.stash_picker else {
            return;
        };
        let rows = popups::STASH_FLAGS.len() + 1;
        match event.keystroke.key.as_str() {
            "escape" => return self.close_stash_picker(window, cx),
            "enter" => {
                let row = *highlighted;
                return self.pick_stash_row(row, window, cx);
            }
            "down" => *highlighted = (*highlighted + 1).min(rows - 1),
            "up" => *highlighted = highlighted.saturating_sub(1),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The Stash picker, as Sublime Merge's: `stash` to run, then its flags
    /// to turn on or off, each with what it does.
    pub(super) fn stash_picker_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (path, highlighted) = self.stash_picker.as_ref()?;
        let highlighted = *highlighted;
        let folder = crate::workspace::folder_name(path);
        let row = |k: usize, cx: &mut Context<Self>| {
            div()
                .id(("stash-row", k))
                .flex()
                .items_center()
                .gap(px(8.))
                .h(px(28.))
                .px(px(10.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(k == highlighted, |row| row.bg(theme.selection))
                .hover(|row| row.bg(theme.selection.opacity(0.5)))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.pick_stash_row(k, window, cx)),
                )
        };
        let mut rows = vec![
            row(0, cx)
                .child(div().w(px(14.)))
                .child(div().italic().child("stash"))
                .child(div().flex_1())
                .child(
                    div()
                        .text_xs()
                        .italic()
                        .text_color(theme.secondary)
                        .child(format!("Select to run in {folder}")),
                ),
        ];
        for (k, (flag, about)) in popups::STASH_FLAGS.iter().enumerate() {
            rows.push(
                row(k + 1, cx)
                    .child(
                        div()
                            .w(px(14.))
                            .text_color(theme.accent)
                            .child(if self.stash_flags[k] { "✓" } else { "" }),
                    )
                    .child(
                        div()
                            .font_family(preferences::code_font())
                            .font_weight(preferences::code_weight())
                            .child(*flag),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .italic()
                            .text_color(theme.secondary)
                            .child(*about),
                    ),
            );
        }
        let panel = div()
            .id("stash-picker")
            .track_focus(&self.stash_focus)
            .on_key_down(cx.listener(Self::stash_key))
            .occlude()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(8.))
            .map(|panel| dialog_chrome(panel, GOTO_WIDTH, theme, window))
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.close_stash_picker(window, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .pb(px(6.))
                    .mb(px(2.))
                    .border_b_1()
                    .border_color(theme.separator)
                    .child(
                        div()
                            .px(px(6.))
                            .rounded(px(4.))
                            .bg(theme.selection)
                            .child("Stash"),
                    )
                    .child(div().text_color(theme.secondary).child("Choose flags")),
            )
            .children(rows);
        Some(dialog_overlay(panel, GOTO_WIDTH, 60., window))
    }
}

/// A dialog's chrome: a rounded, shadowed column `width` wide, or as wide
/// as the window allows.
fn dialog_chrome<E: Styled>(panel: E, width: f32, theme: &Theme, window: &Window) -> E {
    panel
        .w(px(dialog_width(width, window)))
        .rounded(px(8.))
        .border_1()
        .border_color(theme.separator)
        .bg(theme.background)
        .shadow_lg()
        .text_sm()
}

/// Shows a dialog centred `top` below the window's top, over everything.
/// A dialog over the window, on a backdrop that takes every click outside
/// it: nothing behind answers, no menu opens there, and the dialog's own
/// click-outside handler closes it.
fn dialog_overlay(panel: impl IntoElement, width: f32, top: f32, window: &Window) -> AnyElement {
    let viewport = window.viewport_size();
    let left = ((f32::from(viewport.width) - dialog_width(width, window)) / 2.).max(0.);
    deferred(
        anchored().position(Point::new(px(0.), px(0.))).child(
            div()
                .id("dialog-backdrop")
                .occlude()
                .w(viewport.width)
                .h(viewport.height)
                .child(div().absolute().left(px(left)).top(px(top)).child(panel)),
        ),
    )
    .with_priority(2)
    .into_any_element()
}

/// How long the prompt's caret stays lit, and dark.
const CARET_BLINK: std::time::Duration = std::time::Duration::from_millis(530);

fn dialog_width(width: f32, window: &Window) -> f32 {
    width.min(f32::from(window.viewport_size().width) - 32.)
}
