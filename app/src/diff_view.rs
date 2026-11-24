use gpui::{AnyElement, Div, HighlightStyle, Hsla, StyledText, div, prelude::*, px};
use wtf_core::{DiffRow, DiffSide};

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
/// a horizontal scroll, or wrapped at `cols` characters, as line `part` of
/// the row.
#[derive(Clone, Copy)]
pub enum Fit {
    Scroll(f32),
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

/// The longest line of `rows`, in characters.
pub fn widest(rows: &[DiffRow]) -> usize {
    rows.iter()
        .flat_map(|row| [&row.left, &row.right])
        .flatten()
        .map(|side| side.text.chars().count())
        .max()
        .unwrap_or(0)
}

/// A diff row, its code colored by `colors` when its file has them.
pub fn row(row: &DiffRow, theme: &Theme, colors: Option<&Colors>, fit: Fit) -> Div {
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
        ))
        .child(div().w(px(1.)).h_full().bg(theme.separator))
        .child(side(
            row.right.as_ref(),
            colors.and_then(|colors| colors.right(row.right.as_ref())),
            theme,
            theme.inserted,
            theme.inserted_char,
            fit,
        ))
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
) -> AnyElement {
    let cell = div().flex().flex_1().min_w_0().h_full().overflow_hidden();
    let Some(side) = side else {
        return cell.bg(theme.filler).into_any_element();
    };
    let changed = !side.changes.is_empty();
    let highlights = highlights(side, code, theme, char);
    // Wrapped, a row shows a piece of its line, the number on the first.
    let (text, highlights, number, shift) = match fit {
        Fit::Scroll(x) => (side.text.clone(), highlights, true, x),
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
        .child(
            div()
                .flex_none()
                .w(px(gutter_width()))
                .pr(px(8.))
                .text_right()
                .text_color(theme.secondary)
                .when(number, |gutter| gutter.child(side.line_number.to_string())),
        )
        // The code alone moves with a horizontal scroll; its number stays.
        .child(
            div().flex_1().min_w_0().h_full().overflow_hidden().child(
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
            ),
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
        assert_eq!(widest(&[row, empty]), 7);
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
