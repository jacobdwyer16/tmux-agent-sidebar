use serde_json::Value;

use crate::desktop_notification::{
    self, DesktopNotificationEvent, DesktopNotificationKind, DesktopNotificationSettings,
};
use crate::time::{now_epoch_millis, now_epoch_secs};
use crate::tmux;

use super::{read_stdin_json, sanitize_tmux_value, tmux_pane};

fn report_status(input: &Value) -> Result<&'static str, String> {
    if input
        .get("tool_confirmation_pending")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Ok("waiting");
    }
    match input.get("agent_state").and_then(Value::as_str) {
        Some("working" | "running" | "tool_use" | "initializing" | "authenticating") => {
            Ok("running")
        }
        Some("waiting" | "reviewing") => Ok("waiting"),
        Some("idle") if input.get("task_count").and_then(Value::as_u64).unwrap_or(0) > 0 => {
            Ok("background")
        }
        Some("idle") => Ok("idle"),
        Some("error") => Ok("error"),
        Some("stopped") => Ok("stopped"),
        state => Err(format!("unsupported reported agent state {state:?}")),
    }
}

fn model_name(input: &Value) -> Option<&str> {
    match input.get("model")? {
        Value::String(model) => Some(model),
        Value::Object(model) => model
            .get("display_name")
            .or_else(|| model.get("id"))
            .and_then(Value::as_str),
        _ => None,
    }
}

fn update_report(pane: &str, agent: &str, input: &Value) -> Result<(), String> {
    let status = report_status(input)?;
    if status == "stopped" {
        for key in [
            tmux::PANE_AGENT,
            tmux::PANE_SESSION_ID,
            tmux::PANE_STATUS,
            tmux::PANE_NAME,
            "@pane_model",
            tmux::PANE_SUBAGENTS,
            tmux::PANE_STARTED_AT,
            tmux::PANE_WAIT_REASON,
            tmux::PANE_CWD,
            tmux::PANE_PROJECT_DIR,
        ] {
            tmux::unset_pane_option(pane, key);
        }
        return Ok(());
    }
    let old_status = tmux::get_pane_option_value(pane, tmux::PANE_STATUS);
    let id = input
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .or_else(|| input.get("conversation_id").and_then(Value::as_str))
        .unwrap_or_default();
    let previous_id = tmux::get_pane_option_value(pane, tmux::PANE_SESSION_ID);
    let new_session = !id.is_empty() && previous_id != id;
    if new_session {
        for key in [
            tmux::PANE_NAME,
            "@pane_model",
            tmux::PANE_PROMPT,
            tmux::PANE_STARTED_AT,
        ] {
            tmux::unset_pane_option(pane, key);
        }
    }
    tmux::set_pane_option(pane, tmux::PANE_AGENT, agent);
    if !id.is_empty() {
        tmux::set_pane_option(pane, tmux::PANE_SESSION_ID, id);
    }
    if new_session {
        tmux::unset_pane_option(pane, tmux::PANE_PROJECT_DIR);
    }
    if let Some(cwd) = input.get("cwd").and_then(Value::as_str)
        && !cwd.is_empty()
    {
        tmux::set_pane_option(pane, tmux::PANE_CWD, cwd);
        if tmux::get_pane_option_value(pane, tmux::PANE_PROJECT_DIR).is_empty() {
            tmux::set_pane_option(pane, tmux::PANE_PROJECT_DIR, cwd);
        }
    }
    if let Some(model) = model_name(input) {
        tmux::set_pane_option(pane, "@pane_model", &sanitize_tmux_value(model));
    }
    let name = input
        .get("agent")
        .and_then(|agent| agent.get("name"))
        .and_then(Value::as_str)
        .or_else(|| input.get("conversation_title").and_then(Value::as_str));
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        tmux::set_pane_option(pane, tmux::PANE_NAME, &sanitize_tmux_value(name));
    } else if new_session || tmux::get_pane_option_value(pane, tmux::PANE_NAME).is_empty() {
        tmux::set_pane_option(
            pane,
            tmux::PANE_NAME,
            &if id.is_empty() {
                agent.to_owned()
            } else {
                format!("{agent}-{}", id.chars().take(8).collect::<String>())
            },
        );
    }
    if let Some(prompt) = input.get("prompt").and_then(Value::as_str) {
        tmux::set_pane_option(pane, tmux::PANE_PROMPT, &sanitize_tmux_value(prompt));
    }
    if status == "running" && (new_session || old_status == "idle" || old_status.is_empty()) {
        tmux::set_pane_option(pane, tmux::PANE_STARTED_AT, &now_epoch_secs().to_string());
        tmux::set_pane_option(
            pane,
            tmux::PANE_NOTIFICATION_RUN_ID,
            &now_epoch_millis().to_string(),
        );
    }
    tmux::set_pane_option(pane, tmux::PANE_STATUS, status);
    let supplied_reason = input
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let reason = if supplied_reason.is_empty() {
        match status {
            "waiting" => "Input or permission required",
            "error" => "Agent error",
            "background" => "Background task running",
            _ => "",
        }
    } else {
        supplied_reason
    };
    tmux::set_pane_option(pane, tmux::PANE_WAIT_REASON, reason);
    if let Some(subagents) = input.get("subagents").and_then(Value::as_array) {
        let names = subagents
            .iter()
            .filter(|child| child.get("status").and_then(Value::as_str) == Some("running"))
            .filter_map(|child| child.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(",");
        tmux::set_pane_option(pane, tmux::PANE_SUBAGENTS, &names);
    }
    if !new_session && old_status != status {
        let notification = match status {
            "idle" if matches!(old_status.as_str(), "running" | "waiting" | "background") => {
                Some((
                    DesktopNotificationKind::TaskCompleted,
                    DesktopNotificationEvent::Stop,
                    "Agent finished",
                ))
            }
            "waiting" => Some((
                DesktopNotificationKind::PermissionRequired,
                DesktopNotificationEvent::Notification,
                "Agent needs input",
            )),
            "error" => Some((
                DesktopNotificationKind::TaskFailed,
                DesktopNotificationEvent::StopFailure,
                "Agent error",
            )),
            _ => None,
        };
        if let Some((kind, event, title)) = notification {
            let settings = DesktopNotificationSettings::from_tmux();
            let run = tmux::get_pane_option_value(pane, tmux::PANE_NOTIFICATION_RUN_ID);
            desktop_notification::notify_if_allowed(
                &settings,
                pane,
                kind,
                event,
                &format!("{agent}:{id}:{run}:{status}"),
                title,
                reason,
            );
        }
    }
    Ok(())
}

