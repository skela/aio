use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentType {
    Claude,
    Codex,
    Opencode,
    Unknown,
}

impl AgentType {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentType::Claude => "claude",
            AgentType::Codex => "codex",
            AgentType::Opencode => "opencode",
            AgentType::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentStatus {
    Thinking,
    Editing,
    Running,
    WaitingInput,
    Error,
    Idle,
}

impl AgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentStatus::Thinking => "thinking",
            AgentStatus::Editing => "editing",
            AgentStatus::Running => "running",
            AgentStatus::WaitingInput => "waiting_input",
            AgentStatus::Error => "error",
            AgentStatus::Idle => "idle",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PaneInfo {
    pub pane_id: String,
    pub session: String,
    pub window_index: u32,
    pub window_name: String,
    pub pane_index: u32,
    pub pid: i32,
    pub current_cmd: String,
    pub cwd: String,
    pub title: String,
    pub active: bool,
    pub dead: bool,
    pub start_cmd: String,
}

impl PaneInfo {
    pub fn target(&self) -> String {
        format!("{}:{}.{}", self.session, self.window_index, self.pane_index)
    }

    pub fn project_name(&self) -> String {
        self.cwd
            .rsplit('/')
            .next()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "-".to_string())
    }
}

#[derive(Debug, Clone)]
pub struct AgentRecord {
    pub pane: PaneInfo,
    pub agent: AgentType,
    pub status: AgentStatus,
    pub full_argv: String,
    pub last_seen: Instant,
}

#[derive(Debug, Clone)]
pub struct ExternalAgent {
    pub pid: i32,
    pub agent: AgentType,
    pub status: AgentStatus,
    pub cwd: String,
    pub cmdline: String,
}
