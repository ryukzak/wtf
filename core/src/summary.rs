//! Summaries of several commits by Apple's on-device model, through the `fm`
//! command macOS ships with it.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use crate::{Commit, CoreError};

/// What the model reads at most: its context is small, and a summary needs
/// the gist, not every line.
const MAX_INPUT: usize = 6000;

/// What the model is told when it summarizes commits, unless Settings say
/// otherwise: its instructions, then, under `---`, what it reads.
pub const SUMMARY_PROMPT: &str = "You summarize a series of git commits for someone reviewing \
them. Use only facts stated in the input. Reply with one line of at most 72 characters saying what \
the commits change together, for someone using the program or reading the document; avoid vague \
words such as update, improve, various. If more is worth saying, add a blank line and then 2 to 5 \
short bullets starting with '- ', each about a different change, not one per commit. Write in the \
language of the commit messages. Plain text: no Markdown, no code. No preamble.
---
{{commits}}
Files changed:
{{files}}
";

/// What the model is told when it writes a commit message, unless Settings
/// say otherwise. Tried against the on-device model on real commits: a
/// subject about what the change does reads better than one naming code,
/// three examples copy less than five, and without a language the model
/// takes the one of the changed text.
pub const MESSAGE_PROMPT: &str = "{{examples 3}}You write a git commit message for the change \
below. Use only facts in the input. The subject line, at most 72 characters, says what the change \
does for someone using the program or reading the document, not which code or file changed; avoid \
vague words such as update, logic, refactor, improve. Then, if the change needs it, a blank line and \
1 to 4 short bullets starting with '- ', each a plain sentence. Plain text only: no Markdown, no \
code, no backticks, no file sizes. No preamble, no label such as Subject. Write in English.
---
Files changed:
{{files}}
Diff:
{{diff}}
";

/// How much the example subjects take at most, so the diff keeps most of
/// the model's small context.
const MAX_EXAMPLES: usize = 1500;

/// What the model is asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Request {
    Summary,
    Message,
}

/// A request as the model got it, all filled in, and what came of it.
#[derive(Clone, Debug)]
pub struct Sent {
    pub instructions: String,
    pub input: String,
    /// Its answer, or why there is none.
    pub answer: Result<String, String>,
    /// When it was sent, in seconds since the epoch.
    pub time: i64,
}

/// The last request of each kind, to show what a template made.
static LAST: Mutex<[Option<Sent>; 2]> = Mutex::new([None, None]);

/// The last request of a kind sent this run.
pub fn last_sent(request: Request) -> Option<Sent> {
    LAST.lock().unwrap_or_else(|error| error.into_inner())[request as usize].clone()
}

/// How many subjects `{{examples}}` lists without a number.
pub const EXAMPLES: usize = 5;

/// A prompt template filled in, as the model's instructions and what it
/// reads: the parts above and below a line of `---`, or, without one, all
/// of it as what it reads. Each `{{name}}` or `{{name N}}` is `value`'s,
/// which `None` leaves as written.
pub fn render(
    template: &str,
    value: impl Fn(&str, Option<usize>) -> Option<String>,
) -> (String, String) {
    let (instructions, input) = split(template);
    (fill(instructions, &value), fill(input, &value))
}

/// A template's instructions and input, apart at its first `---` line.
fn split(template: &str) -> (&str, &str) {
    let mut at = 0;
    for line in template.split_inclusive('\n') {
        if line.trim() == "---" {
            return (template[..at].trim_end(), &template[at + line.len()..]);
        }
        at += line.len();
    }
    ("", template)
}

fn fill(template: &str, value: &impl Fn(&str, Option<usize>) -> Option<String>) -> String {
    let mut text = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        text.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            rest = &rest[start..];
            break;
        };
        let inside = after[..end].trim();
        let (name, count) = match inside.split_once(char::is_whitespace) {
            Some((name, count)) => (name, count.trim().parse().ok()),
            None => (inside, None),
        };
        match value(name, count) {
            Some(filled) => text.push_str(&filled),
            None => text.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    text.push_str(rest);
    text
}

/// Recent commits' subjects, one a line as `git log --format=%s` lists
/// them, as words to put before the model's instructions: examples of the
/// repository's style, as many as fit. Only subjects, ahead of the
/// instructions and called unrelated, as the on-device model otherwise
/// takes their content for the change's. Empty when there are none.
pub fn style_examples(subjects: &str) -> String {
    let mut examples = String::new();
    for subject in subjects.lines().map(str::trim).filter(|s| !s.is_empty()) {
        let example = format!("\n- {subject}");
        if examples.len() + example.len() > MAX_EXAMPLES {
            break;
        }
        examples.push_str(&example);
    }
    match examples.is_empty() {
        true => examples,
        false => format!(
            "Match the wording style of these subject lines from unrelated past commits \
             (length, capitalization, tone), but never their content:{examples}\n\n"
        ),
    }
}

