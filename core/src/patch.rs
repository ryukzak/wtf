use std::collections::HashSet;

use similar::{ChangeTag, TextDiff};

/// Context lines around each change, as `git diff` writes.
const CONTEXT: usize = 3;

/// A unified patch from `from` to `to` with only some changes: the removed
/// `from` lines and added `to` lines whose 1-based numbers are selected.
/// Unselected removals stay as context; unselected additions are dropped.
/// `None` when nothing selected changes.
pub fn partial(
    path: &str,
    from: &str,
    to: &str,
    removed: &HashSet<u32>,
    added: &HashSet<u32>,
) -> Option<String> {
    // Each line as it goes into the patch: ' ', '-' or '+' and its text.
    let mut lines: Vec<(char, &str)> = Vec::new();
    let diff = TextDiff::from_lines(from, to);
    for change in diff.iter_all_changes() {
        let text = change.value();
        match change.tag() {
            ChangeTag::Equal => lines.push((' ', text)),
            ChangeTag::Delete => {
                let number = change.old_index().map_or(0, |i| i as u32 + 1);
                lines.push((if removed.contains(&number) { '-' } else { ' ' }, text));
            }
            ChangeTag::Insert => {
                let number = change.new_index().map_or(0, |i| i as u32 + 1);
                if added.contains(&number) {
                    lines.push(('+', text));
                }
            }
        }
    }
    let changed: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].0 != ' ').collect();
    if changed.is_empty() {
        return None;
    }

    // Changes close enough to share context form one hunk.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &i in &changed {
        let start = i.saturating_sub(CONTEXT);
        let end = (i + CONTEXT + 1).min(lines.len());
        match ranges.last_mut() {
            Some(last) if start <= last.1 => last.1 = end,
            _ => ranges.push((start, end)),
        }
    }

    let mut patch = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n");
    // Line numbers before each patch line, on both sides.
    let mut from_line = 1;
    let mut to_line = 1;
    let mut at = 0;
    for (start, end) in ranges {
        for &(kind, _) in &lines[at..start] {
            from_line += usize::from(kind != '+');
            to_line += usize::from(kind != '-');
        }
        let hunk = &lines[start..end];
        let from_count = hunk.iter().filter(|(kind, _)| *kind != '+').count();
        let to_count = hunk.iter().filter(|(kind, _)| *kind != '-').count();
        // An empty side is numbered from the line before it, as git does.
        let from_start = if from_count == 0 {
            from_line - 1
        } else {
            from_line
        };
        let to_start = if to_count == 0 { to_line - 1 } else { to_line };
        patch.push_str(&format!(
            "@@ -{from_start},{from_count} +{to_start},{to_count} @@\n"
        ));
        for &(kind, text) in hunk {
            patch.push(kind);
            patch.push_str(text);
            if !text.ends_with('\n') {
                patch.push_str("\n\\ No newline at end of file\n");
            }
        }
        from_line += from_count;
        to_line += to_count;
        at = end;
    }
    Some(patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(numbers: &[u32]) -> HashSet<u32> {
        numbers.iter().copied().collect()
    }

    #[test]
    fn keeps_only_selected_changes() {
        let from = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
        let to = "1\nTWO\n3\n4\n5\n6\n7\n8\nNINE\n10\n";
        let patch = partial("f.txt", from, to, &set(&[2]), &set(&[2])).unwrap();
        assert!(patch.contains("-2\n+TWO\n"));
        assert!(!patch.contains("NINE"), "the other change is left out");
        assert!(patch.contains("@@ -1,5 +1,5 @@"));
    }

    #[test]
    fn unselected_removal_stays_as_context() {
        let patch = partial("f.txt", "a\nb\nc\n", "c\n", &set(&[1]), &set(&[])).unwrap();
        assert!(patch.contains("-a\n b\n c\n"));
        assert!(patch.contains("@@ -1,3 +1,2 @@"));
    }

    #[test]
    fn new_file_lines_and_missing_newline() {
        let patch = partial("new.txt", "", "x\ny", &set(&[]), &set(&[2])).unwrap();
        assert!(patch.contains("@@ -0,0 +1,1 @@\n+y\n\\ No newline at end of file\n"));
    }

    #[test]
    fn nothing_selected_is_no_patch() {
        assert_eq!(partial("f", "a\n", "b\n", &set(&[]), &set(&[])), None);
    }
}
