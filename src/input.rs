use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Filter};

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.search_mode {
        match key.code {
            KeyCode::Esc => {
                app.clear_search();
                app.stop_search();
            }
            KeyCode::Enter => app.stop_search(),
            KeyCode::Backspace => app.pop_search_char(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.push_search_char(c)
            }
            _ => {}
        }
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('o') => {
                app.set_filter(Filter::Opencode);
                return Ok(());
            }
            KeyCode::Char('a') => {
                app.set_filter(Filter::Codex);
                return Ok(());
            }
            KeyCode::Char('c') => {
                app.set_filter(Filter::Claude);
                return Ok(());
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Char('q') => app.running = false,
        KeyCode::Char('/') => app.start_search(),
        KeyCode::Char('p') => app.toggle_preview(),
        KeyCode::Esc => app.hide_preview(),
        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.previous(),
        KeyCode::Char('f') => app.cycle_filter(),
        KeyCode::Char('s') => app.cycle_sort(),
        KeyCode::Char('E') => app.eject_selected()?,
        KeyCode::Enter => app.jump_selected()?,
        _ => {}
    }
    Ok(())
}
