use std::collections::HashMap;
use std::time::Instant;

use crate::tmux;

use super::filter::{RepoFilter, StatusFilter};

#[derive(Debug, Clone, PartialEq)]
pub enum GlobalWrite {
    Filter(StatusFilter),
    Cursor(usize),
    RepoFilter(RepoFilter),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlobalWriteRequest {
    pub revision: u64,
    pub write: GlobalWrite,
}

impl GlobalWrite {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Filter(_) => tmux::SIDEBAR_FILTER,
            Self::Cursor(_) => tmux::SIDEBAR_CURSOR,
            Self::RepoFilter(_) => tmux::SIDEBAR_REPO_FILTER,
        }
    }

    pub fn value(&self) -> String {
        match self {
            Self::Filter(value) => value.as_str().to_string(),
            Self::Cursor(value) => value.to_string(),
            Self::RepoFilter(value) => value.as_str().to_string(),
        }
    }
}

/// State shared across all sidebar instances via tmux global variables.
/// Synced from tmux at startup and on pane focus change (SIGUSR1).
pub struct GlobalState {
    pub status_filter: StatusFilter,
    pub selected_pane_row: usize,
    pub repo_filter: RepoFilter,
    /// Last filter value successfully written to tmux.
    last_saved_filter: StatusFilter,
    /// Last cursor value successfully written to tmux.
    last_saved_cursor: usize,
    /// Last repo filter value successfully written to tmux.
    last_saved_repo_filter: RepoFilter,
    /// When the selected cursor was last changed and still needs persisting.
    pending_cursor_save_since: Option<Instant>,
    pending_writes: Vec<GlobalWriteRequest>,
    outstanding_writes: Vec<GlobalWriteRequest>,
    pub(crate) revision: u64,
}

impl Default for GlobalState {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalState {
    pub fn new() -> Self {
        Self {
            status_filter: StatusFilter::All,
            selected_pane_row: 0,
            repo_filter: RepoFilter::All,
            last_saved_filter: StatusFilter::All,
            last_saved_cursor: 0,
            last_saved_repo_filter: RepoFilter::All,
            pending_cursor_save_since: None,
            pending_writes: Vec::new(),
            outstanding_writes: Vec::new(),
            revision: 0,
        }
    }

    /// Queue a filter write; the action worker acknowledges successful writes.
    pub fn save_filter(&mut self) {
        self.queue_write(GlobalWrite::Filter(self.status_filter));
    }

    /// Queue the cursor position without running a process on the input thread.
    pub fn save_cursor(&mut self) {
        self.queue_write(GlobalWrite::Cursor(self.selected_pane_row));
    }

    /// Mark the cursor as dirty so the main loop can persist it once the
    /// user pauses navigation.
    pub fn queue_cursor_save(&mut self) {
        self.pending_cursor_save_since = Some(Instant::now());
    }

    /// Persist a queued cursor update after it has been idle for at least the
    /// requested debounce duration. Returns true when the queue was consumed.
    pub fn flush_pending_cursor_save(&mut self, debounce: std::time::Duration) -> bool {
        let Some(queued_at) = self.pending_cursor_save_since else {
            return false;
        };
        if queued_at.elapsed() < debounce {
            return false;
        }
        self.save_cursor();
        self.pending_cursor_save_since = None;
        true
    }

    /// Save repo filter to tmux global variable.
    pub fn save_repo_filter(&mut self) {
        self.queue_write(GlobalWrite::RepoFilter(self.repo_filter.clone()));
    }

    fn queue_write(&mut self, write: GlobalWrite) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("global revision overflow");
        self.pending_writes
            .retain(|old| old.write.key() != write.key());
        self.outstanding_writes
            .retain(|old| old.write.key() != write.key());
        let request = GlobalWriteRequest {
            revision: self.revision,
            write,
        };
        self.pending_writes.push(request.clone());
        self.outstanding_writes.push(request);
    }

    pub(crate) fn take_pending_writes(&mut self) -> Vec<GlobalWriteRequest> {
        std::mem::take(&mut self.pending_writes)
    }

