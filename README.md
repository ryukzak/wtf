# WorkTree Flow

A native macOS Git client for reviewing what your AI agents did: worktrees and branches as a flow of work, with a review area under it.

**Before**: writing code was hard. Thinking was hard too. But if we thought ahead, we could write less code and so do less of the hard work.

**Today**: writing code is easy. Thinking is still hard. So we can think behind: choose the best code from several options, throw away what we don't like, and steer the flow of code. Time spent on a prototype doesn't matter, because it's easy to redo.

The tool's purpose: understand what our agents did and why, change direction on the fly, and find the fairway.

## Run

macOS only; needs Rust and Git. Structural diffs need [difftastic](https://difftastic.wilfred.me.uk) (`brew install difftastic`):

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
| ⌥⌘1, ⌥⌘2    | Structural or text diff                                                 |
| ⌘T, ⌘W, ⌃⇧T | New, close, reopen tab                                                  |
| ⌘1…⌘9       | Switch tab                                                              |
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

The app icon is drawn in `app/icon/icon.svg`; `scripts/icon.sh` renders it into `wtf.icns`.

## License

BSD 3-Clause; see [LICENSE](LICENSE).
