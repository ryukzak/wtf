//! The all-files view: every file's diff in one list, with sections and hunks.

use super::*;

impl RepoView {
    /// Loads every file's diff for the all-files view.
    pub(super) fn load_stacked(&mut self, refresh: bool, cx: &mut Context<Self>) {
        let Some(repo_path) = self.repo_path.clone() else {
            return;
        };
        if !refresh {
            self.stacked.clear();
            self.stacked_diffs.clear();
            self.stacked_files.clear();
            self.stacked_scroll.scroll_to_item(0, ScrollStrategy::Top);
            self.is_loading_stacked = true;
        }
        let (files, mode) = (self.files.clone(), self.diff_mode);
        // With too many files to open at once, folded ones wait to be opened
        // before their diffs are worked out.
        let skip: Vec<bool> = files
            .iter()
            .map(|file| self.many_files() && self.is_folded(file.path()))
            .collect();
        let load = cx.background_executor().spawn(async move {
            let diffs = files
                .iter()
                .zip(skip)
                .map(|(file, skip)| match skip {
                    true => Err(NOT_LOADED.to_string()),
                    false => {
                        wtf_core::file_diff(repo_path.clone(), file.clone(), mode).map_err(message)
                    }
                })
                .collect::<Vec<_>>();
            (files, diffs)
        });
        self.stacked_task = Some(cx.spawn(async move |this, cx| {
            let (files, diffs) = load.await;
            this.update(cx, |this, cx| {
                this.is_loading_stacked = false;
                // Picked lines only stay picked while their file's diff holds.
                if let Some((file, _)) = &this.line_selection
                    && (this.stacked_diffs.get(*file) != diffs.get(*file)
                        || this.stacked_files.get(*file) != files.get(*file))
                {
                    this.line_selection = None;
                }
                this.stacked_diffs = diffs;
                this.stacked_files = files;
                this.syntax.borrow_mut().clear();
                this.restack();
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(super) fn restack(&mut self) {
        let folded: Vec<bool> = self
            .files
            .iter()
            .map(|file| self.is_folded(file.path()))
            .collect();
        self.stacked = stack_items(
            &self.stacked_diffs,
            &folded,
            self.worktree_split,
            self.folded_sections,
            self.many_files(),
        );
    }

    fn toggle_section(&mut self, staged: bool, cx: &mut Context<Self>) {
        let folded = &mut self.folded_sections[usize::from(staged)];
        *folded = !*folded;
        self.restack();
        cx.notify();
    }

    /// The files of the unstaged or staged section.
    fn section_files(&self, staged: bool) -> std::ops::Range<usize> {
        let split = self.worktree_split.unwrap_or(0).min(self.files.len());
        match staged {
            true => split..self.files.len(),
            false => 0..split,
        }
    }

    /// Folds every file of a section, or unfolds them all when all are folded.
    fn fold_section_files(&mut self, staged: bool, cx: &mut Context<Self>) {
        let paths: Vec<String> = self.files[self.section_files(staged)]
            .iter()
            .map(|file| file.path().to_string())
            .collect();
        let unfold = paths.iter().all(|path| self.is_folded(path));
        for path in paths {
            self.set_folded(path, !unfold);
        }
        self.after_fold(cx);
    }

    fn toggle_fold(&mut self, file: usize, cx: &mut Context<Self>) {
        let Some(path) = self.files.get(file).map(|file| file.path().to_string()) else {
            return;
        };
        let folded = self.is_folded(&path);
        self.set_folded(path, !folded);
        self.after_fold(cx);
    }

    /// Folds or opens a file; with many files, opening is what is noted.
    pub(super) fn set_folded(&mut self, path: String, fold: bool) {
        match (self.many_files(), fold) {
            (true, true) => self.unfolded.remove(&path),
            (true, false) => self.unfolded.insert(path),
            (false, true) => self.folded.insert(path),
            (false, false) => self.folded.remove(&path),
        };
    }

    /// Shows the folds; a file opened before its diff was worked out gets
    /// it now.
    pub(super) fn after_fold(&mut self, cx: &mut Context<Self>) {
        let missing = self
            .files
            .iter()
            .zip(&self.stacked_diffs)
            .any(|(file, diff)| {
                !self.is_folded(file.path())
                    && diff.as_ref().is_err_and(|error| error == NOT_LOADED)
            });
        self.restack();
        if missing {
            self.load_stacked(true, cx);
        }
        cx.notify();
    }

    /// Code colors for a file's diff: one of the all-files view by its index,
    /// or the one-file view's. Worked out once per file and appearance.
    pub(super) fn diff_colors(
        &self,
        file: Option<usize>,
        theme: &Theme,
    ) -> Option<Rc<syntax::Colors>> {
        let (path, diff) = match file {
            Some(file) => (
                self.files.get(file)?,
                self.stacked_diffs.get(file)?.as_ref().ok()?,
            ),
            None => (self.files.get(self.selected_file?)?, self.diff.as_ref()?),
        };
        let path = path.path().to_string();
        let dark = theme.dark;
        // A file staged in part is listed twice, its two diffs apart, so
        // the all-files view keys colors by entry rather than by path.
        let key = (path.clone(), dark, file);
        if let Some(colors) = self.syntax.borrow().get(&key) {
            return Some(colors.clone());
        }
        let colors = Rc::new(syntax::colors(&path, diff, dark));
        self.syntax.borrow_mut().insert(key, colors.clone());
        Some(colors)
    }

    /// A row of the all-files view. Rows can still name files and lines of
    /// the last listing while a new one loads; those draw empty.
    pub(super) fn stack_row(
        &self,
        item: &StackItem,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.stack_item_fits(item) {
            return div()
                .w_full()
                .h(px(diff_view::row_height()))
                .into_any_element();
        }
        let element = self.stack_row_unchecked(item, theme, cx);
        // Rows share one height, so a section's or file's header fills its
        // own row and the pad row above it, from where it is drawn.
        match item {
            StackItem::Header(_) | StackItem::Section(..) => {
                let height = diff_view::row_height();
                div()
                    .relative()
                    .w_full()
                    .h(px(height))
                    .child(
                        div()
                            .absolute()
                            .top(px(-height))
                            .left_0()
                            .right_0()
                            .child(element),
                    )
                    .into_any_element()
            }
            _ => element,
        }
    }

    /// Whether a row's file and lines are in the files and diffs now held.
    pub(super) fn stack_item_fits(&self, item: &StackItem) -> bool {
        // Rows are drawn only under the file their diff was made from.
        let rows = |file: usize| match self.stacked_diffs.get(file) {
            _ if self
                .files
                .get(file)
                .is_none_or(|f| self.stacked_files.get(file) != Some(f)) =>
            {
                None
            }
            Some(Ok(diff)) => Some(diff.rows.len()),
            Some(Err(_)) => Some(0),
            None => None,
        };
        match *item {
            StackItem::Header(file) => rows(file).is_some(),
            StackItem::Row(file, index) => rows(file).is_some_and(|n| index < n),
            StackItem::Hunk(file, _, end) => rows(file).is_some_and(|n| end <= n),
            _ => true,
        }
    }

    pub(super) fn stack_row_unchecked(
        &self,
        item: &StackItem,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = || {
            div()
                .w_full()
                .h(px(diff_view::row_height()))
                .overflow_hidden()
                .whitespace_nowrap()
        };
        // Sublime Merge's buttons: on sections for all files, on headers for one.
        // Shorter than a row, so buttons on rows next to each other never touch.
        let button = |id: SharedString, label: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .h(px(diff_view::row_height() - 3.))
                .line_height(px(diff_view::row_height() - 5.))
                .px(px(10.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme.separator)
                .font_family(crate::preferences::ui_font())
                // As large as the code they sit by, a little lighter.
                .text_size(px(crate::preferences::code_size() * 0.95))
                .hover(|button| button.bg(theme.selection))
                .cursor_pointer()
                .child(label)
        };
        // Section and file headers are two rows tall: their buttons get
        // room to match.
        let tall = |id: SharedString, label: &'static str| {
            button(id, label)
                .h(px(1.4 * diff_view::row_height()))
                .px(px(12.))
                .rounded(px(5.))
        };
        match item {
            &StackItem::Section(staged, count) => {
                let button = tall;
                let title = if staged {
                    "Staged Files"
                } else {
                    "Unstaged Files"
                };
                let label = if staged { "Unstage All" } else { "Stage All" };
                let section_folded = self.folded_sections[usize::from(staged)];
                let all_folded = self.files[self.section_files(staged)]
                    .iter()
                    .all(|file| self.is_folded(file.path()));
                let fold_label = if all_folded {
                    "Expand Files"
                } else {
                    "Collapse Files"
                };
                div()
                    .w_full()
                    .h(px(2. * diff_view::row_height()))
                    .whitespace_nowrap()
                    .border_t_1()
                    .border_color(theme.separator)
                    .id(SharedString::from(format!("section-row-{staged}")))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .font_family(crate::preferences::ui_font())
                    // The interface font reads smaller than code at one size,
                    // so titles go up to stand over the diff.
                    .text_size(px(crate::preferences::code_size() * 1.2))
                    .font_weight(gpui::FontWeight::BOLD)
                    .map(|section| {
                        section
                            .cursor_pointer()
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_section(staged, cx)),
                            )
                            .child(chevron(
                                !section_folded,
                                theme.secondary,
                                preferences::code_size(),
                            ))
                    })
                    .child(format!("{title} ({count})"))
                    .child(div().flex_1())
                    .when(count > 0 && !section_folded, |section| {
                        section.child(
                            button(
                                SharedString::from(format!("fold-files-{staged}")),
                                fold_label,
                            )
                            .font_weight(gpui::FontWeight::NORMAL)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.fold_section_files(staged, cx)
                                },
                            )),
                        )
                    })
                    .when(count > 0 && self.stash_view.is_none(), |section| {
                        section.child(
                            button(SharedString::from(format!("section-{staged}")), label)
                                .font_weight(gpui::FontWeight::NORMAL)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.stage(staged, Vec::new(), cx)
                                })),
                        )
                    })
                    .into_any_element()
            }
            &StackItem::Header(index) => {
                let button = tall;
                let file = &self.files[index];
                let folded = self.is_folded(file.path());
                // A stash shows read-only: no buttons.
                let staged = self
                    .worktree_split
                    .map(|split| index >= split)
                    .filter(|_| self.stash_view.is_none());
                let path = file.path().to_string();
                div()
                    .w_full()
                    .h(px(2. * diff_view::row_height()))
                    .whitespace_nowrap()
                    .id(("file-header", index))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .bg(theme.graph_background)
                    .border_t_1()
                    .border_color(theme.separator)
                    .font_family(crate::preferences::ui_font())
                    .text_size(px(crate::preferences::code_size() * 1.1))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_fold(index, cx)))
                    .child(chevron(!folded, theme.secondary, preferences::code_size()))
                    .child(
                        div()
                            .text_color(theme.secondary)
                            .child(match file.submodule {
                                // A submodule: its pointer, not a file.
                                true => "⊞".to_string(),
                                false => file.status.clone(),
                            }),
                    )
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(file.path().to_string()),
                    )
                    .when(file.submodule, |header| {
                        let open = path.clone();
                        header.child(
                            button(SharedString::from(format!("open-sub-{index}")), "Open")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_submodule(&open, cx)
                                })),
                        )
                    })
                    .children(staged.map(|staged| {
                        let label = if staged { "Unstage File" } else { "Stage File" };
                        let lines = match &self.line_selection {
                            Some((file, rows)) if *file == index => rows.len(),
                            _ => 0,
                        };
                        let lines_label = if staged {
                            "Unstage Lines"
                        } else {
                            "Stage Lines"
                        };
                        div()
                            .flex_1()
                            .flex()
                            .justify_end()
                            .gap(px(6.))
                            // Unstaged changes can be thrown away, picked
                            // lines or the whole file.
                            .when(lines > 0 && !staged, |buttons| {
                                buttons.child(
                                    button(
                                        SharedString::from(format!("discard-lines-{index}")),
                                        "Discard Lines",
                                    )
                                    .text_color(theme.error)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.discard_rows(index, Vec::new(), window, cx)
                                    }))
                                    .child(format!(" ({lines})")),
                                )
                            })
                            .when(!staged, |buttons| {
                                buttons.child(
                                    button(
                                        SharedString::from(format!("discard-{index}")),
                                        "Discard File",
                                    )
                                    .text_color(theme.error)
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.discard_file(index, window, cx)
                                        },
                                    )),
                                )
                            })
                            .when(lines > 0, |buttons| {
                                buttons.child(
                                    button(
                                        SharedString::from(format!("lines-{index}")),
                                        lines_label,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.stage_selected_lines(staged, cx)
                                    }))
                                    .child(format!(" ({lines})")),
                                )
                            })
                            .child(
                                button(SharedString::from(format!("stage-{index}")), label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.stage(staged, vec![path.clone()], cx)
                                    })),
                            )
                    }))
                    .into_any_element()
            }
            &StackItem::Row(file, index) => {
                let colors = self.diff_colors(Some(file), theme);
                match &self.stacked_diffs[file] {
                    Ok(diff) if self.worktree_split.is_some() && self.stash_view.is_none() => {
                        // Lines are picked for staging: a bar marks the selection.
                        let selected = matches!(&self.line_selection, Some((f, rows)) if *f == file && rows.contains(&index));
                        let staged = self.worktree_split.is_some_and(|split| file >= split);
                        let group = SharedString::from(format!("line-{file}-{index}"));
                        let changed = diff_view::is_change(&diff.rows[index]);
                        let label = if staged { "Unstage Line" } else { "Stage Line" };
                        let hovered = self.hovered_line.get() == Some((file, index));
                        div()
                            .id(("line", file * 100_000 + index))
                            .group(group.clone())
                            .relative()
                            .flex()
                            .w_full()
                            .cursor_pointer()
                            .on_hover(cx.listener(move |this, over: &bool, _, cx| {
                                let line = Some((file, index));
                                match over {
                                    true => this.hovered_line.set(line),
                                    false if this.hovered_line.get() == line => {
                                        this.hovered_line.set(None)
                                    }
                                    false => return,
                                }
                                cx.notify();
                            }))
                            // A changed line offers its own buttons while the
                            // mouse is over it; other lines carry nothing.
                            .when(changed && hovered, |line| {
                                line.child(
                                    div()
                                        .absolute()
                                        .top(px(2.))
                                        .right(px(8.))
                                        .flex()
                                        .gap(px(6.))
                                        .when(!staged, |buttons| {
                                            buttons.child(
                                                button(
                                                    SharedString::from(format!(
                                                        "discard-line-{file}-{index}"
                                                    )),
                                                    "Discard",
                                                )
                                                .h(px(diff_view::row_height() - 4.))
                                                .bg(theme.background)
                                                .text_color(theme.error)
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        this.discard_rows(
                                                            file,
                                                            vec![index],
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                )),
                                            )
                                        })
                                        .child(
                                            button(
                                                SharedString::from(format!(
                                                    "stage-line-{file}-{index}"
                                                )),
                                                label,
                                            )
                                            .h(px(diff_view::row_height() - 4.))
                                            .bg(theme.background)
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.stage_rows(file, vec![index], staged, cx)
                                                }),
                                            ),
                                        ),
                                )
                            })
                            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                this.select_line(file, index, event.modifiers(), cx)
                            }))
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(3.))
                                    .h_full()
                                    .when(selected, |bar| bar.bg(theme.accent)),
                            )
                            .child(
                                diff_view::row(&diff.rows[index], theme, colors.as_deref())
                                    .flex_1(),
                            )
                            .into_any_element()
                    }
                    Ok(diff) => diff_view::row(&diff.rows[index], theme, colors.as_deref())
                        .into_any_element(),
                    Err(_) => row().into_any_element(),
                }
            }
            &StackItem::Hunk(file, start, end) => {
                let staged = self.worktree_split.is_some_and(|split| file >= split);
                let label = if staged { "Unstage Hunk" } else { "Stage Hunk" };
                // The hunk's first lines on each side, as `@@` headers show them.
                let lines = match &self.stacked_diffs[file] {
                    Ok(diff) => {
                        let rows = &diff.rows[start..end];
                        let first = |side: fn(&wtf_core::DiffRow) -> Option<u32>| {
                            rows.iter()
                                .find_map(side)
                                .map_or("—".to_string(), |n| n.to_string())
                        };
                        format!(
                            "@@ −{} +{} @@",
                            first(|row| row.left.as_ref().map(|side| side.line_number)),
                            first(|row| row.right.as_ref().map(|side| side.line_number)),
                        )
                    }
                    Err(_) => String::new(),
                };
                row()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(diff_view::gutter_width()))
                    .pr(px(8.))
                    .bg(theme.filler)
                    .text_color(theme.secondary)
                    .child(div().flex_1().child(lines))
                    .when(!staged && self.stash_view.is_none(), |hunk| {
                        hunk.child(
                            button(
                                SharedString::from(format!("discard-hunk-{file}-{start}")),
                                "Discard Hunk",
                            )
                            .text_color(theme.error)
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.discard_rows(file, (start..end).collect(), window, cx)
                                },
                            )),
                        )
                    })
                    .when(self.stash_view.is_none(), |hunk| {
                        hunk.child(
                            button(SharedString::from(format!("hunk-{file}-{start}")), label)
                                .text_color(theme.foreground)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.stage_rows(file, (start..end).collect(), staged, cx)
                                })),
                        )
                    })
                    .into_any_element()
            }
            StackItem::Gap => diff_view::gap(theme).into_any_element(),
            StackItem::HeaderPad => row().into_any_element(),
            StackItem::Note(text) => row()
                .pl(px(48.))
                .text_color(theme.secondary)
                .child(text.clone())
                .into_any_element(),
        }
    }
}