    pub(crate) fn acknowledge_write(&mut self, request: &GlobalWriteRequest) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("global revision overflow");
        self.outstanding_writes.retain(|pending| pending != request);
        match &request.write {
            GlobalWrite::Filter(value) => self.last_saved_filter = *value,
            GlobalWrite::Cursor(value) => self.last_saved_cursor = *value,
            GlobalWrite::RepoFilter(value) => self.last_saved_repo_filter = value.clone(),
        }
    }

    pub(crate) fn retry_write(&mut self, request: GlobalWriteRequest) {
        if self.outstanding_writes.contains(&request) {
            self.pending_writes
                .retain(|old| old.write.key() != request.write.key());
            self.pending_writes.push(request);
        }
    }

    fn has_pending(&self, key: &str) -> bool {
        self.outstanding_writes
            .iter()
            .any(|request| request.write.key() == key)
    }

    /// Load all global state from tmux variables.
    /// Called at startup and on SIGUSR1 (pane focus change).
    pub fn load_from_tmux(&mut self) {
        let opts = tmux::get_all_global_options();
        self.apply_all(&opts);
    }

    /// Apply all global options from tmux (filter, cursor, repo filter).
    pub fn apply_all(&mut self, opts: &HashMap<String, String>) {
        if let Some(filter_str) = opts.get(tmux::SIDEBAR_FILTER) {
            let tmux_filter = StatusFilter::from_label(filter_str);
            if tmux_filter != self.last_saved_filter && !self.has_pending(tmux::SIDEBAR_FILTER) {
                self.status_filter = tmux_filter;
                self.last_saved_filter = tmux_filter;
            }
        }
        if let Some(cursor_str) = opts.get(tmux::SIDEBAR_CURSOR)
            && let Ok(n) = cursor_str.parse::<usize>()
            && n != self.last_saved_cursor
            && self.pending_cursor_save_since.is_none()
            && !self.has_pending(tmux::SIDEBAR_CURSOR)
        {
            self.selected_pane_row = n;
            self.last_saved_cursor = n;
        }
        if let Some(repo_str) = opts.get(tmux::SIDEBAR_REPO_FILTER) {
            let tmux_repo = RepoFilter::from_label(repo_str);
            if tmux_repo != self.last_saved_repo_filter
                && !self.has_pending(tmux::SIDEBAR_REPO_FILTER)
            {
                self.repo_filter = tmux_repo.clone();
                self.last_saved_repo_filter = tmux_repo;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_filter_changes_coalesce_and_stale_acknowledgements_do_not_win() {
        let mut state = GlobalState::new();
        state.status_filter = StatusFilter::Running;
        state.save_filter();
        let old = state.take_pending_writes().pop().expect("queued write");
        state.status_filter = StatusFilter::All;
        state.save_filter();
        state.acknowledge_write(&old);
        state.apply_all(&HashMap::from([(
            tmux::SIDEBAR_FILTER.into(),
            "running".into(),
        )]));
        assert_eq!(state.status_filter, StatusFilter::All);
        let current = state.take_pending_writes();
        assert_eq!(current.len(), 1);
        assert_eq!(
            current.first().expect("latest write").write,
            GlobalWrite::Filter(StatusFilter::All)
        );
        state.retry_write(old);
        assert!(state.take_pending_writes().is_empty());
        let latest = current.first().expect("latest write").clone();
        state.retry_write(latest.clone());
        assert_eq!(state.take_pending_writes(), vec![latest.clone()]);
        state.acknowledge_write(&latest);
        assert!(state.outstanding_writes.is_empty());
    }

    #[test]
    fn cursor_is_local_until_debounce_then_only_latest_value_is_queued() {
        let mut state = GlobalState::new();
        for cursor in 0..100 {
            state.selected_pane_row = cursor;
            state.queue_cursor_save();
        }
        assert!(!state.flush_pending_cursor_save(std::time::Duration::from_secs(1)));
        assert!(state.take_pending_writes().is_empty());
        assert!(state.flush_pending_cursor_save(std::time::Duration::ZERO));
        let queued = state.take_pending_writes();
        assert_eq!(queued.len(), 1);
        let request = queued.first().expect("cursor write");
        assert_eq!(request.write, GlobalWrite::Cursor(99));
        assert_eq!(state.last_saved_cursor, 0);
        state.acknowledge_write(request);
        assert_eq!(state.last_saved_cursor, 99);
    }

    #[test]
    fn repeated_value_has_distinct_token_and_survives_an_old_acknowledgement() {
        let mut state = GlobalState::new();
        let mut writes = Vec::new();
        for filter in [
            StatusFilter::Running,
            StatusFilter::All,
            StatusFilter::Running,
        ] {
            state.status_filter = filter;
            state.save_filter();
            writes.push(state.take_pending_writes().pop().expect("write"));
        }
        let mut writes = writes.into_iter();
        let first = writes.next().expect("first A");
        let second = writes.next().expect("B");
        let last = writes.next().expect("latest A");
        assert_ne!(first.revision, last.revision);
        state.acknowledge_write(&first);
        state.acknowledge_write(&second);
        state.retry_write(last.clone());
        assert_eq!(state.take_pending_writes(), vec![last]);
        state.apply_all(&HashMap::from([(
            tmux::SIDEBAR_FILTER.into(),
            "all".into(),
        )]));
        assert_eq!(state.status_filter, StatusFilter::Running);
    }
}
