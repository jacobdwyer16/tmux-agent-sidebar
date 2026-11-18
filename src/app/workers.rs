use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::git::{self, GitData};
use crate::session;
use crate::state::{AppState, BottomTab};
use crate::tmux;
use crate::version::{self, UpdateNotice};

/// Channels and shared flags produced by [`spawn`] that the main event loop
/// drains every tick.
pub(super) struct Workers {
    pub git_rx: Receiver<GitUpdate>,
    pub git_target_tx: mpsc::Sender<String>,
    pub session_rx: Receiver<HashMap<String, String>>,
    pub version_rx: Receiver<UpdateNotice>,
    pub git_tab_active: Arc<AtomicBool>,
}

pub(super) struct GitUpdate {
    pub pane_id: String,
    pub data: GitData,
}

/// Spawn the background threads (git polling, session-name polling, version
/// notice fetch) that feed the event loop.
pub(super) fn spawn(state: &AppState) -> Workers {
    let (git_tx, git_rx) = mpsc::channel::<GitUpdate>();
    let (git_target_tx, git_target_rx) = mpsc::channel::<String>();
    let (session_tx, session_rx) = mpsc::channel::<HashMap<String, String>>();
    let (version_tx, version_rx) = mpsc::channel::<UpdateNotice>();
    let git_tab_active = Arc::new(AtomicBool::new(
        state.bottom_panel_height > 0 && state.bottom_tab == BottomTab::GitStatus,
    ));
    let git_tab_flag = Arc::clone(&git_tab_active);
    std::thread::spawn(move || {
        git_poll_loop(&git_tx, &git_tab_flag, git_target_rx);
    });
    std::thread::spawn(move || {
        session_poll_loop(&session_tx);
    });
    std::thread::spawn(move || {
        if let Some(notice) = version::fetch_update_notice() {
            let _ = version_tx.send(notice);
        }
    });

    Workers {
        git_rx,
        git_target_tx,
        session_rx,
        version_rx,
        git_tab_active,
    }
}

/// Session name polling thread. Scans `~/.claude/sessions/*.json` every 10
/// seconds so the main TUI thread never performs blocking filesystem I/O
/// to refresh `/rename`-assigned labels.
pub(super) fn session_poll_loop(tx: &mpsc::Sender<HashMap<String, String>>) {
    loop {
        std::thread::sleep(Duration::from_secs(10));
        let names = session::scan_session_names();
        if tx.send(names).is_err() {
            return;
        }
    }
}

/// Git data polling thread. Fetches git status every 2 seconds while the Git
/// tab is active. Skips fetching when the tab is not visible. PR numbers go
/// through an in-memory `(path, branch)`-keyed cache so `gh pr view` (the only
/// hop that costs GitHub API quota) runs at most once per `PR_CACHE_TTL`
/// instead of every tick.
pub(super) fn git_poll_loop(
    git_tx: &mpsc::Sender<GitUpdate>,
    active: &AtomicBool,
    targets: Receiver<String>,
) {
    let mut target = None;
    let mut pr_cache = git::PrCache::new();
    loop {
        match targets.recv_timeout(Duration::from_secs(2)) {
            Ok(pane) => target = Some(targets.try_iter().last().unwrap_or(pane)),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        if !active.load(Ordering::Relaxed) {
            continue;
        }

        if let Some(pane_id) = &target
            && let Some(path) = tmux::get_pane_path(pane_id)
        {
            let mut data = git::fetch_git_data(&path);
            data.pr_number = pr_cache.get_or_fetch(
                &path,
                &data.branch,
                std::time::Instant::now(),
                git::fetch_pr_number,
            );
            if git_tx
                .send(GitUpdate {
                    pane_id: pane_id.clone(),
                    data,
                })
                .is_err()
            {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_worker_stops_when_its_target_channel_closes() {
        let (targets, requests) = mpsc::channel();
        let (updates, results) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        drop(targets);
        let worker = std::thread::spawn(move || {
            git_poll_loop(&updates, &AtomicBool::new(true), requests);
            done.send(()).expect("worker completion");
        });
        completed
            .recv_timeout(Duration::from_secs(1))
            .expect("worker must stop when the UI closes");
        worker.join().expect("worker exited without panicking");
        assert!(matches!(
            results.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}
