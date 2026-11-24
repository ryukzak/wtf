use wtf_core::{CommandKind, LogEntry};

/// Which of the app's commands the terminal prints: by kind, less those a
/// pattern hides; failures always, and with `all`, everything.
#[derive(Debug, Clone, Default)]
pub struct Shown {
    pub changes: bool,
    pub reads: bool,
    pub refresh: bool,
    /// Globs over the command line, such as `git rev-parse*`.
    pub hidden: Vec<String>,
    pub all: bool,
}

impl Shown {
    fn shows(&self, entry: &LogEntry) -> bool {
        if self.all || !entry.ok {
            return true;
        }
        let kind = match entry.kind() {
            CommandKind::Change => self.changes,
            CommandKind::Read => self.reads,
            CommandKind::Refresh => self.refresh,
        };
        kind && !self
            .hidden
            .iter()
            .any(|pattern| wtf_core::glob(pattern, &entry.command))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tone {
    Plain,
    Error,
}

/// A line the terminal prints for the app's git commands.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub tone: Tone,
}

/// A shell transcript of the log: a `cd` line whenever the directory changes.
/// `dir` is the directory of the last printed command.
pub fn lines(entries: &[LogEntry], shown: &Shown, dir: &mut Option<String>) -> Vec<Line> {
    let mut lines = Vec::new();
    let line = |text: String, tone| Line { text, tone };
    for entry in entries.iter().filter(|entry| shown.shows(entry)) {
        if dir.as_deref() != Some(entry.dir.as_str()) {
            lines.push(line(format!("$ cd {}", entry.dir), Tone::Plain));
            *dir = Some(entry.dir.clone());
        }
        let tone = if entry.ok { Tone::Plain } else { Tone::Error };
        lines.push(line(
            format!("$ {}  ({} ms)", entry.command, entry.millis),
            tone,
        ));
        lines.extend(
            entry
                .output
                .lines()
                .map(|text| line(text.to_string(), Tone::Error)),
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dir: &str, command: &str, refresh: bool) -> LogEntry {
        LogEntry {
            seq: 0,
            dir: dir.to_string(),
            command: command.to_string(),
            ok: true,
            millis: 1,
            output: String::new(),
            refresh,
        }
    }

    fn everything() -> Shown {
        Shown {
            all: true,
            ..Default::default()
        }
    }

    fn texts(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn adds_cd_when_the_directory_changes() {
        let entries = [
            entry("/repo", "git log", false),
            entry("/repo", "git diff", false),
            entry("/repo/wt", "git status", false),
        ];
        let mut dir = None;
        assert_eq!(
            texts(&lines(&entries, &everything(), &mut dir)),
            [
                "$ cd /repo",
                "$ git log  (1 ms)",
                "$ git diff  (1 ms)",
                "$ cd /repo/wt",
                "$ git status  (1 ms)"
            ]
        );
        // The next batch continues in the same directory without a cd.
        let more = lines(
            &[entry("/repo/wt", "git log", false)],
            &everything(),
            &mut dir,
        );
        assert_eq!(texts(&more), ["$ git log  (1 ms)"]);
    }

    #[test]
    fn hides_refresh_commands_and_marks_failures() {
        let mut failed = entry("/repo", "git show nope", false);
        failed.ok = false;
        failed.output = "fatal: bad revision".to_string();
        let entries = [entry("/repo", "git for-each-ref", true), failed];

        let quiet = Shown {
            changes: true,
            ..Default::default()
        };
        let shown = lines(&entries, &quiet, &mut None);
        assert_eq!(
            texts(&shown),
            [
                "$ cd /repo",
                "$ git show nope  (1 ms)",
                "fatal: bad revision"
            ]
        );
        assert_eq!(shown[1].tone, Tone::Error);
        assert_eq!(lines(&entries, &everything(), &mut None).len(), 4);
    }

    #[test]
    fn shows_commands_by_kind_less_hidden_ones() {
        let entries = [
            entry("/repo", "git add -- a", false),
            entry("/repo", "git rev-parse HEAD", false),
            entry("/repo", "git diff", false),
        ];
        let shown = Shown {
            changes: true,
            reads: true,
            hidden: vec!["git rev-parse*".to_string()],
            ..Default::default()
        };
        assert_eq!(
            texts(&lines(&entries, &shown, &mut None)),
            ["$ cd /repo", "$ git add -- a  (1 ms)", "$ git diff  (1 ms)"]
        );
    }
}
