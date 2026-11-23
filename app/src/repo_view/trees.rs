//! Folder trees of file paths and branch names.

use super::*;

/// The file list's rows: folders first, then files, by name, or the two
/// mixed by name as Settings choose; a worktree's files under their
/// sections. Folded folders hide what is in them.
pub(super) fn file_tree(
    files: &[ChangedFile],
    split: Option<usize>,
    collapsed: &HashSet<String>,
) -> Vec<TreeRow> {
    // A section's files as a tree of names, its folders keyed under it.
    let section_rows = |range: std::ops::Range<usize>, section: &str, rows: &mut Vec<TreeRow>| {
        let mut indices: Vec<usize> = range.collect();
        indices.sort_by_key(|&index| files[index].path());
        let names: Vec<&str> = indices.iter().map(|&index| files[index].path()).collect();
        let folded: HashSet<String> = collapsed
            .iter()
            .filter_map(|key| key.strip_prefix(section).map(str::to_string))
            .collect();
        let mixed = !crate::preferences::folders_first();
        rows.extend(
            name_tree(&names, &folded, mixed)
                .into_iter()
                .map(|row| match row {
                    NameRow::Folder { path, label, depth } => TreeRow::Dir {
                        key: format!("{section}{path}"),
                        label: label.trim_end_matches('/').to_string(),
                        depth,
                    },
                    NameRow::Leaf { index, name, depth } => TreeRow::File {
                        index: indices[index],
                        name,
                        depth,
                    },
                }),
        );
    };
    let mut rows = Vec::new();
    match split {
        None => section_rows(0..files.len(), "", &mut rows),
        Some(split) => {
            let split = split.min(files.len());
            for (staged, range) in [(false, 0..split), (true, split..files.len())] {
                rows.push(TreeRow::Section(staged, range.len()));
                let section = if staged { "staged:" } else { "unstaged:" };
                section_rows(range, section, &mut rows);
            }
        }
    }
    rows
}

/// A renamed or copied file's old name, as the file list shows it next to
/// the new one: by its name alone when its folder stayed the same.
pub(super) fn old_name(file: &ChangedFile) -> Option<&str> {
    let (old, new) = (file.old_path.as_deref()?, file.new_path.as_deref()?);
    if old == new {
        return None;
    }
    let folder = |path: &str| path.rfind('/').map_or(0, |i| i + 1);
    Some(match old[..folder(old)] == new[..folder(new)] {
        true => &old[folder(old)..],
        false => old,
    })
}

/// Names as a tree of their `/` folders: folders first, by name, then the
/// names in the given order; or, `mixed`, folders and names together by
/// name. Folders in `folded` hide what is in them.
pub(super) fn name_tree(names: &[&str], folded: &HashSet<String>, mixed: bool) -> Vec<NameRow> {
    #[derive(Default)]
    struct Node {
        folders: std::collections::BTreeMap<String, Node>,
        leaves: Vec<(String, usize)>,
    }
    fn emit(
        node: &Node,
        path: &str,
        depth: usize,
        folded: &HashSet<String>,
        mixed: bool,
        rows: &mut Vec<NameRow>,
    ) {
        // Folders as `Ok`, names as `Err`; mixed, they go by name together.
        let mut entries: Vec<(&str, Result<&Node, usize>)> = node
            .folders
            .iter()
            .map(|(name, inner)| (name.as_str(), Ok(inner)))
            .chain(
                node.leaves
                    .iter()
                    .map(|(name, index)| (name.as_str(), Err(*index))),
            )
            .collect();
        if mixed {
            entries.sort_by(|a, b| a.0.cmp(b.0));
        }
        for (name, entry) in entries {
            let mut inner = match entry {
                Ok(inner) => inner,
                Err(index) => {
                    rows.push(NameRow::Leaf {
                        index,
                        name: name.to_string(),
                        depth,
                    });
                    continue;
                }
            };
            let mut label = format!("{name}/");
            while inner.leaves.is_empty() && inner.folders.len() == 1 {
                let (next, deeper) = inner.folders.iter().next().unwrap();
                label = format!("{label}{next}/");
                inner = deeper;
            }
            let path = format!("{path}{label}");
            let open = !folded.contains(&path);
            rows.push(NameRow::Folder {
                path: path.clone(),
                label,
                depth,
            });
            if open {
                emit(inner, &path, depth + 1, folded, mixed, rows);
            }
        }
    }
    let mut root = Node::default();
    for (index, name) in names.iter().enumerate() {
        let mut parts: Vec<&str> = name.split('/').collect();
        let leaf = parts.pop().unwrap_or_default().to_string();
        let mut node = &mut root;
        for part in parts {
            node = node.folders.entry(part.to_string()).or_default();
        }
        node.leaves.push((leaf, index));
    }
    let mut rows = Vec::new();
    emit(&root, "", 0, folded, mixed, &mut rows);
    rows
}
