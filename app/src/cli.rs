//! The `wtf` shell command: `wtf .` opens the current repository in the app,
//! as `code .` does for VS Code.

use std::ffi::CStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use gpui::{App, PromptLevel};

/// Where the command goes when the user's PATH has no folder of their own:
/// one every shell on macOS has, but only an administrator may write to.
const SYSTEM_DIR: &str = "/usr/local/bin";

/// The user's own folders for commands, in their home, that a shell may
/// put on the PATH. Folders of one tool, such as `~/.cargo/bin`, are not
/// meant for others' commands.
const USER_DIRS: [&str; 3] = [".local/bin", "bin", ".bin"];

/// A line only the script wtf installs has, so another program called wtf
/// is never overwritten.
const MARKER: &str = "# Installed by wtf.app";

/// The command hands its folders to the app through `open`, which launches
/// the app or reaches the running one, as dropping them on its Dock icon does.
/// The same file is in the app bundle, for Homebrew to link.
fn script() -> &'static str {
    include_str!("../../scripts/wtf")
}

/// Installs the command: in a folder of the user's own on their PATH, or,
/// when there is none, in the system's after saying so, since macOS then
/// asks for an administrator's password.
pub fn install(cx: &mut App) {
    let find = cx.background_executor().spawn(async { user_dir() });
    cx.spawn(async move |cx| {
        let dir = find.await;
        cx.update(|cx| match dir {
            Some(dir) => write_in_background(dir, cx),
            None => confirm_system(cx),
        })
        .ok();
    })
    .detach();
}

fn write_in_background(dir: PathBuf, cx: &mut App) {
    let task = cx.background_executor().spawn(async move {
        let target = dir.join("wtf");
        write_script(&target).map(|done| done.then_some(target))
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| report(result, cx)).ok();
    })
    .detach();
}

/// Says where the command will go and why a password is asked, and goes
/// on when the user agrees.
fn confirm_system(cx: &mut App) {
    let Some(window) = front_window(cx) else {
        return;
    };
    let detail = format!(
        "Your PATH has no folder of your own for commands, such as ~/.local/bin, \
         so wtf will put the command in {SYSTEM_DIR}, for every user of this Mac. \
         macOS will ask for an administrator's password."
    );
    window
        .update(cx, |_, window, cx| {
            let answer = window.prompt(
                PromptLevel::Info,
                &format!("Install the wtf command in {SYSTEM_DIR}?"),
                Some(&detail),
                &["Install", "Cancel"],
                cx,
            );
            cx.spawn(async move |cx| {
                if answer.await == Ok(0) {
                    cx.update(|cx| write_in_background(SYSTEM_DIR.into(), cx))
                        .ok();
                }
            })
            .detach();
        })
        .ok();
}

/// The first of the user's own command folders on the PATH their login
/// shell sets up. The app's own PATH is no guide: apps started from the
/// Finder or the Dock get only the system's folders.
fn user_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let path = login_path()?;
    pick_user_dir(&path, &home)
}

/// The first folder on `path` that is one of [`USER_DIRS`] in `home`.
fn pick_user_dir(path: &str, home: &Path) -> Option<PathBuf> {
    let own: Vec<PathBuf> = USER_DIRS.iter().map(|dir| home.join(dir)).collect();
    path.split(':')
        .map(|dir| PathBuf::from(dir.trim_end_matches('/')))
        .find(|dir| own.contains(dir))
}

/// Takes the login shell's PATH when the app was not started from a
/// terminal: started from the Finder, the Dock or `open`, it has only the
/// system's folders, and git hooks would miss the user's tools.
pub fn adopt_login_path() {
    if std::env::var_os("TERM").is_some() {
        return;
    }
    if let Some(path) = login_path() {
        // SAFETY: called first thing in main, before any other thread starts.
        unsafe { std::env::set_var("PATH", path) };
    }
}

