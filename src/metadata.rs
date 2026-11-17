use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::tmux::{AgentType, SessionInfo};

#[derive(Debug, Default)]
pub(crate) struct MetadataCache {
    paths: BTreeMap<String, PathBuf>,
    models: BTreeMap<PathBuf, (u64, String)>,
}

fn claude_path(id: &str) -> Result<Option<PathBuf>, String> {
    let root = PathBuf::from(
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .expect("user home is required"),
    )
    .join(".claude/projects");
    for directory in fs::read_dir(&root).map_err(|error| error.to_string())? {
        let path = directory
            .map_err(|error| error.to_string())?
            .path()
            .join(format!("{id}.jsonl"));
        if path.is_file() {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn read_model(path: &Path, length: u64) -> Result<String, String> {
    let mut count = length.min(65536);
    loop {
        let mut file = File::open(path).map_err(|error| error.to_string())?;
        file.seek(SeekFrom::Start(length - count))
            .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let start = if count < length {
            bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|index| index + 1)
                .unwrap_or(bytes.len())
        } else {
            0
        };
        let end = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(start);
        for line in bytes[start..end]
            .split(|byte| *byte == b'\n')
            .rev()
            .filter(|line| !line.is_empty())
        {
            let record: Value = serde_json::from_slice(line).map_err(|error| {
                format!("invalid Claude transcript {}: {error}", path.display())
            })?;
            if let Some(model) = record
                .get("message")
                .and_then(|message| message.get("model"))
                .and_then(Value::as_str)
            {
                if !model.is_empty() && !model.starts_with('<') {
                    return Ok(model.to_string());
                }
            }
        }
        if count == length {
            return Ok(String::new());
        }
        count = (count * 2).min(length);
    }
}

pub(crate) fn refresh(cache: &mut MetadataCache, sessions: &mut [SessionInfo]) -> Vec<String> {
    let mut errors = Vec::new();
    for session in sessions {
        for window in &mut session.windows {
            for pane in &mut window.panes {
                if pane.agent != AgentType::Claude {
                    continue;
                }
                let Some(id) = &pane.session_id else {
                    continue;
                };
                let result = refresh_model(cache, id);
                match result {
                    Ok(Some(model)) => pane.model = model,
                    Ok(None) => {}
                    Err(error) => errors.push(error),
                }
            }
        }
    }
    errors
}

fn refresh_model(cache: &mut MetadataCache, id: &str) -> Result<Option<String>, String> {
    if !cache.paths.contains_key(id) {
        let Some(path) = claude_path(id)? else {
            return Ok(None);
        };
        cache.paths.insert(id.to_string(), path);
    }
    let path = cache
        .paths
        .get(id)
        .expect("Claude transcript path was just resolved");
    let length = fs::metadata(path).map_err(|error| error.to_string())?.len();
    if cache
        .models
        .get(path)
        .is_none_or(|(previous, _)| *previous != length)
    {
        let model = read_model(path, length)?;
        cache.models.insert(path.clone(), (length, model));
    }
    Ok(cache.models.get(path).map(|(_, model)| model.clone()))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn latest_real_model_survives_partial_records_and_synthetic_messages() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"{\"message\":{\"model\":\"claude-sonnet-4-6\"}}\n{\"message\":{\"model\":\"<synthetic>\"}}\n{\"message\":").unwrap();
        let length = file.as_file().metadata().unwrap().len();
        assert_eq!(
            read_model(file.path(), length).unwrap(),
            "claude-sonnet-4-6"
        );
    }
}
