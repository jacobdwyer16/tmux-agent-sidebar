use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use crate::group::RepoGroup;
use crate::state::{ActivityState, AppState, PaneRuntimeMap};
use crate::tmux;

pub(super) struct Request {
    pub focus_generation: u64,
    pub global_revision: u64,
    pub focused_pane: Option<String>,
}

pub(super) struct Snapshot {
    generation: u64,
    pub options_revision: u64,
    now: u64,
    groups: Vec<RepoGroup>,
    panes: PaneRuntimeMap,
    activity: ActivityState,
    focused_pane: Option<String>,
    sidebar_focused: bool,
    pub window_active: bool,
    pub options: HashMap<String, String>,
    flash: Option<(String, Instant)>,
}

impl Snapshot {
    fn collect(state: &mut AppState, request: Request) -> Self {
        let window_active = state.refresh();
        Self {
            generation: request.focus_generation,
            options_revision: request.global_revision,
            now: state.now,
            groups: state.repo_groups.clone(),
            panes: state.pane_states.clone(),
            activity: state.activity.clone(),
            focused_pane: state.focus_state.focused_pane_id.clone(),
            sidebar_focused: state.focus_state.sidebar_focused,
            window_active,
            options: tmux::get_all_global_options(),
            flash: state.flash.take(),
        }
    }

    pub fn apply(mut self, state: &mut AppState) -> bool {
        let selected = state.selected_pane().map(|pane| pane.pane_id.clone());
        // polling owns telemetry; the input thread owns cursor, scroll and tabs.
        for (id, pane) in &mut self.panes.map {
            if let Some(current) = state.pane_state(id) {
                pane.tab_pref.clone_from(&current.tab_pref);
            }
        }
        self.panes.seen.clone_from(&state.pane_states.seen);
        state.pane_states = self.panes;
        state.repo_groups = self.groups;
        state.now = self.now;
        state.focus_state.sidebar_focused = self.sidebar_focused;
        // a poll issued before a focus action must not undo optimistic feedback.
        let focus_observed = self.generation == state.focus_state.activation_generation
            && !self.sidebar_focused
            && self.focused_pane.is_some();
        if focus_observed {
            state
                .focus_state
                .focused_pane_id
                .clone_from(&self.focused_pane);
        }
        if self.focused_pane == state.focus_state.focused_pane_id {
            state.activity.entries = self.activity.entries;
        }
        if let Some(flash) = self.flash {
            state.flash = Some(flash);
        }
        state.refresh_session_names();
        state.rebuild_row_targets();
        if let Some(selected) = selected
            && let Some(index) = state
                .layout
                .pane_row_targets
                .iter()
                .position(|row| row.pane_id == selected)
        {
            state.global.selected_pane_row = index;
        }
        state.auto_switch_tab();
        focus_observed
    }
}

pub(super) fn spawn(state: &mut AppState) -> (Sender<Request>, Receiver<Snapshot>) {
    let mut backend = AppState::new(state.tmux_pane.clone());
    backend.codex = std::mem::take(&mut state.codex);
    backend.metadata = std::mem::take(&mut state.metadata);
    backend.repo_groups = state.repo_groups.clone();
    backend.pane_states = state.pane_states.clone();
    backend.focus_state = state.focus_state.clone();
    backend.activity = state.activity.clone();
    backend.sessions = state.sessions.clone();
    backend.timers = state.timers.clone();
    backend.bottom_panel_height = state.bottom_panel_height;
    backend.bottom_tab = state.bottom_tab.clone();
    spawn_worker(backend, Snapshot::collect)
}

