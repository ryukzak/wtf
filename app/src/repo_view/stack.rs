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
        let text_diff = preferences::text_diff_rule(&repo_path);
        let load = cx.background_executor().spawn(async move {
            let diffs = files
                .iter()
                .zip(skip)
                .map(|(file, skip)| match skip {
                    true => Err(NOT_LOADED.to_string()),
                    false => wtf_core::file_diff(repo_path.clone(), file.clone(), mode, &text_diff)
                        .map_err(message),
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
                if let Some(selection) = &this.text_selection
                    && (this.stacked_diffs.get(selection.file) != diffs.get(selection.file)
                        || this.stacked_files.get(selection.file) != files.get(selection.file))
                {
                    this.text_selection = None;
                }
                this.stacked_diffs = diffs;
                this.stacked_files = files;
                this.syntax.borrow_mut().clear();
                this.syntax_generation.set(this.syntax_generation.get() + 1);
                this.restack();
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Notes which of the files Settings always wrap.
    pub(super) fn note_wrapped_files(&mut self) {
        let rule = self
            .repo_path
            .as_deref()
            .map(preferences::wrap_rule)
            .unwrap_or_default();
        self.wrapped_files = self
            .files
            .iter()
            .map(|file| rule.takes(file.path(), 0))
            .collect();
        self.wrap_rule = rule;
    }

    pub(super) fn restack(&mut self) {
        self.note_wrapped_files();
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
            &|file| self.file_cols(file),
            self.one_column.get(),
        );
        // Only changed rows shown count: folded files, the lines between
        // hunks and the unchanged ones around changes do not.
        self.stacked_widest = self
            .stacked
            .iter()
            .filter_map(|item| match *item {
                // Wrapped, a file's lines never scroll sideways.
                StackItem::Row(file, _, _) if self.wraps_file(file) => None,
                StackItem::Row(file, row, 0) => match self.stacked_diffs.get(file) {
                    Some(Ok(diff)) => self.shown_rows(diff).get(row),
                    _ => None,
                },
                _ => None,
            })
            .map(|row| diff_view::widest_changed(std::slice::from_ref(row)))
            .max()
            .unwrap_or(0);
    }

    /// A diff's rows as they show now: split or in one column.
    pub(super) fn shown_rows<'a>(&self, diff: &'a FileDiff) -> &'a [wtf_core::DiffRow] {
        diff_view::shown(diff, self.one_column.get())
    }

    /// Shows diffs in one column, or split, until the room changes enough
    /// to call for the other.
    pub(super) fn set_unified(
        &mut self,
        unified: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.unified = Some(unified);
        self.diff_x.set(0.);
        self.sync_wrap(window, cx);
        cx.notify();
    }

    /// Whether a list `width` wide calls for one column: when a side of a
    /// split diff would hold fewer characters than Settings ask.
    fn one_column_for(&self, width: f32, stacked: bool, window: &Window) -> bool {
        let below = preferences::one_column_below();
        if below == 0 || width <= 0. {
            return false;
        }
        let split = self.split_code_width(width, stacked) / code_char_width(window);
        split < below as f32
    }

    /// Where file `file`'s lines wrap, if they do: everywhere with Wrap
    /// chosen, with Auto in the files Settings always wrap, nowhere with
    /// No Wrap.
    pub(super) fn file_cols(&self, file: usize) -> Option<usize> {
        self.wrap_cols.filter(|_| self.wraps_file(file))
    }

    pub(super) fn wraps_file(&self, file: usize) -> bool {
        match self.wrap {
            WrapMode::Wrap => true,
            WrapMode::NoWrap => false,
            WrapMode::Auto => self.wrapped_files.get(file).copied().unwrap_or(false),
        }
    }

    /// Wraps long diff lines, lets them run on to scroll sideways, or, on
    /// Auto, wraps those of the files Settings name.
    pub(super) fn set_wrap(
        &mut self,
        wrap: WrapMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.wrap = wrap;
        self.diff_x.set(0.);
        self.snapshot_x.set(0.);
        self.sync_wrap(window, cx);
        cx.notify();
    }

    /// Follows the diff's width as it was last drawn: the wrapped all-files
    /// view is laid out again for a new one, and a frame more is drawn while
    /// the width settles, as the first frame has none yet.
    pub(super) fn sync_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = |scroll: &UniformListScrollHandle| {
            f32::from(scroll.0.borrow().base_handle.bounds().size.width)
        };
        let widths = (width(&self.stacked_scroll), width(&self.diff_scroll));
        if widths != self.diff_width.get() {
            self.diff_width.set(widths);
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        // One column or two, by choice or by the room there is; picked
        // lines are rows of the other layout, so they go.
        let width = match self.stacked.is_empty() {
            true => widths.1,
            false => widths.0,
        };
        // The room decides when it changes enough to; a choice made by hand
        // stands until then.
        let fits = self.one_column_for(width, !self.stacked.is_empty(), window);
        if self
            .one_column_by_room
            .replace(Some(fits))
            .is_some_and(|was| was != fits)
        {
            self.unified = None;
        }
        let one = self.unified.unwrap_or(fits);
        if one != self.one_column.get() {
            self.one_column.set(one);
            self.line_selection = None;
            self.restack();
            cx.notify();
        }
        let cols = self.wrap_cols_for(widths.0, true, window);
        if cols != self.wrap_cols {
            self.wrap_cols = cols;
            self.restack();
        }
    }

    /// The characters a side of the diff holds, in a list `width` wide, for
    /// lines that wrap.
    pub(super) fn wrap_cols_for(
        &self,
        width: f32,
        stacked: bool,
        window: &Window,
    ) -> Option<usize> {
        if width <= 0. {
            return None;
        }
        let cols = self.code_width(width, stacked) / code_char_width(window);
        Some((cols.floor() as usize).max(1))
    }

    /// The room a side of the diff has for code in a list `width` wide: half
    /// of it, less the line numbers and, where lines are picked for staging,
    /// the bar marking them.
    fn code_width(&self, width: f32, stacked: bool) -> f32 {
        match self.one_column.get() {
            true => (width - self.line_bar(stacked) - 2. * diff_view::gutter_width()).max(0.),
            false => self.split_code_width(width, stacked),
        }
    }

    /// The room a side of a split diff has for code.
    fn split_code_width(&self, width: f32, stacked: bool) -> f32 {
        ((width - self.line_bar(stacked) - 1.) / 2. - diff_view::gutter_width()).max(0.)
    }

    /// How wide the bar marking picked lines is, where lines are picked.
    fn line_bar(&self, stacked: bool) -> f32 {
        match stacked && self.worktree_split.is_some() && self.stash_view.is_none() {
            true => 3.,
            false => 0.,
        }
    }

    /// Notes how far the code can scroll sideways: until the end of its
    /// longest line, `widest` characters, shows.
    pub(super) fn track_widest(&self, widest: usize, width: f32, stacked: bool, window: &Window) {
        let room = self.code_width(width, stacked);
        let max = match width > 0. {
            true => (widest as f32 * code_char_width(window) - room + 8.).max(0.),
            false => 0.,
        };
        self.diff_max_x.set(max);
        self.scroll_room.set((room, code_char_width(window)));
        self.diff_x.set(self.diff_x.get().min(max));
    }

    /// How a row's code fits: wrapped at `cols`, as its line `part`, or
    /// scrolled sideways as far as the diff is.
    pub(super) fn fit(&self, cols: Option<usize>, part: usize) -> diff_view::Fit {
        match cols {
            Some(cols) => diff_view::Fit::Wrap { cols, part },
            None => {
                // The characters up to the view's right edge, as scrolled.
                let (room, char_width) = self.scroll_room.get();
                let shown = match char_width > 0. && room > 0. {
                    true => ((self.diff_x.get() + room) / char_width).floor() as usize,
                    false => usize::MAX,
                };
                diff_view::Fit::Scroll {
                    x: self.diff_x.get(),
                    shown,
                }
            }
        }
    }

    /// A sideways scroll, or ⇧ with a wheel, moves the diff's code while its
    /// lines run on.
    pub(super) fn scroll_diff_sideways(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.wrap == WrapMode::Wrap {
            return;
        }
        let dx = f32::from(event.delta.pixel_delta(px(diff_view::row_height())).x);
        let x = (self.diff_x.get() - dx).clamp(0., self.diff_max_x.get());
        if x != self.diff_x.get() {
            self.diff_x.set(x);
            cx.notify();
        }
    }

    /// A sideways scroll, or ⇧ with a wheel, moves the Snapshot file's
    /// code while its lines run on.
    pub(super) fn scroll_snapshot_sideways(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dx = f32::from(event.delta.pixel_delta(px(diff_view::row_height())).x);
        let x = (self.snapshot_x.get() - dx).clamp(0., self.snapshot_max_x.get());
        if x != self.snapshot_x.get() {
            self.snapshot_x.set(x);
            cx.notify();
        }
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
        if let Some(repo) = &self.repo_path {
            save_folds(repo, &self.folded, &self.unfolded);
        }
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
    /// A file's code colors, once made. They are made away from the
    /// window, a large file's taking seconds, and the diff draws plain till
    /// they come.
    pub(super) fn diff_colors(
        &self,
        file: Option<usize>,
        theme: &Theme,
        cx: &mut Context<Self>,
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
        if !self.syntax_making.borrow_mut().insert(key.clone()) {
            return None;
        }
        let generation = self.syntax_generation.get();
        let diff = diff.clone();
        let make = cx
            .background_executor()
            .spawn(async move { syntax::colors(&path, &diff, dark) });
        cx.spawn(async move |this, cx| {
            let colors = make.await;
            this.update(cx, |this, cx| {
                this.syntax_making.borrow_mut().remove(&key);
                // Made for diffs since replaced: dropped.
                if this.syntax_generation.get() == generation {
                    this.syntax.borrow_mut().insert(key, Rc::new(colors));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        None
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
            Some(Ok(diff)) => Some(self.shown_rows(diff).len()),
            Some(Err(_)) => Some(0),
            None => None,
        };
        match *item {
            StackItem::Header(file) => rows(file).is_some(),
            StackItem::Row(file, index, _) => rows(file).is_some_and(|n| index < n),
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
        let button = |id: SharedString, label: &str| {
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
                .child(label.to_string())
        };
        // Section and file headers are two rows tall: their buttons get
        // room to match.
        let tall = |id: SharedString, label: &str| {
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
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.keep_keys(window, cx);
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
                // A renamed file goes by both its names, so the old one goes
                // as the new one comes.
                let paths: Vec<String> = file
                    .old_path
                    .iter()
                    .chain(&file.new_path)
                    .cloned()
                    .collect();
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
                    .on_mouse_down(MouseButton::Right, this_file_menu(index, cx))
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
                    // A renamed file: its old name, then the new one.
                    .children(
                        file.old_path
                            .as_ref()
                            .filter(|_| old_name(file).is_some())
                            .map(|old| div().text_color(theme.secondary).child(format!("{old} →"))),
                    )
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(file.path().to_string()),
                    )
                    .when(file.submodule, |header| {
                        let open = file.path().to_string();
                        header.child(
                            button(SharedString::from(format!("open-sub-{index}")), "Open")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_submodule(&open, cx)
                                })),
                        )
                    })
                    .children(staged.map(|staged| {
                        // The header says what: the buttons just what they do.
                        let label = if staged { "Unstage" } else { "Stage" };
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
                                        "Discard",
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
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.keep_keys(window, cx);
                                        this.stage_selected_lines(staged, cx)
                                    }))
                                    .child(format!(" ({lines})")),
                                )
                            })
                            .child(
                                button(SharedString::from(format!("stage-{index}")), label)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.keep_keys(window, cx);
                                        this.stage(staged, paths.clone(), cx)
                                    })),
                            )
                    }))
                    .into_any_element()
            }
            &StackItem::Row(file, index, part) => {
                let fit = self.fit(self.file_cols(file), part);
                let colors = self.diff_colors(Some(file), theme, cx);
                match &self.stacked_diffs[file] {
                    // Lines of any diff are picked: to copy, and in a worktree
                    // to stage or discard from their hunk's header.
                    Ok(diff) => {
                        let picked = |row: usize| matches!(&self.line_selection, Some((f, rows)) if *f == file && rows.contains(&row));
                        let selected = picked(index);
                        // Picked lines are framed, as one box per run: its
                        // top on the run's first line, its bottom on the last
                        // part of its last line.
                        let parts = self.file_cols(file).map_or(1, |cols| {
                            diff_view::parts(&self.shown_rows(diff)[index], cols)
                        });
                        let top = selected && part == 0 && (index == 0 || !picked(index - 1));
                        let bottom = selected && part + 1 == parts && !picked(index + 1);
                        let group = SharedString::from(format!("line-{file}-{index}-{part}"));
                        div()
                            .id(group.clone())
                            .group(group.clone())
                            .relative()
                            .flex()
                            .w_full()
                            .cursor(gpui::CursorStyle::IBeam)
                            // Pressed on its number, a line is picked, and
                            // dragged over others, they are picked too; on
                            // its code, text is selected.
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    match this.on_code(file, event.position) {
                                        true => this.press_text(
                                            file,
                                            index,
                                            part,
                                            event.position,
                                            event.click_count,
                                            event.modifiers,
                                            cx,
                                        ),
                                        false => {
                                            this.text_selection = None;
                                            this.press_line(file, index, event.modifiers, cx)
                                        }
                                    }
                                }),
                            )
                            .on_mouse_move(cx.listener(
                                move |this, event: &MouseMoveEvent, _, cx| {
                                    let pressed = event.pressed_button == Some(MouseButton::Left);
                                    this.drag_line(file, index, pressed, cx);
                                    this.drag_text(file, index, part, event.position, pressed, cx)
                                },
                            ))
                            .child(
                                diff_view::row(
                                    &self.shown_rows(diff)[index],
                                    theme,
                                    colors.as_deref(),
                                    fit,
                                    self.one_column.get(),
                                    self.selected_in(file, index),
                                )
                                .flex_1(),
                            )
                            // The frame over a picked line, drawn over the
                            // line's own colors: one per side when
                            // split, across the line when unified.
                            .when(selected, |line| {
                                let frame = || {
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .border_l_2()
                                        .border_r_2()
                                        .when(top, |f| f.border_t_2().rounded_t(px(3.)))
                                        .when(bottom, |f| f.border_b_2().rounded_b(px(3.)))
                                        .border_color(theme.accent.opacity(0.85))
                                };
                                match self.one_column.get() {
                                    true => line.child(frame().left_0().right_0()),
                                    false => line
                                        .child(frame().left_0().w(gpui::relative(0.5)))
                                        .child(frame().left(gpui::relative(0.5)).right_0()),
                                }
                            })
                            .into_any_element()
                    }
                    Err(_) => row().into_any_element(),
                }
            }
            &StackItem::Hunk(file, start, end) => {
                let staged = self.worktree_split.is_some_and(|split| file >= split);
                // Lines picked in the hunk: its buttons take just those.
                let picked: Vec<usize> = match &self.line_selection {
                    Some((f, rows)) if *f == file => rows
                        .iter()
                        .copied()
                        .filter(|row| (start..end).contains(row))
                        .collect(),
                    _ => Vec::new(),
                };
                // The whole hunk goes by the plain verb; picked lines say how
                // many.
                let what = match picked.len() {
                    0 => String::new(),
                    1 => " Line".to_string(),
                    n => format!(" {n} Lines"),
                };
                let label = match staged {
                    true => format!("Unstage{what}"),
                    false => format!("Stage{what}"),
                };
                let rows = match picked.is_empty() {
                    true => (start..end).collect::<Vec<usize>>(),
                    false => picked,
                };
                let discarded = rows.clone();
                // The hunk's first lines on each side, as `@@` headers show them.
                let lines = match &self.stacked_diffs[file] {
                    Ok(diff) => {
                        let rows = &self.shown_rows(diff)[start..end];
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
                                &format!("Discard{what}"),
                            )
                            .text_color(theme.error)
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.discard_rows(file, discarded.clone(), window, cx)
                                },
                            )),
                        )
                    })
                    .when(self.stash_view.is_none(), |hunk| {
                        hunk.child(
                            button(SharedString::from(format!("hunk-{file}-{start}")), &label)
                                .text_color(theme.foreground)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.keep_keys(window, cx);
                                    this.stage_rows(file, rows.clone(), staged, cx)
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
    cols: &dyn Fn(usize) -> Option<usize>,
    one: bool,
) -> Vec<StackItem> {
    let mut items = Vec::new();
    // A row, as many times as it has lines when they wrap at `cols`.
    let lines = |file: usize, row: usize| {
        let parts = match (cols(file), &diffs[file]) {
            (Some(cols), Ok(diff)) => diff_view::parts(&diff_view::shown(diff, one)[row], cols),
            _ => 1,
        };
        (0..parts).map(move |part| StackItem::Row(file, row, part))
    };
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
        // An open section with nothing in it says so.
        if split == 0 && !folded_sections[0] {
            items.push(StackItem::Note("No unstaged changes".to_string()));
        }
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
            Ok(diff) => diff_view::hunks(diff_view::shown(diff, one), CONTEXT_ROWS),
            Err(error) => {
                items.push(StackItem::Note(error.clone()));
                continue;
            }
        };
        if hunks.is_empty() {
            items.push(StackItem::Note("No visible changes".to_string()));
        }
        if split.is_none() {
            for row in hunks {
                match row {
                    Some(row) => items.extend(lines(file, row)),
                    None => items.push(StackItem::Gap),
                }
            }
            continue;
        }
        // A worktree file's hunks each get a header to stage them by.
        for hunk in hunks.split(Option::is_none).filter(|hunk| !hunk.is_empty()) {
            let rows: Vec<usize> = hunk.iter().flatten().copied().collect();
            items.push(StackItem::Hunk(file, rows[0], rows[rows.len() - 1] + 1));
            items.extend(rows.into_iter().flat_map(|row| lines(file, row)));
        }
    }
    if sections.is_some_and(|split| split >= diffs.len()) {
        items.extend([StackItem::HeaderPad, StackItem::Section(true, 0)]);
    }
    items
}

/// The width of a character of code, in its monospaced font.
pub(super) fn code_char_width(window: &Window) -> f32 {
    let text = window.text_system();
    let font = text.resolve_font(&gpui::font(preferences::code_font()));
    text.advance(font, px(preferences::code_size()), 'm')
        .map_or(preferences::code_size() * 0.6, |size| f32::from(size.width))
}
