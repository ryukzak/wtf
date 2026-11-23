use std::env;
use std::fs;
use std::path::PathBuf;

/// How many repositories File › Open Recent lists.
const MAX_RECENT: usize = 10;

/// A file of the app's own, such as its settings, in Application Support.
pub fn support_file(name: &str) -> PathBuf {
    let home = env::var("HOME").unwrap_or_default();
    PathBuf::from(home)
        .join("Library/Application Support/wtf")
        .join(name)
}

/// The lines saved in the file `name`, newest first.
pub fn load_list(name: &str) -> Vec<String> {
    fs::read_to_string(support_file(name))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Saves lines to the file `name`. A list that cannot be saved is only a
/// missing convenience, so errors are ignored.
pub fn save_list(name: &str, lines: &[String]) {
    let file = support_file(name);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).ok();
    }
    fs::write(file, lines.join("\n")).ok();
}

/// Puts `item` first in `list`, without repeats, keeping at most `max`.
pub fn put_first(list: &mut Vec<String>, item: &str, max: usize) {
    list.retain(|known| known != item);
    list.insert(0, item.to_string());
    list.truncate(max);
}

/// Recently opened repositories, newest first.
pub fn load() -> Vec<String> {
    load_list("recent")
}

/// Puts a repository first in the list.
pub fn remember(path: &str) {
    let mut paths = load();
    put_first(&mut paths, path, MAX_RECENT);
    save_list("recent", &paths);
}

pub fn clear() {
    save_list("recent", &[]);
}
