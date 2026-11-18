use std::io::Write;

use serde_json::json;
use tempfile::NamedTempFile;

use super::*;

fn pane(id: &str) -> PaneInfo {
    PaneInfo {
        pane_id: id.into(),
        pane_active: false,
        status: PaneStatus::Unknown,
        attention: false,
        agent: AgentType::Codex,
        path: String::new(),
        current_command: String::new(),
        prompt: String::new(),
        prompt_is_response: false,
        started_at: None,
        wait_reason: String::new(),
        permission_mode: tmux::PermissionMode::Default,
        subagents: Vec::new(),
        pane_pid: None,
        worktree: tmux::WorktreeMetadata::default(),
        session_id: None,
        session_name: String::new(),
        tmux_session_name: String::new(),
        model: String::new(),
        sidebar_spawned: false,
        bg_shell_cmd: None,
    }
}

#[test]
fn a_failed_pane_does_not_prevent_other_codex_panes_from_updating() {
    let _guard = tmux::test_mock::install();
    let mut sessions = vec![SessionInfo {
        session_name: "test".into(),
        windows: vec![tmux::WindowInfo {
            window_id: "@1".into(),
            window_name: "test".into(),
            window_active: true,
            auto_rename: false,
            panes: vec![pane("%BROKEN"), pane("%LIVE")],
        }],
    }];
    let errors = refresh(&mut CodexMonitor::default(), &mut sessions, |id| {
        if id == "%BROKEN" {
            Err("missing pane".into())
        } else {
            Ok("Working (20s • esc to interrupt)\n› Ask Codex to do anything".into())
        }
    });
    assert_eq!(errors, vec!["missing pane"]);
    assert_eq!(sessions[0].windows[0].panes[0].status, PaneStatus::Error);
    assert_eq!(sessions[0].windows[0].panes[1].status, PaneStatus::Running);
}

#[test]
fn notification_identity_is_shared_across_sidebar_instances_and_changes_per_turn() {
    let mut pane = pane("%ROOT");
    pane.session_id = Some("root".into());
    pane.status = PaneStatus::Idle;
    let path = PathBuf::from("root.jsonl");
    let mut first = CodexMonitor::default();
    let mut second = CodexMonitor::default();
    for monitor in [&mut first, &mut second] {
        monitor.paths.insert("root".into(), path.clone());
        monitor
            .cursors
            .entry(path.clone())
            .or_default()
            .state
            .turn_id = "turn-1".into();
    }
    assert_eq!(
        transition_fingerprint(&first, &pane),
        transition_fingerprint(&second, &pane)
    );
    second.cursors.get_mut(&path).unwrap().state.turn_id = "turn-2".into();
    assert_ne!(
        transition_fingerprint(&first, &pane),
        transition_fingerprint(&second, &pane)
    );
}

#[test]
fn resumed_parent_finds_subagents_created_on_a_later_date() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("sessions/2026/11/16");
    let today = root.path().join("sessions/2026/11/17");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&today).unwrap();
    let parent = old.join("root.jsonl");
    fs::write(
        &parent,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"root\"}}\n",
    )
    .unwrap();
    let metadata = json!({"type":"session_meta","payload":{"id":"child","source":{"subagent":{"thread_spawn":{"parent_thread_id":"root","agent_nickname":"scout"}}}}});
    let start = json!({"type":"event_msg","payload":{"type":"task_started","started_at":100}});
    fs::write(today.join("child.jsonl"), format!("{metadata}\n{start}\n")).unwrap();
    assert_eq!(
        active_children(&mut CodexMonitor::default(), "root", &parent).unwrap(),
        vec!["scout (working)"]
    );
}

#[test]
fn lifecycle_distinguishes_running_completed_and_interrupted_turns() {
    let mut state = TranscriptState::default();
    apply_record(
        &mut state,
        &json!({"type":"event_msg","payload":{"type":"task_started","started_at":100}}),
    )
    .unwrap();
    assert_eq!(state.status, Some(PaneStatus::Running));
    assert_eq!(state.started_at, Some(100));
    apply_record(&mut state, &json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"still working"}]}})).unwrap();
    assert_eq!(state.status, Some(PaneStatus::Running));
    apply_record(&mut state, &json!({"type":"event_msg","payload":{"type":"task_complete","last_agent_message":"finished"}})).unwrap();
    assert_eq!(state.status, Some(PaneStatus::Idle));
    assert_eq!(state.response, "finished");
    apply_record(&mut state, &json!({"type":"event_msg","timestamp":"2026-11-17T18:00:00Z","payload":{"type":"task_started"}})).unwrap();
    assert_eq!(state.status, Some(PaneStatus::Running));
    apply_record(
        &mut state,
        &json!({"type":"event_msg","payload":{"type":"turn_aborted","reason":"interrupted"}}),
    )
    .unwrap();
    assert_eq!(state.status, Some(PaneStatus::Idle));
}

