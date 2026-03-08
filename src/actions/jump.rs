use std::env;
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::models::PaneInfo;
use crate::tmux::target::pane_target;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpResult {
    Switched,
    AttachedReturned,
}

pub fn jump_to_pane(pane: &PaneInfo) -> Result<JumpResult> {
    let target = pane_target(pane);
    let window_target = format!("{}:{}", pane.session, pane.window_index);

    // Running inside tmux client: switch current client context directly.
    if env::var("TMUX").is_ok() {
        run_tmux(
            &[
                "switch-client",
                "-t",
                &pane.session,
                ";",
                "select-window",
                "-t",
                &window_target,
                ";",
                "select-pane",
                "-t",
                &target,
            ],
            "switch/select client pane",
        )?;
        return Ok(JumpResult::Switched);
    }

    // Running outside tmux: preselect target in session, then attach.
    run_tmux(
        &[
            "select-window",
            "-t",
            &window_target,
            ";",
            "select-pane",
            "-t",
            &target,
            ";",
            "attach-session",
            "-t",
            &pane.session,
        ],
        "attach and jump to pane",
    )?;

    Ok(JumpResult::AttachedReturned)
}

fn run_tmux(args: &[&str], action: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(args)
        .status()
        .with_context(|| format!("failed to run tmux {action}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("tmux {action} failed"))
    }
}
