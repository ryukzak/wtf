use gpui::{AnyElement, Div, HighlightStyle, Hsla, StyledText, div, prelude::*, px};
use wtf_core::{DiffRow, DiffSide, FileDiff};

use crate::syntax::{Colors, Spans};
use crate::theme::Theme;

/// Diff rows are a line of code tall, as Settings set it; section and file
/// headers share their height.
pub fn row_height() -> f32 {
    crate::preferences::code_line_height()
}

/// Room for line numbers: four digits of code.
pub fn gutter_width() -> f32 {
    crate::preferences::code_size() * 4.
}

pub fn is_change(row: &DiffRow) -> bool {
    match (&row.left, &row.right) {
        (Some(left), Some(right)) => !left.changes.is_empty() || !right.changes.is_empty(),
        _ => true,
    }
}

/// Index of the first changed row, to scroll the diff to it.
pub fn first_change(rows: &[DiffRow]) -> Option<usize> {
    rows.iter().position(is_change)
}

/// The rows to show around changes, keeping `context` rows on each side; `None`
/// marks skipped rows.
pub fn hunks(rows: &[DiffRow], context: usize) -> Vec<Option<usize>> {
    let mut keep = vec![false; rows.len()];
    for (i, _) in rows.iter().enumerate().filter(|(_, row)| is_change(row)) {
        let end = (i + context + 1).min(rows.len());
        keep[i.saturating_sub(context)..end].fill(true);
    }
    let mut shown = Vec::new();
    for (i, &kept) in keep.iter().enumerate() {
        if kept {
            shown.push(Some(i));
        } else if shown.last().is_some_and(Option::is_some) || (i == 0) {
            shown.push(None);
        }
    }
    if shown.last() == Some(&None) {
        shown.pop();
    }
    shown
}

pub fn gap(theme: &Theme) -> Div {
    div()
        .w_full()
        .h(px(row_height()))
        .pl(px(gutter_width()))
        .bg(theme.filler)
        .text_color(theme.secondary)
        .child("⋯")
}

/// How a diff row's code fits its side: cut at the edge and moved left by
/// a horizontal scroll, `x`, where the first `shown` characters show; or
/// wrapped at `cols` characters, as line `part` of the row.
#[derive(Clone, Copy)]
pub enum Fit {
    Scroll { x: f32, shown: usize },
    Wrap { cols: usize, part: usize },
}

/// How many lines a row takes wrapped at `cols` characters: as many as its
/// longer side needs.
pub fn parts(row: &DiffRow, cols: usize) -> usize {
    let lines = |side: &Option<DiffSide>| {
        side.as_ref()
            .map_or(1, |side| wrap_starts(&side.text, cols).len())
    };
    lines(&row.left).max(lines(&row.right))
}

/// Where each line of `text` starts, in characters, wrapped at `cols`: after
/// the last space that fits, so words stay whole, or mid-word when one
/// alone is longer than a line.
pub fn wrap_starts(text: &str, cols: usize) -> Vec<usize> {
    let chars: Vec<char> = text.chars().collect();
    let cols = cols.max(1);
    let mut starts = vec![0];
    let mut start = 0;
    while chars.len() - start > cols {
        let end = start + cols;
        // A cut after a space, with more than spaces before it: indentation
        // alone is no line.
        let cut = (start + 1..=end).rev().find(|&at| {
            chars[at - 1].is_whitespace() && chars[start..at].iter().any(|c| !c.is_whitespace())
        });
        start = cut.unwrap_or(end);
        starts.push(start);
    }
    starts
}

/// The longest changed line of `rows`, in characters: how far the code
/// scrolls sideways, so changes show whole, not long lines around them.
pub fn widest_changed(rows: &[DiffRow]) -> usize {
    rows.iter()
        .filter(|row| is_change(row))
        .flat_map(|row| [&row.left, &row.right])
        .flatten()
        .map(|side| side.text.chars().count())
        .max()
        .unwrap_or(0)
}

/// A diff's rows as they show: side by side, or in `one` column.
pub fn shown(diff: &FileDiff, one: bool) -> &[DiffRow] {
    match one {
        true => &diff.unified,
        false => &diff.rows,
    }
}

/// Which code of a row text is selected in: a side of a split diff, or
/// the one line a unified row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Left,
    Right,
    Unified,
}

