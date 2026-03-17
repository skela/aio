use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::actions::jump;
use crate::actions::jump::JumpResult;
use crate::actions::spawn;
use crate::actions::spawn::SpawnResult;
use crate::detect::{classify, process};
use crate::models::{AgentRecord, AgentStatus, AgentType, ExternalAgent, PaneInfo};
use crate::tmux::query;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Claude,
    Codex,
    Opencode,
}

impl Filter {
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Claude => "claude",
            Filter::Codex => "codex",
            Filter::Opencode => "opencode",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Filter::All => Filter::Claude,
            Filter::Claude => Filter::Codex,
            Filter::Codex => Filter::Opencode,
            Filter::Opencode => Filter::All,
        }
    }

    pub fn matches(self, agent: AgentType) -> bool {
        match self {
            Filter::All => agent != AgentType::Unknown,
            Filter::Claude => agent == AgentType::Claude,
            Filter::Codex => agent == AgentType::Codex,
            Filter::Opencode => agent == AgentType::Opencode,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    LastSeen,
    Project,
    Agent,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::LastSeen => "last_seen",
            SortKey::Project => "project",
            SortKey::Agent => "agent",
        }
    }

    pub fn next(self) -> Self {
        match self {
            SortKey::LastSeen => SortKey::Project,
            SortKey::Project => SortKey::Agent,
            SortKey::Agent => SortKey::LastSeen,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPanel {
    Tmux,
    Outside,
}

pub struct App {
    pub all_records: Vec<AgentRecord>,
    pub records: Vec<AgentRecord>,
    pub external_agents: Vec<ExternalAgent>,
    pub selected: usize,
    pub selected_external: usize,
    pub focus_panel: FocusPanel,
    pub filter: Filter,
    pub sort: SortKey,
    pub running: bool,
    pub status: String,
    pub search_mode: bool,
    pub search_query: String,
    pub preview_visible: bool,
    pub preview_text: String,
    pub needs_terminal_reinit: bool,
    last_seen_by_pane: HashMap<String, Instant>,
    last_tail_sig_by_pane: HashMap<String, u64>,
    last_activity_by_pane: HashMap<String, Instant>,
}

impl App {
    fn keyboard_help() -> &'static str {
        "q:quit  j/k or arrows:move  enter:move to tmux  /:search  p:preview  esc:hide  f:filter  s:sort  ctrl+o/a/c:set filter"
    }

    pub fn new() -> Self {
        Self {
            all_records: Vec::new(),
            records: Vec::new(),
            external_agents: Vec::new(),
            selected: 0,
            selected_external: 0,
            focus_panel: FocusPanel::Tmux,
            filter: Filter::All,
            sort: SortKey::LastSeen,
            running: true,
            status: Self::keyboard_help().to_string(),
            search_mode: false,
            search_query: String::new(),
            preview_visible: false,
            preview_text: String::new(),
            needs_terminal_reinit: false,
            last_seen_by_pane: HashMap::new(),
            last_tail_sig_by_pane: HashMap::new(),
            last_activity_by_pane: HashMap::new(),
        }
    }

    pub fn refresh(&mut self) -> Result<()> {
        let panes = query::list_panes()?;
        let pane_pids: Vec<i32> = panes.iter().map(|p| p.pid).collect();
        let mut new_records = Vec::with_capacity(panes.len());

        for pane in panes {
            let argv = process::resolve_deepest_argv(pane.pid);
            let pane_tail = query::capture_pane_tail(&pane.pane_id, 80).unwrap_or_default();
            let mut record = classify::detect_agent(pane, argv);
            let now = Instant::now();
            let sig = tail_signature(&pane_tail);
            let previous_sig = self
                .last_tail_sig_by_pane
                .insert(record.pane.pane_id.clone(), sig);
            let last_activity = self
                .last_activity_by_pane
                .entry(record.pane.pane_id.clone())
                .or_insert(now);
            if previous_sig != Some(sig) {
                *last_activity = now;
            }
            let since_last_output = now.saturating_duration_since(*last_activity);
            record.status = infer_status(&record.pane, &pane_tail, since_last_output);
            record.last_seen = *self
                .last_seen_by_pane
                .entry(record.pane.pane_id.clone())
                .or_insert(now);
            new_records.push(record);
        }

        self.all_records = new_records;
        self.external_agents = process::list_external_agents(&pane_pids);
        self.rebuild_records();

        self.fix_selection_bounds();
        if self.preview_visible {
            self.refresh_preview();
        }

        self.status = Self::keyboard_help().to_string();
        Ok(())
    }

    pub fn sort_records(&mut self) {
        match self.sort {
            SortKey::LastSeen => self.records.sort_by_key(|r| r.last_seen),
            SortKey::Project => self.records.sort_by(|a, b| a.pane.project_name().cmp(&b.pane.project_name())),
            SortKey::Agent => self.records.sort_by(|a, b| a.agent.as_str().cmp(b.agent.as_str())),
        }
        if self.sort == SortKey::LastSeen {
            self.records.reverse();
        }
    }

    pub fn selected_record(&self) -> Option<&AgentRecord> {
        if self.focus_panel == FocusPanel::Tmux {
            self.records.get(self.selected)
        } else {
            None
        }
    }

    pub fn selected_external_agent(&self) -> Option<&ExternalAgent> {
        if self.focus_panel == FocusPanel::Outside {
            self.external_agents.get(self.selected_external)
        } else {
            None
        }
    }

    pub fn next(&mut self) {
        match self.focus_panel {
            FocusPanel::Tmux => {
                if self.records.is_empty() {
                    if !self.external_agents.is_empty() {
                        self.focus_panel = FocusPanel::Outside;
                        self.selected_external = 0;
                    }
                    return;
                }
                if self.selected + 1 < self.records.len() {
                    self.selected += 1;
                } else if !self.external_agents.is_empty() {
                    self.focus_panel = FocusPanel::Outside;
                    self.selected_external = 0;
                }
            }
            FocusPanel::Outside => {
                if self.external_agents.is_empty() {
                    if !self.records.is_empty() {
                        self.focus_panel = FocusPanel::Tmux;
                    }
                    return;
                }
                if self.selected_external + 1 < self.external_agents.len() {
                    self.selected_external += 1;
                }
            }
        }
        if self.preview_visible {
            self.refresh_preview();
        }
    }

    pub fn previous(&mut self) {
        match self.focus_panel {
            FocusPanel::Tmux => {
                if self.records.is_empty() {
                    return;
                }
                self.selected = self.selected.saturating_sub(1);
            }
            FocusPanel::Outside => {
                if self.external_agents.is_empty() {
                    return;
                }
                if self.selected_external > 0 {
                    self.selected_external -= 1;
                } else if !self.records.is_empty() {
                    self.focus_panel = FocusPanel::Tmux;
                    self.selected = self.records.len() - 1;
                }
            }
        }
        if self.preview_visible {
            self.refresh_preview();
        }
    }

    pub fn cycle_filter(&mut self) {
        self.filter = self.filter.next();
        self.rebuild_records();
        if self.preview_visible {
            self.refresh_preview();
        }
    }

    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        self.rebuild_records();
        if self.preview_visible {
            self.refresh_preview();
        }
    }

    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        self.sort_records();
        if self.preview_visible {
            self.refresh_preview();
        }
    }

    pub fn jump_selected(&mut self) -> Result<()> {
        if self.focus_panel == FocusPanel::Outside {
            let Some(ext) = self.selected_external_agent() else {
                return Ok(());
            };
            let spawn_result = spawn::adopt_external_agent(ext)?;
            match spawn_result {
                SpawnResult::Switched { target: _ } => {}
                SpawnResult::AttachedReturned { target: _ } => {
                    self.needs_terminal_reinit = true;
                }
            }
            return Ok(());
        }
        let Some(rec) = self.selected_record() else {
            return Ok(());
        };
        let jump_result = jump::jump_to_pane(&rec.pane)?;
        if jump_result == JumpResult::AttachedReturned {
            self.needs_terminal_reinit = true;
        }
        Ok(())
    }

    pub fn take_terminal_reinit_request(&mut self) -> bool {
        let requested = self.needs_terminal_reinit;
        self.needs_terminal_reinit = false;
        requested
    }

    pub fn start_search(&mut self) {
        self.search_mode = true;
        self.filter = Filter::All;
        self.rebuild_records();
    }

    pub fn stop_search(&mut self) {
        self.search_mode = false;
    }

    pub fn clear_search(&mut self) {
        self.search_query.clear();
        self.rebuild_records();
        if self.preview_visible {
            self.refresh_preview();
        }
    }

    pub fn push_search_char(&mut self, c: char) {
        self.search_query.push(c);
        self.rebuild_records();
        if self.preview_visible {
            self.refresh_preview();
        }
    }

    pub fn pop_search_char(&mut self) {
        self.search_query.pop();
        self.rebuild_records();
        if self.preview_visible {
            self.refresh_preview();
        }
    }

    pub fn toggle_preview(&mut self) {
        self.preview_visible = !self.preview_visible;
        if self.preview_visible {
            self.refresh_preview();
        } else {
            self.preview_text.clear();
        }
    }

    pub fn hide_preview(&mut self) {
        self.preview_visible = false;
        self.preview_text.clear();
    }

    pub fn refresh_preview_only(&mut self) {
        if self.preview_visible {
            self.refresh_preview();
        }
    }

    fn rebuild_records(&mut self) {
        self.records = self
            .all_records
            .iter()
            .filter(|r| r.agent != AgentType::Unknown)
            .filter(|r| self.filter.matches(r.agent))
            .filter(|r| record_matches_query(r, &self.search_query))
            .cloned()
            .collect();
        self.sort_records();
        self.fix_selection_bounds();
    }

    fn fix_selection_bounds(&mut self) {
        if self.selected >= self.records.len() {
            self.selected = self.records.len().saturating_sub(1);
        }
        if self.selected_external >= self.external_agents.len() {
            self.selected_external = self.external_agents.len().saturating_sub(1);
        }
        if self.focus_panel == FocusPanel::Tmux && self.records.is_empty() && !self.external_agents.is_empty()
        {
            self.focus_panel = FocusPanel::Outside;
        }
        if self.focus_panel == FocusPanel::Outside
            && self.external_agents.is_empty()
            && !self.records.is_empty()
        {
            self.focus_panel = FocusPanel::Tmux;
        }
    }

    fn refresh_preview(&mut self) {
        if let Some(record) = self.selected_record() {
            self.preview_text = query::capture_pane_tail_ansi(&record.pane.pane_id, 120)
                .unwrap_or_else(|_| "".to_string());
            return;
        }
        if let Some(ext) = self.selected_external_agent() {
            self.preview_text = format!(
                "outside tmux process\npid={}\nagent={}\n\nClick Enter to adopt into a tmux session for previews",
                ext.pid,
                ext.agent.as_str()
            );
            return;
        }
        self.preview_text.clear();
    }
}

