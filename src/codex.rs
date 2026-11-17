use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::desktop_notification::{
    self, DesktopNotificationEvent, DesktopNotificationKind, DesktopNotificationSettings,
};
use crate::tmux::{self, AgentType, PaneInfo, PaneStatus, SessionInfo};

pub(crate) const TRANSCRIPT_OPTION: &str = "@pane_transcript_path";
pub(crate) const HEARTBEAT_OPTION: &str = "@pane_codex_monitor_heartbeat";

#[derive(Debug, Default)]
pub(crate) struct CodexMonitor {
    paths: BTreeMap<String, PathBuf>,
    cursors: BTreeMap<PathBuf, Cursor>,
    observed: BTreeMap<String, (Option<String>, PaneStatus)>,
    notifications: Option<DesktopNotificationSettings>,
    indexed: BTreeSet<PathBuf>,
    children: BTreeMap<PathBuf, (String, String)>,
    scans: BTreeMap<PathBuf, Instant>,
}

#[derive(Debug, Default)]
struct Cursor {
    offset: u64,
    pending: Vec<u8>,
    state: TranscriptState,
}

#[derive(Debug, Default)]
struct TranscriptState {
    id: String,
    turn_id: String,
    model: String,
    status: Option<PaneStatus>,
    started_at: Option<u64>,
    prompt: String,
    response: String,
    reason: String,
    pending_input: BTreeSet<String>,
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn message_text(payload: &Value) -> String {
    payload
        .get("content")
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn apply_record(state: &mut TranscriptState, record: &Value) -> Result<(), String> {
    let payload = &record["payload"];
    match record.get("type").and_then(Value::as_str) {
        Some("session_meta") => state.id = text(payload, "id"),
        Some("turn_context") => state.model = text(payload, "model"),
        Some("event_msg") => match payload.get("type").and_then(Value::as_str) {
            Some("task_started" | "turn_started") => {
                state.turn_id = text(payload, "turn_id");
                state.status = Some(PaneStatus::Running);
                state.reason.clear();
                state.pending_input.clear();
                state.started_at =
                    if let Some(started) = payload.get("started_at").and_then(Value::as_u64) {
                        Some(started)
                    } else {
                        let stamp = record
                            .get("timestamp")
                            .and_then(Value::as_str)
                            .ok_or("Codex turn has no timestamp")?;
                        Some(
                            OffsetDateTime::parse(stamp, &Rfc3339)
                                .map_err(|error| error.to_string())?
                                .unix_timestamp() as u64,
                        )
                    };
            }
            Some("task_complete" | "turn_complete") => {
                state.status = Some(PaneStatus::Idle);
                state.response = text(payload, "last_agent_message");
                state.reason.clear();
                state.pending_input.clear();
            }
            Some("turn_aborted") => {
                state.status = Some(PaneStatus::Idle);
                state.reason = text(payload, "reason");
                state.pending_input.clear();
            }
            Some("error") => {
                state.status = Some(PaneStatus::Error);
                state.reason = text(payload, "message");
            }
            Some(
                "request_user_input" | "exec_approval_request" | "apply_patch_approval_request",
            ) => {
                state.status = Some(PaneStatus::Waiting);
                state.reason = "Input or permission required".into();
            }
            Some("user_message") => state.prompt = text(payload, "message"),
            _ => {}
        },
        Some("response_item") => match payload.get("type").and_then(Value::as_str) {
            Some("message") if payload.get("role").and_then(Value::as_str) == Some("user") => {
                state.prompt = message_text(payload);
            }
            Some("message") if payload.get("role").and_then(Value::as_str) == Some("assistant") => {
                state.response = message_text(payload);
            }
            Some("function_call" | "custom_tool_call") => {
                let name = text(payload, "name");
                if name == "request_user_input" || name.ends_with(".request_user_input") {
                    state.pending_input.insert(text(payload, "call_id"));
                    state.status = Some(PaneStatus::Waiting);
                    state.reason = "Answer required".into();
                }
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                if state.pending_input.remove(&text(payload, "call_id"))
                    && state.pending_input.is_empty()
                {
                    state.status = Some(PaneStatus::Running);
                    state.reason.clear();
                }
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

fn update_cursor(cursor: &mut Cursor, path: &Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if file.metadata().map_err(|error| error.to_string())?.len() < cursor.offset {
        *cursor = Cursor::default();
    }
    file.seek(SeekFrom::Start(cursor.offset))
        .map_err(|error| error.to_string())?;
    let read = file
        .read_to_end(&mut cursor.pending)
        .map_err(|error| error.to_string())?;
    cursor.offset += read as u64;
    let Some(end) = cursor.pending.iter().rposition(|byte| *byte == b'\n') else {
        return Ok(());
    };
    for line in cursor.pending[..=end].split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let record: Value = serde_json::from_slice(line)
            .map_err(|error| format!("invalid Codex transcript {}: {error}", path.display()))?;
        apply_record(&mut cursor.state, &record)?;
    }
    cursor.pending.drain(..=end);
    Ok(())
}

fn find_transcript(id: &str) -> Result<PathBuf, String> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                    .expect("user home is required"),
            )
            .join(".codex")
        });
    let mut directories = vec![home.join("sessions")];
    let suffix = format!("{id}.jsonl");
    while let Some(directory) = directories.pop() {
        for entry in
            fs::read_dir(&directory).map_err(|error| format!("{}: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                directories.push(path);
            } else if entry.file_name().to_string_lossy().ends_with(&suffix) {
                return Ok(path);
            }
        }
    }
    Err(format!("Codex transcript not found for session {id}"))
}

fn screen_status(screen: &str) -> Option<(PaneStatus, String)> {
    let lines: Vec<String> = screen
        .lines()
        .map(|line| line.trim().to_ascii_lowercase())
        .filter(|line| !line.is_empty())
        .collect();
    let mut background = false;
    for line in lines.iter().rev().take(20) {
        if line.contains("press enter to confirm or esc to cancel")
            || line.contains("enter to submit answer")
            || line.contains("enter to submit all")
            || line == "allow command?"
        {
            return Some((PaneStatus::Waiting, "Input or permission required".into()));
        }
        if line
            .trim_start_matches(['•', '◦', ' '])
            .starts_with("reconnect failed")
        {
            return Some((PaneStatus::Error, "Connection failed".into()));
        }
        if line.contains(" to interrupt)") && line.contains("s ") {
            return Some((PaneStatus::Running, String::new()));
        }
        if let Some((label, timer)) = line.split_once(" (") {
            let timer = timer.split(')').next().unwrap_or_default();
            if (label.starts_with('•')
                || label.starts_with("working")
                || label.starts_with("thinking"))
                && timer.ends_with('s')
                && timer
                    .chars()
                    .all(|ch| ch.is_ascii_digit() || "hms ".contains(ch))
            {
                return Some((PaneStatus::Running, String::new()));
            }
        }
        background |= line.contains("background terminal running")
            || line.contains("background terminals running");
    }
    background.then_some((PaneStatus::Background, "Background terminal running".into()))
}

fn active_children(
    monitor: &mut CodexMonitor,
    parent: &str,
    path: &Path,
) -> Result<Vec<String>, String> {
    let directory = path
        .ancestors()
        .find(|directory| directory.file_name().is_some_and(|name| name == "sessions"))
        .or_else(|| path.parent())
        .ok_or("Codex transcript has no directory")?
        .to_path_buf();
    if monitor
        .scans
        .get(&directory)
        .is_none_or(|time| time.elapsed() >= Duration::from_secs(10))
    {
        let mut directories = vec![directory.clone()];
        while let Some(current) = directories.pop() {
            for entry in fs::read_dir(&current).map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                let path = entry.path();
                if entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    directories.push(path);
                    continue;
                }
                if path.extension().and_then(|value| value.to_str()) != Some("jsonl")
                    || monitor.indexed.contains(&path)
                {
                    continue;
                }
                let mut first = String::new();
                BufReader::new(File::open(&path).map_err(|error| error.to_string())?)
                    .read_line(&mut first)
                    .map_err(|error| error.to_string())?;
                if !first.ends_with('\n') {
                    continue;
                }
                let meta: Value =
                    serde_json::from_str(&first).map_err(|error| error.to_string())?;
                let spawn = &meta["payload"]["source"]["subagent"]["thread_spawn"];
                if let Some(parent_id) = spawn.get("parent_thread_id").and_then(Value::as_str) {
                    let name = spawn
                        .get("agent_nickname")
                        .and_then(Value::as_str)
                        .or_else(|| spawn.get("agent_path").and_then(Value::as_str))
                        .unwrap_or("subagent");
                    monitor
                        .children
                        .insert(path.clone(), (parent_id.to_string(), name.to_string()));
                }
                monitor.indexed.insert(path);
            }
        }
        monitor.scans.insert(directory, Instant::now());
    }
    let related: Vec<(PathBuf, String)> = monitor
        .children
        .iter()
        .filter(|(_, (parent_id, _))| parent_id == parent)
        .map(|(path, (_, name))| (path.clone(), name.clone()))
        .collect();
    let mut children = Vec::new();
    for (path, name) in related {
        let cursor = monitor.cursors.entry(path.clone()).or_default();
        update_cursor(cursor, &path)?;
        let label = match cursor.state.status {
            Some(PaneStatus::Running) => "working",
            Some(PaneStatus::Waiting) => "waiting",
            _ => continue,
        };
        children.push(format!("{name} ({label})"));
    }
    Ok(children)
}
fn refresh_pane(
    monitor: &mut CodexMonitor,
    pane: &mut PaneInfo,
    screen: &str,
) -> Result<(), String> {
    let visible = screen_status(screen);
    if let Some(id) = &pane.session_id {
        let configured = tmux::get_pane_option_value(&pane.pane_id, TRANSCRIPT_OPTION);
        let path = if !configured.is_empty() {
            PathBuf::from(configured)
        } else if let Some(path) = monitor.paths.get(id) {
            path.clone()
        } else {
            let path = find_transcript(id)?;
            monitor.paths.insert(id.clone(), path.clone());
            path
        };
        monitor.paths.insert(id.clone(), path.clone());
        let cursor = monitor.cursors.entry(path.clone()).or_default();
        update_cursor(cursor, &path)?;
        let state = &cursor.state;
        if state.id != *id {
            return Err(format!(
                "Codex transcript identity mismatch for {}",
                pane.pane_id
            ));
        }
        if let Some(status) = &state.status {
            pane.status = status.clone();
        }
        pane.started_at = if pane.status.is_active() {
            state.started_at
        } else {
            None
        };
        pane.prompt_is_response = pane.status == PaneStatus::Idle;
        pane.prompt = if pane.prompt_is_response {
            state.response.clone()
        } else {
            state.prompt.clone()
        };
        pane.wait_reason = state.reason.clone();
        pane.model = state.model.clone();
        pane.subagents = active_children(monitor, id, &path)?;
    }
    if let Some((status, reason)) = visible {
        pane.status = status;
        pane.wait_reason = reason;
    } else if pane.session_id.is_none()
        && screen
            .lines()
            .any(|line| line.trim_start().starts_with('›'))
    {
        pane.status = PaneStatus::Idle;
    }
    pane.attention = matches!(pane.status, PaneStatus::Waiting | PaneStatus::Error);
    Ok(())
}

