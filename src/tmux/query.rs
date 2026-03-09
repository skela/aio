use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::models::PaneInfo;

const FMT: &str = "#{pane_id}|#{session_name}|#{window_index}|#{window_name}|#{pane_index}|#{pane_pid}|#{pane_current_command}|#{pane_current_path}|#{pane_title}|#{pane_active}|#{pane_dead}|#{pane_start_command}";

pub fn list_panes() -> Result<Vec<PaneInfo>> {
    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", FMT])
        .output()
        .context("failed to invoke tmux list-panes")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("tmux list-panes failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut panes = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(pane) = parse_line(line) {
            panes.push(pane);
        }
    }
    Ok(panes)
}

pub fn capture_pane_tail(pane_id: &str, lines: usize) -> Result<String> {
    let start = format!("-{lines}");
    let output = Command::new("tmux")
        .args(["capture-pane", "-p", "-J", "-S", &start, "-t", pane_id])
        .output()
        .context("failed to invoke tmux capture-pane")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("tmux capture-pane failed: {stderr}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn capture_pane_tail_ansi(pane_id: &str, lines: usize) -> Result<String> {
    let start = format!("-{lines}");
    let output = Command::new("tmux")
        .args(["capture-pane", "-p", "-e", "-J", "-S", &start, "-t", pane_id])
        .output()
        .context("failed to invoke tmux capture-pane (ansi)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("tmux capture-pane (ansi) failed: {stderr}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn parse_line(line: &str) -> Result<PaneInfo> {
    let parts: Vec<&str> = line.splitn(12, '|').collect();
    if parts.len() != 12 {
        return Err(anyhow!("unexpected tmux pane format"));
    }

    Ok(PaneInfo {
        pane_id: parts[0].to_string(),
        session: parts[1].to_string(),
        window_index: parts[2].parse()?,
        window_name: parts[3].to_string(),
        pane_index: parts[4].parse()?,
        pid: parts[5].parse()?,
        current_cmd: parts[6].to_string(),
        cwd: parts[7].to_string(),
        title: parts[8].to_string(),
        active: parts[9] == "1",
        dead: parts[10] == "1",
        start_cmd: parts[11].to_string(),
    })
}
