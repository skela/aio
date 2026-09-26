use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::actions::jump;
use crate::actions::jump::JumpResult;
use crate::actions::lazygit;
use crate::actions::spawn;
use crate::actions::spawn::SpawnResult;
use crate::detect::{classify, process};
use crate::history::{self, ClosedSession};
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
pub enum NewSessionField {
    Name,
    Path,
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
    pub new_session_path: String,
    pub new_session_field: NewSessionField,
    /// Directory candidates shown under the path field after an ambiguous Tab.
    pub new_session_completions: Vec<String>,
    pub preview_visible: bool,
    pub preview_text: String,
    pub help_visible: bool,
    pub pending_terminal_reinits: u8,
    pub delayed_terminal_reinit_at: Option<Instant>,
    /// Tracks an in-progress multi-key sequence (e.g. `space` -> `g` -> `g`).
    pub key_seq: Vec<char>,
    /// Recently closed tmux agent sessions, newest first (persisted).
    pub closed_sessions: Vec<ClosedSession>,
    pub closed_visible: bool,
    pub closed_selected: usize,
    /// Last known agent record per pane, used to notice when an agent goes
    /// away.  Kept while the pane is briefly unclassifiable so a transient
    /// Unknown tick doesn't lose the pane.
    tracked_agents: HashMap<String, AgentRecord>,
    /// Panes aio closed on purpose (eject) — not recorded as "closed".
    suppress_close: HashSet<String>,
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
            new_session_path: String::new(),
            new_session_field: NewSessionField::Name,
            new_session_completions: Vec::new(),
            preview_visible: false,
            preview_text: String::new(),
            help_visible: false,
            pending_terminal_reinits: 0,
            delayed_terminal_reinit_at: None,
            key_seq: Vec::new(),
            closed_sessions: history::load(),
            closed_visible: false,
            closed_selected: 0,
            tracked_agents: HashMap::new(),
            suppress_close: HashSet::new(),
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

