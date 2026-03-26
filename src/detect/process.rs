use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::Path;

use crate::models::{AgentStatus, AgentType, ExternalAgent};

pub fn resolve_deepest_argv(root_pid: i32) -> String {
    let mut best = read_cmdline(root_pid).unwrap_or_default();
    let mut q = VecDeque::from([root_pid]);
    let mut seen = HashSet::from([root_pid]);

    while let Some(pid) = q.pop_front() {
        if let Some(cmdline) = read_cmdline(pid)
            && cmdline.len() > best.len()
        {
            best = cmdline;
        }

        for child in children_of(pid) {
            if seen.insert(child) {
                q.push_back(child);
            }
        }
    }

    best
}

pub fn collect_process_tree(root_pid: i32) -> HashSet<i32> {
    let mut seen = HashSet::from([root_pid]);
    let mut q = VecDeque::from([root_pid]);
    while let Some(pid) = q.pop_front() {
        for child in children_of(pid) {
            if seen.insert(child) {
                q.push_back(child);
            }
        }
    }
    seen
}

pub fn list_external_agents(tmux_root_pids: &[i32]) -> Vec<ExternalAgent> {
    let mut tmux_owned = HashSet::new();
    for pid in tmux_root_pids {
        tmux_owned.extend(collect_process_tree(*pid));
    }

    let mut agents = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return agents;
    };

    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        if tmux_owned.contains(&pid) {
            continue;
        }

        let Some(cmdline) = read_cmdline(pid) else {
            continue;
        };
        let Some(agent) = detect_agent_from_cmdline(&cmdline) else {
            continue;
        };

        let cwd = read_cwd(pid);
        agents.push(ExternalAgent {
            pid,
            shell_pid: find_parent_shell(pid),
            agent,
            status: infer_external_status(&cmdline),
            cwd,
            cmdline,
        });
    }

    agents.sort_by(|a, b| {
        a.agent
            .as_str()
            .cmp(b.agent.as_str())
            .then(a.pid.cmp(&b.pid))
    });
    agents
}

fn infer_external_status(cmdline: &str) -> AgentStatus {
    let c = cmdline.to_ascii_lowercase();
    if c.contains("error:") || c.contains("traceback") || c.contains("exception") {
        return AgentStatus::Error;
    }
    if c.contains("continue") || c.contains("resume") {
        return AgentStatus::WaitingInput;
    }
    AgentStatus::Running
}

fn detect_agent_from_cmdline(cmdline: &str) -> Option<AgentType> {
    if contains_token(cmdline, "claude") || contains_token(cmdline, "claude-code") {
        return Some(AgentType::Claude);
    }
    if contains_token(cmdline, "codex") {
        return Some(AgentType::Codex);
    }
    if contains_token(cmdline, "opencode") || contains_token(cmdline, "open-code") {
        return Some(AgentType::Opencode);
    }
    None
}

fn contains_token(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .any(|token| token == needle)
}

fn read_cwd(pid: i32) -> String {
    let path = format!("/proc/{pid}/cwd");
    fs::read_link(Path::new(&path))
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn children_of(pid: i32) -> Vec<i32> {
    let path = format!("/proc/{pid}/task/{pid}/children");
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.split_whitespace()
        .filter_map(|p| p.parse::<i32>().ok())
        .collect()
}

fn find_parent_shell(pid: i32) -> Option<i32> {
    let mut current = pid;
    let mut seen = HashSet::new();
    while seen.insert(current) {
        let parent = parent_of(current)?;
        if parent <= 1 {
            return None;
        }
        let cmdline = read_cmdline(parent).unwrap_or_default();
        let comm = read_comm(parent).unwrap_or_default();
        if is_shell_process(&cmdline) || is_shell_process(&comm) {
            return Some(parent);
        }
        current = parent;
    }
    None
}

fn parent_of(pid: i32) -> Option<i32> {
    let path = format!("/proc/{pid}/stat");
    let raw = fs::read_to_string(path).ok()?;
    let rparen = raw.rfind(')')?;
    let rest = raw.get(rparen + 2..)?;
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse::<i32>().ok()
}

fn read_comm(pid: i32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn is_shell_process(cmd: &str) -> bool {
    cmd.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/'))
        .filter(|part| !part.is_empty())
        .any(|part| {
            matches!(
                part.rsplit('/').next().unwrap_or(part),
                "bash" | "zsh" | "fish" | "sh" | "dash" | "nu"
            )
        })
}

fn read_cmdline(pid: i32) -> Option<String> {
    let path = format!("/proc/{pid}/cmdline");
    let data = fs::read(path).ok()?;
    if data.is_empty() {
        return None;
    }
    let parts: Vec<String> = data
        .split(|b| *b == 0)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}
