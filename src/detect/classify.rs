use once_cell::sync::Lazy;
use regex::Regex;

use crate::models::{AgentRecord, AgentStatus, AgentType, PaneInfo};

static CLAUDE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bclaude\b").expect("regex"));
static CODEX_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bcodex\b").expect("regex"));
static OPENCODE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:opencode|open-code)\b").expect("regex"));

pub fn detect_agent(pane: PaneInfo, full_argv: String) -> AgentRecord {
    let (agent, _confidence) = best_match(&pane, &full_argv);
    AgentRecord {
        pane,
        agent,
        status: AgentStatus::Idle,
        full_argv,
        last_seen: std::time::Instant::now(),
    }
}

fn best_match(pane: &PaneInfo, argv: &str) -> (AgentType, f32) {
    // Fast path: explicit command names from tmux are highly reliable.
    let current = pane.current_cmd.to_ascii_lowercase();
    if current == "claude" || current == "claude-code" {
        return (AgentType::Claude, 0.99);
    }
    if current == "codex" {
        return (AgentType::Codex, 0.99);
    }
    if current == "opencode" || current == "open-code" {
        return (AgentType::Opencode, 0.99);
    }

    let candidates = [
        (AgentType::Claude, &*CLAUDE_RE),
        (AgentType::Codex, &*CODEX_RE),
        (AgentType::Opencode, &*OPENCODE_RE),
    ];

    let mut best = (AgentType::Unknown, 0.0_f32);
    for (agent, re) in candidates {
        let mut score = 0.0;
        if re.is_match(argv) {
            score += 0.55;
        }
        if re.is_match(&pane.start_cmd) || re.is_match(&pane.title) {
            score += 0.25;
        }
        if re.is_match(&pane.current_cmd) {
            score += 0.2;
        }
        if score > best.1 {
            best = (agent, score);
        }
    }

    if best.1 >= 0.45 {
        best
    } else {
        (AgentType::Unknown, best.1)
    }
}
