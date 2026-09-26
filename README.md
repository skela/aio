# aio

`aio` or AI Overview is a Rust `ratatui` dashboard for managing AI agents running across your terminal workflow.

It is built for tmux-heavy usage and gives you an `htop`-style overview of:

- AI agents running inside tmux panes (`claude`, `codex`, `opencode`)
- AI processes running outside tmux
- Quick jump/adopt actions to bring work into tmux

## What It Does

- Detects tmux panes and classifies agent type
- Shows a live table of AI panes
- Tracks status (`thinking`, `editing`, `running`, `waiting_input`, `error`, `idle`) with a colored dot
- Supports fuzzy search with `/`
- Supports filter shortcuts for agent type
- Shows outside-tmux agents in a separate panel (hidden when empty)
- Lets you:
  - `Enter` on tmux row: jump to that tmux pane
  - `Enter` on outside row: adopt into tmux by resuming in session `ai` (creates session if missing)

## Current UI

- **Agents** table (main)
  - Columns: `Status`, `Name` (tmux window name), `Project`, `Agent`, `CWD`
- **Outside tmux** table (conditional)
  - Shown only when outside agents exist
- **Details** and **Tmux Details** panels
- **Preview** panel (toggle with `p`)

## Keybindings

- `q`: quit
- `j` / `k` or arrow keys: move selection
- `Enter`: move to tmux
  - tmux row: switch to pane
  - outside row: adopt by launching resume command in tmux session `ai`
- `/`: enter fuzzy search mode
  - type to filter
  - `Backspace`: delete character
  - `Enter`: exit search mode (keep query)
  - `Esc`: clear query and exit search mode
- `f`: cycle filter (`all -> claude -> codex -> opencode -> all`)
- `s`: cycle sort (`last_seen -> project -> agent`)
- `Ctrl+O`: set filter `opencode`
- `Ctrl+A`: set filter `codex`
- `Ctrl+C`: set filter `claude`
- `c` / `n`: create a new window running `opencode` in this tmux session
  - prompts for a window name and a starting path (defaults to `~/`; `~` is expanded)
  - `Tab`: on name, move to path; on path, complete the directory (shell-style — unique matches get a trailing `/`, multiple matches extend to the common prefix and are listed; dot-dirs only shown when you type `.`)
  - `Shift+Tab` / `↑` / `↓`: switch between the name and path fields
  - `Enter`: on name, move to path; on path, create the window and jump to it
  - `Esc`: cancel
  - requires `aio` to be running inside tmux; if not, an error is shown and the prompt stays open
- `r`: show recently closed agent sessions
  - `j` / `k` or arrow keys: move selection
  - `Enter`: reopen in the original tmux session (recreated if gone) with the original window name and cwd, then jump to it
  - `d` / `Delete`: forget the selected entry
  - `Esc` / `r` / `q`: close the list
- `p`: toggle preview panel
- `t` / `T`: test the input-needed / done sound
- `Esc` (outside search mode): hide preview

## Agent Resume Commands Used for Adopt

When adopting an outside-tmux agent, `aio` starts a new tmux window and runs:

- Claude: `claude --continue`
- Codex: `codex resume --last`
- OpenCode: `opencode --continue`

A window created with `c` / `n` just runs `opencode` directly (there's no prior session to resume), inside the same tmux session `aio` is running in.

## Recently Closed Sessions

While `aio` is running it remembers tmux agent panes that go away — either the
pane/window was closed, or the agent exited and the pane dropped back to a
shell. Panes `aio` closes itself (eject) are not recorded. The list keeps the
50 most recent entries (newest first, one per agent+cwd+title) and is stored in
`$XDG_STATE_HOME/aio/closed_sessions.json` (default
`~/.local/state/aio/closed_sessions.json`), so it survives restarts.

Reopening runs:

- OpenCode: `opencode --session <id>` — the id is found by matching the pane
  title opencode sets (`OC | <session title>`) and the cwd against
  `opencode session list`. Falls back to `opencode --continue` when no unique
  match exists (e.g. the session was never titled).
- Claude: `claude --continue`
- Codex: `codex resume --last`

Limitations: closes that happen while `aio` isn't running are not seen, and
Claude/Codex resume the most recent conversation in that directory, which may
not be the one that was closed if several ran in the same cwd.

## Build and Run

```bash
cargo run
```

## Notes

- Unknown agents are hidden from the tmux table.
- Status is inferred from the current screen footer rather than the full
  transcript. `aio` recognizes each CLI's interrupt and composer controls,
  and uses separate sounds when a tracked tmux pane completes work or moves
  from working to `waiting_input`.
- Outside-tmux status is currently coarse compared to tmux status.
