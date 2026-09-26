//! Recently-closed tmux agent sessions, persisted across `aio` restarts so an
//! accidentally closed window can be reopened (and, where possible, resumed
//! into the exact same conversation).

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::models::AgentType;

/// Keep the list short; this is an "undo close", not a full session browser.
const MAX_ENTRIES: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedSession {
    /// `claude` / `codex` / `opencode`.
    pub agent: String,
    pub tmux_session: String,
    pub window_name: String,
    pub cwd: String,
    /// Last pane title seen before the close (opencode: `OC | <session title>`).
    pub title: String,
    /// Unix seconds.
    pub closed_at: u64,
}

impl ClosedSession {
    pub fn agent_type(&self) -> AgentType {
        match self.agent.as_str() {
            "claude" => AgentType::Claude,
            "codex" => AgentType::Codex,
            "opencode" => AgentType::Opencode,
            _ => AgentType::Unknown,
        }
    }

    /// The conversation title, with agent-specific decorations stripped.
    pub fn display_title(&self) -> String {
        let t = self.title.trim();
        if let Some(rest) = t.strip_prefix("OC |") {
            return rest.trim().to_string();
        }
        // Claude prefixes its title with a spinner/status glyph.
        let t = t.trim_start_matches(|c: char| !c.is_alphanumeric() && !c.is_whitespace());
        t.trim().to_string()
    }

    fn same_conversation(&self, other: &ClosedSession) -> bool {
        self.agent == other.agent && self.cwd == other.cwd && self.title == other.title
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn store_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("aio").join("closed_sessions.json"))
}

pub fn load() -> Vec<ClosedSession> {
    let Some(path) = store_path() else {
        return Vec::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(entries: &[ClosedSession]) {
    let Some(path) = store_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(entries) {
        // Write-then-rename so a crash mid-write can't truncate the history.
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

/// Inserts `entry` at the front (newest first), replacing any older entry for
/// the same conversation, and trims to `MAX_ENTRIES`.
pub fn push(entries: &mut Vec<ClosedSession>, entry: ClosedSession) {
    entries.retain(|e| !e.same_conversation(&entry));
    entries.insert(0, entry);
    entries.truncate(MAX_ENTRIES);
}

/// Human-friendly "3m ago" style age.
pub fn format_age(closed_at: u64, now: u64) -> String {
    let secs = now.saturating_sub(closed_at);
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

#[derive(Deserialize)]
struct OpencodeSessionRow {
    id: String,
    title: String,
    directory: String,
    #[serde(default)]
    updated: u64,
}

/// Finds the opencode session id for a closed pane by matching the title the
/// TUI put in the pane title (`OC | <title>`) and the working directory.
pub fn find_opencode_session_id(closed: &ClosedSession) -> Option<String> {
    let title = closed.display_title();
    if title.is_empty() {
        return None;
    }
    let output = std::process::Command::new("opencode")
        .args(["session", "list", "--format", "json", "-n", "200"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let rows: Vec<OpencodeSessionRow> = serde_json::from_slice(&output.stdout).ok()?;
    pick_opencode_session(&rows, &closed.cwd, &title)
}

fn pick_opencode_session(rows: &[OpencodeSessionRow], cwd: &str, title: &str) -> Option<String> {
    rows.iter()
        .filter(|r| r.directory == cwd && r.title.trim() == title)
        .max_by_key(|r| r.updated)
        .map(|r| r.id.clone())
}

/// Shell command used to bring the conversation back.  opencode resumes the
/// exact session when its id can be resolved; otherwise each agent falls back
/// to "continue the most recent session in this directory".
pub fn resume_command(closed: &ClosedSession) -> Option<String> {
    match closed.agent_type() {
        AgentType::Opencode => Some(match find_opencode_session_id(closed) {
            Some(id) => format!("opencode --session {id}"),
            None => "opencode --continue".to_string(),
        }),
        AgentType::Claude => Some("claude --continue".to_string()),
        AgentType::Codex => Some("codex resume --last".to_string()),
        AgentType::Unknown => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, cwd: &str) -> ClosedSession {
        ClosedSession {
            agent: "opencode".into(),
            tmux_session: "ai".into(),
            window_name: "w".into(),
            cwd: cwd.into(),
            title: title.into(),
            closed_at: 0,
        }
    }

    #[test]
    fn display_title_strips_agent_prefixes() {
        assert_eq!(entry("OC | Fix the bug", "/").display_title(), "Fix the bug");
        assert_eq!(entry("✳ Refactor app", "/").display_title(), "Refactor app");
        assert_eq!(entry("plain", "/").display_title(), "plain");
    }

    #[test]
    fn push_dedupes_and_orders_newest_first() {
        let mut list = Vec::new();
        push(&mut list, entry("OC | a", "/x"));
        push(&mut list, entry("OC | b", "/x"));
        push(&mut list, entry("OC | a", "/x"));
        let titles: Vec<_> = list.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, vec!["OC | a", "OC | b"]);
    }

    #[test]
    fn picks_latest_matching_opencode_session() {
        let row = |id: &str, title: &str, dir: &str, updated| OpencodeSessionRow {
            id: id.into(),
            title: title.into(),
            directory: dir.into(),
            updated,
        };
        let rows = vec![
            row("s1", "Fix bug", "/a", 1),
            row("s2", "Fix bug", "/a", 5),
            row("s3", "Fix bug", "/b", 9),
            row("s4", "Other", "/a", 10),
        ];
        assert_eq!(pick_opencode_session(&rows, "/a", "Fix bug"), Some("s2".into()));
        assert_eq!(pick_opencode_session(&rows, "/c", "Fix bug"), None);
    }

    #[test]
    fn formats_age() {
        assert_eq!(format_age(100, 130), "30s ago");
        assert_eq!(format_age(0, 600), "10m ago");
        assert_eq!(format_age(0, 7200), "2h ago");
        assert_eq!(format_age(0, 172_800), "2d ago");
    }
}