/// The line of `row` that `pane` shows, if any.
pub fn pane_side(row: &DiffRow, pane: Pane) -> Option<&DiffSide> {
    match pane {
        Pane::Left => row.left.as_ref(),
        Pane::Right => row.right.as_ref(),
        Pane::Unified => row.right.as_ref().or(row.left.as_ref()),
    }
}

/// Text selected in a row: in which pane, and which bytes of its line.
pub type Selected = Option<(Pane, std::ops::Range<usize>)>;

/// A diff row, its code colored by `colors` when its file has them; in
/// `one` column, a row of the unified rows. `selected` text is marked.
pub fn row(
    row: &DiffRow,
    theme: &Theme,
    colors: Option<&Colors>,
    fit: Fit,
    one: bool,
    selected: Selected,
) -> Div {
    let in_pane = |pane: Pane| {
        selected
            .clone()
            .filter(|(at, _)| *at == pane)
            .map(|(_, range)| range)
    };
    if one {
        return unified_row(row, theme, colors, fit, in_pane(Pane::Unified));
    }
    div()
        .flex()
        .w_full()
        .h(px(row_height()))
        .child(side(
            row.left.as_ref(),
            colors.and_then(|colors| colors.left(row.left.as_ref())),
            theme,
            theme.deleted,
            theme.deleted_char,
            fit,
            in_pane(Pane::Left),
        ))
        .child(div().w(px(1.)).h_full().bg(theme.separator))
        .child(side(
            row.right.as_ref(),
            colors.and_then(|colors| colors.right(row.right.as_ref())),
            theme,
            theme.inserted,
            theme.inserted_char,
            fit,
            in_pane(Pane::Right),
        ))
}

/// A row of one column: an old line, a new one, or one both sides share,
/// its numbers on the left, old then new, as `git diff` reads.
fn unified_row(
    row: &DiffRow,
    theme: &Theme,
    colors: Option<&Colors>,
    fit: Fit,
    selected: Option<std::ops::Range<usize>>,
) -> Div {
    let (side, code, line, char, old, new) = match (&row.left, &row.right) {
        (Some(left), None) => (
            left,
            colors.and_then(|colors| colors.left(Some(left))),
            Some(theme.deleted),
            theme.deleted_char,
            Some(left.line_number),
            None,
        ),
        (None, Some(right)) => (
            right,
            colors.and_then(|colors| colors.right(Some(right))),
            Some(theme.inserted),
            theme.inserted_char,
            None,
            Some(right.line_number),
        ),
        (Some(left), Some(right)) => (
            right,
            colors.and_then(|colors| colors.right(Some(right))),
            None,
            theme.inserted_char,
            Some(left.line_number),
            Some(right.line_number),
        ),
        (None, None) => return div().w_full().h(px(row_height())),
    };
    let first = matches!(fit, Fit::Scroll { .. } | Fit::Wrap { part: 0, .. });
    let number = |n: Option<u32>| {
        div()
            .flex_none()
            .w(px(gutter_width()))
            .pr(px(8.))
            .text_right()
            .text_color(theme.secondary)
            // Lines are picked from their numbers; the code selects text.
            .cursor_pointer()
            .when(first, |gutter| gutter.children(n.map(|n| n.to_string())))
    };
    // The code as a side of its own, its number left out: both are drawn
    // here.
    let cell = side_cell(
        Some(side),
        code,
        theme,
        line.unwrap_or(theme.background),
        char,
        fit,
        false,
        selected,
    );
    div()
        .flex()
        .w_full()
        .h(px(row_height()))
        .when_some(line, |row, line| row.bg(line))
        .child(number(old))
        .child(number(new))
        .child(cell)
}

