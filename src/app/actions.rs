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

pub(super) fn spawn() -> (Sender<Action>, Receiver<Completion>) {
    let (tx, requests) = mpsc::channel();
    let (results, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("sidebar-actions".into())
        .spawn(move || run(requests, results))
        .expect("cannot start sidebar action worker");
    (tx, rx)
}

fn run(requests: Receiver<Action>, results: Sender<Completion>) {
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
            let result = execute(&action);
            if results.send(Completion { action, result }).is_err() {
                return;
            }
        }
    }
}

fn execute(action: &Action) -> Result<(), String> {
    match action {
        Action::Persist(request) => {
            let write = &request.write;
            tmux::run_tmux_capture(&["set", "-g", write.key(), &write.value()]).map(|_| ())
        }
        Action::Activate(request) => tmux::select_pane(&request.pane_id),
    }
}
