use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::text::{Line, Span};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};
use ratatui::{Frame, layout::Rect};
use std::env;
use std::sync::LazyLock;

use crate::app::{App, FocusPanel};
use crate::models::AgentStatus;

static HOME_DIR: LazyLock<Option<String>> = LazyLock::new(|| env::var("HOME").ok());

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
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(8),
                Constraint::Length(6),
                Constraint::Length(5),
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
    }

    if app.help_visible {
        draw_help_overlay(f);
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
            Cell::from(compact_home(&r.pane.cwd)),
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
            Cell::from(compact_home(&p.cwd)),
            Cell::from(compact_home_in_text(&p.cmdline)),
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
            compact_home(&r.pane.cwd)
        )
    } else if let Some(p) = app.selected_external_agent() {
        format!(
            "outside tmux\nstatus={}\npid={}\nagent={}\ncwd={}\ncmd={}",
            p.status.as_str(),
            p.pid,
            p.agent.as_str(),
            compact_home(&p.cwd),
            compact_home_in_text(&p.cmdline)
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

fn draw_help_overlay(f: &mut Frame<'_>) {
    let lines = vec![
        Line::from(Span::styled(" Keybindings ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(vec![Span::styled("  q          ", Style::default().fg(Color::Cyan)), Span::raw("quit")]),
        Line::from(vec![Span::styled("  j/k  ↑↓    ", Style::default().fg(Color::Cyan)), Span::raw("move selection")]),
        Line::from(vec![Span::styled("  enter      ", Style::default().fg(Color::Cyan)), Span::raw("jump to tmux pane")]),
        Line::from(vec![Span::styled("  E          ", Style::default().fg(Color::Cyan)), Span::raw("eject to terminal")]),
        Line::from(vec![Span::styled("  <spc>gg    ", Style::default().fg(Color::Cyan)), Span::raw("open lazygit in agent's cwd")]),
        Line::from(vec![Span::styled("  /          ", Style::default().fg(Color::Cyan)), Span::raw("search")]),
        Line::from(vec![Span::styled("  p          ", Style::default().fg(Color::Cyan)), Span::raw("toggle preview")]),
        Line::from(vec![Span::styled("  f          ", Style::default().fg(Color::Cyan)), Span::raw("cycle filter")]),
        Line::from(vec![Span::styled("  s          ", Style::default().fg(Color::Cyan)), Span::raw("cycle sort")]),
        Line::from(vec![Span::styled("  ctrl+o/a/c ", Style::default().fg(Color::Cyan)), Span::raw("set filter: opencode/codex/claude")]),
        Line::from(vec![Span::styled("  ?          ", Style::default().fg(Color::Cyan)), Span::raw("toggle this help")]),
        Line::from(vec![Span::styled("  esc        ", Style::default().fg(Color::Cyan)), Span::raw("close overlays")]),
    ];

    let width = 52u16;
    let height = lines.len() as u16 + 2;
    let area = f.area();
    let x = area.width.saturating_sub(width) / 2;
    let y = area.height.saturating_sub(height) / 2;
    let popup_area = Rect::new(x, y, width.min(area.width), height.min(area.height));

    f.render_widget(Clear, popup_area);
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).style(Style::default().bg(Color::Black))),
        popup_area,
    );
}

fn draw_preview(f: &mut Frame<'_>, area: Rect, app: &App) {
    let text = if app.preview_text.trim().is_empty() {
        "(no preview available)".to_string()
    } else {
        app.preview_text.clone()
    };
    let outside_selected = app.selected_record().is_none() && app.selected_external_agent().is_some();
    if outside_selected {
        let line_count = text.lines().count();
        let inner_height = area.height.saturating_sub(2) as usize;
        let top_padding = inner_height.saturating_sub(line_count) / 2;
        let centered_text = format!("{}{}", "\n".repeat(top_padding), text);
        f.render_widget(
            Paragraph::new(centered_text)
                .alignment(Alignment::Center)
                .block(Block::default().title("Preview").borders(Borders::ALL)),
            area,
        );
        return;
    }

    let lines = ansi_to_lines(&text);
    let visible_lines = area.height.saturating_sub(2) as usize;
    let total_lines = lines.len();
    let scroll_y = total_lines.saturating_sub(visible_lines) as u16;
    f.render_widget(
        Paragraph::new(lines)
            .scroll((scroll_y, 0))
            .block(Block::default().title("Preview").borders(Borders::ALL)),
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

#[derive(Clone, Copy, Debug, Default)]
struct AnsiStyleState {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    dim: bool,
    italic: bool,
    underlined: bool,
}

impl AnsiStyleState {
    fn to_style(self) -> Style {
        let mut style = Style::default();
        style.fg = self.fg;
        style.bg = self.bg;
        if self.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.dim {
            style = style.add_modifier(Modifier::DIM);
        }
        if self.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.underlined {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        style
    }
}

fn ansi_to_lines(input: &str) -> Vec<Line<'static>> {
    let bytes = input.as_bytes();
    let mut i = 0usize;
    let mut segment_start = 0usize;
    let mut style_state = AnsiStyleState::default();
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\x1b' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            push_fragment(
                &mut lines,
                &input[segment_start..i],
                style_state.to_style(),
            );
            let mut j = i + 2;
            while j < bytes.len() && !is_csi_final_byte(bytes[j]) {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'm' {
                apply_sgr(&mut style_state, &input[i + 2..j]);
            }
            i = if j < bytes.len() { j + 1 } else { bytes.len() };
            segment_start = i;
            continue;
        }
        if b == b'\n' {
            push_fragment(
                &mut lines,
                &input[segment_start..i],
                style_state.to_style(),
            );
            lines.push(Vec::new());
            i += 1;
            segment_start = i;
            continue;
        }
        if b == b'\r' {
            push_fragment(
                &mut lines,
                &input[segment_start..i],
                style_state.to_style(),
            );
            if let Some(current) = lines.last_mut() {
                current.clear();
            }
            i += 1;
            segment_start = i;
            continue;
        }
        i += 1;
    }

    if segment_start < bytes.len() {
        push_fragment(
            &mut lines,
            &input[segment_start..],
            style_state.to_style(),
        );
    }

    if lines.is_empty() {
        return vec![Line::default()];
    }
    lines.into_iter().map(Line::from).collect()
}

fn push_fragment(lines: &mut Vec<Vec<Span<'static>>>, fragment: &str, style: Style) {
    if fragment.is_empty() {
        return;
    }
    if let Some(current) = lines.last_mut() {
        current.push(Span::styled(fragment.to_string(), style));
    }
}

fn is_csi_final_byte(byte: u8) -> bool {
    (0x40..=0x7e).contains(&byte)
}

fn apply_sgr(style: &mut AnsiStyleState, sgr: &str) {
    let mut codes: Vec<u16> = if sgr.is_empty() {
        vec![0]
    } else {
        sgr.split(';')
            .map(|part| {
                if part.is_empty() {
                    0
                } else {
                    part.parse::<u16>().unwrap_or(0)
                }
            })
            .collect()
    };
    if codes.is_empty() {
        codes.push(0);
    }

    let mut idx = 0usize;
    while idx < codes.len() {
        match codes[idx] {
            0 => *style = AnsiStyleState::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underlined = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underlined = false,
            30..=37 => style.fg = Some(sgr_4bit_color(codes[idx] - 30, false)),
            39 => style.fg = None,
            40..=47 => style.bg = Some(sgr_4bit_color(codes[idx] - 40, false)),
            49 => style.bg = None,
            90..=97 => style.fg = Some(sgr_4bit_color(codes[idx] - 90, true)),
            100..=107 => style.bg = Some(sgr_4bit_color(codes[idx] - 100, true)),
            38 | 48 => {
                let is_fg = codes[idx] == 38;
                let (consumed, color) = parse_extended_sgr_color(&codes[idx + 1..]);
                if let Some(color) = color {
                    if is_fg {
                        style.fg = Some(color);
                    } else {
                        style.bg = Some(color);
                    }
                }
                idx += consumed;
            }
            _ => {}
        }
        idx += 1;
    }
}

fn parse_extended_sgr_color(codes: &[u16]) -> (usize, Option<Color>) {
    if codes.is_empty() {
        return (0, None);
    }
    match codes[0] {
        5 => {
            if codes.len() >= 2 {
                return (2, Some(Color::Indexed(codes[1] as u8)));
            }
            (1, None)
        }
        2 => {
            if codes.len() >= 4 {
                let r = codes[1].min(255) as u8;
                let g = codes[2].min(255) as u8;
                let b = codes[3].min(255) as u8;
                return (4, Some(Color::Rgb(r, g, b)));
            }
            (1, None)
        }
        _ => (1, None),
    }
}

fn sgr_4bit_color(value: u16, bright: bool) -> Color {
    match (value, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::Red,
        (2, false) => Color::Green,
        (3, false) => Color::Yellow,
        (4, false) => Color::Blue,
        (5, false) => Color::Magenta,
        (6, false) => Color::Cyan,
        (7, false) => Color::Gray,
        (0, true) => Color::DarkGray,
        (1, true) => Color::LightRed,
        (2, true) => Color::LightGreen,
        (3, true) => Color::LightYellow,
        (4, true) => Color::LightBlue,
        (5, true) => Color::LightMagenta,
        (6, true) => Color::LightCyan,
        (7, true) => Color::White,
        _ => Color::Reset,
    }
}

fn compact_home(path: &str) -> String {
    let Some(home) = HOME_DIR.as_deref() else {
        return path.to_string();
    };
    if path == home {
        return "~".to_string();
    }
    if let Some(rest) = path.strip_prefix(home)
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return format!("~{rest}");
    }
    path.to_string()
}

fn compact_home_in_text(text: &str) -> String {
    let Some(home) = HOME_DIR.as_deref() else {
        return text.to_string();
    };
    text.replace(home, "~")
}
