use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::actions::jump;
use crate::actions::jump::JumpResult;
use crate::actions::lazygit;
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
    pub new_session_mode: bool,
    pub new_session_name: String,
    pub preview_visible: bool,
    pub preview_text: String,
    pub help_visible: bool,
    pub pending_terminal_reinits: u8,
    pub delayed_terminal_reinit_at: Option<Instant>,
    /// Tracks an in-progress multi-key sequence (e.g. `space` -> `g` -> `g`).
    pub key_seq: Vec<char>,
    last_seen_by_pane: HashMap<String, Instant>,
    last_tail_sig_by_pane: HashMap<String, u64>,
    last_activity_by_pane: HashMap<String, Instant>,
    last_status_by_pane: HashMap<String, AgentStatus>,
    /// Tracks when a pane most recently entered an active status
    /// (Thinking/Editing/Running). Used to gate the "done" sound so we
    /// don't fire on transient one-tick blips.
    last_became_active_by_pane: HashMap<String, Instant>,
}

impl App {
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
            status: String::new(),
            search_mode: false,
            search_query: String::new(),
            new_session_mode: false,
            new_session_name: String::new(),
            preview_visible: false,
            preview_text: String::new(),
            help_visible: false,
            pending_terminal_reinits: 0,
            delayed_terminal_reinit_at: None,
            key_seq: Vec::new(),
            last_seen_by_pane: HashMap::new(),
            last_tail_sig_by_pane: HashMap::new(),
            last_activity_by_pane: HashMap::new(),
            last_status_by_pane: HashMap::new(),
            last_became_active_by_pane: HashMap::new(),
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
            record.status = infer_status(record.agent, &record.pane, &pane_tail, since_last_output);
            let prev_status = self
                .last_status_by_pane
                .insert(record.pane.pane_id.clone(), record.status);

            let is_active = matches!(
                record.status,
                AgentStatus::Thinking | AgentStatus::Editing | AgentStatus::Running
            );
            let was_active = prev_status.map_or(false, |p| {
                matches!(p, AgentStatus::Thinking | AgentStatus::Editing | AgentStatus::Running)
            });

            // Track when this pane first became continuously active.
            if is_active && !was_active {
                self.last_became_active_by_pane.insert(record.pane.pane_id.clone(), now);
            } else if !is_active {
                // Clear when no longer active so a future burst starts fresh.
                self.last_became_active_by_pane.remove(&record.pane.pane_id);
            }