/// A line's runs: code colors, with changed words marked over them.
fn highlights(
    side: &DiffSide,
    code: Option<&Spans>,
    theme: &Theme,
    char: Hsla,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    // Changed words are marked only within a line that is partly changed; a
    // line changed as a whole is marked by its background alone, past its
    // indentation.
    let indent = side.text.len() - side.text.trim_start().len();
    let whole = side
        .changes
        .iter()
        .any(|change| change.start <= indent && change.end >= side.text.trim_end().len());
    let changes: &[wtf_core::TextRange] = match whole {
        true => &[],
        false => &side.changes,
    };
    // Code colors, with changed words marked over them: runs cut wherever
    // either starts or ends, so they never overlap.
    let mut cuts = vec![0, side.text.len()];
    for change in changes {
        cuts.extend([change.start, change.end]);
    }
    // Colors made for other text are left out rather than cut a character.
    let fits = |range: &std::ops::Range<usize>| {
        range.end <= side.text.len()
            && side.text.is_char_boundary(range.start)
            && side.text.is_char_boundary(range.end)
    };
    let code: Vec<&(std::ops::Range<usize>, Hsla)> = code
        .into_iter()
        .flatten()
        .filter(|(range, _)| fits(range))
        .collect();
    for (range, _) in &code {
        cuts.extend([range.start, range.end]);
    }
    cuts.retain(|&cut| side.text.is_char_boundary(cut));
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|cut| {
            let (start, end) = (cut[0], cut[1]);
            let in_change = changes
                .iter()
                .any(|change| change.start <= start && end <= change.end);
            let color = code
                .iter()
                .find(|(range, _)| range.start <= start && end <= range.end)
                .map(|(_, color)| *color);
            let style = HighlightStyle {
                color: match (color, in_change) {
                    (Some(color), _) => Some(color),
                    (None, true) => Some(theme.changed_char_text),
                    (None, false) => None,
                },
                background_color: in_change.then_some(char),
                ..Default::default()
            };
            (start..end, style)
        })
        .collect()
}

fn side(
    side: Option<&DiffSide>,
    code: Option<&Spans>,
    theme: &Theme,
    line: Hsla,
    char: Hsla,
    fit: Fit,
    selected: Option<std::ops::Range<usize>>,
) -> AnyElement {
    side_cell(side, code, theme, line, char, fit, true, selected)
}

/// `highlights`, covering a line end to end, with the `selected` bytes'
/// background the selection's.
fn with_selection(
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    selected: Option<std::ops::Range<usize>>,
    color: Hsla,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let Some(selected) = selected.filter(|range| range.start < range.end) else {
        return highlights;
    };
    highlights
        .into_iter()
        .flat_map(|(range, style)| {
            let inside = range.start.max(selected.start)..range.end.min(selected.end);
            if inside.start >= inside.end {
                return vec![(range, style)];
            }
            let marked = HighlightStyle {
                background_color: Some(color),
                ..style
            };
            [
                (range.start..inside.start, style),
                (inside.clone(), marked),
                (inside.end..range.end, style),
            ]
            .into_iter()
            .filter(|(range, _)| range.start < range.end)
            .collect()
        })
        .collect()
}

