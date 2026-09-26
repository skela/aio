use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Filter};

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.new_session_mode {
        match key.code {
            KeyCode::Esc => app.cancel_new_session_prompt(),
            KeyCode::Enter => app.confirm_new_session_field()?,
            KeyCode::Tab => app.tab_new_session_field(),
            KeyCode::BackTab | KeyCode::Up | KeyCode::Down => app.toggle_new_session_field(),
            KeyCode::Backspace => app.pop_new_session_char(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.push_new_session_char(c)
            }
            _ => {}
        }
        return Ok(());
    }

    if app.closed_visible {
        match key.code {
            KeyCode::Esc | KeyCode::Char('r') | KeyCode::Char('q') => app.hide_closed_sessions(),
            KeyCode::Char('j') | KeyCode::Down => app.closed_next(),
            KeyCode::Char('k') | KeyCode::Up => app.closed_previous(),
            KeyCode::Char('d') | KeyCode::Delete => app.forget_selected_closed(),
            KeyCode::Enter => app.reopen_selected_closed()?,
            _ => {}
        }
        return Ok(());
    }

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

    // Handle multi-key sequences (e.g. <space>gg to open lazygit).
    if let KeyCode::Char(c) = key.code {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            let seq = app.key_seq.clone();
            match (seq.as_slice(), c) {
                // First key: space starts the sequence.
                ([], ' ') => {
                    app.key_seq.push(' ');
                    return Ok(());
                }
                // Second key: <space>g — wait for the second g.
                ([' '], 'g') => {
                    app.key_seq.push('g');
                    return Ok(());
                }
                // Third key: <space>gg — fire lazygit.
                ([' ', 'g'], 'g') => {
                    app.key_seq.clear();
                    app.open_lazygit_selected()?;
                    return Ok(());
                }
                // Any other key after a partial sequence: clear and fall through.
                _ if !seq.is_empty() => {
                    app.key_seq.clear();
                    // Fall through so the key still fires its normal binding.
                }
                _ => {}
            }
        }
    } else if !app.key_seq.is_empty() {
        // Non-char key (e.g. Esc, Enter) cancels any pending sequence.
        app.key_seq.clear();
    }

    match key.code {
        KeyCode::Char('q') => app.running = false,
        KeyCode::Char('?') => app.toggle_help(),
        KeyCode::Char('/') => app.start_search(),
        KeyCode::Char('p') => app.toggle_preview(),
        KeyCode::Esc => {
            app.hide_preview();
            app.hide_help();
        }
        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.previous(),
        KeyCode::Char('f') => app.cycle_filter(),
        KeyCode::Char('s') => app.cycle_sort(),
        KeyCode::Char('t') => app.test_sound(true),
        KeyCode::Char('T') => app.test_sound(false),
        KeyCode::Char('E') => app.eject_selected()?,
        KeyCode::Char('c') | KeyCode::Char('n') => app.start_new_session_prompt(),
        KeyCode::Char('r') => app.toggle_closed_sessions(),
        KeyCode::Enter => app.jump_selected()?,
        _ => {}
    }
    Ok(())
}