#[test]
fn synchronous_questions_wait_until_the_matching_reply() {
    let mut state = TranscriptState {
        status: Some(PaneStatus::Running),
        ..TranscriptState::default()
    };
    apply_record(&mut state, &json!({"type":"response_item","payload":{"type":"function_call","name":"request_user_input","call_id":"question-1"}})).unwrap();
    assert_eq!(state.status, Some(PaneStatus::Waiting));
    apply_record(&mut state, &json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"other-tool"}})).unwrap();
    assert_eq!(state.status, Some(PaneStatus::Waiting));
    apply_record(&mut state, &json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"question-1"}})).unwrap();
    assert_eq!(state.status, Some(PaneStatus::Running));
    apply_record(&mut state, &json!({"type":"response_item","payload":{"type":"function_call","name":"request_user_input_async","call_id":"optional-1"}})).unwrap();
    assert_eq!(state.status, Some(PaneStatus::Running));
}

#[test]
fn reader_waits_for_complete_jsonl_records_and_resumes_at_offset() {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"root\"}}\n{\"type\":\"event_msg\",\"payload\":{").unwrap();
    file.flush().unwrap();
    let mut cursor = Cursor::default();
    update_cursor(&mut cursor, file.path()).unwrap();
    assert_eq!(cursor.state.id, "root");
    assert_eq!(cursor.state.status, None);
    file.write_all(b"\"type\":\"task_started\",\"started_at\":100}}\n")
        .unwrap();
    file.flush().unwrap();
    update_cursor(&mut cursor, file.path()).unwrap();
    assert_eq!(cursor.state.status, Some(PaneStatus::Running));
    let offset = cursor.offset;
    update_cursor(&mut cursor, file.path()).unwrap();
    assert_eq!(cursor.offset, offset);
    assert!(cursor.pending.is_empty());
}

#[test]
fn screen_uses_current_working_footer_instead_of_old_prompt_text() {
    let screen = "press enter to confirm or esc to cancel\ncommand output\n• Working (17m 58s • esc to interrupt)\n› Ask Codex to do anything\nGPT-6";
    assert_eq!(
        screen_status(screen).map(|state| state.0),
        Some(PaneStatus::Running)
    );
    assert_eq!(
        screen_status(
            "Would you like to run this command?\n› Yes\npress enter to confirm or esc to cancel"
        )
        .map(|state| state.0),
        Some(PaneStatus::Waiting)
    );
    assert_eq!(
        screen_status("Reconnect failed — check the endpoint, then relaunch (10s)")
            .map(|state| state.0),
        Some(PaneStatus::Error)
    );
    assert_eq!(
        screen_status("Working (1m 12s)\n› Ask Codex to do anything").map(|state| state.0),
        Some(PaneStatus::Running)
    );
    assert_eq!(
        screen_status("1 background terminal running · /ps to view\n› Ask Codex to do anything")
            .map(|state| state.0),
        Some(PaneStatus::Background)
    );
}

#[test]
fn invalid_complete_records_are_reported() {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(b"not-json\n").unwrap();
    assert!(
        update_cursor(&mut Cursor::default(), file.path())
            .unwrap_err()
            .contains("invalid Codex transcript")
    );
}

#[test]
fn subagent_states_are_bound_to_parent_identity_not_working_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root.jsonl");
    fs::write(
        &root,
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"root\"}}\n",
    )
    .unwrap();
    for (file, parent, event) in [
        ("child", "root", "task_started"),
        ("other", "other-root", "task_started"),
        ("finished", "root", "task_complete"),
    ] {
        let records = [
            json!({"type":"session_meta","payload":{"id":file,"source":{"subagent":{"thread_spawn":{"parent_thread_id":parent,"agent_nickname":file}}}}}),
            json!({"type":"event_msg","payload":{"type":event,"started_at":100}}),
        ];
        fs::write(
            directory.path().join(format!("{file}.jsonl")),
            records
                .iter()
                .map(|record| format!("{record}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
    let children = active_children(&mut CodexMonitor::default(), "root", &root).unwrap();
    assert_eq!(children, vec!["child (working)"]);
}
