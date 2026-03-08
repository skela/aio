use std::env;
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::models::{AgentType, ExternalAgent};

const DEFAULT_AI_SESSION: &str = "ai";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnResult {
    Switched { target: String },
    AttachedReturned { target: String },
}

pub fn adopt_external_agent(agent: &ExternalAgent) -> Result<SpawnResult> {
    let in_tmux = env::var("TMUX").is_ok();
    let session = preferred_session(in_tmux)?;
    let window_name = format!("{}-{}", agent.agent.as_str(), basename(&agent.cwd));
    let resume_cmd = resume_command(agent.agent)?;
    let target = if session_exists(&session)? {
        tmux_capture(
            &[
                "new-window",
                "-d",
                "-P",
                "-F",
                "#{session_name}:#{window_index}.#{pane_index}",
                "-t",
                &session,
                "-n",
                &window_name,
                "-c",
                &agent.cwd,
                resume_cmd,
            ],
            "create tmux window",
        )?
    } else {
        tmux_capture(
            &[
                "new-session",
                "-d",
                "-P",
                "-F",
                "#{session_name}:#{window_index}.#{pane_index}",
                "-s",
                &session,
                "-n",
                &window_name,
                "-c",
                &agent.cwd,
                resume_cmd,
            ],
            "create tmux session",
        )?
    };

    if in_tmux {
        tmux_run(
            &[
                "switch-client",
                "-t",
                &session,
                ";",
                "select-pane",
                "-t",
                &target,
            ],
            "switch to adopted pane",
        )?;
        return Ok(SpawnResult::Switched { target });
    }

    tmux_run(
        &[
            "select-pane",
            "-t",
            &target,
            ";",
            "attach-session",
            "-t",
            &session,
        ],
        "attach to adopted pane",
    )?;
    Ok(SpawnResult::AttachedReturned { target })
}

fn resume_command(agent: AgentType) -> Result<&'static str> {
    match agent {
        AgentType::Claude => Ok("claude --continue"),
        AgentType::Codex => Ok("codex resume --last"),
        AgentType::Opencode => Ok("opencode --continue"),
        AgentType::Unknown => Err(anyhow!("cannot adopt unknown agent")),
    }
}

fn preferred_session(in_tmux: bool) -> Result<String> {
    if in_tmux {
        let current = tmux_capture(&["display-message", "-p", "#S"], "read current tmux session")?;
        if !current.is_empty() {
            return Ok(current);
        }
    }
    // Outside tmux we always adopt into the dedicated AI session.
    Ok(DEFAULT_AI_SESSION.to_string())
}

fn session_exists(session: &str) -> Result<bool> {
    let status = Command::new("tmux")
        .args(["has-session", "-t", session])
        .status()
        .context("failed to run tmux has-session")?;
    Ok(status.success())
}

fn basename(path: &str) -> String {
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("agent")
        .to_string()
}

fn tmux_capture(args: &[&str], action: &str) -> Result<String> {
    let output = Command::new("tmux")
        .args(args)
        .output()
        .with_context(|| format!("failed to run tmux {action}"))?;
    if !output.status.success() {
        return Err(anyhow!("tmux {action} failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn tmux_run(args: &[&str], action: &str) -> Result<()> {
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
