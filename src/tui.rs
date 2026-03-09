use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::{Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};
use ratatui::{Frame, layout::Rect};

use crate::app::{App, FocusPanel};
use crate::models::AgentStatus;

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let area = f.area();
    f.render_widget(Clear, area);
    f.render_widget(Block::default().style(Style::default().bg(Color::Black)), area);

    if app.external_agents.is_empty() {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(10),
                Constraint::Length(5),
                Constraint::Length(1),
            ])
            .split(f.area());

        draw_header(f, chunks[0], app);
        draw_search(f, chunks[1], app);
        if app.preview_visible {
            draw_preview(f, chunks[2], app);
        } else {
            draw_table(f, chunks[2], app);
        }
        draw_detail_panels(f, chunks[3], app);
        draw_status(f, chunks[4], app);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(8),
                Constraint::Length(6),
                Constraint::Length(5),
                Constraint::Length(1),
            ])
            .split(f.area());

        draw_header(f, chunks[0], app);
        draw_search(f, chunks[1], app);
        if app.preview_visible {
            draw_preview(f, chunks[2], app);
        } else {
            draw_table(f, chunks[2], app);
        }
        draw_external_agents(f, chunks[3], app);
        draw_detail_panels(f, chunks[4], app);
        draw_status(f, chunks[5], app);
    }
}

fn draw_header(f: &mut Frame<'_>, area: Rect, app: &App) {
    let text = format!(
        "agenttop | filter={} sort={} records={}",
        app.filter.label(),
        app.sort.label(),
        app.records.len()
    );
    let p = Paragraph::new(text).style(Style::default().fg(Color::Cyan));
    f.render_widget(p, area);
}

fn draw_search(f: &mut Frame<'_>, area: Rect, app: &App) {
    let prompt = if app.search_mode { "/" } else { "" };
    let query = if app.search_query.is_empty() {
        "(type / to search, esc to clear)"
    } else {
        &app.search_query
    };
    let text = format!("search: {prompt}{query}");
    let style = if app.search_mode {
        Style::default().fg(Color::LightYellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(Paragraph::new(text).style(style), area);
}

fn draw_table(f: &mut Frame<'_>, area: Rect, app: &App) {
    let header = Row::new(vec![
        Cell::from("Status"),
        Cell::from("Project"),
        Cell::from("Agent"),
        Cell::from("CWD"),
    ])
    .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let rows = app.records.iter().enumerate().map(|(idx, r)| {
        let style = if app.focus_panel == FocusPanel::Tmux && idx == app.selected {
            Style::default().bg(Color::Blue)
        } else {
            Style::default()
        };
        Row::new(vec![
            Cell::from(status_cell(r.status)),
            Cell::from(r.pane.project_name()),
            Cell::from(r.agent.as_str()),
            Cell::from(r.pane.cwd.clone()),
        ])
        .style(style)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(16),
            Constraint::Length(10),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(Block::default().title("Agents").borders(Borders::ALL));

    f.render_widget(table, area);
}

fn draw_external_agents(f: &mut Frame<'_>, area: Rect, app: &App) {
    let header = Row::new(vec![
        Cell::from("Status"),
        Cell::from("Agent"),
        Cell::from("PID"),
        Cell::from("CWD"),
        Cell::from("Cmd"),
    ])
    .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let rows = app.external_agents.iter().enumerate().map(|(idx, p)| {
        let style = if app.focus_panel == FocusPanel::Outside && idx == app.selected_external {
            Style::default().bg(Color::Blue)
        } else {
            Style::default()
        };
        Row::new(vec![
            Cell::from(status_cell(p.status)),
            Cell::from(p.agent.as_str()),
            Cell::from(p.pid.to_string()),
            Cell::from(p.cwd.clone()),
            Cell::from(p.cmdline.clone()),
        ])
        .style(style)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(30),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(Block::default().title("Outside tmux").borders(Borders::ALL));

    f.render_widget(table, area);
}

fn draw_detail_panels(f: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);
    draw_details(f, chunks[0], app);
    draw_tmux_details(f, chunks[1], app);
}

fn draw_details(f: &mut Frame<'_>, area: Rect, app: &App) {
    let detail = if let Some(r) = app.selected_record() {
        format!(
            "status={}\nagent={}\ncwd={}",
            r.status.as_str(),
            r.agent.as_str(),
            r.pane.cwd
        )
    } else if let Some(p) = app.selected_external_agent() {
        format!(
            "outside tmux\nstatus={}\npid={}\nagent={}\ncwd={}\ncmd={}",
            p.status.as_str(),
            p.pid,
            p.agent.as_str(),
            p.cwd,
            p.cmdline
        )
    } else {
        "No row selected".to_string()
    };
    f.render_widget(
        Paragraph::new(detail).block(Block::default().title("Details").borders(Borders::ALL)),
        area,
    );
}

fn draw_tmux_details(f: &mut Frame<'_>, area: Rect, app: &App) {
    let detail = if let Some(r) = app.selected_record() {
        format!(
            "tmux={}:w{}.p{}\nsession={}\nwindow={} ({})\npane={}\ntarget={}",
            r.pane.session,
            r.pane.window_index,
            r.pane.pane_index,
            r.pane.session,
            r.pane.window_index,
            r.pane.window_name,
            r.pane.pane_index,
            r.pane.target()
        )
    } else {
        "outside-tmux selection\n\nEnter to move to tmux".to_string()
    };
    f.render_widget(
        Paragraph::new(detail).block(Block::default().title("Tmux Details").borders(Borders::ALL)),
        area,
    );
}

fn draw_status(f: &mut Frame<'_>, area: Rect, app: &App) {
    let p = Paragraph::new(app.status.clone()).style(Style::default().fg(Color::Green));
    f.render_widget(p, area);
}

fn draw_preview(f: &mut Frame<'_>, area: Rect, app: &App) {
    let text = if app.preview_text.trim().is_empty() {
        "(no preview available)".to_string()
    } else {
        app.preview_text.clone()
    };
    f.render_widget(
        Paragraph::new(text).block(Block::default().title("Preview").borders(Borders::ALL)),
        area,
    );
}

fn status_cell(status: AgentStatus) -> Line<'static> {
    let color = match status {
        AgentStatus::Thinking | AgentStatus::Editing | AgentStatus::Running => Color::Green,
        AgentStatus::WaitingInput => Color::LightYellow,
        AgentStatus::Error => Color::Red,
        AgentStatus::Idle => Color::DarkGray,
    };
    Line::from(vec![
        Span::styled("●", Style::default().fg(color)),
        Span::raw(" "),
        Span::raw(status.as_str()),
    ])
}