        self.track_closed_agents(&new_records);
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
        let pane_id = rec.pane.pane_id.clone();
        spawn::eject_tmux_agent(rec)?;
        self.suppress_close.insert(pane_id);
        self.request_terminal_reinit(Duration::from_secs(1));
        self.status = format!("ejected {} to a new terminal", agent.as_str());
        Ok(())
    }

    /// Compares this tick's panes with the last known agent panes and records
    /// any agent whose pane disappeared, or whose pane dropped back to a
    /// plain shell (agent quit), as a closed session.
    fn track_closed_agents(&mut self, new_records: &[AgentRecord]) {
        let by_id: HashMap<&str, &AgentRecord> = new_records
            .iter()
            .map(|r| (r.pane.pane_id.as_str(), r))
            .collect();

        let mut closed = Vec::new();
        self.tracked_agents.retain(|pane_id, prev| {
            let gone = match by_id.get(pane_id.as_str()) {
                None => true,
                Some(now) => now.agent == AgentType::Unknown && is_shell(&now.pane.current_cmd),
            };
            if gone {
                closed.push(prev.clone());
            }
            !gone
        });

        for rec in new_records {
            if rec.agent != AgentType::Unknown {
                self.tracked_agents.insert(rec.pane.pane_id.clone(), rec.clone());
            }
        }

        let mut changed = false;
        for rec in closed {
            if self.suppress_close.remove(&rec.pane.pane_id) {
                continue;
            }
            history::push(
                &mut self.closed_sessions,
                ClosedSession {
                    agent: rec.agent.as_str().to_string(),
                    tmux_session: rec.pane.session.clone(),
                    window_name: rec.pane.window_name.clone(),
                    cwd: rec.pane.cwd.clone(),
                    title: rec.pane.title.clone(),
                    closed_at: history::now_secs(),
                },
            );
            changed = true;
        }
        if changed {
            history::save(&self.closed_sessions);
            self.fix_closed_selection();
        }
    }

    pub fn toggle_closed_sessions(&mut self) {
        self.closed_visible = !self.closed_visible;
        self.closed_selected = 0;
    }

    pub fn hide_closed_sessions(&mut self) {
        self.closed_visible = false;
    }

    pub fn closed_next(&mut self) {
        if self.closed_selected + 1 < self.closed_sessions.len() {
            self.closed_selected += 1;
        }
    }

    pub fn closed_previous(&mut self) {
        self.closed_selected = self.closed_selected.saturating_sub(1);
    }

    fn fix_closed_selection(&mut self) {
        if self.closed_selected >= self.closed_sessions.len() {
            self.closed_selected = self.closed_sessions.len().saturating_sub(1);
        }
    }

    pub fn forget_selected_closed(&mut self) {
        if self.closed_selected < self.closed_sessions.len() {
            self.closed_sessions.remove(self.closed_selected);
            history::save(&self.closed_sessions);
            self.fix_closed_selection();
        }
    }

    pub fn reopen_selected_closed(&mut self) -> Result<()> {
        let Some(entry) = self.closed_sessions.get(self.closed_selected).cloned() else {
            return Ok(());
        };
        let Some(cmd) = history::resume_command(&entry) else {
            self.status = "cannot reopen unknown agent".to_string();
            return Ok(());
        };
        match spawn::reopen_window(&entry.tmux_session, &entry.window_name, &entry.cwd, &cmd) {
            Ok(result) => {
                self.closed_sessions.remove(self.closed_selected);
                history::save(&self.closed_sessions);
                self.fix_closed_selection();
                self.closed_visible = false;
                self.status = format!("reopened '{}' ({cmd})", entry.window_name);
                if matches!(result, SpawnResult::AttachedReturned { .. }) {
                    self.request_terminal_reinit(Duration::from_millis(0));
                }
            }
            Err(err) => self.status = format!("{err}"),
        }
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
        self.new_session_path = "~/".to_string();
        self.new_session_field = NewSessionField::Name;
        self.new_session_completions.clear();
    }

    pub fn cancel_new_session_prompt(&mut self) {
        self.new_session_mode = false;
        self.new_session_name.clear();
        self.new_session_path.clear();
        self.new_session_field = NewSessionField::Name;
        self.new_session_completions.clear();
    }

    pub fn toggle_new_session_field(&mut self) {
        self.new_session_completions.clear();
        self.new_session_field = match self.new_session_field {
            NewSessionField::Name => NewSessionField::Path,
            NewSessionField::Path => NewSessionField::Name,
        };
    }

    /// Tab: on the name field, move to the path field; on the path field,
    /// complete the directory name (shell-style).
    pub fn tab_new_session_field(&mut self) {
        match self.new_session_field {
            NewSessionField::Name => self.toggle_new_session_field(),
            NewSessionField::Path => self.complete_new_session_path(),
        }
    }

    fn complete_new_session_path(&mut self) {
        let home = std::env::var("HOME").unwrap_or_default();
        let completion = complete_dir_path(&self.new_session_path, &home);
        self.new_session_path = completion.input;
        self.new_session_completions = if completion.candidates.len() > 1 {
            completion.candidates
        } else {
            Vec::new()
        };
        if completion.no_match {
            self.status = "no matching directories".to_string();
        }
    }

    pub fn push_new_session_char(&mut self, c: char) {
        self.new_session_completions.clear();
        match self.new_session_field {
            NewSessionField::Name => self.new_session_name.push(c),
            NewSessionField::Path => self.new_session_path.push(c),
        }
    }

    pub fn pop_new_session_char(&mut self) {
        self.new_session_completions.clear();
        match self.new_session_field {
            NewSessionField::Name => self.new_session_name.pop(),
            NewSessionField::Path => self.new_session_path.pop(),
        };
    }

    /// Enter on the name field advances to the path field; Enter on the path
    /// field creates the window.
    pub fn confirm_new_session_field(&mut self) -> Result<()> {
        match self.new_session_field {
            NewSessionField::Name => {
                self.new_session_field = NewSessionField::Path;
                Ok(())
            }
            NewSessionField::Path => self.submit_new_session(),
        }
    }

    pub fn submit_new_session(&mut self) -> Result<()> {
        let name = self.new_session_name.trim().to_string();
        if name.is_empty() {
            self.status = "window name cannot be empty".to_string();
            self.new_session_field = NewSessionField::Name;
            return Ok(());
        }

        let cwd = match resolve_new_session_path(&self.new_session_path) {
            Ok(dir) => dir,
            Err(msg) => {
                self.status = msg;
                self.new_session_field = NewSessionField::Path;
                return Ok(());
            }
        };

        match spawn::create_named_window(&name, Some(&cwd)) {
            Ok(SpawnResult::Switched { target: _ }) => {
                self.cancel_new_session_prompt();
                self.status = format!("created window '{name}' in {cwd}");
            }
            Ok(SpawnResult::AttachedReturned { target: _ }) => {
                self.cancel_new_session_prompt();
                self.status = format!("created window '{name}' in {cwd}");
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

/// Whether a pane's foreground command is an interactive shell, i.e. the
/// agent that used to run there has exited.
fn is_shell(cmd: &str) -> bool {
    let cmd = cmd.trim_start_matches('-');
    matches!(
        cmd,
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "elvish" | "xonsh"
    )
}

#[derive(Debug, PartialEq, Eq)]
struct PathCompletion {
    /// The (possibly extended) path input.
    input: String,
    /// All matching directory names in the parent directory.
    candidates: Vec<String>,
    no_match: bool,
}

/// Shell-style directory completion for the new-window path field.
///
/// Completes the last path component against directories in its parent.  A
/// unique match is completed with a trailing `/`; multiple matches extend the
/// input to their longest common prefix and are returned as candidates.
/// Hidden directories are only offered when the typed prefix starts with `.`.
/// Relative paths resolve against `home`, matching `resolve_new_session_path`.
fn complete_dir_path(input: &str, home: &str) -> PathCompletion {
    let input = if input.is_empty() || input == "~" {
        "~/".to_string()
    } else {
        input.to_string()
    };

    let (dir_part, prefix) = match input.rfind('/') {
        Some(idx) => (&input[..=idx], &input[idx + 1..]),
        None => ("", input.as_str()),
    };

    let home = home.trim_end_matches('/');
    let search_dir = if dir_part.is_empty() {
        std::path::PathBuf::from(home)
    } else if let Some(rest) = dir_part.strip_prefix("~/") {
        std::path::Path::new(home).join(rest)
    } else if dir_part.starts_with('/') {
        std::path::PathBuf::from(dir_part)
    } else {
        std::path::Path::new(home).join(dir_part)
    };

    let show_hidden = prefix.starts_with('.');
    let mut candidates: Vec<String> = std::fs::read_dir(&search_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|name| name.starts_with(prefix))
                .filter(|name| show_hidden || !name.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    candidates.sort();

    match candidates.len() {
        0 => PathCompletion {
            input,
            candidates,
            no_match: true,
        },
        1 => PathCompletion {
            input: format!("{dir_part}{}/", candidates[0]),
            candidates,
            no_match: false,
        },
        _ => {
            let common = longest_common_prefix(&candidates);
            let input = if common.len() > prefix.len() {
                format!("{dir_part}{common}")
            } else {
                input
            };
            PathCompletion {
                input,
                candidates,
                no_match: false,
            }
        }
    }
}

fn longest_common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut end = first.len();
    for item in &items[1..] {
        end = first
            .char_indices()
            .zip(item.chars())
            .take_while(|((_, a), b)| a == b)
            .map(|((i, a), _)| i + a.len_utf8())
            .last()
            .unwrap_or(0)
            .min(end);
    }
    first[..end].to_string()
}

/// Expands `~` and validates the directory typed into the new-window prompt.
/// An empty input (or plain `~`) resolves to the home directory.
fn resolve_new_session_path(input: &str) -> std::result::Result<String, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let trimmed = input.trim();
    let expanded = if trimmed.is_empty() || trimmed == "~" {
        home.clone()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        format!("{}/{}", home.trim_end_matches('/'), rest)
    } else {
        trimmed.to_string()
    };
    if expanded.is_empty() {
        return Err("could not determine home directory; enter a path".to_string());
    }

    let path = std::path::Path::new(&expanded);
    let path = if path.is_relative() {
        std::path::Path::new(&home).join(path)
    } else {
        path.to_path_buf()
    };
    if !path.is_dir() {
        return Err(format!("not a directory: {}", path.display()));
    }
    let path = path.canonicalize().unwrap_or(path);
    path.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "path is not valid UTF-8".to_string())
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

    fn temp_home(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("aio-complete-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["code/aio", "code/aiox", "config", ".cache", "docs"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("code/afile"), "").unwrap();
        dir
    }

    #[test]
    fn completes_unique_match_with_slash() {
        let home = temp_home("unique");
        let h = home.to_str().unwrap();
        assert_eq!(complete_dir_path("~/co", h).input, "~/co");
        assert_eq!(complete_dir_path("~/cod", h).input, "~/code/");
        assert_eq!(complete_dir_path("~/d", h).input, "~/docs/");
        assert_eq!(complete_dir_path("docs", h).input, "docs/");
        let abs = format!("{h}/cod");
        assert_eq!(complete_dir_path(&abs, h).input, format!("{h}/code/"));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn extends_to_common_prefix_and_lists_candidates() {
        let home = temp_home("common");
        let h = home.to_str().unwrap();
        let c = complete_dir_path("~/code/a", h);
        assert_eq!(c.input, "~/code/aio");
        assert_eq!(c.candidates, vec!["aio".to_string(), "aiox".to_string()]);
        let c = complete_dir_path("~/co", h);
        assert_eq!(c.candidates, vec!["code".to_string(), "config".to_string()]);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn hides_dotdirs_and_files_and_reports_no_match() {
        let home = temp_home("hidden");
        let h = home.to_str().unwrap();
        let c = complete_dir_path("~/", h);
        assert!(!c.candidates.contains(&".cache".to_string()));
        assert_eq!(complete_dir_path("~/.ca", h).input, "~/.cache/");
        let c = complete_dir_path("~/code/af", h);
        assert!(c.no_match);
        assert_eq!(c.input, "~/code/af");
        assert_eq!(complete_dir_path("", h).input, "~/");
        std::fs::remove_dir_all(home).unwrap();
    }
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
