use std::env;
use std::path::Path;
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
    let fallback_session = if in_tmux {
        current_session().ok().filter(|s| !s.is_empty())
    } else {
        None
    };
    let window_name = format!(
        "{}-{}",
        agent.agent.as_str(),
        sanitize_name(&basename(&agent.cwd))
    );
    let resume_cmd = resume_command(agent.agent)?;
    let cwd = valid_cwd_arg(&agent.cwd);
    let (target, target_session) = create_adopt_target(
        &session,
        fallback_session.as_deref(),
        &window_name,
        cwd.as_deref(),
        resume_cmd,
    )?;

    if in_tmux {
        tmux_run(&["switch-client", "-t", &target_session], "switch to adopted session")?;
        tmux_run(&["select-pane", "-t", &target], "select adopted pane")?;
        terminate_original_agent(agent)?;
        return Ok(SpawnResult::Switched { target });
    }

    tmux_run(&["select-pane", "-t", &target], "select adopted pane")?;
    tmux_run(
        &["attach-session", "-t", &target_session],
        "attach to adopted session",
    )?;
    terminate_original_agent(agent)?;
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
    let _ = in_tmux;
    // Always adopt into the dedicated AI session.
    Ok(DEFAULT_AI_SESSION.to_string())
}

fn current_session() -> Result<String> {
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#S"])
        .output()
        .context("failed to run tmux display-message")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("tmux display-message failed: {stderr}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

fn sanitize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "agent".to_string()
    } else {
        trimmed.to_string()
    }
}

fn valid_cwd_arg(cwd: &str) -> Option<String> {
    if cwd.is_empty() || cwd == "-" {
        return None;
    }
    let path = Path::new(cwd);
    if path.is_dir() {
        Some(cwd.to_string())
    } else {
        None
    }
}

fn create_adopt_target(
    preferred_session: &str,
    fallback_session: Option<&str>,
    window_name: &str,
    cwd: Option<&str>,
    resume_cmd: &str,
) -> Result<(String, String)> {
    let mut plans: Vec<(String, Vec<String>)> = Vec::new();
    push_session_create_plans(&mut plans, preferred_session, window_name, cwd, resume_cmd)?;
    if let Some(fallback) = fallback_session
        && fallback != preferred_session
    {
        push_session_create_plans(&mut plans, fallback, window_name, cwd, resume_cmd)?;
    }

    let mut failures = Vec::new();
    for (session, args) in plans {
        match tmux_capture_owned(&args, "create adopted target") {
            Ok(target) => return Ok((target, session)),
            Err(err) => failures.push(format!("{session}: {err}")),
        }
    }

    Err(anyhow!(
        "unable to adopt external agent; attempts failed: {}",
        failures.join(" | ")
    ))
}

fn push_session_create_plans(
    plans: &mut Vec<(String, Vec<String>)>,
    session: &str,
    window_name: &str,
    cwd: Option<&str>,
    resume_cmd: &str,
) -> Result<()> {
    if session_exists(session)? {
        let target_window = format!("{session}:{}", next_window_index(session)?);
        plans.push((
            session.to_string(),
            new_window_args(&target_window, window_name, cwd, resume_cmd),
        ));
        if cwd.is_some() {
            plans.push((
                session.to_string(),
                new_window_args(&target_window, window_name, None, resume_cmd),
            ));
        }
    } else {
        plans.push((
            session.to_string(),
            new_session_args(session, window_name, cwd, resume_cmd),
        ));
        if cwd.is_some() {
            plans.push((
                session.to_string(),
                new_session_args(session, window_name, None, resume_cmd),
            ));
        }
    }
    Ok(())
}

fn new_window_args(
    target_window: &str,
    window_name: &str,
    cwd: Option<&str>,
    resume_cmd: &str,
) -> Vec<String> {
    let mut args = vec![
        "new-window".to_string(),
        "-d".to_string(),
        "-P".to_string(),
        "-F".to_string(),
        "#{session_name}:#{window_index}.#{pane_index}".to_string(),
        "-t".to_string(),
        target_window.to_string(),
        "-n".to_string(),
        window_name.to_string(),
    ];
    if let Some(dir) = cwd {
        args.push("-c".to_string());
        args.push(dir.to_string());
    }
    args.push(resume_cmd.to_string());
    args
}

fn next_window_index(session: &str) -> Result<u32> {
    let output = Command::new("tmux")
        .args(["list-windows", "-t", session, "-F", "#{window_index}"])
        .output()
        .context("failed to run tmux list-windows")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("tmux list-windows failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let max = stdout
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    Ok(max.saturating_add(1))
}

fn new_session_args(session: &str, window_name: &str, cwd: Option<&str>, resume_cmd: &str) -> Vec<String> {
    let mut args = vec![
        "new-session".to_string(),
        "-d".to_string(),
        "-P".to_string(),
        "-F".to_string(),
        "#{session_name}:#{window_index}.#{pane_index}".to_string(),
        "-s".to_string(),
        session.to_string(),
        "-n".to_string(),
        window_name.to_string(),
    ];
    if let Some(dir) = cwd {
        args.push("-c".to_string());
        args.push(dir.to_string());
    }
    args.push(resume_cmd.to_string());
    args
}

fn tmux_capture_owned(args: &[String], action: &str) -> Result<String> {
    let output = Command::new("tmux")
        .args(args)
        .output()
        .with_context(|| format!("failed to run tmux {action}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("tmux {action} failed: {stderr}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn tmux_run(args: &[&str], action: &str) -> Result<()> {
    let output = Command::new("tmux")
        .args(args)
        .output()
        .with_context(|| format!("failed to run tmux {action}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow!("tmux {action} failed: {stderr}"))
    }
}

fn terminate_original_agent(agent: &ExternalAgent) -> Result<()> {
    let pid = agent.shell_pid.unwrap_or(agent.pid);
    let output = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .output()
        .with_context(|| format!("failed to run kill for original agent process {pid}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "failed to terminate original agent process {pid}: {stderr}"
        ));
    }
    Ok(())
}
