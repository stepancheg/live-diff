# live-diff

A live pager for `sl diff` or `git diff`. While an agent is editing the
tree, you can watch the diff, ask for a follow-up, and see that change
as soon as it lands. The diff is re-run on an interval. When the output
changes, the scroll position is kept; the current
file header stays pinned.

Sapling is used when the repository already has sl metadata (`.sl` or
`.git/sl`). Otherwise the repository is treated as git.

## Features

- Git and Sapling
- Live refresh that keeps the scroll position
- Pinned file header
- Untracked files on the status bar and in the diff stat
- Diff stat (`s`)
- Wrapped lines stay one logical line, so a double-click selects the whole word
- `less`-like navigation, including search and marks

## Screenshots

Diff view, scrolled partway down. The first file's header stays
pinned and the top of its diff is off-screen; the pane shows the tail
of that diff and the start of the next file. `/bar` highlights the
matches. The status bar shows the untracked count and the scroll
position.

![Diff view](screenshots/diff.png)

Diff stat for the same tree.

![Diff stat](screenshots/diffstat.png)

## Installation

```
cargo install --git https://github.com/stepancheg/live-diff
```

or `bin/live-diff`

## Options

| Flag | Meaning |
| --- | --- |
| `-n`, `--interval` | Seconds between runs (default `0.5`, or `LIVE_DIFF_INTERVAL`) |
| `-r`, `--rev` | Revision to compare against. Without this, the tool's default diff |
| `-c`, `--change` | Diff introduced by one commit. No live refresh |
| `-U`, `--unified` | Initial context lines (default 3). `u` / `U` change it later |
| `--sl` | Force `sl diff` |
| `--git` | Force `git diff` |

## Keys

| Key | Action |
| --- | --- |
| `h` | Help |
| `q` | Quit |
| `r` | Run the diff now |
| `j` / `k`, arrows | Scroll |
| `Space` / `PgDn`, `PgUp` | Page |
| `g` / `G` | Top / bottom |
| `/` `?` `n` `N` | Search |
| `s` | Diff stat (Enter or `s` opens the file, Esc returns) |
| `u` / `U` | Decrease / increase context |
| `mx` / `'x` | Set / go to mark |

Untracked files are counted on the status bar and listed after the stat.
