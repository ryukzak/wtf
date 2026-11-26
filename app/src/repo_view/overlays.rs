//! Menus and dialogs over the window: context menu, Go To, prompts, prune and stash.

use super::*;

impl RepoView {
    pub fn open_context_menu(
        &mut self,
        target: Target,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let target = self.widen_to_selection(target);
        self.pick_generation += 1;
        self.pick_branches.clear();
        self.pick_places.clear();
        let picks = target.picks();
        if !picks.is_empty() {
            self.pick_places = self
                .checkout_worktrees(&target)
                .into_iter()
                .map(|(path, place)| PickPlace {
                    place,
                    path: Some(path),
                    hint: None,
                })
                .collect();
            self.forecast_picks(false, picks, cx);
        }
        self.context_menu = Some((target, position));
        cx.notify();
    }

    /// A commit among several picked in the commit list, or in the graph's
    /// beads, stands for all of them: the menu acts on the selection.
    fn widen_to_selection(&self, target: Target) -> Target {
        let Target::Commit { commit, .. } = &target else {
            return target;
        };
        let picked = self.commits.iter().position(|c| c.id == *commit);
        if self.narrowed.len() < 2 || !picked.is_some_and(|i| self.narrowed.contains(&i)) {
            return target;
        }
        // The list runs newest first: oldest first is from the end.
        let mut indices = self.narrowed.clone();
        indices.sort_unstable_by(|a, b| b.cmp(a));
        let commits: Vec<(String, String)> = indices
            .iter()
            .filter_map(|&i| self.commits.get(i))
            .map(|c| (c.id.clone(), super::header::full_message(c)))
            .collect();
        Target::Commits {
            title: format!("{} commits", commits.len()),
            commits,
            section: false,
        }
    }

    /// The branches to cherry-pick onto in place of the menu: the local ones
    /// no worktree has, channels first, then the latest, as many as fit.
    fn open_pick_branches(
        &mut self,
        picks: Vec<String>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let mut branches: Vec<&wtf_core::Branch> = self
            .graph
            .branches
            .iter()
            .filter(|b| !b.remote && !b.worktree && !b.unrelated && (b.shown || b.channel))
            .collect();
        branches.sort_by_key(|b| (!b.channel, -b.time));
        branches.truncate(PICK_BRANCHES);
        let branches: Vec<PickPlace> = branches
            .into_iter()
            .map(|b| PickPlace {
                place: popups::Place::new(&b.name, false, false),
                path: None,
                hint: None,
            })
            .collect();
        self.pick_generation += 1;
        self.pick_branches = branches;
        self.forecast_picks(true, picks.clone(), cx);
        self.context_menu = Some((Target::PickBranches { commits: picks }, position));
        cx.notify();
    }