fn spawn_worker<F>(mut backend: AppState, mut collect: F) -> (Sender<Request>, Receiver<Snapshot>)
where
    F: FnMut(&mut AppState, Request) -> Snapshot + Send + 'static,
{
    let (tx, requests) = mpsc::channel::<Request>();
    let (results, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("sidebar-monitor".into())
        .spawn(move || {
            while let Ok(request) = requests.recv() {
                backend
                    .focus_state
                    .focused_pane_id
                    .clone_from(&request.focused_pane);
                let snapshot = collect(&mut backend, request);
                if results.send(snapshot).is_err() {
                    return;
                }
            }
        })
        .expect("cannot start sidebar monitoring worker");
    (tx, rx)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::state::{BottomTab, RowTarget};

    fn snapshot(state: &AppState, request: Request) -> Snapshot {
        Snapshot {
            generation: request.focus_generation,
            options_revision: request.global_revision,
            now: state.now,
            groups: state.repo_groups.clone(),
            panes: state.pane_states.clone(),
            activity: state.activity.clone(),
            focused_pane: state.focus_state.focused_pane_id.clone(),
            sidebar_focused: false,
            window_active: true,
            options: HashMap::new(),
            flash: None,
        }
    }

    #[test]
    fn keyboard_navigation_does_not_wait_for_a_blocked_monitor() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (tx, rx) = spawn_worker(AppState::new("%99".into()), move |state, request| {
            entered_tx.send(()).expect("test observer");
            release_rx
                .recv()
                .expect("test must release blocked collector");
            snapshot(state, request)
        });
        tx.send(Request {
            focus_generation: 0,
            global_revision: 0,
            focused_pane: None,
        })
        .expect("worker request");
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker entered collector");
        let mut ui = AppState::new("%99".into());
        ui.bottom_panel_height = 0;
        ui.layout.pane_row_targets = vec![
            RowTarget {
                pane_id: "%1".into(),
            },
            RowTarget {
                pane_id: "%2".into(),
            },
        ];
        let flag = AtomicBool::new(false);
        let start = Instant::now();
        for _ in 0..1000 {
            for code in [KeyCode::Char('j'), KeyCode::Char('k')] {
                super::super::input::handle_key_event(
                    KeyEvent::new(code, KeyModifiers::NONE),
                    &mut ui,
                    &flag,
                );
            }
        }
        assert_eq!(ui.global.selected_pane_row, 0);
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "input was blocked by polling"
        );
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release_tx.send(()).expect("release worker");
        rx.recv_timeout(Duration::from_secs(1))
            .expect("completed snapshot");
    }

    #[test]
    fn stale_snapshot_cannot_undo_new_focus_or_user_tab_preference() {
        let mut backend = AppState::new("%99".into());
        backend.focus_state.focused_pane_id = Some("%old".into());
        backend.pane_state_mut("%keep").tab_pref = Some(BottomTab::Activity);
        let update = snapshot(
            &backend,
            Request {
                focus_generation: 0,
                global_revision: 0,
                focused_pane: None,
            },
        );
        let mut ui = AppState::new("%99".into());
        ui.focus_state.focused_pane_id = Some("%new".into());
        ui.focus_state.request_activation("%new".into());
        ui.pane_state_mut("%keep").tab_pref = Some(BottomTab::GitStatus);
        update.apply(&mut ui);
        assert_eq!(ui.focus_state.focused_pane_id.as_deref(), Some("%new"));
        assert_eq!(
            ui.pane_state("%keep").expect("pane state").tab_pref,
            Some(BottomTab::GitStatus)
        );
    }

    #[test]
    fn returning_to_sidebar_between_polls_keeps_the_requested_activity_target() {
        let mut backend = AppState::new("%99".into());
        backend.focus_state.focused_pane_id = Some("%old".into());
        let (tx, rx) = spawn_worker(backend, |state, request| {
            let mut update = snapshot(state, request);
            update.sidebar_focused = true;
            update
        });
        tx.send(Request {
            focus_generation: 1,
            global_revision: 0,
            focused_pane: Some("%new".into()),
        })
        .expect("request");
        let update = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("updated target");
        assert_eq!(update.focused_pane.as_deref(), Some("%new"));
    }
}
