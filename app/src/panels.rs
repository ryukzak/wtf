//! The sizes the panels were last dragged to, kept for the next tab and the
//! next start, in the file `panels` as `key=value` lines.

use std::collections::HashMap;
use std::fs;

/// Panel sizes in pixels; `None` for a size left to fit its content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sizes {
    pub sidebar: f32,
    pub dock: f32,
    pub graph: Option<f32>,
    pub commits: f32,
    pub snapshot_header: Option<f32>,
    pub commit_header: Option<f32>,
}

/// The sizes saved, each over its default in `sizes` when there is one.
pub fn load(sizes: Sizes) -> Sizes {
    let text = fs::read_to_string(crate::recent::support_file("panels")).unwrap_or_default();
    parse(&text, sizes)
}

/// Saves the sizes; one that cannot be saved is only a missing convenience.
pub fn save(sizes: Sizes) {
    let file = crate::recent::support_file("panels");
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).ok();
    }
    fs::write(file, format(sizes)).ok();
}

fn format(sizes: Sizes) -> String {
    let mut text = format!(
        "sidebar={}\ndock={}\ncommits={}\n",
        sizes.sidebar, sizes.dock, sizes.commits
    );
    if let Some(graph) = sizes.graph {
        text.push_str(&format!("graph={graph}\n"));
    }
    if let Some(header) = sizes.snapshot_header {
        text.push_str(&format!("snapshot_header={header}\n"));
    }
    if let Some(header) = sizes.commit_header {
        text.push_str(&format!("commit_header={header}\n"));
    }
    text
}

fn parse(text: &str, defaults: Sizes) -> Sizes {
    let values: HashMap<&str, f32> = text
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.trim(), value.trim().parse().ok()?))
        })
        .filter(|(_, value): &(&str, f32)| value.is_finite() && *value > 0.)
        .collect();
    Sizes {
        sidebar: values.get("sidebar").copied().unwrap_or(defaults.sidebar),
        dock: values.get("dock").copied().unwrap_or(defaults.dock),
        graph: values.get("graph").copied().or(defaults.graph),
        commits: values.get("commits").copied().unwrap_or(defaults.commits),
        snapshot_header: values
            .get("snapshot_header")
            .copied()
            .or(defaults.snapshot_header),
        commit_header: values
            .get("commit_header")
            .copied()
            .or(defaults.commit_header),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_survive_saving_and_loading() {
        let defaults = Sizes {
            sidebar: 260.,
            dock: 200.,
            graph: None,
            commits: 300.,
            snapshot_header: None,
            commit_header: None,
        };
        let dragged = Sizes {
            graph: Some(340.),
            commits: 420.,
            commit_header: Some(120.),
            ..defaults
        };
        assert_eq!(parse(&format(dragged), defaults), dragged);
        assert_eq!(parse("commits=x\ndock=-3\n", defaults), defaults);
    }
}
