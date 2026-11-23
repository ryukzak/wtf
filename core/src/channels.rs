//! Channel branches: where finished work goes, such as the default branch
//! and release branches like `v2410`. A branch merged into any of them is
//! done. They are named by glob patterns kept in the repository's own git
//! config, as `wtf.channel`, so every worktree of it shares them and git
//! never commits them.

use crate::{CoreError, git};

const KEY: &str = "wtf.channel";

/// The repository's channel patterns, in the order they were added.
pub fn channel_patterns(repo_path: &str) -> Vec<String> {
    // Without any, git exits with an error: none is no error here.
    git::run(repo_path, &["config", "--local", "--get-all", KEY])
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Adds a pattern, unless the repository has it already.
pub fn add_channel(repo_path: &str, pattern: &str) -> Result<(), CoreError> {
    let pattern = pattern.trim();
    if pattern.is_empty() || channel_patterns(repo_path).iter().any(|p| p == pattern) {
        return Ok(());
    }
    git::run(repo_path, &["config", "--local", "--add", KEY, pattern]).map(|_| ())
}

/// Removes a pattern, taken as written, not as a pattern of git's.
pub fn remove_channel(repo_path: &str, pattern: &str) -> Result<(), CoreError> {
    git::run(
        repo_path,
        &[
            "config",
            "--local",
            "--fixed-value",
            "--unset-all",
            KEY,
            pattern,
        ],
    )
    .map(|_| ())
}

/// Whether a branch is a channel: the main branch, or one a pattern names.
/// A remote branch, such as `origin/v2410`, is matched without its remote.
pub fn is_channel(patterns: &[String], main: &str, name: &str, remote: bool) -> bool {
    let name = match remote {
        true => name.split_once('/').map_or(name, |(_, rest)| rest),
        false => name,
    };
    name == main || patterns.iter().any(|pattern| glob(pattern, name))
}

/// A shell-style match of the whole name: `*` any run of characters, `?`
/// any one, `[0-9]` or `[!abc]` one of a set.
pub fn glob(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    matches(&pattern, &name)
}

fn matches(pattern: &[char], name: &[char]) -> bool {
    match pattern.first() {
        None => name.is_empty(),
        Some('*') => (0..=name.len()).any(|skip| matches(&pattern[1..], &name[skip..])),
        Some('?') => !name.is_empty() && matches(&pattern[1..], &name[1..]),
        Some('[') => match (class(&pattern[1..]), name.first()) {
            (Some((hit, rest)), Some(&c)) => hit(c) && matches(rest, &name[1..]),
            // An unclosed `[` is a plain character.
            (None, Some('[')) => matches(&pattern[1..], &name[1..]),
            _ => false,
        },
        Some(&c) => name.first() == Some(&c) && matches(&pattern[1..], &name[1..]),
    }
}

/// A `[...]` set after its `[`: a test for a character, and the pattern
/// after the `]`; `None` when it never closes.
#[allow(clippy::type_complexity)]
fn class(pattern: &[char]) -> Option<(Box<dyn Fn(char) -> bool + '_>, &[char])> {
    let negated = matches!(pattern.first(), Some('!' | '^'));
    let body_start = usize::from(negated);
    // A `]` first in the set is one of its characters.
    let close = pattern
        .iter()
        .enumerate()
        .skip(body_start + 1)
        .find(|(_, c)| **c == ']')
        .map(|(i, _)| i)?;
    let body = &pattern[body_start..close];
    let test = move |c: char| {
        let mut i = 0;
        let mut hit = false;
        while i < body.len() {
            if i + 2 < body.len() && body[i + 1] == '-' {
                hit |= (body[i]..=body[i + 2]).contains(&c);
                i += 3;
            } else {
                hit |= body[i] == c;
                i += 1;
            }
        }
        hit != negated
    };
    Some((Box::new(test), &pattern[close + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_whole_names() {
        let release = "v[0-9][0-9][0-9][0-9]";
        assert!(glob(release, "v2410"));
        assert!(!glob(release, "v2410-fix/base-image"));
        assert!(!glob(release, "v241"));
        assert!(glob("*-lts", "2402-lts"));
        assert!(glob("release/*", "release/1.2"));
        assert!(glob("stable", "stable"));
        assert!(!glob("stable", "stable-2405"));
        assert!(glob("v24?0", "v2410"));
        assert!(glob("[!x]*", "abc"));
        assert!(!glob("[!a]*", "abc"));
        assert!(glob("a[", "a["));
    }

    #[test]
    fn channels_are_the_main_branch_and_matches_without_their_remote() {
        let patterns = vec!["v[0-9][0-9][0-9][0-9]".to_string()];
        assert!(is_channel(&patterns, "master", "master", false));
        assert!(is_channel(&patterns, "master", "origin/master", true));
        assert!(is_channel(&patterns, "master", "origin/v2410", true));
        assert!(is_channel(&patterns, "master", "v2410", false));
        assert!(!is_channel(&patterns, "master", "origin/v2410-fix/x", true));
        // A local name with a slash keeps it.
        assert!(!is_channel(&patterns, "master", "origin/v2410", false));
    }
}
