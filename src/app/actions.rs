use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};

use crate::state::{GlobalWriteRequest, PaneActivation};
use crate::tmux;

#[derive(Debug, Clone)]
pub(super) enum Action {
    Activate(PaneActivation),
    Persist(GlobalWriteRequest),
}

pub(super) struct Completion {
    pub action: Action,
    pub result: Result<(), String>,
}

pub(super) fn spawn(sidebar: String) -> (Sender<Action>, Receiver<Completion>) {
    let (tx, requests) = mpsc::channel();
    let (results, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("sidebar-actions".into())
        .spawn(move || run(&sidebar, requests, results))
        .expect("cannot start sidebar action worker");
    (tx, rx)
}

fn run(sidebar: &str, requests: Receiver<Action>, results: Sender<Completion>) {
    while let Ok(first) = requests.recv() {
        let mut activation = None;
        let mut writes = HashMap::new();
        for action in std::iter::once(first).chain(requests.try_iter()) {
            match action {
                Action::Activate(request) => activation = Some(request),
                Action::Persist(write) => {
                    writes.insert(write.write.key(), write);
                }
            }
        }
        let actions = activation
            .map(Action::Activate)
            .into_iter()
            .chain(writes.into_values().map(Action::Persist));
        for action in actions {
            let result = execute(sidebar, &action);
            if results.send(Completion { action, result }).is_err() {
                return;
            }
        }
    }
}

fn execute(sidebar: &str, action: &Action) -> Result<(), String> {
    match action {
        Action::Persist(request) => {
            let write = &request.write;
            tmux::run_tmux_capture(&["set", "-g", write.key(), &write.value()]).map(|_| ())
        }
        Action::Activate(request) => {
            tmux::run_tmux_capture(&["select-pane", "-t", &request.pane_id])?;
            if !request.ensure_sidebar {
                return Ok(());
            }
            let target_window = tmux::display_message(&request.pane_id, "#{window_id}");
            let source_window = tmux::display_message(sidebar, "#{window_id}");
            if target_window.is_empty() || source_window.is_empty() {
                return Err("cannot resolve window after focusing pane".into());
            }
            if target_window == source_window {
                return Ok(());
            }
            let directory = tmux::display_message(&request.pane_id, "#{pane_current_path}");
            if directory.is_empty() {
                return Err("cannot resolve selected pane directory".into());
            }
            let result =
                crate::cli::toggle::cmd_toggle(&["--create-only".into(), target_window, directory]);
            if result != 0 {
                return Err(format!("cannot create destination sidebar: exit {result}"));
            }
            Ok(())
        }
    }
}
