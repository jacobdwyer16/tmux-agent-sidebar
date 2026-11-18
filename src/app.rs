//! Main application orchestration: prime the [`AppState`], spawn background
//! workers, and run the crossterm event loop. Split out from `src/main.rs` so
//! the binary entry point only handles CLI arg parsing, signal wiring, and
//! TUI session setup.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

use crossterm::event::{self};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::SPINNER_PULSE;
use crate::state::BottomTab;

mod actions;
mod input;
mod monitor;
mod render;
mod setup;
mod workers;

/// Run the TUI event loop. Returns when the loop exits (currently only on
/// fatal I/O error, since the loop is `loop { ... }`).
///
/// `needs_refresh` is the process-wide SIGUSR1 flag owned by `main.rs` — the
/// signal handler must reference a static visible at signal-handler time,
/// so the static stays with the `extern "C"` handler in the binary crate and
/// we just borrow it here.
pub fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    tmux_pane: String,
    needs_refresh: &'static AtomicBool,
) -> io::Result<()> {
    let mut state = setup::init_state(tmux_pane);
    let mut window_inactive_count: u32 = 0;

    let workers = workers::spawn(&state);
    let workers::Workers {
        git_rx,
        git_target_tx,
        session_rx,
        version_rx,
        git_tab_active,
    } = workers;
    let (monitor_tx, monitor_rx) = monitor::spawn(&mut state);
    let (action_tx, action_rx) = actions::spawn();
    let mut refresh_in_flight = false;
    let mut completed_focus_generation = 0;
    let mut confirmed_focused_pane = state.focus_state.focused_pane_id.clone();
    let mut last_git_request = None;

    let mut last_refresh = std::time::Instant::now();
    let mut last_spinner = std::time::Instant::now();
    let refresh_interval = Duration::from_secs(1);
    let spinner_interval = Duration::from_millis(200);
    let mut needs_redraw = true;

    loop {
        #[cfg(windows)]
        if crate::tmux::get_pane_option_value(&state.tmux_pane, crate::tmux::SIDEBAR_CLOSE_REQUEST)
            == "1"
        {
            return Ok(());
        }
        if needs_redraw {
            render::render_frame(terminal, &mut state)?;
            needs_redraw = false;
        }

        let refresh_timeout = if refresh_in_flight {
            refresh_interval
        } else {
            refresh_interval.saturating_sub(last_refresh.elapsed())
        };
        let spinner_timeout = spinner_interval.saturating_sub(last_spinner.elapsed());
        let timeout = if needs_refresh.load(Ordering::Relaxed) {
            Duration::ZERO
        } else {
            refresh_timeout
                .min(spinner_timeout)
                .min(Duration::from_millis(16))
        };
        if event::poll(timeout)? {
            // bound input batches so held keys cannot starve rendering.
            for _ in 0..32 {
                let ev = event::read()?;
                if input::handle_event(ev, &mut state, &git_tab_active, terminal) {
                    needs_redraw = true;
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }

        if needs_redraw {
            render::render_frame(terminal, &mut state)?;
            needs_redraw = false;
        }
        state
            .global
            .flush_pending_cursor_save(Duration::from_millis(120));
        if let Some(request) = state.focus_state.pending_activation.take() {
            action_tx
                .send(actions::Action::Activate(request))
                .map_err(|_| io::Error::other("sidebar action worker stopped"))?;
        }
        for write in state.global.take_pending_writes() {
            action_tx
                .send(actions::Action::Persist(write))
                .map_err(|_| io::Error::other("sidebar action worker stopped"))?;
        }
        for completion in action_rx.try_iter() {
            match completion.action {
                actions::Action::Activate(request) => {
                    completed_focus_generation = request.generation;
                    match completion.result {
                        Ok(()) => confirmed_focused_pane = Some(request.pane_id),
                        Err(error) => {
                            if request.generation == state.focus_state.activation_generation {
                                state
                                    .focus_state
                                    .focused_pane_id
                                    .clone_from(&confirmed_focused_pane);
                            }
                            state.set_flash(error);
                        }
                    }
                }
                actions::Action::Persist(write) => match completion.result {
                    Ok(()) => state.global.acknowledge_write(&write),
                    Err(error) => {
                        state.global.retry_write(write);
                        state.set_flash(error);
                    }
                },
            }
            needs_redraw = true;
        }

        if last_spinner.elapsed() >= spinner_interval {
            state.spinner_frame = (state.spinner_frame + 1) % SPINNER_PULSE.len();
            if state.pet_enabled {
                let term_width = terminal.size().map(|s| s.width).unwrap_or(60);
                state.tick_pet(term_width);
            }
            last_spinner = std::time::Instant::now();
            needs_redraw = true;
        }

        let sigusr1 = needs_refresh.swap(false, Ordering::Relaxed);
        if !refresh_in_flight && (sigusr1 || last_refresh.elapsed() >= refresh_interval) {
            monitor_tx
                .send(monitor::Request {
                    focus_generation: completed_focus_generation,
                    global_revision: state.global.revision,
                    focused_pane: state.focus_state.focused_pane_id.clone(),
                })
                .map_err(|_| io::Error::other("sidebar monitoring worker stopped"))?;
            refresh_in_flight = true;
            last_refresh = std::time::Instant::now();
        }
        let snapshot = match monitor_rx.try_recv() {
            Ok(snapshot) => Some(snapshot),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                return Err(io::Error::other("sidebar monitoring worker stopped"));
            }
        };
        if let Some(snapshot) = snapshot {
            refresh_in_flight = false;
            needs_redraw = true;
            if snapshot.window_active {
                if window_inactive_count >= 2 && snapshot.options_revision == state.global.revision
                {
                    state.global.apply_all(&snapshot.options);
                }
                window_inactive_count = 0;
            } else {
                window_inactive_count = window_inactive_count.saturating_add(1);
            }
            if snapshot.apply(&mut state) {
                confirmed_focused_pane.clone_from(&state.focus_state.focused_pane_id);
            }
            git_tab_active.store(
                state.bottom_panel_height > 0 && state.bottom_tab == BottomTab::GitStatus,
                Ordering::Relaxed,
            );
        }

        let git_active = state.bottom_panel_height > 0 && state.bottom_tab == BottomTab::GitStatus;
        git_tab_active.store(git_active, Ordering::Relaxed);
        let git_request = state
            .focus_state
            .focused_pane_id
            .clone()
            .map(|id| (id, git_active));
        if git_request != last_git_request {
            if let Some((pane_id, _)) = &git_request {
                git_target_tx
                    .send(pane_id.clone())
                    .map_err(|_| io::Error::other("sidebar Git worker stopped"))?;
            }
            last_git_request = git_request;
        }
        if let Ok(update) = git_rx.try_recv()
            && state.focus_state.focused_pane_id.as_ref() == Some(&update.pane_id)
        {
            state.apply_git_data(update.data);
            needs_redraw = true;
        }

        if let Ok(names) = session_rx.try_recv() {
            state.sessions.names.extend(names);
            state.sessions.dirty = true;
            needs_redraw = true;
        }

        if let Ok(notice) = version_rx.try_recv() {
            state.version_notice = Some(notice);
            needs_redraw = true;
        }
    }
}
