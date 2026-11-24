use similar::{ChangeTag, DiffOp, TextDiff};

use crate::{DiffRow, DiffSide, FileDiff, TextRange};

pub fn diff(old: &[u8], new: &[u8]) -> FileDiff {
    if old.contains(&0) || new.contains(&0) {
        return FileDiff::new("Binary".to_string(), Vec::new());
    }
    let old = String::from_utf8_lossy(old);
    let new = String::from_utf8_lossy(new);
    let lines = TextDiff::from_lines(old.as_ref(), new.as_ref());
    let old_lines: Vec<&str> = lines.iter_old_slices().collect();
    let new_lines: Vec<&str> = lines.iter_new_slices().collect();

    let mut rows = Vec::new();
    for op in lines.ops() {
        match *op {
            DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for n in 0..len {
                    rows.push(DiffRow {
                        left: Some(side(old_index + n, old_lines[old_index + n], Vec::new())),
                        right: Some(side(new_index + n, new_lines[new_index + n], Vec::new())),
                    });
                }
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => rows.extend((old_index..old_index + old_len).map(|n| DiffRow {
                left: Some(whole_line(n, old_lines[n])),
                right: None,
            })),
            DiffOp::Insert {
                new_index, new_len, ..
            } => rows.extend((new_index..new_index + new_len).map(|n| DiffRow {
                left: None,
                right: Some(whole_line(n, new_lines[n])),
            })),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                for n in 0..old_len.max(new_len) {
                    let old_n = (n < old_len).then_some(old_index + n);
                    let new_n = (n < new_len).then_some(new_index + n);
                    rows.push(match (old_n, new_n) {
                        (Some(l), Some(r)) => {
                            let (left, right) = word_changes(old_lines[l], new_lines[r]);
                            DiffRow {
                                left: Some(side(l, old_lines[l], left)),
                                right: Some(side(r, new_lines[r], right)),
                            }
                        }
                        _ => DiffRow {
                            left: old_n.map(|l| whole_line(l, old_lines[l])),
                            right: new_n.map(|r| whole_line(r, new_lines[r])),
                        },
                    });
                }
            }
        }
    }

    FileDiff::new("Text".to_string(), rows)
}

fn trimmed(line: &str) -> &str {
    line.trim_end_matches(['\n', '\r'])
}

fn side(number: usize, line: &str, changes: Vec<TextRange>) -> DiffSide {
    DiffSide {
        line_number: number as u32 + 1,
        text: trimmed(line).to_string(),
        changes,
    }
}

fn whole_line(number: usize, line: &str) -> DiffSide {
    let end = trimmed(line).len();
    side(number, line, vec![TextRange { start: 0, end }])
}

fn word_changes(old: &str, new: &str) -> (Vec<TextRange>, Vec<TextRange>) {
    let (old, new) = (trimmed(old), trimmed(new));
    let words = TextDiff::from_words(old, new);
    let (mut left, mut right) = (Vec::new(), Vec::new());
    let (mut left_at, mut right_at) = (0, 0);
    for change in words.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                left_at += len;
                right_at += len;
            }
            ChangeTag::Delete => {
                push_range(&mut left, left_at, left_at + len);
                left_at += len;
            }
            ChangeTag::Insert => {
                push_range(&mut right, right_at, right_at + len);
                right_at += len;
            }
        }
    }
    (left, right)
}

fn push_range(ranges: &mut Vec<TextRange>, start: usize, end: usize) {
    match ranges.last_mut() {
        Some(last) if last.end == start => last.end = end,
        _ => ranges.push(TextRange { start, end }),
    }
}

/// Where line `line` (from 0) of `old` is in `new`: the same line when it
/// is kept, else where the change that replaced it begins.
pub fn carry_line(old: &str, new: &str, line: usize) -> usize {
    let diff = TextDiff::from_lines(old, new);
    for op in diff.ops() {
        let (tag, before, after) = op.as_tag_tuple();
        if before.contains(&line) {
            return match tag {
                similar::DiffTag::Equal => after.start + (line - before.start),
                _ => after.start,
            };
        }
    }
    new.lines().count().min(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: usize, end: usize) -> TextRange {
        TextRange { start, end }
    }

    #[test]
    fn keeps_unchanged_lines_and_highlights_changed_words() {
        let diff = diff(b"same\nlet a = 1;\n", b"same\nlet b = 1;\n");

        assert_eq!(diff.language, "Text");
        assert_eq!(diff.rows.len(), 2);
        assert!(diff.rows[0].left.as_ref().unwrap().changes.is_empty());
        assert_eq!(
            diff.rows[1].left.as_ref().unwrap().changes,
            vec![range(4, 5)]
        );
        assert_eq!(
            diff.rows[1].right.as_ref().unwrap().changes,
            vec![range(4, 5)]
        );
    }

    #[test]
    fn word_offsets_are_bytes() {
        let diff = diff("привет a\n".as_bytes(), "привет bc\n".as_bytes());

        assert_eq!(
            diff.rows[0].left.as_ref().unwrap().changes,
            vec![range(13, 14)]
        );
        assert_eq!(
            diff.rows[0].right.as_ref().unwrap().changes,
            vec![range(13, 15)]
        );
    }

    #[test]
    fn added_and_removed_lines_fill_one_side() {
        let added = diff(b"a\n", b"a\nb\n");
        assert_eq!(added.rows[1].left, None);
        assert_eq!(added.rows[1].right.as_ref().unwrap().line_number, 2);
        assert_eq!(
            added.rows[1].right.as_ref().unwrap().changes,
            vec![range(0, 1)]
        );

        let removed = diff(b"a\nb\n", b"a\n");
        assert_eq!(removed.rows[1].right, None);
        assert_eq!(
            removed.rows[1].left.as_ref().unwrap().changes,
            vec![range(0, 1)]
        );
    }

    #[test]
    fn longer_replacement_leaves_gaps_on_the_short_side() {
        let diff = diff(b"one\n", b"two\nthree\n");

        assert_eq!(diff.rows.len(), 2);
        assert!(diff.rows[0].left.is_some());
        assert_eq!(diff.rows[1].left, None);
        assert_eq!(diff.rows[1].right.as_ref().unwrap().text, "three");
    }

    #[test]
    fn binary_content_has_no_rows() {
        let diff = diff(b"a\0b", b"a\0c");
        assert_eq!(diff.language, "Binary");
        assert!(diff.rows.is_empty());
    }

    #[test]
    fn carries_a_line_across_versions() {
        let old = "a\nb\nc\nd\n";
        let new = "new\nnew\na\nb\nd\n";
        assert_eq!(carry_line(old, new, 0), 2, "kept, two lines lower");
        assert_eq!(carry_line(old, new, 3), 4);
        assert_eq!(
            carry_line(old, new, 2),
            4,
            "deleted: where the change begins"
        );
    }
}
