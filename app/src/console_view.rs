use wtf_core::LogEntry;

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
pub fn lines(entries: &[LogEntry], hide_refresh: bool, dir: &mut Option<String>) -> Vec<Line> {
    let mut lines = Vec::new();
    let line = |text: String, tone| Line { text, tone };
    for entry in entries
        .iter()
        .filter(|entry| !(hide_refresh && entry.refresh))
    {
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
            texts(&lines(&entries, false, &mut dir)),
            [
                "$ cd /repo",
                "$ git log  (1 ms)",
                "$ git diff  (1 ms)",
                "$ cd /repo/wt",
                "$ git status  (1 ms)"
            ]
        );
        // The next batch continues in the same directory without a cd.
        let more = lines(&[entry("/repo/wt", "git log", false)], false, &mut dir);
        assert_eq!(texts(&more), ["$ git log  (1 ms)"]);
    }

    #[test]
    fn hides_refresh_commands_and_marks_failures() {
        let mut failed = entry("/repo", "git show nope", false);
        failed.ok = false;
        failed.output = "fatal: bad revision".to_string();
        let entries = [entry("/repo", "git for-each-ref", true), failed];

        let shown = lines(&entries, true, &mut None);
        assert_eq!(
            texts(&shown),
            [
                "$ cd /repo",
                "$ git show nope  (1 ms)",
                "fatal: bad revision"
            ]
        );
        assert_eq!(shown[1].tone, Tone::Error);
        assert_eq!(lines(&entries, false, &mut None).len(), 4);
    }
}
