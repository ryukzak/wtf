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

/// A diff row, its code colored by `colors` when its file has them.
pub fn row(row: &DiffRow, theme: &Theme, colors: Option<&Colors>) -> Div {
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
        ))
        .child(div().w(px(1.)).h_full().bg(theme.separator))
        .child(side(
            row.right.as_ref(),
            colors.and_then(|colors| colors.right(row.right.as_ref())),
            theme,
            theme.inserted,
            theme.inserted_char,
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
) -> AnyElement {
    let cell = div().flex().flex_1().min_w_0().h_full().overflow_hidden();
    let Some(side) = side else {
        return cell.bg(theme.filler).into_any_element();
    };
    let changed = !side.changes.is_empty();
    let highlights = highlights(side, code, theme, char);

    cell.when(changed, |cell| cell.bg(line))
        .child(
            div()
                .flex_none()
                .w(px(gutter_width()))
                .pr(px(8.))
                .text_right()
                .text_color(theme.secondary)
                .child(side.line_number.to_string()),
        )
        .child(
            div()
                .whitespace_nowrap()
                .text_color(if changed {
                    theme.changed_text
                } else {
                    theme.foreground
                })
                .child(StyledText::new(side.text.clone()).with_highlights(highlights)),
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