    /// Works out, one place after another, what cherry-picking `picks` into
    /// each worktree, or onto each branch, would do, for the menu's hints.
    fn forecast_picks(&mut self, branches: bool, picks: Vec<String>, cx: &mut Context<Self>) {
        let Some(repo) = self.repo_path.clone() else {
            return;
        };
        let list = match branches {
            true => &self.pick_branches,
            false => &self.pick_places,
        };
        let ontos: Vec<(String, String)> = list
            .iter()
            .map(|pick| match &pick.path {
                Some(path) => (path.clone(), "HEAD".to_string()),
                None => (repo.clone(), format!("refs/heads/{}", pick.place.name)),
            })
            .collect();
        let generation = self.pick_generation;
        cx.spawn(async move |this, cx| {
            for (k, (dir, onto)) in ontos.into_iter().enumerate() {
                let picks = picks.clone();
                let hint = cx
                    .background_executor()
                    .spawn(async move {
                        if picks.iter().all(|c| wtf_core::has_commit(&dir, &onto, c)) {
                            return "already there".to_string();
                        }
                        match wtf_core::pick_forecast(&dir, &onto, &picks) {
                            Ok(forecast) => forecast.hint(picks.len()),
                            Err(_) => "can't tell".to_string(),
                        }
                    })
                    .await;
                let going = this.update(cx, |this, cx| {
                    if this.pick_generation != generation {
                        return false;
                    }
                    let list = match branches {
                        true => &mut this.pick_branches,
                        false => &mut this.pick_places,
                    };
                    if let Some(pick) = list.get_mut(k) {
                        pick.hint = Some(hint);
                    }
                    cx.notify();
                    true
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Cherry-picks onto a branch no worktree has, in git's object store:
    /// done when every commit applies cleanly; otherwise nothing changes and
    /// it offers a worktree for the branch to pick in, where git stops on
    /// the conflict.
    fn pick_onto_branch(
        &mut self,
        branch: String,
        picks: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.repo_path.clone() else {
            return;
        };
        let task = cx.background_executor().spawn({
            let (repo, branch, picks) = (repo.clone(), branch.clone(), picks.clone());
            async move {
                let merges = wtf_core::has_merges(&repo, &picks);
                (wtf_core::pick_onto_branch(&repo, &branch, &picks), merges)
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let (result, merges) = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(wtf_core::Forecast::Clean { .. }) => this.start_refresh(cx),
                Ok(wtf_core::Forecast::Conflicts { step, files }) => {
                    let path = popups::worktree_path(&repo, &branch);
                    let commit = picks.get(step).map(|c| popups::short_hash(c));
                    let detail = format!(
                        "{} conflicts with {branch} in {}. Nothing was changed. In a worktree \
                         for {branch}, git stops on the conflict for you to resolve.",
                        commit.unwrap_or_default(),
                        files.join(", ")
                    );
                    let line = popups::pick_in_new_worktree(&repo, &path, &branch, &picks, merges);
                    this.confirm(
                        format!("Cherry-pick onto {branch} conflicts"),
                        &detail,
                        "Pick in New Worktree",
                        line,
                        window,
                        cx,
                    );
                }
                Err(error) => {
                    this.error_message = Some(message(error));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Over the changes of a worktree where a cherry-pick stopped: what to
    /// do, and the ways on.
    pub(super) fn picking_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.picking {
            return None;
        }
        let path = self.staging_worktree()?;
        let button = |id: &'static str, label: &'static str, step: &'static str| {
            let line = popups::cherry_pick_step(&path, step);
            div()
                .id(id)
                .px(px(10.))
                .py(px(2.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.separator)
                .bg(theme.background)
                .cursor_pointer()
                .hover(|button| button.bg(theme.selection))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_in_terminal(line.clone(), window, cx)
                }))
        };
        Some(
            div()
                .flex()
                .flex_none()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(6.))
                .border_b_1()
                .border_color(theme.separator)
                .bg(theme.error.opacity(0.08))
                .text_sm()
                .child(
                    div().flex_1().min_w_0().text_color(theme.foreground).child(
                        "Cherry-pick stopped: fix the conflicts, stage them, then Continue.",
                    ),
                )
                .child(button("pick-continue", "Continue", "--continue"))
                .child(button("pick-skip", "Skip Commit", "--skip"))
                .child(button("pick-abort", "Abort", "--abort"))
                .into_any_element(),
        )
    }

    fn run_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, position)) = self.context_menu.take() else {
            return;
        };
        // A section's newest commit stands for it where one commit is meant.
        let target = match (target, command) {
            (
                Target::Commits {
                    commits,
                    section: true,
                    ..
                },
                Command::CheckoutIn(_) | Command::NewBranch | Command::NewWorktree,
            ) => match commits.last() {
                Some((commit, message)) => Target::Commit {
                    commit: commit.clone(),
                    message: message.clone(),
                },
                None => return,
            },
            (target, _) => target,
        };
        let picks = target.picks();
        if let Target::Group { remote } = target {
            let section = if remote {
                Section::Remote
            } else {
                Section::Local
            };
            self.attention_group(section, command, cx);
            return;
        }
        if let Target::File { name, path, .. } = &target {
            return self.file_command(name, path, command, cx);
        }
        if let Target::Facts { facts } = &target {
            let text = match command {
                Command::CopyFact(k) => facts.get(k).map(|(_, value)| value.clone()),
                _ => None,
            };
            if let Some(text) = text {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
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
            Target::Group { .. } | Target::File { .. } | Target::Facts { .. } => return,
            Target::Commits { .. } | Target::PickBranches { .. } => (None, None, String::new()),
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
            Command::CopyHashes => copy(picks.join("\n"), cx),
            Command::CopyMessages => {
                if let Target::Commits { commits, .. } = &target {
                    let firsts: Vec<&str> = commits
                        .iter()
                        .map(|(_, message)| message.lines().next().unwrap_or_default())
                        .collect();
                    copy(firsts.join("\n"), cx)
                }
            }
            Command::Title => {}
            Command::CherryPick(k) => {
                if let Some(path) = self.pick_places.get(k).and_then(|pick| pick.path.clone()) {
                    let merges = wtf_core::has_merges(&repo, &picks);
                    self.run_in_terminal(popups::cherry_pick(&path, &picks, merges), window, cx);
                }
            }
            Command::CherryPickInto => {
                self.open_pick_branches(picks, position, cx);
                return;
            }
            Command::CherryPickOnto(k) => {
                if let Some(pick) = self.pick_branches.get(k) {
                    let branch = pick.place.name.clone();
                    self.pick_onto_branch(branch, picks, window, cx);
                }
            }
            // Files only, handled above.
            Command::CopyRelativePath | Command::Open | Command::CopyFact(_) => {}
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
                // By its name alone, git makes a local branch tracking it.
                Target::Branch { name, .. } | Target::Remote { branch: name, .. } => {
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
            Command::SelectWorktree(k) => {
                let Some((path, _)) = self.checkout_worktrees(&target).get(k).cloned() else {
                    return;
                };
                let index = self
                    .graph
                    .nodes
                    .iter()
                    .position(|node| matches!(&node.tip, Revision::Worktree(p) if *p == path));
                if let Some(index) = index {
                    self.select_and_reveal(index, cx);
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
                let gone = matches!(
                    target,
                    Target::Worktree {
                        stale: true,
                        leaf: None | Some(false),
                        ..
                    }
                );
                if let (Some(path), Some(name)) = (path, name) {
                    let detail = match merged {
                        false if gone => {
                            "Its remote branch is gone, as after a pull request is merged. \
                             Commits kept only here are lost: a squashed or rebased merge \
                             has them in another form."
                        }
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

    /// Lists what looks stale, worktrees, branches and stashes, merged
    /// branches ticked to go.
    pub(super) fn open_prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prune = Some(self.prune_items());
        self.prune_focus.focus(window);
        self.measure_prune_worktrees(cx);
        cx.notify();
    }

    /// The folders of the worktrees the prune dialog offers.
    fn prune_paths(&self) -> Vec<(String, String)> {
        self.prune
            .iter()
            .flatten()
            .filter(|item| item.kind == PruneKind::Worktree)
            .filter_map(|item| match &self.prune_worktree(&item.name)?.tip {
                Revision::Worktree(path) => Some((item.name.clone(), path.clone())),
                Revision::Commit(_) => None,
            })
            .collect()
    }

    /// Measures each offered worktree's folder on disk, one after another,
    /// for the dialog to say what deleting it frees.
    fn measure_prune_worktrees(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .prune_paths()
            .into_iter()
            .map(|(_, path)| path)
            .filter(|path| !self.prune_sizes.contains_key(path))
            .collect();
        cx.spawn(async move |this, cx| {
            for path in paths {
                let measured = path.clone();
                let size = cx
                    .background_executor()
                    .spawn(async move { folder_size(&measured) })
                    .await;
                let Some(size) = size else {
                    continue;
                };
                if this
                    .update(cx, |this, cx| {
                        this.prune_sizes.insert(path, size);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// The size on disk of a ticked worktree's folder, once measured.
    fn prune_size(&self, name: &str) -> Option<u64> {
        let path = match &self.prune_worktree(name)?.tip {
            Revision::Worktree(path) => path,
            Revision::Commit(_) => return None,
        };
        self.prune_sizes.get(path).copied()
    }

    /// Watches the folders a prune removes in the terminal; once they are
    /// gone, or it gives up waiting, says how much was freed.
    fn watch_prune(&mut self, folders: Vec<(String, u64)>, cx: &mut Context<Self>) {
        if folders.is_empty() {
            return;
        }
        self.prune_watch = Some(cx.spawn(async move |this, cx| {
            let gone = |folders: &[(String, u64)]| {
                folders
                    .iter()
                    .filter(|(path, _)| !std::path::Path::new(path).exists())
                    .map(|(_, size)| *size)
                    .collect::<Vec<u64>>()
            };
            // A few minutes at most: what is still there by then stays.
            for _ in 0..PRUNE_WAIT_SECONDS {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if gone(&folders).len() == folders.len() {
                    break;
                }
            }
            let freed = gone(&folders);
            this.update(cx, |this, cx| {
                this.prune_watch = None;
                let text = match freed.len() {
                    0 => "Prune removed no worktree folder".to_string(),
                    n => format!(
                        "Prune freed {} ({n} worktree{})",
                        human_size(freed.iter().sum()),
                        if n == 1 { "" } else { "s" }
                    ),
                };
                this.show_notice(text, cx);
            })
            .ok();
        }));
    }

    /// A notice at the foot of the window, for a while.
    pub(super) fn show_notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.notice = Some(text);
        self.notice_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE_TIME).await;
            this.update(cx, |this, cx| {
                this.notice = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(super) fn notice_view(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = self.notice.clone()?;
        let panel = div()
            .id("notice")
            .occlude()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(14.))
            .py(px(8.))
            .rounded(px(8.))
            .map(|panel| floating(panel, theme))
            .text_sm()
            .child(text)
            .child(
                div()
                    .id("notice-close")
                    .px(px(4.))
                    .text_color(theme.secondary)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    }))
                    .child("×"),
            );
        Some(
            div()
                .absolute()
                .bottom(px(24.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(panel)
                .into_any_element(),
        )
    }

    /// What the prune dialog offers, in its sections' order: worktrees with
    /// their branches, branches alone, then stashes; each with whether it
    /// starts ticked. Merged branches start ticked; the rest, idle or with a
    /// gone remote, may still hold work, and stashes are work set aside, so
    /// they wait to be ticked.
    pub(super) fn prune_items(&self) -> Vec<PruneItem> {
        let status_of = |name: &str| {
            let branch = self.graph.branches.iter().find(|b| b.name == name);
            let has = |reason: &str| branch.is_some_and(|b| b.stale.iter().any(|r| r == reason));
            match (branch, has("merged"), has("remote gone")) {
                (_, true, _) => PruneStatus::Merged,
                // A branch that is no more is as gone as its remote one.
                (None, _, _) | (_, _, true) => PruneStatus::Gone,
                _ => PruneStatus::Idle,
            }
        };
        let mut items: Vec<PruneItem> = self
            .graph
            .branches
            .iter()
            .filter(|branch| !branch.remote && !branch.unrelated && branch.name != self.graph.main)
            .filter(|branch| graph_view::stale_note(&self.graph, &branch.name).is_some())
            .map(|branch| {
                let status = status_of(&branch.name);
                PruneItem {
                    kind: match self.prune_worktree(&branch.name).is_some() {
                        true => PruneKind::Worktree,
                        false => PruneKind::Branch,
                    },
                    name: branch.name.clone(),
                    status,
                    on: status == PruneStatus::Merged,
                }
            })
            .collect();
        // A stash goes by its branch: merged or gone, its work is likely in;
        // else by its age.
        let now = now_seconds();
        let idle = i64::from(
            self.graph
                .idle_days
                .unwrap_or(preferences::DEFAULT_IDLE_DAYS),
        ) * 86_400;
        items.extend(self.graph.stashes.iter().map(|stash| {
            let status = match stash.branch.as_deref().map(status_of) {
                Some(status @ (PruneStatus::Merged | PruneStatus::Gone)) => status,
                _ if now - stash.time >= idle => PruneStatus::Idle,
                _ => PruneStatus::Recent,
            };
            PruneItem {
                kind: PruneKind::Stash,
                name: stash.name.clone(),
                status,
                on: false,
            }
        }));
        // Worktrees, branches, then stashes; in each, merged first.
        items.sort_by_key(|item| (item.kind, item.status));
        items
    }

    /// The worktree that has a branch the prune dialog offers checked out.
    fn prune_worktree(&self, name: &str) -> Option<&wtf_core::GraphNode> {
        self.graph.nodes.iter().find(|node| {
            node.kind == wtf_core::NodeKind::Uncommitted && node.branch.as_deref() == Some(name)
        })
    }

    fn close_prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prune = None;
        self.focus.focus(window);
        cx.notify();
    }

    /// Deletes what is ticked, in the terminal: each worktree, then its
    /// branch, a merged one as git checks it, others with their commits;
    /// then the stashes, the last first, so the others keep their names.
    /// One that fails leaves the rest to go on.
    fn run_prune(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(list), Some(repo)) = (self.prune.take(), self.repo_path.clone()) else {
            return;
        };
        let ticked: Vec<PruneItem> = list.into_iter().filter(|item| item.on).collect();
        // The worktree folders going, with their sizes, to say what is freed.
        let folders: Vec<(String, u64)> = ticked
            .iter()
            .filter(|item| item.kind == PruneKind::Worktree)
            .filter_map(|item| {
                let size = self.prune_size(&item.name).unwrap_or(0);
                match &self.prune_worktree(&item.name)?.tip {
                    Revision::Worktree(path) => Some((path.clone(), size)),
                    Revision::Commit(_) => None,
                }
            })
            .collect();
        let mut lines: Vec<String> = ticked
            .iter()
            .filter(|item| item.kind != PruneKind::Stash)
            .filter_map(|item| {
                let name = &item.name;
                let branch = self.graph.branches.iter().find(|b| b.name == *name)?;
                let merged = branch.stale.iter().any(|reason| reason == "merged");
                let delete = match merged {
                    true => popups::delete_branch(&repo, name),
                    false => popups::delete_branch_with_commits(&repo, name),
                };
                Some(match self.prune_worktree(name).map(|node| &node.tip) {
                    Some(Revision::Worktree(path)) => {
                        format!("{} && {delete}", popups::remove_worktree(&repo, path))
                    }
                    _ => delete,
                })
            })
            .collect();
        let mut stashes: Vec<&str> = ticked
            .iter()
            .filter(|item| item.kind == PruneKind::Stash)
            .map(|item| item.name.as_str())
            .collect();
        stashes.sort_by_key(|name| std::cmp::Reverse(stash_index(name)));
        lines.extend(
            stashes
                .into_iter()
                .map(|name| popups::stash_drop(&repo, name)),
        );
        self.focus.focus(window);
        if !lines.is_empty() {
            self.run_in_terminal(lines.join(" ; "), window, cx);
            self.watch_prune(folders, cx);
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
    /// A prune dialog section's heading, folding its section on a click;
    /// its checkbox ticks all its rows, or clears them when all are.
    #[allow(clippy::too_many_arguments)]
    fn prune_header(
        &self,
        fold: PruneFold,
        title: String,
        count: String,
        top: bool,
        group: Vec<usize>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folded = self.prune_folds.contains(&fold);
        let list = self.prune.as_deref().unwrap_or_default();
        let picked = group
            .iter()
            .filter(|&&k| list.get(k).is_some_and(|item| item.on))
            .count();
        let state = match picked {
            0 => Some(false),
            n if n == group.len() => Some(true),
            _ => None,
        };
        let checkbox = (!group.is_empty()).then(|| {
            attention_checkbox(
                SharedString::from(format!("prune-all-{fold:?}")),
                state,
                true,
                theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                let on = state != Some(true);
                if let Some(list) = this.prune.as_mut() {
                    for &k in &group {
                        if let Some(item) = list.get_mut(k) {
                            item.on = on;
                        }
                    }
                }
                cx.notify();
            }))
        });
        let kind_level = fold.1.is_none();
        div()
            .id(SharedString::from(format!("prune-fold-{fold:?}")))
            .flex()
            .items_center()
            .gap(px(4.))
            .pt(px(match (kind_level, top) {
                (true, true) => 0.,
                (true, false) => 10.,
                (false, _) => 4.,
            }))
            .pb(px(2.))
            .pl(px(if kind_level { 2. } else { 16. }))
            .pr(px(8.))
            .text_xs()
            .cursor_pointer()
            .when(kind_level, |header| {
                header
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.foreground)
            })
            .when(!kind_level, |header| header.text_color(theme.secondary))
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.prune_folds.remove(&fold) {
                    this.prune_folds.insert(fold);
                }
                cx.notify();
            }))
            .child(panel_chevron(!folded, theme))
            .children(checkbox)
            .child(div().flex_1().child(title))
            .child(div().text_color(theme.secondary).child(count))
            .into_any_element()
    }

    /// A row of the prune dialog, ticked or not by a click: a worktree by
    /// its folder with its branch beside it, a branch by its name, a stash
    /// by what it says with the branch it was made on; why it looks stale,
    /// or for a stash its place and age, after.
    fn prune_row(&self, k: usize, now: i64, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.prune.as_ref().and_then(|list| list.get(k)) else {
            return div().into_any_element();
        };
        let (name, on) = (&item.name, item.on);
        let (title, beside, note) = match item.kind {
            PruneKind::Stash => {
                let stash = self.graph.stashes.iter().find(|stash| stash.name == *name);
                match stash {
                    Some(stash) => (
                        stash_words(&stash.message),
                        Some(stash.branch.clone().unwrap_or_else(|| "detached".into())),
                        format!("{} · {}", name, short_age(stash.time, now)),
                    ),
                    None => (name.clone(), None, String::new()),
                }
            }
            kind => {
                let note = graph_view::stale_note(&self.graph, name).unwrap_or_default();
                match self
                    .prune_worktree(name)
                    .filter(|_| kind == PruneKind::Worktree)
                {
                    // A worktree says how much room its folder takes.
                    Some(node) => {
                        let size = self
                            .prune_size(name)
                            .map_or("measuring…".to_string(), human_size);
                        let note = match note.is_empty() {
                            true => size,
                            false => format!("{size} · {note}"),
                        };
                        (
                            graph_view::worktree_label(node, WorktreeNames::Folder),
                            Some(name.clone()),
                            note,
                        )
                    }
                    None => (name.clone(), None, note),
                }
            }
        };
        div()
            .id(("prune", k))
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(26.))
            .pl(px(20.))
            .pr(px(8.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|row| row.bg(theme.selection.opacity(0.5)))
            .when(!on, |row| row.text_color(theme.secondary))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(item) = this.prune.as_mut().and_then(|list| list.get_mut(k)) {
                    item.on = !item.on;
                    cx.notify();
                }
            }))
            .child(attention_checkbox(
                ("prune-check", k),
                Some(on),
                true,
                theme,
            ))
            .child(div().min_w_0().truncate().child(title))
            .children(beside.map(|beside| {
                div()
                    .flex_none()
                    .px(px(5.))
                    .rounded(px(3.))
                    .bg(theme.tag_background)
                    .text_xs()
                    .child(beside)
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
            .into_any_element()
    }

    pub(super) fn prune_view(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let list = self.prune.as_ref()?;
        let ticked = list.iter().filter(|item| item.on).count();
        // What deleting frees: the ticked worktrees' folders, as measured.
        let freed: u64 = list
            .iter()
            .filter(|item| item.on && item.kind == PruneKind::Worktree)
            .filter_map(|item| self.prune_size(&item.name))
            .sum();
        let delete_label = match freed {
            0 => format!("Delete {ticked}"),
            bytes => format!("Delete {ticked} · frees {}", human_size(bytes)),
        };
        let now = now_seconds();
        // A heading for each kind, there with none too, and under it one
        // for each status; each folds with a click, as the attention
        // panel's do.
        let mut rows: Vec<AnyElement> = Vec::new();
        for (n, kind) in [PruneKind::Worktree, PruneKind::Branch, PruneKind::Stash]
            .into_iter()
            .enumerate()
        {
            let of_kind: Vec<usize> = (0..list.len()).filter(|&k| list[k].kind == kind).collect();
            let picked = of_kind.iter().filter(|&&k| list[k].on).count();
            let title = match kind {
                PruneKind::Worktree => "Worktrees, with their branches",
                PruneKind::Branch => "Branches",
                PruneKind::Stash => "Stashes",
            };
            let count = match picked {
                0 => of_kind.len().to_string(),
                picked => format!("{picked} of {}", of_kind.len()),
            };
            rows.push(self.prune_header(
                (kind, None),
                title.to_string(),
                count,
                n == 0,
                of_kind.clone(),
                theme,
                cx,
            ));
            if self.prune_folds.contains(&(kind, None)) {
                continue;
            }
            if of_kind.is_empty() {
                rows.push(
                    div()
                        .pl(px(20.))
                        .py(px(2.))
                        .text_xs()
                        .italic()
                        .text_color(theme.secondary)
                        .child(match kind {
                            PruneKind::Worktree => "No stale worktrees",
                            PruneKind::Branch => "No stale branches",
                            PruneKind::Stash => "No stashes",
                        })
                        .into_any_element(),
                );
                continue;
            }
            for status in [
                PruneStatus::Merged,
                PruneStatus::Gone,
                PruneStatus::Idle,
                PruneStatus::Recent,
            ] {
                let of_status: Vec<usize> = of_kind
                    .iter()
                    .copied()
                    .filter(|&k| list[k].status == status)
                    .collect();
                if of_status.is_empty() {
                    continue;
                }
                let title = match status {
                    PruneStatus::Merged => "Merged",
                    PruneStatus::Gone => "Gone from the remote",
                    PruneStatus::Idle => "Idle",
                    PruneStatus::Recent => "Recent",
                };
                rows.push(self.prune_header(
                    (kind, Some(status)),
                    title.to_string(),
                    of_status.len().to_string(),
                    false,
                    of_status.clone(),
                    theme,
                    cx,
                ));
                if self.prune_folds.contains(&(kind, Some(status))) {
                    continue;
                }
                rows.extend(
                    of_status
                        .into_iter()
                        .map(|k| self.prune_row(k, now, theme, cx)),
                );
            }
        }
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
                        .child("Prune stale worktrees, branches and stashes"),
                )
                .child(div().text_xs().text_color(theme.secondary).child(
                    "Tick what goes. Worktrees go first, then their branches: merged ones as \
                 git checks them, others with their commits. Stashes are dropped.",
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
                                for item in this.prune.iter_mut().flatten() {
                                    item.on = on;
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
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        // What counts as merged: the repository's channels.
                        .child(button("prune-channels", "Channels…".to_string()).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.close_prune(window, cx);
                                if let Some(repo) = this.repo_path.clone() {
                                    preferences::open_channels(repo, cx);
                                }
                            }),
                        ))
                        .child(div().flex_1())
                        .child(button("prune-cancel", "Cancel".to_string()).on_click(
                            cx.listener(|this, _, window, cx| this.close_prune(window, cx)),
                        ))
                        .when(ticked > 0, |buttons| {
                            buttons.child(
                                button("prune-run", delete_label.clone())
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

    /// Notes what the palette ran, for the top of its list next time.
    fn remember_palette_run(&mut self, kind: &str, what: &str) {
        popups::remember(&mut self.palette_recent, &format!("{kind}\t{what}"));
        recent::save_list("palette-recent", &self.palette_recent);
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
        // The last few run come first, the app's commands and the shell's
        // alike, as many as Settings ask; then the app's commands, then the
        // shell's run before, for a search to find.
        let app = palette::entries(window);
        let mut top: Vec<goto::Entry> = Vec::new();
        for run in self
            .palette_recent
            .iter()
            .take(preferences::palette_recent())
        {
            match run.split_once('\t') {
                Some(("app", name)) => {
                    top.extend(app.iter().find(|entry| entry.label == name).cloned())
                }
                Some(("sh", command)) => {
                    top.extend(path.as_ref().map(|path| goto::shell_entry(command, path)))
                }
                _ => {}
            }
        }
        let shown = |entry: &goto::Entry| top.iter().any(|run| run.label == entry.label);
        let rest_app: Vec<goto::Entry> = app.iter().filter(|e| !shown(e)).cloned().collect();
        let rest_shell: Vec<goto::Entry> = path
            .iter()
            .flat_map(|path| {
                self.shell_commands
                    .iter()
                    .map(move |command| goto::shell_entry(command, path))
            })
            .filter(|entry| !shown(entry))
            .collect();
        let entries: Vec<goto::Entry> = top.into_iter().chain(rest_app).chain(rest_shell).collect();
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
            if let Some((name, action)) = palette::commands().into_iter().nth(entry.node) {
                self.remember_palette_run("app", name);
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
            self.remember_palette_run("sh", command);
            return;
        }
        if entry.kind == goto::Kind::Hidden {
            return self.show_hidden_branch(&entry.label, cx);
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
                goto::Kind::Hidden => ("⎇", "hidden branch · shows it"),
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

    /// The menu for a changed file: its path in the repository and where it
    /// is in the worktree the diff is of, or the tab's own.
    pub(super) fn file_target(&self, index: usize) -> Option<Target> {
        self.path_target(self.files.get(index)?.path())
    }

    /// The menu for a file or folder by its path in the repository, as
    /// `file_target`'s.
    pub(super) fn path_target(&self, name: &str) -> Option<Target> {
        let name = name.trim_end_matches('/').to_string();
        let root = match &self.range {
            Some((_, Revision::Worktree(path))) => Some(path.clone()),
            _ => self
                .current_worktree_path()
                .map(str::to_string)
                .or_else(|| self.repo_path.clone()),
        }?;
        let path = Path::new(&root).join(&name);
        Some(Target::File {
            exists: path.exists(),
            path: path.to_string_lossy().into_owned(),
            name,
        })
    }

    fn file_command(&mut self, name: &str, path: &str, command: Command, cx: &mut Context<Self>) {
        let copy = |text: String, cx: &mut Context<Self>| {
            cx.write_to_clipboard(ClipboardItem::new_string(text))
        };
        let open = |args: &[&str]| std::process::Command::new("open").args(args).spawn();
        let opened = match command {
            Command::CopyName => return copy(file_name(name), cx),
            Command::CopyRelativePath => return copy(name.to_string(), cx),
            Command::CopyPath => return copy(path.to_string(), cx),
            Command::Open => open(&[path]),
            Command::RevealInFinder => open(&["-R", path]),
            _ => return,
        };
        if let Err(error) = opened {
            self.error_message = Some(format!("Cannot open {name}: {error}"));
            cx.notify();
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
            // Not one with the branch already.
            .find(|(path, place)| path.trim_end_matches('/') == here && !place.has);
        if let Some((path, _)) = found {
            self.check_out(target, &path, window, cx);
        }
    }

    /// The worktrees a menu sends commands to, the current one first, each
    /// by its folder; for a branch, marked where it is checked out already.
    fn checkout_worktrees(&self, target: &Target) -> Vec<(String, popups::Place)> {
        let name = match target {
            Target::Branch { name, .. } => Some(name),
            Target::Stash { .. }
            | Target::Commit { .. }
            | Target::Commits { .. }
            | Target::Remote { .. } => None,
            _ => return Vec::new(),
        };
        let current = self
            .current_worktree_path()
            .or(self.repo_path.as_deref())
            .map(|path| path.trim_end_matches('/').to_string());
        let mut places: Vec<(String, popups::Place)> = self
            .graph
            .nodes
            .iter()
            .filter(|node| node.kind == wtf_core::NodeKind::Uncommitted)
            .filter_map(|node| match &node.tip {
                Revision::Worktree(path) => {
                    let is_current = current.as_deref() == Some(path.trim_end_matches('/'));
                    let has = name.is_some() && node.branch.as_ref() == name;
                    let place =
                        popups::Place::new(&crate::workspace::folder_name(path), is_current, has);
                    Some((path.clone(), place))
                }
                Revision::Commit(_) => None,
            })
            .collect();
        places.sort_by_key(|(_, place)| !place.current);
        places
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
            // A click on an item stays in the menu, off what is under it.
            .occlude()
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
        let worktrees: Vec<popups::Place> = self
            .checkout_worktrees(target)
            .into_iter()
            .map(|(_, place)| place)
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
            &match target {
                Target::PickBranches { .. } => &self.pick_branches,
                _ => &self.pick_places,
            }
            .iter()
            .map(|pick| pick.place.clone())
            .collect::<Vec<_>>(),
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
            let label = popups::Label {
                text: label.to_string(),
                place: None,
                danger: false,
            };
            items.insert(at, Some((Command::ToggleChannel, label)));
        }
        // What a cherry-pick would do there, once worked out.
        let pick_hint = |pick: Option<&PickPlace>| {
            pick.map(|pick| pick.hint.clone().unwrap_or_else(|| "…".to_string()))
        };
        for (k, item) in items.into_iter().enumerate() {
            menu = match item {
                None => menu.child(div().h(px(1.)).my(px(4.)).bg(theme.separator)),
                // The first line names what the menu acts on.
                Some((Command::Title, label)) => menu.child(
                    div()
                        .px(px(12.))
                        .py(px(3.))
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.secondary)
                        .child(label.text.to_uppercase()),
                ),
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
                        .child(menu_label(
                            &label,
                            match command {
                                Command::CherryPick(k) => pick_hint(self.pick_places.get(k)),
                                Command::CherryPickOnto(k) => pick_hint(self.pick_branches.get(k)),
                                _ => copy_hint(target, command),
                            },
                            theme,
                        )),
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
        };
        let hint = "Enter to run in the terminal · Esc to cancel";
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

/// A menu item's label: its name, red when it deletes, and the worktree it
/// acts in on the right, in bold when it is the current one.
/// What a copy command of the menu copies, as its right side shows it: the
/// name, the hash, the message's first line or the path, cut short.
fn copy_hint(target: &Target, command: Command) -> Option<String> {
    let (name, path, commit, message) = match target {
        Target::Branch { name, commit, .. } => (Some(name.clone()), None, commit.clone(), None),
        Target::Commit { commit, message } => (None, None, commit.clone(), Some(message.clone())),
        Target::Remote {
            remote,
            branch,
            commit,
        } => (
            Some(format!("{remote}/{branch}")),
            None,
            commit.clone(),
            None,
        ),
        Target::Tag { name, commit } => (Some(name.clone()), None, commit.clone(), None),
        Target::Stash { name, id } => (Some(name.clone()), None, id.clone(), None),
        Target::Worktree {
            path,
            branch,
            commit,
            ..
        } => (branch.clone(), Some(path.clone()), commit.clone(), None),
        Target::File { name, path, .. } => {
            let text = match command {
                Command::CopyName => file_name(name),
                Command::CopyRelativePath => name.clone(),
                Command::CopyPath => popups::tilde(path),
                _ => return None,
            };
            return Some(cut_short(text));
        }
        Target::Facts { facts } => {
            let text = match command {
                Command::CopyFact(k) => facts.get(k)?.1.lines().next()?.to_string(),
                _ => return None,
            };
            return Some(cut_short(text));
        }
        Target::Group { .. } | Target::PickBranches { .. } => return None,
        Target::Commits { commits, .. } => {
            let lines = |n: usize| format!("{n} line{}", if n == 1 { "" } else { "s" });
            return match command {
                Command::CopyHashes | Command::CopyMessages => Some(lines(commits.len())),
                _ => None,
            };
        }
    };
    let text = match command {
        Command::CopyName => name?,
        // The full hash runs long: its start, and that it goes on.
        Command::CopyHash => format!("{}…", popups::short_hash(&commit)),
        Command::CopyShortHash => popups::short_hash(&commit),
        Command::CopyMessage => message?.lines().next().unwrap_or_default().to_string(),
        Command::CopyPath => popups::tilde(&path?),
        _ => return None,
    };
    Some(cut_short(text))
}

/// A copy hint cut to fit the menu.
fn cut_short(text: String) -> String {
    let cut = 36;
    match text.chars().count() > cut {
        true => format!("{}…", text.chars().take(cut - 1).collect::<String>()),
        false => text,
    }
}

/// The last part of a path in the repository.
fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn menu_label(label: &popups::Label, hint: Option<String>, theme: &Theme) -> AnyElement {
    let color = if label.danger {
        theme.error
    } else {
        theme.foreground
    };
    let row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(24.))
        .whitespace_nowrap()
        .child(div().text_color(color).child(label.text.clone()));
    // What a copy takes, or how a cherry-pick would go, on the right:
    // conflicts in red, a clean one in green.
    let mut right = div().flex().items_center().gap(px(10.));
    if let Some(hint) = hint {
        let color = match hint.as_str() {
            "clean" => super::header::ADDED,
            hint if hint.ends_with("conflict") || hint.ends_with("conflicts") => theme.error,
            _ => theme.secondary,
        };
        right = right.child(div().text_color(color).child(hint));
    }
    // A worktree, as the graph's list shows one: its folder on a chip,
    // filled for the current one.
    if let Some(place) = &label.place {
        let chip = div()
            .px(px(6.))
            .rounded(px(4.))
            .text_xs()
            .child(format!("⌂ {}", place.name));
        right = right.child(match place.current {
            true => chip
                .bg(theme.accent)
                .text_color(gpui::white())
                .font_weight(gpui::FontWeight::SEMIBOLD),
            false => chip.bg(theme.tag_background).text_color(theme.foreground),
        });
    }
    row.child(right).into_any_element()
}

/// A section of the prune dialog that folds: a kind, or a status in it.
pub(super) type PruneFold = (PruneKind, Option<PruneStatus>);

/// What a stash says, as git words it less its lead: "WIP on main: 1a2b3c
/// Fix it" reads "Fix it", "On main: try this" reads "try this".
fn stash_words(message: &str) -> String {
    let rest = message
        .strip_prefix("WIP on ")
        .or_else(|| message.strip_prefix("On "))
        .and_then(|rest| rest.split_once(": ").map(|(_, rest)| rest));
    let Some(rest) = rest else {
        return message.to_string();
    };
    // A WIP stash names the commit it was made on first.
    let words = match message.starts_with("WIP on ") {
        true => rest.split_once(' ').map_or(rest, |(_, subject)| subject),
        false => rest,
    };
    match message.starts_with("WIP on ") {
        true => format!("Work on: {words}"),
        false => words.to_string(),
    }
}

/// What a prune dialog row deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum PruneKind {
    /// A worktree, and then its branch.
    Worktree,
    Branch,
    Stash,
}

/// Why a prune dialog row looks stale, in the order the rows go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum PruneStatus {
    /// A channel has its work: safe to go.
    Merged,
    /// Its remote branch, or for a stash its branch, is no more.
    Gone,
    /// Left alone long.
    Idle,
    /// A stash made lately.
    Recent,
}

/// A row of the prune dialog: what, by its name, why, and whether it is
/// ticked.
#[derive(Debug, Clone)]
pub(super) struct PruneItem {
    pub kind: PruneKind,
    pub name: String,
    pub status: PruneStatus,
    pub on: bool,
}

/// How long a prune is watched for its folders to go.
const PRUNE_WAIT_SECONDS: u32 = 300;
/// How long a notice stays.
const NOTICE_TIME: Duration = Duration::from_secs(10);

/// A folder's size on disk, as `du` counts it, in bytes.
fn folder_size(path: &str) -> Option<u64> {
    let out = std::process::Command::new("du")
        .args(["-sk", path])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let kilobytes: u64 = text.split_whitespace().next()?.parse().ok()?;
    Some(kilobytes * 1024)
}

/// A size as people read it: `840 KB`, `312 MB`, `1.2 GB`.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    let mut size = bytes as f64 / 1024.;
    let mut unit = 0;
    while size >= 1024. && unit + 1 < UNITS.len() {
        size /= 1024.;
        unit += 1;
    }
    match size < 10. && unit > 0 {
        true => format!("{size:.1} {}", UNITS[unit]),
        false => format!("{size:.0} {}", UNITS[unit]),
    }
}

/// A stash's place in the list, from its name: 2 for `stash@{2}`.
fn stash_index(name: &str) -> usize {
    name.trim_start_matches("stash@{")
        .trim_end_matches('}')
        .parse()
        .unwrap_or(0)
}

#[cfg(test)]
mod prune_tests {
    use super::{human_size, stash_words};

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(human_size(840 * 1024), "840 KB");
        assert_eq!(human_size(312 * 1024 * 1024), "312 MB");
        assert_eq!(human_size(1_288_490_189), "1.2 GB");
    }

    #[test]
    fn stashes_read_without_git_s_lead() {
        assert_eq!(
            stash_words("WIP on main: 1a2b3c4 Fix the parser"),
            "Work on: Fix the parser"
        );
        assert_eq!(stash_words("On feature/x: try this"), "try this");
        assert_eq!(stash_words("something else"), "something else");
    }
}