/// A commit message for staged changes, as the model writes it from
/// `input`, their files and diff: its likeliest, or, `varied`, another
/// wording each time.
pub fn commit_message(instructions: &str, input: &str, varied: bool) -> Result<String, CoreError> {
    let input: String = input.chars().take(MAX_INPUT).collect();
    let text = respond(Request::Message, instructions, &input, !varied)?;
    let summary = parse(&text).ok_or_else(|| CoreError::new("The model wrote no message"))?;
    // The model sometimes labels the subject line.
    let mut message = summary
        .header
        .strip_prefix("Subject:")
        .map_or(summary.header.clone(), |subject| subject.trim().to_string());
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

/// Commits, newest first, as the model reads them: each subject, and a few
/// lines of its body.
pub fn commits_text(commits: &[Commit]) -> String {
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
    input
}

/// Summarizes commits, as the model reads them in `input`.
pub fn summarize(instructions: &str, input: &str) -> Result<Summary, CoreError> {
    let input: String = input.chars().take(MAX_INPUT).collect();
    let text = respond(Request::Summary, instructions, &input, true)?;
    parse(&text).ok_or_else(|| CoreError::new("The model gave no summary"))
}

/// The model's answer to `input` under `instructions`, without colors:
/// its likeliest one when `greedy`, else one sampled anew. Kept, with the
/// request, as the last of its kind.
fn respond(
    request: Request,
    instructions: &str,
    input: &str,
    greedy: bool,
) -> Result<String, CoreError> {
    let answer = ask(instructions, input, greedy);
    let sent = Sent {
        instructions: instructions.to_string(),
        input: input.to_string(),
        answer: answer
            .as_ref()
            .map(Clone::clone)
            .map_err(ToString::to_string),
        time: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_secs() as i64),
    };
    LAST.lock().unwrap_or_else(|error| error.into_inner())[request as usize] = Some(sent);
    answer
}

/// Runs `fm respond`.
fn ask(instructions: &str, input: &str, greedy: bool) -> Result<String, CoreError> {
    let mut child = Command::new("fm")
        .args(["respond", "--no-stream"])
        .args(greedy.then_some("--greedy"))
        .args(match instructions.is_empty() {
            true => vec![],
            false => vec!["--instructions", instructions],
        })
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
    fn style_examples_list_subjects_while_they_fit() {
        assert_eq!(style_examples(""), "");
        let examples = style_examples("Fix the bar\nAdd foo\n");
        assert!(examples.ends_with("their content:\n- Fix the bar\n- Add foo\n\n"));
        let long = format!("Short\n{}\n", "x".repeat(2000));
        assert!(style_examples(&long).ends_with("- Short\n\n"));
    }

    #[test]
    fn renders_instructions_above_the_dashes_and_input_below() {
        let value = |name: &str, count: Option<usize>| match name {
            "diff" => Some("+a".to_string()),
            "examples" => Some(format!("[{}]", count.unwrap_or(EXAMPLES))),
            _ => None,
        };
        assert_eq!(
            render(
                "{{examples}}Write.{{ examples 3 }}\n---\nDiff: {{diff}}\n",
                value
            ),
            ("[5]Write.[3]".to_string(), "Diff: +a\n".to_string())
        );
        assert_eq!(
            render("Diff: {{diff}} {{unknown}} {{open", value),
            (String::new(), "Diff: +a {{unknown}} {{open".to_string())
        );
        // Only a line of its own divides them.
        assert_eq!(
            render("a --- b", value),
            (String::new(), "a --- b".to_string())
        );
    }

    #[test]
    fn built_in_templates_keep_the_data_below_the_dashes() {
        let value = |name: &str, _| Some(format!("<{name}>"));
        let (instructions, input) = render(MESSAGE_PROMPT, value);
        assert!(instructions.starts_with("<examples>You write"));
        assert_eq!(input, "Files changed:\n<files>\nDiff:\n<diff>\n");
        let (_, input) = render(SUMMARY_PROMPT, value);
        assert_eq!(input, "<commits>\nFiles changed:\n<files>\n");
    }

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
