#[cfg(all(windows, not(test)))]
use std::os::windows::process::CommandExt;
#[cfg(not(test))]
use std::process::Command;

#[cfg(not(test))]
fn tmux_command(args: &[&str]) -> Command {
    let mut command = Command::new(if cfg!(windows) { "psmux" } else { "tmux" });
    #[cfg(windows)]
    {
        command.creation_flags(0x08000000);
        command.env_remove("PSMUX_TARGET_SESSION");
        command.env_remove("PSMUX_TARGET_FULL");
    }
    for arg in args {
        #[cfg(windows)]
        if matches!(*arg, "-hfb" | "-hf") {
            for flag in arg[1..].chars() {
                command.arg(format!("-{flag}"));
            }
            continue;
        }
        #[cfg(windows)]
        let arg = super::windows::prepare_format(&super::windows::qualify_format(arg));
        command.arg(arg);
    }
    command
}

pub fn current_pane_target() -> String {
    let pane = std::env::var("TMUX_PANE").unwrap_or_default();
    if pane.is_empty() || !cfg!(windows) {
        return pane;
    }
    display_message(&pane, "#{pane_id}")
}

#[cfg(test)]
pub fn run_tmux(_args: &[&str]) -> Option<String> {
    None
}

#[cfg(not(test))]
pub fn run_tmux(args: &[&str]) -> Option<String> {
    if cfg!(debug_assertions) && std::env::var_os("TMUX_AGENT_SIDEBAR_TEST").is_some() {
        return None;
    }
    let output = tmux_command(args).output().ok()?;
    if output.status.success() {
        #[cfg(windows)]
        return Some(super::windows::expand_output(&String::from_utf8_lossy(
            &output.stdout,
        )));
        #[cfg(not(windows))]
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}

/// Run a tmux command, returning trimmed stdout on success and stderr on failure.
/// Used by the spawn/remove flow so the UI can surface a meaningful error message
/// instead of a silent fallthrough.
#[cfg(test)]
pub fn run_tmux_capture(_args: &[&str]) -> Result<String, String> {
    Err("multiplexer subprocesses are disabled in unit tests".into())
}

#[cfg(not(test))]
pub fn run_tmux_capture(args: &[&str]) -> Result<String, String> {
    if cfg!(debug_assertions) && std::env::var_os("TMUX_AGENT_SIDEBAR_TEST").is_some() {
        return Err("multiplexer subprocesses are disabled by the test runner".into());
    }
    let output = tmux_command(args)
        .output()
        .map_err(|e| format!("failed to spawn tmux: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("tmux exited with status {}", output.status)
        } else {
            stderr
        })
    }
}

pub fn display_message(target: &str, format: &str) -> String {
    run_tmux(&["display-message", "-t", target, "-p", format])
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Resolve the session name containing `pane_id`. Returns `None` when tmux
/// can't find the pane (e.g. it has just been closed).
pub fn pane_session_name(pane_id: &str) -> Option<String> {
    Some(display_message(pane_id, "#{session_name}")).filter(|s| !s.is_empty())
}

/// Create a new tmux window in `session` whose initial cwd is `cwd` and whose
/// title is `name`. Returns `(pane_id, window_id)` on success — the window id
/// is used by the spawn flow to set markers at window scope so split panes
/// (e.g. Claude Code subagents) inherit them.
pub fn new_window(session: &str, cwd: &str, name: &str) -> Result<(String, String), String> {
    let out = run_tmux_capture(&[
        "new-window",
        "-t",
        session,
        "-c",
        cwd,
        "-n",
        name,
        "-P",
        "-F",
        "#{pane_id} #{window_id}",
    ])?;
    let mut parts = out.split_whitespace();
    let pane = parts
        .next()
        .ok_or_else(|| "new-window returned no pane id".to_string())?
        .to_string();
    let window = parts
        .next()
        .ok_or_else(|| "new-window returned no window id".to_string())?
        .to_string();
    Ok((pane, window))
}

/// Set a user option at window scope. Needed so markers survive through
/// split panes that inherit from the window. Returns an error so the
/// spawn flow can roll back when a marker the remove path relies on
/// cannot be written — silently dropping the failure would leave an
/// un-removable pane.
pub fn set_window_option(window: &str, key: &str, value: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        super::windows::set(window, key, Some(value));
        return Ok(());
    }
    #[cfg(not(windows))]
    run_tmux_capture(&["set", "-w", "-t", window, key, value]).map(|_| ())
}

/// Send a command line to `target` (a pane id) and press Enter so the shell
/// executes it. Used to launch the agent binary right after window creation.
/// The text is sent with `-l` (literal) so nothing in `command` can collide
/// with tmux key names (e.g. `Tab`, `BSpace`); Enter is issued as a
/// separate invocation so it's interpreted as the Return key.
pub fn send_command(target: &str, command: &str) -> Result<(), String> {
    run_tmux_capture(&["send-keys", "-t", target, "-l", command])?;
    run_tmux_capture(&["send-keys", "-t", target, "Enter"]).map(|_| ())
}

/// Kill the tmux window identified by `window_id` (e.g. `@7`).
pub fn kill_window(window_id: &str) -> Result<(), String> {
    run_tmux_capture(&["kill-window", "-t", window_id]).map(|_| ())
}

pub fn select_pane(pane_id: &str) {
    // Find the session containing this pane and switch to it first
    let session_id = display_message(
        pane_id,
        if cfg!(windows) {
            "#{session_name}"
        } else {
            "#{session_id}"
        },
    );
    if !session_id.is_empty() {
        let _ = run_tmux(&["switch-client", "-t", &session_id]);
    }
    // Then switch to the correct window
    let window_id = display_message(pane_id, "#{window_id}");
    if !window_id.is_empty() {
        let _ = run_tmux(&["select-window", "-t", &window_id]);
    }
    let _ = run_tmux(&["select-pane", "-t", pane_id]);
}