/// Lays out the all-files view: each file's header, then, unless the file is
/// folded, its changes with context or a note when there is nothing to show.
/// With `split`, the files before it are unstaged and the rest staged, each
/// group under its section.
/// Stands for a diff not worked out yet: its file was folded among too many
/// to open at once.
const NOT_LOADED: &str = "Loading…";

pub(super) fn stack_items(
    diffs: &[Result<FileDiff, String>],
    folded: &[bool],
    split: Option<usize>,
    folded_sections: [bool; 2],
    many: bool,
) -> Vec<StackItem> {
    let mut items = Vec::new();
    // Why every file is folded, when there are too many to open at once.
    if many {
        items.push(StackItem::Note(format!(
            "{} files: too many to open at once. Open them one by one, or a \
             section's at a time.",
            diffs.len()
        )));
    }
    let sections = split;
    if let Some(split) = sections {
        items.extend([StackItem::HeaderPad, StackItem::Section(false, split)]);
    }
    for (file, diff) in diffs.iter().enumerate() {
        if sections == Some(file) {
            items.extend([
                StackItem::HeaderPad,
                StackItem::Section(true, diffs.len() - file),
            ]);
        }
        if let Some(split) = sections
            && folded_sections[usize::from(file >= split)]
        {
            continue;
        }
        items.extend([StackItem::HeaderPad, StackItem::Header(file)]);
        if folded.get(file).copied().unwrap_or(false) {
            continue;
        }
        let hunks = match diff {
            Ok(diff) => diff_view::hunks(&diff.rows, CONTEXT_ROWS),
            Err(error) => {
                items.push(StackItem::Note(error.clone()));
                continue;
            }
        };
        if hunks.is_empty() {
            items.push(StackItem::Note("No visible changes".to_string()));
        }
        if split.is_none() {
            items.extend(hunks.into_iter().map(|row| match row {
                Some(row) => StackItem::Row(file, row),
                None => StackItem::Gap,
            }));
            continue;
        }
        // A worktree file's hunks each get a header to stage them by.
        for hunk in hunks.split(Option::is_none).filter(|hunk| !hunk.is_empty()) {
            let rows: Vec<usize> = hunk.iter().flatten().copied().collect();
            items.push(StackItem::Hunk(file, rows[0], rows[rows.len() - 1] + 1));
            items.extend(rows.into_iter().map(|row| StackItem::Row(file, row)));
        }
    }
    if sections.is_some_and(|split| split >= diffs.len()) {
        items.extend([StackItem::HeaderPad, StackItem::Section(true, 0)]);
    }
    items
}
