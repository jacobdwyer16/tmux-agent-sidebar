use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_NO_MORE_FILES, GetLastError, INVALID_HANDLE_VALUE},
    System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    },
};

use crate::tmux::AgentType;

pub(crate) const CODEX_HOST: &str = "codex-code-mode-host";

#[cfg(windows)]
fn windows_command_lines() -> HashMap<u32, String> {
    let output = Command::new("powershell.exe")
        .creation_flags(0x08000000)
        .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference = 'Stop'; [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); Get-CimInstance Win32_Process | Select-Object ProcessId,CommandLine | ConvertTo-Json -Compress"])
        .output().expect("cannot query Windows process command lines");
    assert!(
        output.status.success(),
        "Windows process command query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).expect("invalid Windows process command data");
    let commands: HashMap<u32, String> = rows
        .into_iter()
        .filter_map(|row| {
            let pid = row.get("ProcessId")?.as_u64()? as u32;
            let command = row.get("CommandLine")?.as_str()?;
            Some((pid, command.to_string()))
        })
        .collect();
    commands
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessInfo {
    pub(crate) comm: String,
    pub(crate) args: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessSnapshot {
    pub(crate) children_of: HashMap<u32, Vec<u32>>,
    pub(crate) info_by_pid: HashMap<u32, ProcessInfo>,
}

impl ProcessSnapshot {
    #[cfg(not(windows))]
    pub(crate) fn scan() -> Option<Self> {
        let output = Command::new("ps")
            .args(["-eo", "pid=,ppid=,comm=,args="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(Self::from_ps_output(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }

    #[cfg(windows)]
    pub(crate) fn scan() -> Option<Self> {
        let command_lines = windows_command_lines();
        let mut children_of: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut info_by_pid = HashMap::new();
        unsafe {
            let handle = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            assert_ne!(
                handle,
                INVALID_HANDLE_VALUE,
                "process snapshot failed: {}",
                GetLastError()
            );
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut found = Process32FirstW(handle, &mut entry);
            while found != 0 {
                let length = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let comm = String::from_utf16_lossy(&entry.szExeFile[..length]);
                children_of
                    .entry(entry.th32ParentProcessID)
                    .or_default()
                    .push(entry.th32ProcessID);
                info_by_pid.insert(
                    entry.th32ProcessID,
                    ProcessInfo {
                        args: command_lines
                            .get(&entry.th32ProcessID)
                            .cloned()
                            .unwrap_or_else(|| comm.clone()),
                        comm,
                    },
                );
                found = Process32NextW(handle, &mut entry);
            }
            let error = GetLastError();
            CloseHandle(handle);
            assert_eq!(error, ERROR_NO_MORE_FILES, "process enumeration failed");
        }
        Some(Self {
            children_of,
            info_by_pid,
        })
    }

    #[cfg(any(not(windows), test))]
    pub(crate) fn from_ps_output(ps_output: &str) -> Self {
        let mut children_of: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut info_by_pid: HashMap<u32, ProcessInfo> = HashMap::new();

        for line in ps_output.lines() {
            let mut parts = line.split_whitespace();
            let Some(pid_str) = parts.next() else {
                continue;
            };
            let Some(ppid_str) = parts.next() else {
                continue;
            };
            let Ok(pid) = pid_str.parse::<u32>() else {
                continue;
            };
            let Ok(ppid) = ppid_str.parse::<u32>() else {
                continue;
            };
            let Some(comm) = parts.next() else {
                continue;
            };

            children_of.entry(ppid).or_default().push(pid);
            info_by_pid.insert(
                pid,
                ProcessInfo {
                    comm: comm.to_string(),
                    args: parts.collect::<Vec<_>>().join(" "),
                },
            );
        }

        Self {
            children_of,
            info_by_pid,
        }
    }

    pub(crate) fn descendants(&self, seed_pids: &[u32]) -> HashSet<u32> {
        let mut seen = HashSet::new();
        let mut queue: VecDeque<u32> = seed_pids.iter().copied().collect();

        while let Some(pid) = queue.pop_front() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(children) = self.children_of.get(&pid) {
                for &child in children {
                    if !seen.contains(&child) {
                        queue.push_back(child);
                    }
                }
            }
        }

        seen
    }

    pub(crate) fn tree_has_agent(&self, seed_pids: &[u32], agent: &AgentType) -> bool {
        let agent_name = agent.as_str();
        self.descendants(seed_pids).into_iter().any(|pid| {
            self.info_by_pid
                .get(&pid)
                .map(|info| process_matches_agent(info, agent_name))
                .unwrap_or(false)
        })
    }

    pub(crate) fn tree_has_codex_host(&self, pid: u32) -> bool {
        self.descendants(&[pid]).into_iter().any(|pid| {
            self.info_by_pid
                .get(&pid)
                .is_some_and(|info| command_basename(&info.comm) == CODEX_HOST)
        })
    }

    pub(crate) fn agent_for_tree(&self, pid: u32) -> Option<AgentType> {
        let mut pending = VecDeque::from([pid]);
        let mut seen = HashSet::new();
        while let Some(pid) = pending.pop_front() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(info) = self.info_by_pid.get(&pid) {
                for agent in [
                    AgentType::Claude,
                    AgentType::Codex,
                    AgentType::Pi,
                    AgentType::Antigravity,
                    AgentType::OpenCode,
                ] {
                    if process_matches_agent(info, agent.as_str()) {
                        return Some(agent);
                    }
                }
            }
            if let Some(children) = self.children_of.get(&pid) {
                pending.extend(children);
            }
        }
        None
    }

    pub(crate) fn command_lines_for_tree(&self, seed_pids: &[u32]) -> Vec<String> {
        self.descendants(seed_pids)
            .into_iter()
            .filter_map(|pid| self.info_by_pid.get(&pid))
            .map(|info| {
                if info.args.is_empty() {
                    info.comm.clone()
                } else {
                    info.args.trim().to_string()
                }
            })
            .collect()
    }
}

pub(crate) fn command_basename(command: &str) -> &str {
    let name = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command);
    name.strip_suffix(".exe.old")
        .or_else(|| name.strip_suffix(".exe"))
        .unwrap_or(name)
}

pub(crate) fn process_matches_agent(info: &ProcessInfo, agent_name: &str) -> bool {
    let command = command_basename(&info.comm);
    if (agent_name == "antigravity" && command == "agy")
        || (agent_name == "pi"
            && command == "node"
            && info
                .args
                .replace('\\', "/")
                .contains("pi-coding-agent/dist/"))
    {
        return true;
    }
    if command_basename(&info.comm) == agent_name
        || (agent_name == "codex" && command_basename(&info.comm) == CODEX_HOST)
    {
        return true;
    }

    let Some(command) = info.args.split_whitespace().next() else {
        return false;
    };
    command_basename(command.trim_matches('"')) == agent_name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_and_antigravity_are_recognized_without_mistaking_nested_agents_for_the_parent() {
        let snapshot = ProcessSnapshot::from_ps_output(
            "100 1 powershell.exe powershell.exe\n101 100 node.exe node.exe C:/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js\n102 101 agy.exe agy.exe\n",
        );
        assert_eq!(snapshot.agent_for_tree(100), Some(AgentType::Pi));
        assert!(snapshot.tree_has_agent(&[102], &AgentType::Antigravity));
    }

    #[cfg(windows)]
    #[test]
    fn native_snapshot_contains_current_process_and_command_line() {
        let snapshot = ProcessSnapshot::scan().expect("Windows process snapshot missing");
        let process = snapshot
            .info_by_pid
            .get(&std::process::id())
            .expect("current process missing");
        assert!(process.args.contains("tmux_agent_sidebar"));
        assert!(process.args.len() > process.comm.len());
    }

    #[test]
    fn descendants_walks_process_tree() {
        let snapshot = ProcessSnapshot {
            children_of: HashMap::from([(1, vec![2, 3]), (2, vec![4]), (4, vec![5])]),
            info_by_pid: HashMap::new(),
        };
        let seen = snapshot.descendants(&[1]);
        assert!(seen.contains(&1));
        assert!(seen.contains(&2));
        assert!(seen.contains(&3));
        assert!(seen.contains(&4));
        assert!(seen.contains(&5));
    }

    #[test]
    fn parse_ps_processes_preserves_spaced_args() {
        let snapshot = ProcessSnapshot::from_ps_output(
            "100 1 codex /Applications/Codex App/bin/codex --full-auto\n101 100 sh sh -c wrapper\n",
        );

        assert_eq!(snapshot.children_of.get(&1).cloned(), Some(vec![100]));
        let info = snapshot.info_by_pid.get(&100).expect("process info");
        assert_eq!(info.comm, "codex");
        assert_eq!(info.args, "/Applications/Codex App/bin/codex --full-auto");
    }

    #[test]
    fn tree_has_agent_matches_descendant_process_name() {
        let snapshot = ProcessSnapshot::from_ps_output(
            "100 1 fish fish -c opencode\n101 100 opencode opencode\n",
        );

        assert!(snapshot.tree_has_agent(&[100], &AgentType::OpenCode));
        assert!(!snapshot.tree_has_agent(&[100], &AgentType::Codex));
    }

    #[test]
    fn process_matches_agent_requires_command_name_match() {
        assert!(process_matches_agent(
            &ProcessInfo {
                comm: "claude".to_string(),
                args: "/opt/homebrew/bin/claude --flag".to_string(),
            },
            "claude",
        ));
        assert!(process_matches_agent(
            &ProcessInfo {
                comm: "node".to_string(),
                args: "/usr/local/bin/opencode".to_string(),
            },
            "opencode",
        ));
        assert!(!process_matches_agent(
            &ProcessInfo {
                comm: "not-opencode".to_string(),
                args: "/usr/local/bin/not-opencode".to_string(),
            },
            "opencode",
        ));
    }
}
