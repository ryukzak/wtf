//! Summaries of several commits by Apple's on-device model, through the `fm`
//! command macOS ships with it.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::{Commit, CoreError};

/// What the model reads at most: its context is small, and a summary needs
/// the gist, not every line.
const MAX_INPUT: usize = 6000;

/// What the model is told when it summarizes commits, unless Settings say
/// otherwise.
pub const SUMMARY_PROMPT: &str = "You summarize a series of git commits for a code reviewer. \
Use only facts stated in the input. Reply with one line of at most 72 characters saying \
what the commits do together. If more is worth saying, add a blank line and then 2 to 5 \
short bullets starting with '- '. No preamble.";

/// What the model is told when it writes a commit message, unless Settings
/// say otherwise.
pub const MESSAGE_PROMPT: &str = "You write a git commit message for the staged changes \
below. Use only facts in the input. Reply with a subject line of at most 72 characters in \
the imperative mood, without a trailing period. If the changes need it, add a blank line \
and then 1 to 4 short bullets starting with '- ', each a plain sentence about what changed. \
Plain text only: no Markdown, no code blocks, no backticks, no code or constants copied from \
the diff. No preamble, no quotes.";

/// A commit message for staged changes, from their files and diff.
pub fn commit_message(changes: &str, prompt: &str) -> Result<String, CoreError> {
    let input: String = changes.chars().take(MAX_INPUT).collect();
    let text = respond(prompt, &input)?;
    let summary = parse(&text).ok_or_else(|| CoreError::new("The model wrote no message"))?;
    let mut message = summary.header;
    if !summary.details.is_empty() {
        message.push_str("\n\n");
        let bullets: Vec<String> = summary.details.iter().map(|d| format!("- {d}")).collect();
        message.push_str(&bullets.join("\n"));
    }
    Ok(message)
}

/// A summary: a line that always shows, and bullets when there is more.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub header: String,
    pub details: Vec<String>,
}

/// What `fm models` says, on either stream and without colors. It exits
/// with an error when any model is missing, as the cloud one often is, so
/// its status says little.
pub(crate) fn models() -> std::io::Result<String> {
    let output = Command::new("fm")
        .arg("models")
        .env("NO_COLOR", "1")
        .output()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(strip_ansi(&text))
}

/// Whether `fm models` ticks the on-device model.
pub(crate) fn system_model_ready(models: &str) -> bool {
    models
        .lines()
        .any(|line| line.trim_start().starts_with("✓ system"))
}

/// Summarizes commits, newest first, with the files their changes touch.
pub fn summarize(commits: &[Commit], files: &str, prompt: &str) -> Result<Summary, CoreError> {
    let mut input = String::new();
    for commit in commits {
        input.push_str(&format!("- {}\n", commit.summary));
        for line in commit
            .body
            .lines()
            .filter(|line| !line.trim().is_empty())
            .take(6)
        {
            input.push_str(&format!("  {line}\n"));
        }
    }
    input.push_str("\nFiles changed:\n");
    input.push_str(files);
    let input: String = input.chars().take(MAX_INPUT).collect();
    let text = respond(prompt, &input)?;
    parse(&text).ok_or_else(|| CoreError::new("The model gave no summary"))
}

/// The model's answer to `input` under `instructions`, without colors.
fn respond(instructions: &str, input: &str) -> Result<String, CoreError> {
    let mut child = Command::new("fm")
        .args([
            "respond",
            "--no-stream",
            "--greedy",
            "--instructions",
            instructions,
        ])
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(CoreError::new)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).map_err(CoreError::new)?;
    }
    let output = child.wait_with_output().map_err(CoreError::new)?;
    if !output.status.success() {
        let error = strip_ansi(&String::from_utf8_lossy(&output.stderr));
        return Err(CoreError::new(error.trim()));
    }
    Ok(strip_ansi(&String::from_utf8_lossy(&output.stdout)))
}

/// The first line as the header, then each bullet.
fn parse(text: &str) -> Option<Summary> {
    // Code the model quotes despite being told not to is left out, fences
    // and all, even when it marks their lines as bullets.
    let mut fenced = false;
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|line| {
            let fence = line
                .trim_start_matches(['-', '•', '*', ' '])
                .starts_with("```");
            fenced ^= fence;
            !fence && !fenced
        })
        .filter(|line| !line.is_empty());
    let header = lines
        .next()?
        .trim_start_matches(['-', '•', '*', ' '])
        .to_string();
    let details = lines
        .map(|line| line.trim_start_matches(['-', '•', '*', ' ']).to_string())
        .filter(|line| !line.is_empty())
        .collect();
    Some(Summary { header, details })
}

/// Text without terminal color codes, which `fm` may print anyway.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_out_quoted_code() {
        let text = "Update graph constants\n\n- ```\n- const FORK_GAP: f32 = 22.;\n- ```\n\
                    - Stack forked worktrees\n";
        assert_eq!(
            parse(text),
            Some(Summary {
                header: "Update graph constants".to_string(),
                details: vec!["Stack forked worktrees".to_string()],
            })
        );
    }

    #[test]
    fn reads_a_header_and_bullets() {
        let text = "\x1b[1mAdds tabs and summaries.\x1b[0m\n\n- Tabs come back  \n• Summaries\n";
        assert_eq!(
            parse(&strip_ansi(text)),
            Some(Summary {
                header: "Adds tabs and summaries.".to_string(),
                details: vec!["Tabs come back".to_string(), "Summaries".to_string()],
            })
        );
        assert_eq!(
            parse("Only a line\n").unwrap().details,
            Vec::<String>::new()
        );
        assert_eq!(parse("  \n"), None);
    }
}
