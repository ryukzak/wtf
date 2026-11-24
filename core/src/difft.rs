use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::{CoreError, DiffRow, DiffSide, FileDiff, TextRange};

#[derive(Deserialize)]
struct Output {
    language: String,
    status: String,
    #[serde(default)]
    aligned_lines: Vec<(Option<usize>, Option<usize>)>,
    #[serde(default)]
    chunks: Vec<Vec<ChunkLine>>,
}

#[derive(Deserialize)]
struct ChunkLine {
    lhs: Option<ChunkSide>,
    rhs: Option<ChunkSide>,
}

#[derive(Deserialize)]
struct ChunkSide {
    line_number: usize,
    changes: Vec<Change>,
}

#[derive(Deserialize)]
struct Change {
    start: usize,
    end: usize,
}

pub fn diff(path: &str, old: &[u8], new: &[u8]) -> Result<FileDiff, CoreError> {
    let difft = find_difft().ok_or_else(|| {
        CoreError::new("difft not found. Install difftastic: brew install difftastic")
    })?;

    // difft picks the language from the file name, so both sides keep it.
    let name = Path::new(path).file_name().unwrap_or("file".as_ref());
    let dir = tempfile::tempdir().map_err(CoreError::new)?;
    let old_path = write_side(dir.path(), "old", name, old)?;
    let new_path = write_side(dir.path(), "new", name, new)?;

    let output = Command::new(difft)
        .args(["--display", "json", "--color", "never"])
        .arg(old_path)
        .arg(new_path)
        .env("DFT_UNSTABLE", "yes")
        .output()
        .map_err(CoreError::new)?;
    if !output.status.success() {
        return Err(CoreError::new(String::from_utf8_lossy(&output.stderr)));
    }
    build_file_diff(
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(old),
        &String::from_utf8_lossy(new),
    )
}

/// The one `WTF_DIFFT` names, else one on the PATH. GUI apps do not inherit
/// the shell PATH, so Homebrew prefixes are checked too.
pub(crate) fn find_difft() -> Option<PathBuf> {
    if let Some(path) = env::var_os("WTF_DIFFT") {
        return Some(path.into());
    }
    let path = env::var_os("PATH").unwrap_or_default();
    env::split_paths(&path)
        .chain(["/opt/homebrew/bin".into(), "/usr/local/bin".into()])
        .map(|dir| dir.join("difft"))
        .find(|candidate| candidate.is_file())
}

fn write_side(
    root: &Path,
    side: &str,
    name: &std::ffi::OsStr,
    content: &[u8],
) -> Result<PathBuf, CoreError> {
    let dir = root.join(side);
    fs::create_dir(&dir).map_err(CoreError::new)?;
    let path = dir.join(name);
    fs::write(&path, content).map_err(CoreError::new)?;
    Ok(path)
}

fn build_file_diff(json: &str, old: &str, new: &str) -> Result<FileDiff, CoreError> {
    let output: Output = serde_json::from_str(json).map_err(CoreError::new)?;
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // difft sends no line data when one side is empty.
    let aligned = match output.status.as_str() {
        "created" => (0..new_lines.len()).map(|n| (None, Some(n))).collect(),
        "deleted" => (0..old_lines.len()).map(|n| (Some(n), None)).collect(),
        _ => output.aligned_lines,
    };

    let mut left_changes = HashMap::new();
    let mut right_changes = HashMap::new();
    for line in output.chunks.into_iter().flatten() {
        if let Some(side) = line.lhs {
            left_changes.insert(side.line_number, side.changes);
        }
        if let Some(side) = line.rhs {
            right_changes.insert(side.line_number, side.changes);
        }
    }

    let whole_line = output.status != "changed";
    let rows = aligned
        .into_iter()
        .filter_map(|(left, right)| {
            let left = left.and_then(|n| side(n, &old_lines, &left_changes, whole_line));
            let right = right.and_then(|n| side(n, &new_lines, &right_changes, whole_line));
            (left.is_some() || right.is_some()).then_some(DiffRow { left, right })
        })
        .collect();

    Ok(FileDiff::new(output.language, rows))
}

