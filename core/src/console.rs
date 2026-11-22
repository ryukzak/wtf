use std::cell::Cell;
use std::sync::Mutex;
use std::time::Instant;

const MAX_ENTRIES: usize = 5000;
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Increases with every entry.
    pub seq: u64,
    pub dir: String,
    pub command: String,
    pub ok: bool,
    pub millis: u64,
    /// The error of a failed command.
    pub output: String,
    /// Run by the periodic graph refresh.
    pub refresh: bool,
}

static LOG: Mutex<(u64, Vec<LogEntry>)> = Mutex::new((0, Vec::new()));

thread_local! {
    static REFRESH: Cell<bool> = const { Cell::new(false) };
}

/// Marks the commands `run` makes on this thread as refresh commands.
pub(crate) fn as_refresh<T>(run: impl FnOnce() -> T) -> T {
    REFRESH.set(true);
    let result = run();
    REFRESH.set(false);
    result
}

pub(crate) fn record(dir: &str, command: String, ok: bool, started: Instant, output: String) {
    let mut output = output;
    if output.len() > MAX_OUTPUT {
        let mut end = MAX_OUTPUT;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
        output.push_str("\n…");
    }
    let mut log = LOG.lock().unwrap_or_else(|error| error.into_inner());
    log.0 += 1;
    let entry = LogEntry {
        seq: log.0,
        dir: dir.to_string(),
        command,
        ok,
        millis: started.elapsed().as_millis() as u64,
        output,
        refresh: REFRESH.get(),
    };
    log.1.push(entry);
    if log.1.len() > MAX_ENTRIES {
        log.1.drain(..MAX_ENTRIES / 5);
    }
}

/// Entries recorded after `seq`, oldest first.
pub fn log_since(seq: u64) -> Vec<LogEntry> {
    let log = LOG.lock().unwrap_or_else(|error| error.into_inner());
    let start = log.1.partition_point(|entry| entry.seq <= seq);
    log.1[start..].to_vec()
}

/// A command line as it would be typed in a shell.
pub(crate) fn shell_line<S: AsRef<std::ffi::OsStr>>(program: &str, args: &[S]) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_-./=:@%^+,".contains(c);
    let mut line = program.to_string();
    for arg in args {
        let arg = arg.as_ref().to_string_lossy();
        line.push(' ');
        if !arg.is_empty() && arg.chars().all(safe) {
            line.push_str(&arg);
        } else {
            line.push('\'');
            line.push_str(&arg.replace('\'', r"'\''"));
            line.push('\'');
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_what_the_shell_needs() {
        let line = shell_line("git", &["log", "--format=%H%x1f", "it's here", ""]);
        assert_eq!(line, r"git log --format=%H%x1f 'it'\''s here' ''");
    }
}
