# AGENTS.md

Instructions for AI coding agents working on `aio`.

## Source Of Truth

- Read [README.md](../README.md) first for current product behavior, UI layout, keybindings, and user-facing semantics.
- Keep `README.md` accurate whenever you change behavior.

## Change Hygiene

- If you change keybindings, filters, panel layout, status logic, or adopt/jump semantics:
  - update `README.md` in the same change.
- If a feature is incomplete or heuristic-based, document the limitation clearly in `README.md`.
- Prefer small incremental changes; keep `cargo check` passing.

## UX Guardrails

- Preserve tmux-first workflows.
- Avoid adding noise to the bottom status line; keep it focused on keyboard actions.
- Unknown tmux agents should stay hidden unless explicitly requested otherwise.

## Behavior Expectations

- `Enter` on tmux row: jump to selected tmux pane.
- `Enter` on outside row: adopt into tmux via resume command in session `ai` (create session if missing).
- `/` starts search mode; `Esc` clears/exits search mode.
- `p` toggles preview; `Esc` hides preview outside search mode.