fn side(
    number: usize,
    lines: &[&str],
    changes: &HashMap<usize, Vec<Change>>,
    whole_line: bool,
) -> Option<DiffSide> {
    let text = *lines.get(number)?;
    let changes = if whole_line {
        vec![TextRange {
            start: 0,
            end: text.len(),
        }]
    } else {
        changes.get(&number).map_or_else(Vec::new, |changes| {
            changes
                .iter()
                .map(|change| TextRange {
                    start: char_boundary(text, change.start),
                    end: char_boundary(text, change.end),
                })
                .collect()
        })
    };
    Some(DiffSide {
        line_number: number as u32 + 1,
        text: text.to_string(),
        changes,
    })
}

/// Clamps a difft byte offset into the line and onto a char boundary.
fn char_boundary(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: usize, end: usize) -> TextRange {
        TextRange { start, end }
    }

    #[test]
    fn aligns_rows_and_keeps_unchanged_lines() {
        let json = r#"{"aligned_lines":[[0,0],[null,1],[1,2],[2,3]],
            "chunks":[[{"rhs":{"line_number":1,"changes":[{"start":0,"end":3}]}}]],
            "language":"Rust","status":"changed"}"#;
        let diff = build_file_diff(json, "a\nb\n", "a\nnew\nb\n").unwrap();

        assert_eq!(diff.language, "Rust");
        assert_eq!(diff.rows.len(), 3, "the past-the-end line is dropped");
        assert_eq!(diff.rows[0].left.as_ref().unwrap().text, "a");
        assert!(diff.rows[0].left.as_ref().unwrap().changes.is_empty());
        assert_eq!(diff.rows[1].left, None);
        let added = diff.rows[1].right.as_ref().unwrap();
        assert_eq!(added.line_number, 2);
        assert_eq!(added.changes, vec![range(0, 3)]);
    }

    #[test]
    fn keeps_byte_offsets() {
        let json = r#"{"aligned_lines":[[0,0]],
            "chunks":[[{"lhs":{"line_number":0,"changes":[{"start":15,"end":16}]},
                        "rhs":{"line_number":0,"changes":[{"start":15,"end":17}]}}]],
            "language":"Rust","status":"changed"}"#;
        let diff = build_file_diff(json, "\"привет\" a\n", "\"привет\" bc\n").unwrap();

        assert_eq!(
            diff.rows[0].left.as_ref().unwrap().changes,
            vec![range(15, 16)]
        );
        assert_eq!(
            diff.rows[0].right.as_ref().unwrap().changes,
            vec![range(15, 17)]
        );
    }

    #[test]
    fn offsets_outside_the_line_are_clamped() {
        assert_eq!(char_boundary("ab", 99), 2);
        assert_eq!(char_boundary("п", 1), 0);
    }

    #[test]
    fn created_file_is_fully_highlighted() {
        let json = r#"{"language":"Rust","status":"created"}"#;
        let diff = build_file_diff(json, "", "fn a() {}\nx\n").unwrap();

        assert_eq!(diff.rows.len(), 2);
        assert_eq!(diff.rows[0].left, None);
        assert_eq!(
            diff.rows[0].right.as_ref().unwrap().changes,
            vec![range(0, 9)]
        );
    }

    #[test]
    fn deleted_file_is_fully_highlighted() {
        let json = r#"{"language":"Rust","status":"deleted"}"#;
        let diff = build_file_diff(json, "gone\n", "").unwrap();

        assert_eq!(diff.rows.len(), 1);
        assert_eq!(diff.rows[0].right, None);
        assert_eq!(
            diff.rows[0].left.as_ref().unwrap().changes,
            vec![range(0, 4)]
        );
    }

    #[test]
    fn unchanged_file_has_no_rows() {
        let json = r#"{"language":"Text","status":"unchanged"}"#;
        assert!(
            build_file_diff(json, "same\n", "same\n")
                .unwrap()
                .rows
                .is_empty()
        );
    }
}