            // Sound: the pane has newly entered a state that needs attention.
            // Do not play on first discovery, since aio may be opened while
            // several agents are already waiting.
            if prev_status.is_some_and(|status| status != AgentStatus::WaitingInput)
                && record.status == AgentStatus::WaitingInput
            {
                crate::sound::play_input_needed();
            }
            // Sound: done — agent was active for at least 10s and just went idle.
            // The 10s floor prevents blips from transient keyword matches.
            if was_active && record.status == AgentStatus::Idle {
                let active_since = self.last_became_active_by_pane.get(&record.pane.pane_id);
                let active_duration = active_since.map_or(Duration::ZERO, |t| now.saturating_duration_since(*t));
                if active_duration >= Duration::from_secs(10) {
                    crate::sound::play_done();
                }
            }
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
                    self.request_terminal_reinit(Duration::from_millis(0));
                }
            }
            return Ok(());
        }
        let Some(rec) = self.selected_record() else {
            return Ok(());
        };
        let jump_result = jump::jump_to_pane(&rec.pane)?;
        if jump_result == JumpResult::AttachedReturned {
            self.request_terminal_reinit(Duration::from_millis(0));
        }
        Ok(())
    }

    pub fn eject_selected(&mut self) -> Result<()> {
        let Some(rec) = self.selected_record() else {
            self.status = "eject only works for tmux agents".to_string();
            return Ok(());
        };
        let agent = rec.agent;
        spawn::eject_tmux_agent(rec)?;
        self.request_terminal_reinit(Duration::from_secs(1));
        self.status = format!("ejected {} to a new terminal", agent.as_str());
        Ok(())
    }

    pub fn open_lazygit_selected(&mut self) -> Result<()> {
        let Some(rec) = self.selected_record() else {
            self.status = "lazygit requires a selected tmux agent".to_string();
            return Ok(());
        };
        let pane = rec.pane.clone();
        lazygit::open_lazygit(&pane)?;
        // After switch-client returns we need to redraw the TUI.
        self.request_terminal_reinit(Duration::from_millis(0));
        Ok(())
    }

    pub fn take_terminal_reinit_request(&mut self) -> bool {
        if self.pending_terminal_reinits > 0 {
            self.pending_terminal_reinits -= 1;
            true
        } else {
            false
        }
    }

    pub fn poll_delayed_terminal_reinit(&mut self) -> bool {
        let Some(deadline) = self.delayed_terminal_reinit_at else {
            return false;
        };
        if Instant::now() >= deadline {
            self.delayed_terminal_reinit_at = None;
            self.pending_terminal_reinits = self.pending_terminal_reinits.max(1);
            true
        } else {
            false
        }
    }

    pub fn delayed_terminal_reinit_timeout(&self) -> Option<Duration> {
        self.delayed_terminal_reinit_at
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    fn request_terminal_reinit(&mut self, delayed_by: Duration) {
        self.pending_terminal_reinits = self.pending_terminal_reinits.max(2);
        if !delayed_by.is_zero() {
            self.delayed_terminal_reinit_at = Some(Instant::now() + delayed_by);
        }
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

    pub fn start_new_session_prompt(&mut self) {
        self.new_session_mode = true;
        self.new_session_name.clear();
    }

    pub fn cancel_new_session_prompt(&mut self) {
        self.new_session_mode = false;
        self.new_session_name.clear();
    }

    pub fn push_new_session_char(&mut self, c: char) {
        self.new_session_name.push(c);
    }

    pub fn pop_new_session_char(&mut self) {
        self.new_session_name.pop();
    }

    pub fn submit_new_session(&mut self) -> Result<()> {
        let name = self.new_session_name.trim().to_string();
        if name.is_empty() {
            self.status = "window name cannot be empty".to_string();
            return Ok(());
        }

        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| p.to_str().map(|s| s.to_string()));

        match spawn::create_named_window(&name, cwd.as_deref()) {
            Ok(SpawnResult::Switched { target: _ }) => {
                self.new_session_mode = false;
                self.new_session_name.clear();
                self.status = format!("created window '{name}'");
            }
            Ok(SpawnResult::AttachedReturned { target: _ }) => {
                self.new_session_mode = false;
                self.new_session_name.clear();
                self.status = format!("created window '{name}'");
                self.request_terminal_reinit(Duration::from_millis(0));
            }
            Err(err) => {
                // Keep the prompt open with the typed name so the user can
                // edit and retry (e.g. when not running inside tmux).
                self.status = format!("{err}");
            }
        }
        Ok(())
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

    pub fn toggle_help(&mut self) {
        self.help_visible = !self.help_visible;
    }

    pub fn test_sound(&mut self, input_needed: bool) {
        if input_needed {
            crate::sound::play_input_needed();
            self.status = "played input-needed sound".to_string();
        } else {
            crate::sound::play_done();
            self.status = "played done sound".to_string();
        }
    }

    pub fn hide_help(&mut self) {
        self.help_visible = false;
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

fn infer_status(
    agent: AgentType,
    pane: &PaneInfo,
    tail: &str,
    since_last_output: Duration,
) -> AgentStatus {
    if pane.dead {
        return AgentStatus::Idle;
    }

    // Only inspect the current screen footer.  capture-pane includes old
    // transcript lines, so matching the whole tail makes a completed tool
    // invocation (or an old error) look like the current state indefinitely.
    let recent = collapse_spaces(&take_last_lines(&tail.to_ascii_lowercase(), 8));

    match agent {
        AgentType::Codex => infer_codex_status(&recent, since_last_output),
        AgentType::Opencode => infer_opencode_status(&recent),
        AgentType::Claude => infer_claude_status(&recent, since_last_output),
        AgentType::Unknown => AgentStatus::Idle,
    }
}

fn infer_codex_status(recent: &str, since_last_output: Duration) -> AgentStatus {
    // Codex replaces its composer with this footer while it is generating or
    // executing.  It is substantially more reliable than matching verbs in
    // the conversation transcript.
    if contains_any(recent, &["esc to interrupt", "ctrl+c to interrupt", "working..."]) {
        return AgentStatus::Running;
    }
    if contains_any(recent, &["do you want to proceed", "would you like to", "select an option", "enter to select", "press enter to", "allow this", "approve this"]) {
        return AgentStatus::WaitingInput;
    }
    // `›` is Codex's visible composer.  It is displayed only once control is
    // back with the user (including after a completed response).
    if has_composer(recent, &["›", "❯"]) {
        return AgentStatus::WaitingInput;
    }
    if has_error(recent) {
        return AgentStatus::Error;
    }
    if contains_any(recent, &["thinking", "analyzing", "planning", "reasoning"]) {
        return AgentStatus::Thinking;
    }
    if since_last_output <= Duration::from_secs(4) {
        AgentStatus::Running
    } else {
        AgentStatus::Idle
    }
}

fn infer_claude_status(recent: &str, since_last_output: Duration) -> AgentStatus {
    if contains_any(recent, &["esc to interrupt", "ctrl+c to interrupt", "working..."]) {
        return AgentStatus::Running;
    }
    if contains_any(recent, &["do you want to proceed", "would you like to", "enter to select", "press enter to", "allow this", "approve this", "waiting for input", "awaiting input"]) {
        return AgentStatus::WaitingInput;
    }
    // Claude's composer is normally a bare `>` at the bottom of the screen.
    if has_composer(recent, &[">"]) {
        return AgentStatus::WaitingInput;
    }
    if has_error(recent) {
        return AgentStatus::Error;
    }
    if contains_any(recent, &["thinking", "analyzing", "planning", "reasoning"]) {
        return AgentStatus::Thinking;
    }
    if since_last_output <= Duration::from_secs(4) {
        AgentStatus::Running
    } else {
        AgentStatus::Idle
    }
}

fn infer_opencode_status(recent: &str) -> AgentStatus {
    // Waiting for permission approval or user input
    if contains_any(
        recent,
        &[
            "permission required",
            "allow once",
            "allow always",
            "reject permission",
            // question tool prompts
            "enter submit",
            "enter toggle",
        ],
    ) {
        return AgentStatus::WaitingInput;
    }

    if has_error(recent) { return AgentStatus::Error; }

    // Editing / writing files
    if contains_any(
        recent,
        &[
            // completed tool icons in lowercase
            "\u{2190} edit",   // ← edit
            "\u{2190} write",  // ← write
            "\u{2190} patch",  // ← patch
            "# wrote",
            "# created",
            "# deleted",
            "# moved",
            "patched ",
            "updated file",
            "apply patch",
        ],
    ) {
        return AgentStatus::Editing;
    }

    // Active tool use / running
    if contains_any(
        recent,
        &[
            // active model generation indicator (most reliable signal)
            "esc interrupt",
            "esc again to interrupt",
            // read / glob / grep / fetch tool indicators (arrow prefix = in-progress)
            "\u{2192} read",    // → read
            "\u{2731} glob",    // ✱ glob
            "\u{2731} grep",    // ✱ grep
            "% webfetch",
            "% websearch",
            // subagent / task actively running
            "\u{2502} ",        // │ task running
            // startup
            "loading plugins",
            "finishing startup",
            // retry banner
            "retrying in",
        ],
    ) {
        return AgentStatus::Running;
    }

    // Thinking / reasoning
    if contains_any(
        recent,
        &[
            "thinking",
            "thought:",
            "analyzing",
            "planning",
            "reasoning",
        ],
    ) {
        return AgentStatus::Thinking;
    }

    // OpenCode's idle footer always keeps the composer controls visible. Once
    // its interrupt marker disappears, it is ready for user input.  This also
    // avoids treating a quiet, completed OpenCode turn as merely "idle".
    if contains_any(recent, &["ctrl+p commands", "ctrl+p to open commands"])
        || has_composer(recent, &[">", "❯"])
    {
        AgentStatus::WaitingInput
    } else {
        AgentStatus::Idle
    }
}

fn has_error(recent: &str) -> bool {
    // A transcript can contain many historical command failures.  Treat an
    // error as current only when it is rendered in the bottom status area,
    // rather than anywhere in the captured screen.
    recent
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(2)
        .any(|line| {
            let line = line.trim_start();
            line.starts_with("error:")
                || line.starts_with("fatal:")
                || line.starts_with("exception:")
                || line.contains("permission denied")
        })
}

/// Checks the last visible non-empty line for a CLI's composer prompt.  A
/// prompt elsewhere in the transcript is just quoted conversation text.
fn has_composer(recent: &str, prompts: &[&str]) -> bool {
    recent
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| {
            let line = line.trim_start();
            prompts.iter().any(|prompt| line == *prompt || line.starts_with(&format!("{prompt} ")))
        })
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

/// Collapses runs of the ASCII space character into a single space, leaving
/// newlines and other whitespace untouched. Terminal UIs pad status text
/// with an unpredictable number of spaces (icons, progress bars, right-hand
/// alignment), so keyword matches should not depend on exact spacing.
fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = false;
    for c in s.chars() {
        if c == ' ' {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
        } else {
            out.push(c);
            last_was_space = false;
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_pane() -> PaneInfo {
        PaneInfo {
            pane_id: "%1".to_string(),
            session: "s".to_string(),
            window_index: 1,
            window_name: "w".to_string(),
            pane_index: 1,
            pid: 1,
            current_cmd: "opencode".to_string(),
            cwd: "/tmp".to_string(),
            title: String::new(),
            active: true,
            dead: false,
            start_cmd: "opencode".to_string(),
        }
    }

    #[test]
    fn opencode_active_generation_is_running_not_idle() {
        // Real footer captured from a live pane while opencode was actively
        // generating (progress icons + variable padding before "esc
        // interrupt"). This previously misclassified as Idle because the
        // keyword literal had two spaces ("esc  interrupt") while the real
        // UI only ever emits one.
        let tail = "some earlier tool output\n\
             \u{2b1b}\u{2b1b}\u{2b1b}\u{22c5}\u{22c5}  esc interrupt          139.6K (14%) \u{b7} $2.55  ctrl+p commands\n";
        let status = infer_status(AgentType::Opencode, &dummy_pane(), tail, Duration::from_secs(1));
        assert_eq!(status, AgentStatus::Running);
    }

    #[test]
    fn opencode_thinking_keyword_still_matches() {
        let tail = "thinking about the best approach\n";
        let status = infer_status(AgentType::Opencode, &dummy_pane(), tail, Duration::from_secs(1));
        assert_eq!(status, AgentStatus::Thinking);
    }

    #[test]
    fn codex_interrupt_footer_is_running() {
        let tail = "I will inspect the project.\n\n  • Working (12s) · esc to interrupt\n";
        let status = infer_status(AgentType::Codex, &dummy_pane(), tail, Duration::from_secs(20));
        assert_eq!(status, AgentStatus::Running);
    }

    #[test]
    fn codex_composer_is_waiting_for_input() {
        let tail = "Finished the changes.\n\n› \n";
        let status = infer_status(AgentType::Codex, &dummy_pane(), tail, Duration::from_secs(20));
        assert_eq!(status, AgentStatus::WaitingInput);
    }

    #[test]
    fn opencode_idle_footer_is_waiting_for_input() {
        let tail = "Implemented the request.\n\n  ctrl+p commands\n";
        let status = infer_status(AgentType::Opencode, &dummy_pane(), tail, Duration::from_secs(20));
        assert_eq!(status, AgentStatus::WaitingInput);
    }

    #[test]
    fn claude_composer_is_waiting_for_input() {
        let tail = "Task complete.\n\n> \n";
        let status = infer_status(AgentType::Claude, &dummy_pane(), tail, Duration::from_secs(20));
        assert_eq!(status, AgentStatus::WaitingInput);
    }

    #[test]
    fn old_error_does_not_override_current_codex_composer() {
        let tail = "error: the previous command failed\nmore output\n\n› fix it\n";
        let status = infer_status(AgentType::Codex, &dummy_pane(), tail, Duration::from_secs(20));
        assert_eq!(status, AgentStatus::WaitingInput);
    }

    #[test]
    fn transcript_error_is_not_a_current_error_state() {
        let tail = "error: a previous command failed\nmore transcript\nstatus summary\n";
        let status = infer_status(AgentType::Codex, &dummy_pane(), tail, Duration::from_secs(20));
        assert_eq!(status, AgentStatus::Idle);
    }

    #[test]
    fn collapse_spaces_preserves_newlines_and_collapses_runs() {
        assert_eq!(collapse_spaces("a   b\n\nc    d"), "a b\n\nc d");
        assert_eq!(collapse_spaces("no extra spaces"), "no extra spaces");
        assert_eq!(collapse_spaces(""), "");
    }
}
