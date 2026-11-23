//! Search in a snapshot: files by name first, then lines of their text.

use super::*;

/// How long typing pauses before the files' text is searched.
const SEARCH_PAUSE: Duration = Duration::from_millis(200);
/// How few characters search the text: fewer find too much.
const MIN_TEXT_QUERY: usize = 2;
/// How many files a search lists by name.
const MAX_NAMED: usize = 50;

/// A row of the search's results.
#[derive(Debug, PartialEq)]
pub(super) enum SearchRow {
    /// Files found by name, or lines in their text, with how many; `None`
    /// while the text is still searched.
    Header(&'static str, Option<usize>),
    /// A file, found by name or holding lines found.
    File(String),
    /// A line found in a file's text.
    Line(String, u32, String),
}

/// The search's rows: files whose path has `query`, those whose name has it
/// first; then the lines found, under their files.
pub(super) fn search_rows(
    files: &[String],
    query: &str,
    found: Option<&[wtf_core::Found]>,
) -> Vec<SearchRow> {
    let query = query.trim().to_lowercase();
    let name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_lowercase();
    let mut named: Vec<&String> = files
        .iter()
        .filter(|path| path.to_lowercase().contains(&query))
        .collect();
    named.sort_by_key(|path| (!name(path).contains(&query), path.len()));
    let mut rows = vec![SearchRow::Header("Files", Some(named.len()))];
    rows.extend(
        named
            .into_iter()
            .take(MAX_NAMED)
            .map(|path| SearchRow::File(path.clone())),
    );
    if query.chars().count() < MIN_TEXT_QUERY {
        return rows;
    }
    rows.push(SearchRow::Header("Text", found.map(<[_]>::len)));
    let mut last: Option<&str> = None;
    for line in found.unwrap_or_default() {
        if last != Some(line.path.as_str()) {
            rows.push(SearchRow::File(line.path.clone()));
            last = Some(&line.path);
        }
        rows.push(SearchRow::Line(
            line.path.clone(),
            line.line,
            line.text.clone(),
        ));
    }
    rows
}

impl RepoView {
    /// Searches the snapshot's text for what the search field holds, after
    /// a pause in typing, again when the state shown changes.
    pub(super) fn sync_search(&mut self, cx: &mut Context<Self>) {
        let query = self.snapshot.search.trim().to_string();
        let (Some(repo), Some(revision)) = (self.repo_path.clone(), self.snapshot_revision())
        else {
            return;
        };
        if query.chars().count() < MIN_TEXT_QUERY {
            self.snapshot.found = None;
            self.snapshot.searching = None;
            return;
        }
        let wanted = (query.clone(), revision.clone());
        let done = self
            .snapshot
            .found
            .as_ref()
            .is_some_and(|(q, r, _)| (q, r) == (&wanted.0, &wanted.1));
        if done || self.snapshot.searching.as_ref() == Some(&wanted) {
            return;
        }
        self.snapshot.searching = Some(wanted.clone());
        self.snapshot_tasks[3] = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_PAUSE).await;
            let found = cx
                .background_executor()
                .spawn(async move { wtf_core::snapshot_grep(&repo, &revision, &query) })
                .await;
            this.update(cx, |this, cx| {
                if this.snapshot.searching.as_ref() == Some(&wanted) {
                    this.snapshot.searching = None;
                    let (query, revision) = wanted;
                    this.snapshot.found = Some((query, revision, found));
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// The search's rows for what the field holds now; lines found for an
    /// earlier query stay out.
    pub(super) fn current_search_rows(&self) -> Vec<SearchRow> {
        let query = self.snapshot.search.trim();
        let changed = self.changed_paths();
        let only = self.snapshot.only_changed;
        let keep = |path: &str| !only || changed.contains_key(path);
        let files: Vec<String> = self
            .snapshot
            .files
            .iter()
            .filter(|path| keep(path))
            .cloned()
            .collect();
        let found: Option<Vec<wtf_core::Found>> = self
            .snapshot
            .found
            .as_ref()
            .filter(|(q, _, _)| q == query)
            .map(|(_, _, found)| found.iter().filter(|f| keep(&f.path)).cloned().collect());
        search_rows(&files, query, found.as_deref())
    }

    /// Opens a file found, at a line found in it when there is one, which
    /// shows marked.
    pub(super) fn open_found(&mut self, path: String, line: Option<u32>, cx: &mut Context<Self>) {
        self.open_snapshot_file(path.clone());
        self.snapshot.found_line = line.map(|line| (path.clone(), line));
        match line {
            Some(_) if self.snapshot_text_is_current(&path) => self.scroll_to_found(),
            Some(_) => {}
            None => self.snapshot_scroll.scroll_to_item(0, ScrollStrategy::Top),
        }
        // Its changes load too, for its tints.
        let index = self.files.iter().position(|file| file.path() == path);
        if index.is_some() && self.selected_file != index {
            self.select_file(index, cx);
        }
        cx.notify();
    }

    /// Scrolls the open file to the line found in it, once its text is in.
    pub(super) fn scroll_to_found(&mut self) {
        if let Some((_, line)) = &self.snapshot.found_line {
            let row = (*line as usize).saturating_sub(1);
            self.snapshot_scroll
                .scroll_to_item(row, ScrollStrategy::Center);
        }
    }

    /// The search's results, in place of the tree.
    pub(super) fn search_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.current_search_rows().len();
        uniform_list(
            "snapshot-search",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                let theme = Theme::for_appearance(window.appearance());
                let rows = this.current_search_rows();
                let open = this.snapshot.path.clone();
                let marked = this.snapshot.found_line.clone();
                range
                    .filter_map(|k| rows.get(k).map(|row| (k, row)))
                    .map(|(k, row)| {
                        let item = div()
                            .id(("search-row", k))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .w_full()
                            .h(px(FILE_ROW_HEIGHT))
                            .px(px(8.))
                            .whitespace_nowrap()
                            .overflow_hidden();
                        match row {
                            SearchRow::Header(title, count) => item
                                .text_xs()
                                .text_color(theme.secondary)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(match count {
                                    Some(count) => format!("{title} · {count}"),
                                    None => format!("{title} · …"),
                                }),
                            SearchRow::File(path) => {
                                let (folder, name) = match path.rsplit_once('/') {
                                    Some((folder, name)) => (folder.to_string(), name.to_string()),
                                    None => (String::new(), path.clone()),
                                };
                                let selected =
                                    open.as_deref() == Some(path.as_str()) && marked.is_none();
                                let path = path.clone();
                                item.cursor_pointer()
                                    .hover(|item| item.bg(theme.selection.opacity(0.5)))
                                    .when(selected, |item| item.bg(theme.selection))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_found(path.clone(), None, cx)
                                    }))
                                    .child(div().flex_none().child(name))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_xs()
                                            .text_color(theme.secondary)
                                            .child(folder),
                                    )
                            }
                            SearchRow::Line(path, line, text) => {
                                let selected = marked.as_ref() == Some(&(path.clone(), *line));
                                let (path, line) = (path.clone(), *line);
                                item.cursor_pointer()
                                    .pl(px(16.))
                                    .hover(|item| item.bg(theme.selection.opacity(0.5)))
                                    .when(selected, |item| item.bg(theme.selection))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_found(path.clone(), Some(line), cx)
                                    }))
                                    .font_family(preferences::code_font())
                                    .font_weight(preferences::code_weight())
                                    .text_xs()
                                    .child(
                                        div()
                                            .flex_none()
                                            .min_w(px(28.))
                                            .text_right()
                                            .text_color(theme.secondary)
                                            .child(line.to_string()),
                                    )
                                    .child(div().truncate().child(text.clone()))
                            }
                        }
                    })
                    .collect()
            }),
        )
        .flex_1()
        .py(px(4.))
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(path: &str, line: u32) -> wtf_core::Found {
        wtf_core::Found {
            path: path.to_string(),
            line,
            text: "x".to_string(),
        }
    }

    #[test]
    fn names_come_first_then_lines_under_their_files() {
        let files = ["src/main.rs", "main/lib.rs", "README.md"].map(String::from);
        let lines = [found("src/main.rs", 3), found("src/main.rs", 9)];
        let rows = search_rows(&files, "Main", Some(&lines));
        assert_eq!(
            rows,
            [
                SearchRow::Header("Files", Some(2)),
                // The name has it, so before a folder that has it.
                SearchRow::File("src/main.rs".to_string()),
                SearchRow::File("main/lib.rs".to_string()),
                SearchRow::Header("Text", Some(2)),
                SearchRow::File("src/main.rs".to_string()),
                SearchRow::Line("src/main.rs".to_string(), 3, "x".to_string()),
                SearchRow::Line("src/main.rs".to_string(), 9, "x".to_string()),
            ]
        );
        // One character searches names only; text still searched shows so.
        assert_eq!(search_rows(&files, "m", None).len(), 4);
        assert_eq!(
            search_rows(&files, "re", None).last(),
            Some(&SearchRow::Header("Text", None))
        );
    }
}
