use std::env;
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::models::PaneInfo;

/// Open lazygit in a new tmux window inside the selected agent's session,
/// using the agent's working directory. Switches the client to that window
/// so the user lands directly in lazygit. When lazygit exits (`q`), the
/// window closes automatically (because it was the only process) and the
/// user is back where they were.
pub fn open_lazygit(pane: &PaneInfo) -> Result<()> {
    if !program_exists("lazygit") {
        return Err(anyhow!(
            "lazygit not found in PATH; install it from https://github.com/jesseduffield/lazygit"
        ));
    }

    let cwd = if pane.cwd.is_empty() || pane.cwd == "-" {
        None
    } else {
        Some(pane.cwd.as_str())
    };

    // Create a new window (detached so we can switch to it after).
    // -P -F prints the new pane's target so we can select it.
    let window_name = format!(
        "lazygit-{}",
        pane.cwd.rsplit('/').find(|s| !s.is_empty()).unwrap_or("repo")
    );

    let mut new_win_args = vec![
        "new-window",
        "-d",       // detached — don't switch yet
        "-P",       // print target
        "-F", "#{session_name}:#{window_index}",
        "-t", &pane.session,
        "-n", &window_name,
    ];
    let cwd_owned;
    if let Some(dir) = cwd {
        cwd_owned = dir.to_string();
        new_win_args.push("-c");
        new_win_args.push(&cwd_owned);
    }
    new_win_args.push("lazygit");

    let output = Command::new("tmux")
        .args(&new_win_args)
        .output()
        .context("failed to run tmux new-window for lazygit")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("tmux new-window failed: {stderr}"));
    }

    let window_target = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Switch the client to the new window.
    if env::var("TMUX").is_ok() {
        // Inside tmux: switch-client to bring the window into focus.
        let status = Command::new("tmux")
            .args(["switch-client", "-t", &window_target])
            .status()
            .context("failed to run tmux switch-client")?;
        if !status.success() {
            return Err(anyhow!("tmux switch-client to lazygit window failed"));
        }
    } else {
        // Outside tmux: attach to the session (blocks until detach).
        let session = window_target
            .split(':')
            .next()
            .unwrap_or(&pane.session)
            .to_string();
        Command::new("tmux")
            .args(["attach-session", "-t", &session])
            .status()
            .context("failed to attach tmux session for lazygit")?;
    }

    Ok(())
}

fn program_exists(program: &str) -> bool {
    Command::new("which")
        .arg(program)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