pub(crate) fn refresh(
    monitor: &mut CodexMonitor,
    sessions: &mut [SessionInfo],
    mut capture: impl FnMut(&str) -> Result<String, String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for session in sessions {
        for window in &mut session.windows {
            for pane in &mut window.panes {
                if pane.agent == AgentType::Codex {
                    let result = capture(&pane.pane_id)
                        .and_then(|screen| refresh_pane(monitor, pane, &screen));
                    match result {
                        Ok(()) => {
                            tmux::set_pane_option(
                                &pane.pane_id,
                                HEARTBEAT_OPTION,
                                &crate::time::now_epoch_secs().to_string(),
                            );
                            notify_transition(monitor, pane);
                        }
                        Err(error) => {
                            pane.status = PaneStatus::Error;
                            pane.attention = true;
                            pane.wait_reason = error.clone();
                            errors.push(error);
                        }
                    }
                }
            }
        }
    }
    errors
}

fn notify_transition(monitor: &mut CodexMonitor, pane: &PaneInfo) {
    let previous = monitor.observed.insert(
        pane.pane_id.clone(),
        (pane.session_id.clone(), pane.status.clone()),
    );
    let Some((session, previous)) = previous else {
        return;
    };
    if session != pane.session_id || previous == pane.status {
        return;
    }
    let (kind, event, title) = match pane.status {
        PaneStatus::Idle
            if matches!(
                previous,
                PaneStatus::Running | PaneStatus::Waiting | PaneStatus::Background
            ) =>
        {
            (
                DesktopNotificationKind::TaskCompleted,
                DesktopNotificationEvent::Stop,
                "Codex finished",
            )
        }
        PaneStatus::Waiting => (
            DesktopNotificationKind::PermissionRequired,
            DesktopNotificationEvent::Notification,
            "Codex needs input",
        ),
        PaneStatus::Error => (
            DesktopNotificationKind::TaskFailed,
            DesktopNotificationEvent::StopFailure,
            "Codex error",
        ),
        _ => return,
    };
    let fingerprint = transition_fingerprint(monitor, pane);
    let settings = monitor
        .notifications
        .get_or_insert_with(DesktopNotificationSettings::from_tmux);
    let body: String = if pane.wait_reason.is_empty() {
        &pane.prompt
    } else {
        &pane.wait_reason
    }
    .chars()
    .take(200)
    .collect();
    desktop_notification::notify_if_allowed(
        settings,
        &pane.pane_id,
        kind,
        event,
        &fingerprint,
        title,
        &body,
    );
}

fn transition_fingerprint(monitor: &CodexMonitor, pane: &PaneInfo) -> String {
    let turn = pane
        .session_id
        .as_ref()
        .and_then(|id| monitor.paths.get(id))
        .and_then(|path| monitor.cursors.get(path))
        .map(|cursor| cursor.state.turn_id.as_str())
        .unwrap_or_default();
    format!(
        "codex:{}:{turn}:{:?}",
        pane.session_id.as_deref().unwrap_or(&pane.pane_id),
        pane.status
    )
}

#[cfg(test)]
mod tests;
