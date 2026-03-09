mod actions;
mod app;
mod detect;
mod input;
mod models;
mod tmux;
mod tui;

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::App;

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let tick_rate = Duration::from_millis(900);
    let preview_tick_rate = Duration::from_millis(150);
    let mut last_tick = Instant::now() - tick_rate;
    let mut last_preview_tick = Instant::now() - preview_tick_rate;
    let mut app = App::new();

    let mut run_result = Ok(());
    while app.running {
        if last_tick.elapsed() >= tick_rate {
            if let Err(err) = app.refresh() {
                app.status = format!("refresh error: {err}");
            }
            last_tick = Instant::now();
        }
        if app.preview_visible && last_preview_tick.elapsed() >= preview_tick_rate {
            app.refresh_preview_only();
            last_preview_tick = Instant::now();
        }

        terminal.draw(|f| tui::draw(f, &app))?;

        let mut timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if app.preview_visible {
            timeout = timeout.min(preview_tick_rate.saturating_sub(last_preview_tick.elapsed()));
        }
        if event::poll(timeout)? && let Event::Key(key) = event::read()? {
            if let Err(err) = input::handle_key(&mut app, key) {
                app.status = format!("action error: {err}");
                run_result = Err(err);
            }
            if app.take_terminal_reinit_request() {
                reinitialize_terminal(&mut terminal)?;
                last_tick = Instant::now() - tick_rate;
                last_preview_tick = Instant::now() - preview_tick_rate;
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    run_result
}

fn reinitialize_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    terminal.clear()?;
    Ok(())
}