fn infer_status(pane: &PaneInfo, tail: &str, since_last_output: Duration) -> AgentStatus {
    if pane.dead {
        return AgentStatus::Idle;
    }

    let t = tail.to_ascii_lowercase();
    let recent = take_last_lines(&t, 16);

    if contains_any(
        &recent,
        &[
            "press enter",
            "continue?",
            "y/n",
            "[y/n]",
            "approve",
            "waiting for input",
            "awaiting input",
            "what would you like",
            "next task",
            "enter to continue",
        ],
    ) {
        return AgentStatus::WaitingInput;
    }

    if contains_any(
        &recent,
        &["error:", " failed", "exception", "traceback", "permission denied"],
    ) {
        return AgentStatus::Error;
    }

    if contains_any(
        &recent,
        &["editing", "apply patch", "diff", "updated file", "writing"],
    ) {
        return AgentStatus::Editing;
    }

    if contains_any(
        &recent,
        &[
            "running",
            "executing",
            "building",
            "compiling",
            "testing",
            "searching",
            "fetching",
        ],
    ) {
        return AgentStatus::Running;
    }

    if contains_any(&recent, &["thinking", "analyzing", "planning", "reasoning"]) {
        return AgentStatus::Thinking;
    }

    if since_last_output > Duration::from_secs(45) {
        return AgentStatus::Idle;
    }

    if pane.active && since_last_output <= Duration::from_secs(15) {
        AgentStatus::Running
    } else {
        AgentStatus::Idle
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn take_last_lines(s: &str, count: usize) -> String {
    let mut lines: Vec<&str> = s.lines().filter(|line| !line.trim().is_empty()).collect();
    if lines.len() > count {
        lines = lines.split_off(lines.len() - count);
    }
    lines.join("\n")
}

fn tail_signature(tail: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tail.hash(&mut hasher);
    hasher.finish()
}

fn record_matches_query(record: &AgentRecord, query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }

    let fields = [
        record.agent.as_str().to_string(),
        record.pane.session.clone(),
        record.pane.window_name.clone(),
        record.pane.window_index.to_string(),
        record.pane.pane_index.to_string(),
        record.pane.target(),
        record.pane.project_name(),
        record.pane.cwd.clone(),
        record.pane.title.clone(),
        record.pane.start_cmd.clone(),
        record.full_argv.clone(),
    ];

    fields.iter().any(|field| fuzzy_subsequence_match(field, q))
}

fn fuzzy_subsequence_match(haystack: &str, needle: &str) -> bool {
    let mut needle_chars = needle.chars().flat_map(char::to_lowercase);
    let mut current = needle_chars.next();
    if current.is_none() {
        return true;
    }

    for hc in haystack.chars().flat_map(char::to_lowercase) {
        if Some(hc) == current {
            current = needle_chars.next();
            if current.is_none() {
                return true;
            }
        }
    }

    false
}