/// The PATH of an interactive login shell, as a terminal gets it: the
/// user's shell reads its startup files, so folders they add are there.
fn login_path() -> Option<String> {
    let output = Command::new(login_shell())
        .args(["-l", "-i", "-c", "printenv PATH"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    // Startup files may print greetings first; the PATH is the last line.
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().rev().find(|line| line.contains('/'))?;
    Some(line.trim().to_string())
}

/// The user's shell as their account names it, which may differ from
/// `$SHELL`; zsh, macOS's default, when neither says.
fn login_shell() -> String {
    // SAFETY: getpwuid returns a pointer into static storage or null; the
    // shell is copied out before anything else could call it again.
    let from_account = unsafe {
        let entry = libc::getpwuid(libc::getuid());
        (!entry.is_null() && !(*entry).pw_shell.is_null()).then(|| {
            CStr::from_ptr((*entry).pw_shell)
                .to_string_lossy()
                .into_owned()
        })
    };
    from_account
        .filter(|shell| !shell.is_empty())
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/zsh".to_string())
}

/// Writes the script to `target`: `Ok(true)` when done, `Ok(false)` when
/// the user cancelled the password prompt.
fn write_script(target: &Path) -> Result<bool, String> {
    if let Ok(existing) = fs::read_to_string(target)
        && !existing.contains(MARKER)
    {
        return Err(format!(
            "{} already exists and is not wtf's. Remove it, then try again.",
            target.display()
        ));
    }
    match write_executable(target) {
        Ok(()) => return Ok(true),
        // Only the system's folder is worth a password.
        Err(error) if !target.starts_with(SYSTEM_DIR) => return Err(error),
        Err(_) => {}
    }
    // /usr/local/bin belongs to root: macOS asks for a password to copy
    // the script there.
    let staged = std::env::temp_dir().join("wtf-command");
    fs::write(&staged, script()).map_err(|error| error.to_string())?;
    let target = shell_quote(&target.to_string_lossy());
    let shell = format!(
        "mkdir -p {SYSTEM_DIR} && cp {} {target} && chmod 755 {target}",
        shell_quote(&staged.to_string_lossy())
    );
    let apple_script = format!(
        "do shell script \"{}\" with administrator privileges",
        shell.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let output = Command::new("osascript")
        .args(["-e", &apple_script])
        .output()
        .map_err(|error| error.to_string())?;
    fs::remove_file(&staged).ok();
    if output.status.success() {
        return Ok(true);
    }
    let error = String::from_utf8_lossy(&output.stderr);
    // -128 is the password prompt's Cancel.
    if error.contains("-128") {
        return Ok(false);
    }
    Err(error.trim().to_string())
}

/// Writes the script and makes it runnable, creating its folder if need be.
fn write_executable(target: &Path) -> Result<(), String> {
    let write = || -> std::io::Result<()> {
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(target, script())?;
        fs::set_permissions(target, fs::Permissions::from_mode(0o755))
    };
    write().map_err(|error| format!("Cannot write {}: {error}", target.display()))
}

/// A path for `sh`, in single quotes.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn front_window(cx: &App) -> Option<gpui::AnyWindowHandle> {
    cx.active_window().or_else(|| cx.windows().first().copied())
}

/// Says how the install went: where the command is, or what went wrong.
/// Nothing when the user cancelled.
fn report(result: Result<Option<PathBuf>, String>, cx: &mut App) {
    let (level, title, detail) = match result {
        Ok(None) => return,
        Ok(Some(target)) => (
            PromptLevel::Info,
            "The wtf command is installed",
            format!(
                "It is at {}.\n\nIn a new terminal, `wtf .` opens the repository \
                 you are in, and `wtf <path>` any other.",
                target.display()
            ),
        ),
        Err(error) => (
            PromptLevel::Critical,
            "The wtf command could not be installed",
            error,
        ),
    };
    let Some(window) = front_window(cx) else {
        eprintln!("{title}: {detail}");
        return;
    };
    window
        .update(cx, |_, window, cx| {
            // The answer needs nothing done.
            drop(window.prompt(level, title, Some(&detail), &["OK"], cx));
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_single_quotes() {
        assert_eq!(shell_quote("/tmp/it's"), r"'/tmp/it'\''s'");
    }

    #[test]
    fn script_is_marked_and_opens_the_app() {
        let script = script();
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains(MARKER));
        assert!(script.contains("open -b dev.wtf.wtf"));
    }

    #[test]
    fn picks_the_first_own_folder_on_the_path() {
        let home = Path::new("/Users/me");
        let path = "/Users/me/Library/pnpm/bin:/Users/me/.config/emacs/bin:\
                    /usr/local/bin:/Users/me/bin/:/Users/me/.local/bin:/usr/bin";
        assert_eq!(
            pick_user_dir(path, home),
            Some(PathBuf::from("/Users/me/bin"))
        );
    }

    #[test]
    fn tool_folders_and_system_ones_are_not_the_users() {
        let home = Path::new("/Users/me");
        let path = "/Users/me/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin";
        assert_eq!(pick_user_dir(path, home), None);
    }
}