pub(super) fn cmd_report(args: &[String]) -> i32 {
    let Some(agent) = args.first().map(String::as_str) else {
        eprintln!("report requires pi or antigravity");
        return 2;
    };
    if !matches!(agent, "pi" | "antigravity") {
        eprintln!("unsupported reporting agent {agent}");
        return 2;
    }
    let pane = tmux_pane();
    if pane.is_empty() {
        return 0;
    }
    match update_report(&pane, agent, &read_stdin_json()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("agent report failed: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn official_antigravity_states_keep_confirmations_and_background_work_distinct() {
        assert_eq!(
            report_status(&json!({"agent_state":"tool_use","tool_confirmation_pending":true}))
                .unwrap(),
            "waiting"
        );
        assert_eq!(
            report_status(&json!({"agent_state":"idle","task_count":2})).unwrap(),
            "background"
        );
        assert_eq!(
            report_status(&json!({"agent_state":"working"})).unwrap(),
            "running"
        );
        assert_eq!(
            report_status(&json!({"agent_state":"idle"})).unwrap(),
            "idle"
        );
        assert_eq!(
            model_name(&json!({"model":{"id":"gemini-4","display_name":"Gemini 4 Argon"}})),
            Some("Gemini 4 Argon")
        );
    }

    #[test]
    fn omitted_agent_name_does_not_replace_the_stable_name() {
        let _guard = tmux::test_mock::install();
        update_report("%REPORT", "pi", &json!({"session_id":"pi-root","agent_state":"idle","model":"glm-5.3-flash","agent":{"name":"researcher"}})).unwrap();
        update_report(
            "%REPORT",
            "pi",
            &json!({"session_id":"pi-root","agent_state":"idle","model":"glm-5.3-flash"}),
        )
        .unwrap();
        assert_eq!(
            tmux::get_pane_option_value("%REPORT", tmux::PANE_NAME),
            "researcher"
        );
    }

    #[test]
    fn reports_pin_the_project_until_a_different_session_starts() {
        let _guard = tmux::test_mock::install();
        let pane = "%REPORT_PROJECT";
        for cwd in ["/repo", "/tmp/scratchpad"] {
            update_report(
                pane,
                "pi",
                &json!({
                    "session_id": "first", "agent_state": "working", "cwd": cwd
                }),
            )
            .expect("valid report");
            assert_eq!(
                tmux::get_pane_option_value(pane, tmux::PANE_PROJECT_DIR),
                "/repo"
            );
            assert_eq!(tmux::get_pane_option_value(pane, tmux::PANE_CWD), cwd);
        }
        update_report(
            pane,
            "pi",
            &json!({
                "session_id": "second", "agent_state": "idle", "cwd": "/other"
            }),
        )
        .expect("valid report");
        assert_eq!(
            tmux::get_pane_option_value(pane, tmux::PANE_PROJECT_DIR),
            "/other"
        );
        update_report(pane, "pi", &json!({"agent_state": "stopped"})).expect("valid stop report");
        assert!(tmux::get_pane_option_value(pane, tmux::PANE_PROJECT_DIR).is_empty());
    }
}
