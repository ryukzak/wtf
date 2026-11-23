//! Code colors for diffs, by Sublime Text's grammars and themes through
//! syntect: each side of a file is read top to bottom, so strings and
//! comments over several lines keep their color.

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::LazyLock;

use gpui::Hsla;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use wtf_core::{DiffSide, FileDiff};

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// How light code colors are at least on a dark background.
const MIN_DARK_LIGHTNESS: f32 = 0.68;

/// A line's text runs and their colors.
pub type Spans = Vec<(Range<usize>, Hsla)>;

/// Colors for both sides of a file's diff, by line number.
#[derive(Default)]
pub struct Colors {
    pub left: HashMap<u32, Spans>,
    pub right: HashMap<u32, Spans>,
}

impl Colors {
    pub fn left(&self, side: Option<&DiffSide>) -> Option<&Spans> {
        self.left.get(&side?.line_number)
    }

    pub fn right(&self, side: Option<&DiffSide>) -> Option<&Spans> {
        self.right.get(&side?.line_number)
    }
}

fn theme(dark: bool) -> &'static Theme {
    // Eighties has the brightest colors of the dark themes syntect ships.
    let name = if dark {
        "base16-eighties.dark"
    } else {
        "InspiredGitHub"
    };
    &THEMES.themes[name]
}

/// Colors a file's diff, or nothing for a file no grammar knows.
pub fn colors(path: &str, diff: &FileDiff, dark: bool) -> Colors {
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    // By extension, then by whole name, such as `Makefile`, then by a first
    // line such as a shebang.
    let first_line = || {
        let side = diff
            .rows
            .iter()
            .find_map(|row| row.right.as_ref().or(row.left.as_ref()));
        side.filter(|side| side.line_number == 1)
            .map_or("", |side| side.text.as_str())
    };
    let Some(syntax) = SYNTAXES
        .find_syntax_by_extension(extension)
        .or_else(|| SYNTAXES.find_syntax_by_extension(name))
        .or_else(|| SYNTAXES.find_syntax_by_first_line(first_line()))
    else {
        return Colors::default();
    };
    let side = |pick: fn(&wtf_core::DiffRow) -> Option<&DiffSide>| {
        let mut lines = HighlightLines::new(syntax, theme(dark));
        let mut spans = HashMap::new();
        for side in diff.rows.iter().filter_map(pick) {
            let line = format!("{}\n", side.text);
            let Ok(styled) = lines.highlight_line(&line, &SYNTAXES) else {
                break;
            };
            let mut at = 0;
            let mut runs = Spans::new();
            for (style, text) in styled {
                let end = (at + text.len()).min(side.text.len());
                if at < end {
                    let color = style.foreground;
                    let rgba = u32::from_be_bytes([color.r, color.g, color.b, color.a]);
                    let mut color: Hsla = gpui::rgba(rgba).into();
                    // On a dark diff, over its red and green lines, dim colors
                    // such as comments' are lifted so they still read.
                    if dark {
                        color.l = color.l.max(MIN_DARK_LIGHTNESS);
                    }
                    runs.push((at..end, color));
                }
                at += text.len();
            }
            spans.insert(side.line_number, runs);
        }
        spans
    };
    Colors {
        left: side(|row| row.left.as_ref()),
        right: side(|row| row.right.as_ref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wtf_core::DiffRow;

    #[test]
    fn colors_known_code_and_skips_the_rest() {
        let side = |n: u32, text: &str| DiffSide {
            line_number: n,
            text: text.to_string(),
            changes: Vec::new(),
        };
        let diff = FileDiff {
            language: String::new(),
            rows: vec![
                DiffRow {
                    left: Some(side(1, "fn main() {")),
                    right: Some(side(1, "fn main() {")),
                },
                DiffRow {
                    left: None,
                    right: Some(side(2, "    let s = \"hi\";")),
                },
            ],
        };
        let colors = colors("src/main.rs", &diff, false);
        let line = &colors.right[&2];
        assert_eq!(line.last().map(|run| run.0.end), Some(17));
        assert!(
            line.iter()
                .map(|run| run.1)
                .collect::<Vec<_>>()
                .windows(2)
                .any(|w| w[0] != w[1])
        );
        assert!(!colors.left.contains_key(&2));
        assert!(
            super::colors("notes.unknown-ext", &diff, false)
                .right
                .is_empty()
        );
    }
}
