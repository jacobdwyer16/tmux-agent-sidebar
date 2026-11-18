<h1 align="center">tmux-agent-sidebar</h1>

This fork preserves the native Windows psmux/WezTerm port and customized agent
tree, including Codex monitoring and local Pi and Antigravity CLI integrations.
See [Native Windows with psmux](#native-windows-with-psmux) for setup and controls.

<p align="center">One tmux sidebar that tracks every Claude Code, Codex, and OpenCode pane across every session and window. See status, background shells, prompts, Git state, activity, and worktrees without switching windows.</p>

<p align="center"><img src="website/src/assets/captures/hero.png" alt="tmux-agent-sidebar hero" /></p>

<p align="center">
  <a href="https://hiroppy.github.io/tmux-agent-sidebar/">Documentation</a> ·
  <a href="https://hiroppy.github.io/tmux-agent-sidebar/getting-started/installation/">Getting Started</a> ·
  <a href="https://hiroppy.github.io/tmux-agent-sidebar/features/agent-pane/">Features</a>
</p>

## Features

- **Every pane, one view** 
  — tracks Claude Code, Codex, and OpenCode panes across all tmux sessions and windows
- **Live metadata** 
  — prompts, tool calls, response previews, background shell state, wait reasons, task progress, and subagent trees refresh as the agents work
- **Worktrees, included** 
  — spawn a fresh worktree + agent from the sidebar and tear it down — window, worktree, and branch — in one keystroke
- **Desktop notifications** 
  — native alerts when an agent finishes, needs permission, or errors out

OpenCode uses a small local plugin bridge instead of per-event hook config. The plugin lives at `.opencode/plugins/tmux-agent-sidebar.js` and can be symlinked as a single file into `~/.config/opencode/plugins/` so it coexists with any existing plugins.

## Requirements

- tmux 3.0+
- [TPM](https://github.com/tmux-plugins/tpm) (or the manual install in [Installation](https://hiroppy.github.io/tmux-agent-sidebar/getting-started/installation/))
- [GitHub CLI](https://cli.github.com/) (optional — required only for PR numbers in the Git tab)

## Quick Start

### 1. Install the plugin

Using [TPM](https://github.com/tmux-plugins/tpm):

```tmux
set -g @plugin 'hiroppy/tmux-agent-sidebar'
```

Reload tmux (`tmux source ~/.tmux.conf`), then press `prefix + I`. The install wizard downloads a pre-built binary or builds from source.

### 2. Wire up the agent hooks

- **Claude Code** — register the plugin inside Claude Code:

  ```sh
  /plugin marketplace add ~/.tmux/plugins/tmux-agent-sidebar
  /plugin install tmux-agent-sidebar@hiroppy
  ```

- **Codex** — open a Codex pane, press `prefix + e`, click the yellow `ⓘ` badge, copy the setup snippet, paste it into the Codex pane.
- **OpenCode** — symlink just the plugin file so your existing `~/.config/opencode/plugins/` contents stay untouched:

  ```sh
  mkdir -p ~/.config/opencode/plugins
  ln -sf ~/.tmux/plugins/tmux-agent-sidebar/.opencode/plugins/tmux-agent-sidebar.js \
    ~/.config/opencode/plugins/tmux-agent-sidebar.js
  ```

Full walkthroughs: [Claude Code setup](https://hiroppy.github.io/tmux-agent-sidebar/getting-started/claude-code/) · [Codex setup](https://hiroppy.github.io/tmux-agent-sidebar/getting-started/codex/) · [OpenCode setup](https://hiroppy.github.io/tmux-agent-sidebar/getting-started/opencode/)

### 3. Toggle the sidebar

`prefix + e` toggles the sidebar in the current window, `prefix + E` toggles it everywhere.

## Documentation

The [documentation site](https://hiroppy.github.io/tmux-agent-sidebar/) covers every feature and option:

- [Agent pane breakdown](https://hiroppy.github.io/tmux-agent-sidebar/features/agent-pane/)
- [Worktree lifecycle](https://hiroppy.github.io/tmux-agent-sidebar/features/worktree/)
- [Activity log](https://hiroppy.github.io/tmux-agent-sidebar/features/activity-log/) · [Git tab](https://hiroppy.github.io/tmux-agent-sidebar/features/git-status/) · [Notifications](https://hiroppy.github.io/tmux-agent-sidebar/features/notifications/)
- [Agent support matrix](https://hiroppy.github.io/tmux-agent-sidebar/agents/)
- [Keybindings](https://hiroppy.github.io/tmux-agent-sidebar/reference/keybindings/) · [tmux options](https://hiroppy.github.io/tmux-agent-sidebar/reference/tmux-options/) · [Scripting](https://hiroppy.github.io/tmux-agent-sidebar/reference/scripting/)

## Development

Symlink the plugin directory to your working copy so builds are picked up without copying:

```sh
rm -rf ~/.tmux/plugins/tmux-agent-sidebar
ln -s <path-to-this-repo> ~/.tmux/plugins/tmux-agent-sidebar
cargo build --release
```

Toggle the sidebar off → on to pick up the new binary.

### Picking up local builds for the Claude Code plugin

If you also installed this as a Claude Code plugin (`/plugin`), its install path
holds a copy of the released binary that hooks resolve before falling back to
`target/release/`. To make local builds flow through Claude Code hooks too,
replace that copy with a symlink to your working copy:

```sh
# Replace the cached plugin install with a symlink to your repo
PLUGIN_CACHE=~/.claude/plugins/cache/<owner>/tmux-agent-sidebar/<version>
rm -rf "$PLUGIN_CACHE"
ln -s <path-to-this-repo> "$PLUGIN_CACHE"
```

Also remove the stale release binary at `bin/tmux-agent-sidebar` in your repo
if present — both the tmux launcher and `hook.sh` prefer `bin/` over
`target/release/`, so a leftover binary there will mask `cargo build --release`
output:

```sh
rm -f bin/tmux-agent-sidebar
```

Note: Claude Code's plugin updater may overwrite the symlink on a future
update; re-run the symlink step if that happens.

## Native Windows with psmux

This local checkout includes a Windows port for the psmux and WezTerm setup.
The installed executable is `bin/tmux-agent-sidebar.exe`.

- `prefix + e` toggles the sidebar in the current window.
- The sidebar opens on the left at full window height. Set `@sidebar_position`
  to `right` for right placement; existing terminal splits retain their order.
- Windows close requests stop the sidebar's refresh loop before its pane exits,
  preventing transient deleted-pane errors when closing a focused sidebar.
- `prefix + E` toggles sidebars across windows.
- Agent rows show the tmux session name in brackets beside the model. Project
  groups stay pinned to the agent session's starting project while temporary
  scratchpad visits continue to update its live working-directory metadata.
- Navigation redraws immediately while monitoring and tmux commands run on
  background workers. Windows path aliases share one project group.
- The bottom Git/Activity panel is hidden in the local Windows configuration.
- Inside the sidebar, `j`/`k` select agents, `l` or Enter opens the selected agent,
  without adding a sidebar or changing its window layout. `h` returns to the
  previously focused terminal. `r` opens the repo picker;
  Tab cycles status filters. Mouse targets follow the evenly spaced status icons.
- Up/Down also visit each agent in order. Windows key-release events are ignored
  so one tap moves one row. Existing prefix + arrow pane bindings still work.
- Models appear in muted red and stable session names in pastel yellow. Permission
  mode badges are hidden. Each header icon has a space before its count.
- TCP port listings are hidden from agent rows.
- Codex status follows the exact bound session transcript and current terminal,
  including running, completion, errors, approvals, and active child agents.
- `install-agent-integrations.ps1` installs the local Pi extension and Google's
  Antigravity CLI status-line reporter, preserving existing settings with backups.
  Restart an already-running Pi or Antigravity terminal to load its integration.
- `windows-sidebar.ps1 toggle -Window 'session:@1'` targets a specific window.
- Restart or resume existing Claude Code and Codex sessions to load the new hooks.
- Claude and Codex hook configurations retain their existing entries. The installer
  creates timestamped `.sidebar-backup-*` copies before editing them.

Windows uses session-qualified stable window and pane IDs, locked JSON metadata
under `%TEMP%/tmux-agent-sidebar-psmux`, Windows process discovery, `netstat`, and
native toast notifications. WezTerm needs no configuration changes.

Rebuild from this directory with `cargo build --release`, close the sidebar, and
copy `target/release/tmux-agent-sidebar.exe` to `bin/tmux-agent-sidebar.exe`.
Run `install-windows.ps1` to reinstall the hooks and psmux bindings.

Run tests through `powershell.exe -NoProfile -File ./test-windows.ps1`. This disables
real multiplexer calls in the integration-test process. Unit builds also disable
those calls. Running the upstream tests without isolation can alter live sessions.

Validated on this machine: release build, 1,295 passing Rust tests, two ignored
upstream live-tmux tests, formatting, session discovery, sidebar toggle through the
actual WezTerm prefix key sequence (open, close, reopen), sequential agent navigation,
Pi and Antigravity startup with live model/status reporting, prompt/activity/completion hooks, scratch window creation,
and Windows toast acceptance. Visual toast delivery depends on Windows notification
settings. Worktree marker handling has regression coverage; a complete agent/worktree
creation and deletion cycle has not been exercised against live repositories.

Clippy completes with warnings. The machine's global Cargo configuration denies
indexing, unwrap, and panic patterns already present upstream; validation used
`cargo clippy --offline --all-targets -- -A clippy::unwrap_used -A clippy::panic -A clippy::indexing_slicing`.
Ruff, ty, and pytest do not apply to these Rust and PowerShell changes.

## License

[MIT](./LICENSE)
