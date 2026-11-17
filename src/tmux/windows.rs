use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};

const CONTEXT: &str = "||SIDEBAR_CONTEXT||";
const PREFIX: &str = "__SIDEBAR_OPTION_";

pub(super) fn qualify_format(format: &str) -> String {
    format
        .replace("#{q:pane_id}", "#{q:session_name}:PSMUX_WINDOW.PSMUX_PANE")
        .replace("#{q:window_id}", "#{q:session_name}:PSMUX_WINDOW")
        .replace("#{pane_id}", "#{session_name}:PSMUX_WINDOW.PSMUX_PANE")
        .replace("#{window_id}", "#{session_name}:PSMUX_WINDOW")
        .replace("PSMUX_PANE", "#{pane_id}")
        .replace("PSMUX_WINDOW", "#{window_id}")
}

fn local_option(field: &str) -> bool {
    field.starts_with("@pane_") || field == "@sidebar_pid" || field.starts_with("@agent-sidebar-")
}

pub(super) fn prepare_format(format: &str) -> String {
    if format.contains('\n') {
        return format
            .split('\n')
            .map(prepare_format)
            .collect::<Vec<_>>()
            .join("\n");
    }
    let mut output = String::new();
    let mut rest = format;
    let mut changed = false;
    while let Some(start) = rest.find("#{") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            output.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let field = &after[..end];
        let key = field.strip_prefix("q:").unwrap_or(field);
        if local_option(key) {
            output.push_str(&format!(
                "{PREFIX}{}{}__",
                if field.starts_with("q:") { "Q" } else { "R" },
                key
            ));
            changed = true;
        } else {
            output.push_str(&rest[start..start + end + 3]);
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    if changed {
        output.push_str(CONTEXT);
        output.push_str("#{session_name}:#{window_id}.#{pane_id}");
    }
    output
}

fn metadata_file(target: &str) -> File {
    assert!(!target.is_empty(), "psmux metadata target must be explicit");
    let directory = std::env::temp_dir().join("tmux-agent-sidebar-psmux");
    fs::create_dir_all(&directory).expect("cannot create psmux metadata directory");
    let name: String = target.bytes().map(|byte| format!("{byte:02x}")).collect();
    File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(format!("{name}.json")))
        .expect("cannot open psmux pane metadata")
}

fn read_locked(file: &mut File) -> BTreeMap<String, String> {
    let mut content = String::new();
    file.read_to_string(&mut content)
        .expect("cannot read psmux pane metadata");
    if content.is_empty() {
        BTreeMap::new()
    } else {
        serde_json::from_str(&content).expect("invalid psmux pane metadata")
    }
}

fn read(target: &str) -> BTreeMap<String, String> {
    let mut file = metadata_file(target);
    file.lock_shared().expect("cannot lock psmux pane metadata");
    read_locked(&mut file)
}

fn canonical_target(target: &str) -> String {
    if target.contains(':') {
        target.to_string()
    } else {
        super::commands::display_message(target, "#{pane_id}")
    }
}

pub(super) fn get(target: &str, key: &str) -> String {
    read(&canonical_target(target))
        .remove(key)
        .unwrap_or_default()
}

pub(super) fn set(target: &str, key: &str, value: Option<&str>) {
    let mut file = metadata_file(&canonical_target(target));
    file.lock().expect("cannot lock psmux pane metadata");
    let mut values = read_locked(&mut file);
    if let Some(value) = value {
        values.insert(key.to_string(), value.to_string());
    } else {
        values.remove(key);
    }
    let bytes = serde_json::to_vec(&values).expect("cannot encode psmux pane metadata");
    file.seek(SeekFrom::Start(0))
        .expect("cannot seek psmux pane metadata");
    file.write_all(&bytes)
        .expect("cannot write psmux pane metadata");
    file.set_len(bytes.len() as u64)
        .expect("cannot truncate psmux pane metadata");
}

