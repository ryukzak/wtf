use std::env;
use std::fs;
use std::path::PathBuf;

/// How many repositories File › Open Recent lists.
const MAX_RECENT: usize = 10;

fn file() -> PathBuf {
    let home = env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/wtf/recent")
}

/// Recently opened repositories, newest first.
pub fn load() -> Vec<String> {
    fs::read_to_string(file())
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Puts a repository first in the list. A list that cannot be saved is
/// only a missing convenience, so errors are ignored.
pub fn remember(path: &str) {
    let mut paths = load();
    paths.retain(|known| known != path);
    paths.insert(0, path.to_string());
    paths.truncate(MAX_RECENT);
    save(&paths);
}

pub fn clear() {
    save(&[]);
}

fn save(paths: &[String]) {
    let file = file();
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).ok();
    }
    fs::write(file, paths.join("\n")).ok();
}
