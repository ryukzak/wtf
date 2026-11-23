//! The Pull and Push buttons' options, as Sublime Merge offers them.

/// A pull or push option: its git arguments and what it does.
pub struct SyncOption {
    pub args: &'static [&'static str],
    pub about: &'static str,
}

/// The Pull (`false`) or Push (`true`) options.
pub fn options(push: bool) -> &'static [SyncOption] {
    match push {
        true => &PUSH,
        false => &PULL,
    }
}

/// Fetching alone is the Fetch button's: Pull always pulls.
pub const PULL: [SyncOption; 4] = [
    SyncOption {
        args: &["pull"],
        about: "Retrieves changes from the remote and merges them into the current branch",
    },
    SyncOption {
        args: &["pull", "--ff-only"],
        about: "Retrieves changes from the remote and fast-forwards the current branch",
    },
    SyncOption {
        args: &["pull", "--rebase"],
        about: "Retrieves changes from the remote and rebases the current branch onto them",
    },
    SyncOption {
        args: &["pull", "--rebase", "--autostash"],
        about: "Stashes any uncommitted changes before running pull --rebase",
    },
];

/// Push options after `push`; a plain `--force` is left out on purpose.
pub const PUSH: [SyncOption; 3] = [
    SyncOption {
        args: &[],
        about: "Sends the current branch to its remote; the first push sets where",
    },
    SyncOption {
        args: &["--force-with-lease"],
        about: "Replaces the remote branch, unless it changed since the last fetch",
    },
    SyncOption {
        args: &["--no-verify"],
        about: "Pushes without running the pre-push hook",
    },
];

/// How an option reads in its menu: `pull --rebase`, `push --no-verify`.
pub fn label(push: bool, option: &SyncOption) -> String {
    match push {
        true => ["push"]
            .iter()
            .chain(option.args)
            .copied()
            .collect::<Vec<_>>()
            .join(" "),
        false => option.args.join(" "),
    }
}
