//! The programs wtf runs but does not ship: whether each is here and works,
//! and what to do when it does not.

use std::io::ErrorKind;
use std::process::Command;

use crate::{difft, summary};

/// How one external program fared.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCheck {
    /// The program's name, such as "difftastic".
    pub name: &'static str,
    /// What wtf needs it for.
    pub purpose: &'static str,
    /// It is here and answers.
    pub ok: bool,
    /// Its version when it works, else what is wrong.
    pub detail: String,
    /// How to fix a problem, in words.
    pub advice: Option<String>,
    /// A command that fixes it, to run in a terminal.
    pub fix: Option<String>,
}

impl ToolCheck {
    fn ok(name: &'static str, purpose: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            purpose,
            ok: true,
            detail: detail.into(),
            advice: None,
            fix: None,
        }
    }

    fn problem(
        name: &'static str,
        purpose: &'static str,
        detail: impl Into<String>,
        advice: &str,
        fix: Option<&str>,
    ) -> Self {
        Self {
            name,
            purpose,
            ok: false,
            detail: detail.into(),
            advice: Some(advice.to_string()),
            fix: fix.map(str::to_string),
        }
    }
}

/// Checks every external program. Each check runs the program, so this
/// belongs off the main thread.
pub fn check_tools() -> Vec<ToolCheck> {
    vec![check_git(), check_difftastic(), check_fm()]
}

const GIT: &str = "Git";
const GIT_PURPOSE: &str = "Everything: wtf reads and changes repositories through it.";

fn check_git() -> ToolCheck {
    match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            ToolCheck::ok(GIT, GIT_PURPOSE, first_line(&output.stdout))
        }
        // Without the Command Line Tools, /usr/bin/git is only a stub that
        // offers to install them.
        Ok(output) => ToolCheck::problem(
            GIT,
            GIT_PURPOSE,
            first_line(&output.stderr),
            "Install Apple's Command Line Tools, which include Git.",
            Some("xcode-select --install"),
        ),
        Err(_) => ToolCheck::problem(
            GIT,
            GIT_PURPOSE,
            "git is not installed.",
            "Install Apple's Command Line Tools, which include Git.",
            Some("xcode-select --install"),
        ),
    }
}

const DIFFTASTIC: &str = "difftastic";
const DIFFTASTIC_PURPOSE: &str = "Structural diffs, which compare code by its syntax.";

fn check_difftastic() -> ToolCheck {
    let install = "Install it with Homebrew; text diffs work without it.";
    let Some(path) = difft::find_difft() else {
        return ToolCheck::problem(
            DIFFTASTIC,
            DIFFTASTIC_PURPOSE,
            "difft is not installed.",
            install,
            Some("brew install difftastic"),
        );
    };
    match Command::new(&path).arg("--version").output() {
        Ok(output) if output.status.success() => ToolCheck::ok(
            DIFFTASTIC,
            DIFFTASTIC_PURPOSE,
            format!("{} at {}", first_line(&output.stdout), path.display()),
        ),
        Ok(output) => ToolCheck::problem(
            DIFFTASTIC,
            DIFFTASTIC_PURPOSE,
            format!("{} fails: {}", path.display(), first_line(&output.stderr)),
            "Reinstall it with Homebrew.",
            Some("brew reinstall difftastic"),
        ),
        Err(error) => ToolCheck::problem(
            DIFFTASTIC,
            DIFFTASTIC_PURPOSE,
            format!("{} does not run: {error}", path.display()),
            "Reinstall it with Homebrew.",
            Some("brew reinstall difftastic"),
        ),
    }
}

const FM: &str = "fm";
const FM_PURPOSE: &str =
    "Commit summaries and commit messages by Apple's on-device model. Optional.";

fn check_fm() -> ToolCheck {
    match summary::models() {
        Ok(text) => fm_status(&text),
        Err(error) if error.kind() == ErrorKind::NotFound => ToolCheck::problem(
            FM,
            FM_PURPOSE,
            "fm is not installed.",
            "It comes with macOS 26 or later on a Mac with Apple Intelligence.",
            None,
        ),
        Err(error) => ToolCheck::problem(
            FM,
            FM_PURPOSE,
            format!("fm does not run: {error}"),
            "It comes with macOS 26 or later on a Mac with Apple Intelligence.",
            None,
        ),
    }
}

/// What `fm models` says about the on-device model.
fn fm_status(models: &str) -> ToolCheck {
    if summary::system_model_ready(models) {
        return ToolCheck::ok(FM, FM_PURPOSE, "The on-device model is ready.");
    }
    // Until someone agrees to its terms, fm answers only with them.
    if models.contains("LEGAL NOTICE") || models.contains("fm license") {
        return ToolCheck::problem(
            FM,
            FM_PURPOSE,
            "Its terms have not been accepted on this Mac.",
            "Read and accept them in a terminal, as an administrator: \
             it applies to every user of this Mac.",
            Some("sudo fm license"),
        );
    }
    ToolCheck::problem(
        FM,
        FM_PURPOSE,
        "The on-device model is not ready.",
        "Turn on Apple Intelligence in System Settings and let its model \
         finish downloading.",
        None,
    )
}

/// The first non-empty line of a program's output.
fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fm_without_accepted_terms_asks_for_them() {
        let text = "YOU HAVE NOT AGREED TO THE FOUNDATION MODELS CLI LEGAL NOTICE & TERMS.\n\
                    Agreeing ... (e.g. 'sudo fm license').\n";
        let check = fm_status(text);
        assert!(!check.ok);
        assert_eq!(check.fix.as_deref(), Some("sudo fm license"));
    }

    #[test]
    fn fm_with_the_system_model_is_ready() {
        let check = fm_status("  ✓ system\n  ✗ cloud\n");
        assert!(check.ok);
        assert_eq!(check.fix, None);
    }

    #[test]
    fn fm_without_the_system_model_points_to_settings() {
        let check = fm_status("  ✗ system\n");
        assert!(!check.ok);
        assert!(check.advice.unwrap().contains("Apple Intelligence"));
    }
}