pub(super) fn expand_output(output: &str) -> String {
    output
        .lines()
        .map(|line| {
            let Some((content, target)) = line.rsplit_once(CONTEXT) else {
                return line.to_string();
            };
            let mut values = read(target);
            if let Some((window, _)) = target.rsplit_once('.') {
                for (key, value) in read(window) {
                    if key.starts_with("@agent-sidebar-") {
                        values.insert(key, value);
                    }
                }
            }
            let mut rest = content;
            let mut result = String::new();
            while let Some(start) = rest.find(PREFIX) {
                result.push_str(&rest[..start]);
                let after = &rest[start + PREFIX.len()..];
                let (field, tail) = after.split_once("__").expect("invalid metadata marker");
                let value = values.get(&field[1..]).cloned().unwrap_or_default();
                if field.starts_with('Q') {
                    result.push_str(&value.replace('\\', "\\\\").replace('|', "\\|"));
                } else {
                    result.push_str(&value);
                }
                rest = tail;
            }
            result.push_str(rest);
            result
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_metadata_isolated_by_session_and_quoted_on_read() {
        let first = format!("sidebar-test-{}:0.%98701", std::process::id());
        let second = format!("sidebar-test-{}:0.%98702", std::process::id());
        set(&first, "@pane_prompt", Some("C:\\work|hello"));
        set(&second, "@pane_prompt", Some("other"));
        assert_eq!(get(&first, "@pane_prompt"), "C:\\work|hello");
        let output = format!("__SIDEBAR_OPTION_Q@pane_prompt__{CONTEXT}{first}\n");
        assert_eq!(expand_output(&output), "C:\\\\work\\|hello");
        assert_eq!(get(&second, "@pane_prompt"), "other");
        set(&first, "@pane_prompt", None);
        assert!(get(&first, "@pane_prompt").is_empty());
    }

    #[test]
    fn concurrent_hook_updates_preserve_other_fields() {
        let target = format!("sidebar-test-{}:0.%98703", std::process::id());
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let target = target.clone();
                std::thread::spawn(move || {
                    set(&target, &format!("@pane_test_{index}"), Some("value"))
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("metadata writer failed");
        }
        for index in 0..8 {
            assert_eq!(get(&target, &format!("@pane_test_{index}")), "value");
        }
    }

    #[test]
    fn formats_keep_global_options_and_replace_pane_options() {
        assert_eq!(prepare_format("#{@sidebar_width}"), "#{@sidebar_width}");
        assert_eq!(
            prepare_format("#{q:@pane_prompt}|#{pane_id}"),
            format!(
                "__SIDEBAR_OPTION_Q@pane_prompt__|#{{pane_id}}{CONTEXT}#{{session_name}}:#{{window_id}}.#{{pane_id}}"
            )
        );
    }

    #[test]
    fn multiline_formats_inherit_window_markers() {
        let window = format!("sidebar-test-{}:0", std::process::id());
        let pane = format!("{window}.%98704");
        set(&window, "@agent-sidebar-spawned", Some("1"));
        set(&window, "@agent-sidebar-spawned-branch", Some("agent/task"));
        let format = prepare_format("#{@agent-sidebar-spawned}\n#{@agent-sidebar-spawned-branch}");
        assert_eq!(format.matches(CONTEXT).count(), 2);
        let output = format.replace("#{session_name}:#{window_id}.#{pane_id}", &pane);
        assert_eq!(expand_output(&output), "1\nagent/task");
    }

    #[test]
    fn pane_targets_and_metadata_context_are_qualified_once() {
        let format = prepare_format(&qualify_format(
            "#{q:pane_id}|#{q:window_id}|#{q:@pane_role}",
        ));
        assert_eq!(
            format,
            format!(
                "#{{q:session_name}}:#{{window_id}}.#{{pane_id}}|#{{q:session_name}}:#{{window_id}}|__SIDEBAR_OPTION_Q@pane_role__{CONTEXT}#{{session_name}}:#{{window_id}}.#{{pane_id}}"
            )
        );
    }
}