/// A side's code, tinted as `line` when changed, after its line number
/// when `numbered`.
fn side_cell(
    side: Option<&DiffSide>,
    code: Option<&Spans>,
    theme: &Theme,
    line: Hsla,
    char: Hsla,
    fit: Fit,
    numbered: bool,
    selected: Option<std::ops::Range<usize>>,
) -> AnyElement {
    let cell = div().flex().flex_1().min_w_0().h_full().overflow_hidden();
    let Some(side) = side else {
        return cell.bg(theme.filler).into_any_element();
    };
    let changed = !side.changes.is_empty();
    let cropped = match fit {
        Fit::Scroll { shown, .. } => side.text.chars().count() > shown,
        Fit::Wrap { .. } => false,
    };
    let highlights = with_selection(
        highlights(side, code, theme, char),
        selected,
        theme.selection,
    );
    // Wrapped, a row shows a piece of its line, the number on the first.
    let (text, highlights, number, shift) = match fit {
        Fit::Scroll { x, .. } => (side.text.clone(), highlights, true, x),
        Fit::Wrap { cols, part } => {
            let at = |n: usize| {
                side.text
                    .char_indices()
                    .nth(n)
                    .map_or(side.text.len(), |(i, _)| i)
            };
            let starts = wrap_starts(&side.text, cols);
            let (start, end) = match starts.get(part) {
                Some(&from) => (
                    at(from),
                    starts.get(part + 1).map_or(side.text.len(), |&to| at(to)),
                ),
                None => (side.text.len(), side.text.len()),
            };
            let highlights = highlights
                .into_iter()
                .filter_map(|(range, style)| {
                    let (from, to) = (range.start.max(start), range.end.min(end));
                    (from < to).then(|| (from - start..to - start, style))
                })
                .collect();
            (side.text[start..end].to_string(), highlights, part == 0, 0.)
        }
    };

    cell.when(changed, |cell| cell.bg(line))
        .when(numbered, |cell| {
            cell.child(
                div()
                    .flex_none()
                    .w(px(gutter_width()))
                    .pr(px(8.))
                    .text_right()
                    .text_color(theme.secondary)
                    .cursor_pointer()
                    .when(number, |gutter| gutter.child(side.line_number.to_string())),
            )
        })
        // The code alone moves with a horizontal scroll; its number stays.
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_hidden()
                .child(
                    div()
                        .relative()
                        .left(px(-shift))
                        .whitespace_nowrap()
                        .text_color(if changed {
                            theme.changed_text
                        } else {
                            theme.foreground
                        })
                        .child(StyledText::new(text).with_highlights(highlights)),
                )
                // A line running on past the edge says so there.
                .when(cropped, |code| {
                    code.child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .h_full()
                            .bg(theme.background)
                            .child(
                                div()
                                    .h_full()
                                    .px(px(3.))
                                    .when(changed, |mark| mark.bg(line))
                                    .text_color(theme.secondary)
                                    .child("›"),
                            ),
                    )
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use wtf_core::{DiffSide, TextRange};

    use super::*;

    fn line(changed: bool) -> DiffRow {
        let side = DiffSide {
            line_number: 1,
            text: String::new(),
            changes: if changed {
                vec![TextRange { start: 0, end: 1 }]
            } else {
                Vec::new()
            },
        };
        DiffRow {
            left: Some(side.clone()),
            right: Some(side),
        }
    }

    #[test]
    fn keeps_context_around_changes_and_marks_gaps() {
        let mut rows: Vec<DiffRow> = (0..20).map(|_| line(false)).collect();
        rows[10] = line(true);
        assert_eq!(
            hunks(&rows, 2),
            vec![None, Some(8), Some(9), Some(10), Some(11), Some(12)]
        );

        rows[0] = line(true);
        assert_eq!(hunks(&rows, 2)[..4], [Some(0), Some(1), Some(2), None]);
    }

    #[test]
    fn a_row_wraps_as_its_longer_side_needs() {
        let side = |text: &str| {
            Some(DiffSide {
                line_number: 1,
                text: text.to_string(),
                changes: Vec::new(),
            })
        };
        let row = DiffRow {
            left: side("abc"),
            right: side("абвгдеж"),
        };
        assert_eq!(parts(&row, 3), 3, "by characters, not bytes");
        // Words stay whole; one longer than a line is cut.
        assert_eq!(wrap_starts("let total = sum", 10), [0, 10]);
        assert_eq!(wrap_starts("abcdefghij klm", 4), [0, 4, 8, 11]);
        // Indentation alone does not make a line.
        assert_eq!(wrap_starts("    abcdefgh", 6), [0, 6]);
        assert_eq!(parts(&row, 10), 1);
        let empty = DiffRow {
            left: None,
            right: side(""),
        };
        assert_eq!(parts(&empty, 3), 1);
        // Only changed lines count: `row` is unchanged, `empty` has none.
        assert_eq!(widest_changed(&[row, empty]), 0);
    }

    #[test]
    fn unchanged_file_shows_nothing() {
        let rows: Vec<DiffRow> = (0..5).map(|_| line(false)).collect();
        assert!(hunks(&rows, 3).is_empty());
    }

    #[test]
    fn colors_for_other_text_never_cut_a_character() {
        let side = DiffSide {
            line_number: 1,
            text: "a — b".to_string(),
            changes: vec![TextRange { start: 0, end: 1 }],
        };
        let red = gpui::hsla(0., 1., 0.5, 1.);
        // Made for a line where the dash was a plain `-`.
        let code: Spans = vec![(0..3, red), (3..5, red)];
        let theme = Theme::for_appearance(gpui::WindowAppearance::Light);
        for (range, _) in highlights(&side, Some(&code), &theme, red) {
            assert!(side.text.is_char_boundary(range.start));
            assert!(side.text.is_char_boundary(range.end));
        }
    }
}
