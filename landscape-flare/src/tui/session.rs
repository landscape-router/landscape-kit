use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use landscape_terrain_proto::cli::parse_forward;

use crate::client::ForwardCommand;

use super::{DashFocus, DashState};

pub(super) fn handle_key(dash: &mut DashState, key: KeyEvent) -> bool {
    if key.kind != KeyEventKind::Press {
        return false;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let editable = !ctrl && !key.modifiers.contains(KeyModifiers::ALT);

    if dash.forward_edit {
        match key.code {
            KeyCode::Char(c) if editable => dash.forward_input.push(c),
            KeyCode::Backspace => {
                dash.forward_input.pop();
            }
            KeyCode::Enter => match parse_forward(dash.forward_input.trim()) {
                Ok(forward) if dash.forwards.contains(&forward) => {
                    dash.forward_error = Some(crate::tr!("tui.duplicate_mapping"));
                }
                Ok(forward) if !dash.advertised_ports.contains(&forward.1) => {
                    dash.forward_error =
                        Some(crate::tr!("tui.port_not_advertised", port = forward.1));
                }
                Ok(forward) => {
                    let _ = dash.forward_tx.send(ForwardCommand::Add(forward));
                    dash.forwards.push(forward);
                    dash.forward_index = dash.forwards.len() - 1;
                    dash.forward_input.clear();
                    dash.forward_error = None;
                    dash.forward_edit = false;
                }
                Err(error) => {
                    dash.forward_error = Some(crate::tr!("tui.invalid_mapping", error = error));
                }
            },
            KeyCode::Esc => {
                dash.forward_input.clear();
                dash.forward_error = None;
                dash.forward_edit = false;
            }
            _ => {}
        }
        return false;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => true,
        KeyCode::Char('c') if ctrl => true,
        KeyCode::Char('l' | 'L') if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
            crate::i18n::toggle();
            false
        }
        KeyCode::Tab | KeyCode::BackTab => {
            dash.focus = match dash.focus {
                DashFocus::Forwards => DashFocus::Logs,
                DashFocus::Logs => DashFocus::Forwards,
            };
            false
        }
        KeyCode::Up | KeyCode::PageUp => {
            let step = if matches!(key.code, KeyCode::PageUp) {
                10
            } else {
                1
            };
            scroll(dash, step);
            false
        }
        KeyCode::Down | KeyCode::PageDown => {
            let step = if matches!(key.code, KeyCode::PageDown) {
                10
            } else {
                1
            };
            scroll(dash, -(step as i64));
            false
        }
        KeyCode::Home => {
            match dash.focus {
                DashFocus::Forwards => dash.forward_index = 0,
                DashFocus::Logs => dash.scroll = usize::MAX,
            }
            false
        }
        KeyCode::End => {
            match dash.focus {
                DashFocus::Forwards => dash.forward_index = dash.forwards.len().saturating_sub(1),
                DashFocus::Logs => dash.scroll = 0,
            }
            false
        }
        KeyCode::Char('a') if dash.focus == DashFocus::Forwards => {
            if dash.session_ready {
                dash.forward_input.clear();
                dash.forward_error = None;
                dash.forward_edit = true;
            } else {
                dash.forward_error = Some(crate::tr!("tui.handshake_required"));
            }
            false
        }
        KeyCode::Char('d') if dash.focus == DashFocus::Forwards => {
            if dash.session_ready && !dash.forwards.is_empty() {
                let index = dash.forward_index.min(dash.forwards.len() - 1);
                let forward = dash.forwards.remove(index);
                let _ = dash.forward_tx.send(ForwardCommand::Remove(forward));
                dash.forward_index = dash
                    .forward_index
                    .min(dash.forwards.len().saturating_sub(1));
                dash.forward_error = None;
            }
            false
        }
        _ => false,
    }
}

fn scroll(dash: &mut DashState, delta: i64) {
    match dash.focus {
        DashFocus::Logs => {
            dash.scroll = (dash.scroll as i64 + delta).max(0) as usize;
        }
        DashFocus::Forwards => {
            let max = dash.forwards.len().saturating_sub(1) as i64;
            dash.forward_index = (dash.forward_index as i64 + delta).clamp(0, max) as usize;
        }
    }
}
