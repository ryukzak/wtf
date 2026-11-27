# WorkTree Flow

A native macOS Git client for reviewing what your AI agents did: worktrees and branches as a flow of work, with a review area under it.

**Before**: writing code was hard. Thinking was hard too. But if we thought ahead, we could write less code and so do less of the hard work.

**Today**: writing code is easy. Thinking is still hard. So we can think behind: choose the best code from several options, throw away what we don't like, and steer the flow of code. Time spent on a prototype doesn't matter, because it's easy to redo.

The tool's purpose: understand what our agents did and why, change direction on the fly, and find the fairway.

Sources of ideas:

- [Sublime Merge](https://www.sublimemerge.com), [Magit](https://magit.vc) (how we work with commits, UI logic)
- [Git time machine](https://github.com/emacsmirror/git-timemachine) (Tree view and read specific commit history)
- [Difftastic](https://github.com/wilfred/difftastic) (Structured diff)
- Original (working with worktrees from above)

## What it does

- **Workflow graph**: branches as lanes, runs of commits as nodes, with worktrees, stashes and tags drawn on it.
- **Review**: structural ([difftastic](https://difftastic.wilfred.me.uk)) or text diffs; select and copy text, show more lines around a change, browse any commit's files in Snapshot.
- **Commit**: stage files and commit; Apple Intelligence writes commit messages and summaries of commits on your Mac, from prompt templates you can edit.
- **Attention**: choose what the graph shows; channels group agents' branches.
- **Prune**: delete merged branches, branches whose remote is gone and stale worktrees.
- **Project settings in git config**: prompts, text-diff files, idle time, fetch interval, prune, pull and push, author name and email; the Global project writes them to `~/.gitconfig`.

## Install

With [Homebrew](https://brew.sh), which builds the app on your Mac, so the first install takes several minutes:

    brew install --cask ryukzak/tap/wtf

It also puts the `wtf` command on your PATH: `wtf .` opens the repository you are in, and `wtf path/to/file` shows the file in its worktree. Update with `brew upgrade --cask wtf`.

## Run

macOS only; needs Rust and Git. Structural diffs need [difftastic](https://difftastic.wilfred.me.uk) (`brew install difftastic`); commit messages and summaries need macOS 26 with Apple Intelligence:

    scripts/build.sh
    open build/wtf.app --args /path/to/repo

To install, build and copy the app to `/Applications`, replacing an installed copy:

    scripts/install.sh

Or into another folder, such as `~/Applications`: `scripts/install.sh ~/Applications`.

Or, during development: `cargo run -p wtf -- /path/to/repo`.

To open repositories from a terminal, choose **wtf › Install 'wtf' Command…**; then `wtf .` opens the one you are in. It goes in `~/.local/bin`, `~/bin` or `~/.bin` when your shell's PATH has one, otherwise in `/usr/local/bin`, which asks for your password.

## Interface

```
 ● ● ●   Tabs: one repository each
┌────────────────────────────────────────────────────────┬────────────────────┬───┐
│ Workflow graph                                         │ Side panel  ⌘R     │ A │
│ branches as lanes, runs of                             │                    │ t │
│ commits as nodes; worktrees                            │ Attention:         │ t │
│ listed on the right                                    │ what the graph     │ e │
│                                                        │ shows              │ n │
├──────────────────────┬─────────────────────────────────┤                    │ t │
│ Commit history       │ Commit panel,                   │ Files:             │ i │
│                      │ commit box for a worktree,      │ changed files      │ o │
│                      │ staging, diff                   │ as a tree          │ n │
│                      │                                 │                    │   │
│                      │                                 │                    │ F │
│                      │                                 │                    │ i │
│                      │                                 │                    │ l │
│                      │                                 │                    │ e │
├──────────────────────┴─────────────────────────────────┤                    │ s │
│ Terminal  ⌘J                                           │                    │ T │
│ the app's Git commands, too                            │                    │ e │
└────────────────────────────────────────────────────────┴────────────────────┴───┘
```

## Keyboard

| Keys        | Action                                                                  |
| ----------- | ----------------------------------------------------------------------- |
| ⌘P          | Go to a worktree, branch or commit (⌘↩ opens a worktree in another app) |
| ⇧⌘P         | Command palette                                                         |
| ⌘R          | Toggle the side panel                                                   |
| ⌘J, ⌃`      | Toggle the terminal                                                     |
| ⌘↩          | Commit                                                                  |
| ⌘S          | Stash changes                                                           |
| ⌥⌘F         | Fetch                                                                   |
| ⌥⌘1, ⌥⌘2    | Structural or text diff                                                 |
| ⌥⌘3         | Toggle Snapshot                                                         |
| ⌘T, ⌘W, ⇧⌘T | New, close, reopen tab (⌃⇧T too)                                        |
| ⌘1…⌘9       | Switch tab                                                              |
| ⇧⌘[, ⇧⌘]    | Previous or next tab (⌃⇧⇥, ⌃⇥ too)                                      |
| ⇧⌘N         | New window                                                              |
| ⌘O          | Open a repository                                                       |
| ⌘,          | Settings                                                                |

## Development

The workspace has two crates:

- `core` (`wtf_core`): Git, the workflow graph, diffs and attention. No UI.
- `app`: the GPUI interface, the terminal (alacritty_terminal) and syntax highlighting (syntect).

Before sending a change, run what CI runs:

    cargo fmt --all
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

The app icon is drawn in `app/icon/icon.svg`; `scripts/icon.sh` renders it into `wtf.icns` and `icon.png`, which the About window shows.

## License

BSD 3-Clause; see [LICENSE](LICENSE).
