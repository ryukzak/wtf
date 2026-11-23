//! Folder trees of file paths and branch names.

use super::*;

/// The file list's rows: folders first, then files, by name; a worktree's
/// files under their sections. Folded folders hide what is in them.
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
        rows.extend(name_tree(&names, &folded).into_iter().map(|row| match row {
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
        }));
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

/// Branch names as a tree of their `/` folders: folders first, by name, then
/// branches in the given order; folders in `folded` hide what is in them.
pub(super) fn name_tree(names: &[&str], folded: &HashSet<String>) -> Vec<NameRow> {
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
        rows: &mut Vec<NameRow>,
    ) {
        for (name, mut inner) in &node.folders {
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
                emit(inner, &path, depth + 1, folded, rows);
            }
        }
        for (name, index) in &node.leaves {
            rows.push(NameRow::Leaf {
                index: *index,
                name: name.clone(),
                depth,
            });
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
    emit(&root, "", 0, folded, &mut rows);
    rows
}
