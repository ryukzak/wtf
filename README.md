# wtf — WorkTree Flow

A native macOS Git client for reviewing what your AI agents did: worktrees and branches as a flow of work, with a review area under it.

**Before**: writing code was hard. Thinking was hard too. But if we thought ahead, we could write less code and so reduce the hard work.

**Today**: writing code is easy. Thinking is still hard. So we can think behind: choose the best code from several options, easily throw away what we don't like, and steer the flow of code.

The key purpose of the tool: understand what our agents did and why, change direction on the fly, and find the fairway.

## Run

macOS only, needs Rust and Git:

    scripts/build.sh
    open build/wtf.app --args /path/to/repo

Or, during development: `cargo run -p wtf -- /path/to/repo`.

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
│ Commits              │ Review                          │ Files:             │ i │
│ of the selected      │ message or summary,             │ changed files      │ o │
│ node                 │ commit box for a worktree,      │ as a tree          │ n │
│                      │ staging, diff                   │                    │   │
│                      │                                 │                    │ F │
│                      │                                 │                    │ i │
│                      │                                 │                    │ l │
│                      │                                 │                    │ e │
├──────────────────────┴─────────────────────────────────┤                    │ s │
│ Terminal  ⌘J                                           │                    │   │
│                                                        │                    │   │
├────────────────────────────────────────────────────────┴────────────────────┴───┤
│ Status bar: terminal toggle, last Git command                                   │
└─────────────────────────────────────────────────────────────────────────────────┘
```

## Keyboard

| Keys          | Action                                                                  |
| ------------- | ----------------------------------------------------------------------- |
| ⌘P            | Go to a worktree, branch or commit (⌘↩ opens a worktree in another app) |
| ⇧⌘P           | Command palette                                                         |
| ⌘R            | Toggle the side panel                                                   |
| ⌘J, ⌃`        | Toggle the terminal                                                     |
| ⌃⌘1, ⌃⌘2, ⌃⌘3 | Topology, compressed time, time                                         |
| ⌥⌘1, ⌥⌘2      | Structural or text diff                                                 |
| ⌘T, ⌘W, ⌃⇧T   | New, close, reopen tab                                                  |
| ⌘1…⌘9         | Switch tab                                                              |
| ⌘O            | Open a repository                                                       |
| ⌘,            | Settings                                                                |

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

BSD 3-Clause, see [LICENSE](LICENSE).
