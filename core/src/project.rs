//! Settings of a project, the kind that differ from one repository to
//! another, kept in git config: wtf's own as `wtf.<key>`, and git's where
//! git has one, such as `fetch.prune`. The global config's are for every
//! repository, a repository's own over them. As git reads them, the nearest
//! wins, and every worktree of a repository shares its own.

use crate::{CoreError, git};

/// Where a project setting is kept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConfigScope<'a> {
    /// The user's global git config, for every repository.
    Global,
    /// A repository's own config.
    Repo(&'a str),
}

impl ConfigScope<'_> {
    /// Where to run git, and the option naming the config file.
    fn place(self) -> (String, &'static str) {
        match self {
            ConfigScope::Global => (
                std::env::var("HOME").unwrap_or_else(|_| "/".into()),
                "--global",
            ),
            ConfigScope::Repo(repo) => (repo.to_string(), "--local"),
        }
    }
}

/// The values `scope` keeps for `key`, such as `wtf.channel`, as added;
/// none when it has none.
pub fn project_values(scope: ConfigScope, key: &str) -> Vec<String> {
    let (dir, file) = scope.place();
    // Without any, git exits with an error: none is no error here. Values
    // end with NUL, as they may hold line breaks.
    git::run(&dir, &["config", file, "--null", "--get-all", key])
        .map(|text| {
            text.split('\0')
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The one value `scope` keeps for `key`, if any.
pub fn project_value(scope: ConfigScope, key: &str) -> Option<String> {
    project_values(scope, key).pop()
}

/// Replaces the values `scope` keeps for `key`; none drops it, for
/// the setting to come from further out again.
pub fn set_project_values(scope: ConfigScope, key: &str, values: &[&str]) -> Result<(), CoreError> {
    let (dir, file) = scope.place();
    if !project_values(scope, key).is_empty() {
        git::run(&dir, &["config", file, "--unset-all", key])?;
    }
    for value in values {
        git::run(&dir, &["config", file, "--add", key, value])?;
    }
    Ok(())
}

/// The days after which a branch without commits is idle, as `repo` goes
/// by them: its own, else the global ones.
pub fn idle_days(repo: &str) -> Option<u32> {
    git::run(repo, &["config", "--get", "wtf.idleDays"])
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_values_of_several_lines_and_drops_them() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_string_lossy().into_owned();
        git::run(&repo, &["init", "-q"]).unwrap();
        let scope = ConfigScope::Repo(&repo);
        assert_eq!(project_value(scope, "wtf.messageTemplate"), None);
        let template = "Write it.\n\n{{diff}}\n";
        set_project_values(scope, "wtf.messageTemplate", &[template]).unwrap();
        assert_eq!(
            project_value(scope, "wtf.messageTemplate").as_deref(),
            Some(template)
        );
        set_project_values(scope, "wtf.textDiff", &["*.lock", "*.snap"]).unwrap();
        assert_eq!(project_values(scope, "wtf.textDiff"), ["*.lock", "*.snap"]);
        set_project_values(scope, "wtf.idleDays", &["30"]).unwrap();
        assert_eq!(idle_days(&repo), Some(30));
        set_project_values(scope, "wtf.messageTemplate", &[]).unwrap();
        set_project_values(scope, "wtf.messageTemplate", &[]).unwrap();
        assert_eq!(project_value(scope, "wtf.messageTemplate"), None);
    }
}
