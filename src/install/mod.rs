//! Transactional hook installation.

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    error::{GovError, Result},
    scheduler::write_private_atomic,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Agent {
    Claude,
    Cursor,
}

impl Agent {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor",
        }
    }
}

pub fn install(agents: &[Agent], binary: &Path, rtk: Option<&Path>) -> Result<Vec<PathBuf>> {
    validate_executable_path(binary)?;
    if let Some(path) = rtk {
        validate_executable_path(path)?;
    }
    let home = user_home()?;
    let mut changed = Vec::new();
    for agent in agents {
        let path = settings_path(&home, *agent);
        ensure_parent(&path)?;
        let original = read_json_or_empty(&path)?;
        backup_once(&path, &original)?;
        let updated = match agent {
            Agent::Claude => patch_claude(original, binary, rtk)?,
            Agent::Cursor => patch_cursor(original, binary, rtk)?,
        };
        write_private_atomic(&path, &serde_json::to_vec_pretty(&updated)?)?;
        changed.push(path);
    }
    Ok(changed)
}

pub fn uninstall(agents: &[Agent], binary: &Path) -> Result<Vec<PathBuf>> {
    let home = user_home()?;
    let mut changed = Vec::new();
    for agent in agents {
        let path = settings_path(&home, *agent);
        if !path.exists() {
            continue;
        }
        let original = read_json_or_empty(&path)?;
        let updated = match agent {
            Agent::Claude => unpatch_claude(original, binary),
            Agent::Cursor => unpatch_cursor(original, binary),
        };
        write_private_atomic(&path, &serde_json::to_vec_pretty(&updated)?)?;
        changed.push(path);
    }
    Ok(changed)
}

#[must_use]
pub fn settings_path(home: &Path, agent: Agent) -> PathBuf {
    match agent {
        Agent::Claude => home.join(".claude/settings.json"),
        Agent::Cursor => home.join(".cursor/hooks.json"),
    }
}

fn patch_claude(mut root: Value, binary: &Path, rtk: Option<&Path>) -> Result<Value> {
    let object = object_mut(&mut root)?;
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = object_mut(hooks)?;
    let groups = hooks
        .entry("PreToolUse")
        .or_insert_with(|| Value::Array(Vec::new()));
    let groups = array_mut(groups)?;
    groups.retain(|group| !is_agent_gov_claude(group, binary) && !is_known_rtk_claude(group));

    let mut args = vec![json!("hook"), json!("claude")];
    if let Some(path) = rtk {
        args.push(json!("--rtk"));
        args.push(json!(path_string(path)?));
    }
    groups.push(json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": path_string(binary)?,
            "args": args,
            "timeout": 5
        }]
    }));
    Ok(root)
}

fn patch_cursor(mut root: Value, binary: &Path, rtk: Option<&Path>) -> Result<Value> {
    let object = object_mut(&mut root)?;
    object.entry("version").or_insert(json!(1));
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = object_mut(hooks)?;
    let groups = hooks
        .entry("preToolUse")
        .or_insert_with(|| Value::Array(Vec::new()));
    let groups = array_mut(groups)?;
    let binary_text = path_string(binary)?;
    groups.retain(|group| {
        let command = group
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        !command.contains(&binary_text) && !is_known_rtk_cursor(command)
    });
    let mut command = format!("{} hook cursor", quote_posix(&binary_text));
    if let Some(path) = rtk {
        command.push_str(" --rtk ");
        command.push_str(&quote_posix(&path_string(path)?));
    }
    groups.push(json!({"command": command, "matcher": "Shell"}));
    Ok(root)
}

fn unpatch_claude(mut root: Value, binary: &Path) -> Value {
    if let Some(groups) = root
        .pointer_mut("/hooks/PreToolUse")
        .and_then(Value::as_array_mut)
    {
        groups.retain(|group| !is_agent_gov_claude(group, binary));
    }
    root
}

fn unpatch_cursor(mut root: Value, binary: &Path) -> Value {
    let binary = binary.to_string_lossy();
    if let Some(groups) = root
        .pointer_mut("/hooks/preToolUse")
        .and_then(Value::as_array_mut)
    {
        groups.retain(|group| {
            !group
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| command.contains(binary.as_ref()))
        });
    }
    root
}

fn is_agent_gov_claude(group: &Value, binary: &Path) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                hook.get("command").and_then(Value::as_str) == binary.to_str()
                    && hook
                        .get("args")
                        .and_then(Value::as_array)
                        .is_some_and(|args| args.first().and_then(Value::as_str) == Some("hook"))
            })
        })
}

fn is_known_rtk_claude(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| {
            hooks.iter().any(|hook| {
                let command = hook
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                command == "rtk hook claude" || command.ends_with("/rtk-rewrite.sh")
            })
        })
}

fn is_known_rtk_cursor(command: &str) -> bool {
    command == "rtk hook cursor" || command.ends_with("/rtk-rewrite.sh")
}

fn read_json_or_empty(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let bytes = fs::read(path)?;
    if bytes.len() > 1024 * 1024 {
        return Err(GovError::InvalidConfig(format!(
            "settings file {} exceeds 1 MiB",
            path.display()
        )));
    }
    let root: Value = serde_json::from_slice(&bytes)?;
    if !root.is_object() {
        return Err(GovError::InvalidConfig(format!(
            "settings file {} must contain a JSON object",
            path.display()
        )));
    }
    Ok(root)
}

fn backup_once(path: &Path, value: &Value) -> Result<()> {
    let backup = path.with_extension("json.agent-gov-backup");
    if backup.exists() {
        return Ok(());
    }
    write_private_atomic(&backup, &serde_json::to_vec_pretty(value)?)?;
    let hash = Sha256::digest(serde_json::to_vec(value)?);
    write_private_atomic(
        &path.with_extension("json.agent-gov-backup.sha256"),
        format!("{hash:x}\n").as_bytes(),
    )
}

fn ensure_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| GovError::Internal("settings path has no parent".into()))?;
    fs::create_dir_all(parent)?;
    if fs::metadata(parent)?.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn validate_executable_path(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.to_string_lossy().contains(['\0', '\n']) {
        return Err(GovError::InvalidInput(
            "hook executable paths must be absolute and single-line".into(),
        ));
    }
    Ok(())
}

fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| GovError::InvalidInput("path is not UTF-8".into()))
}

fn object_mut(value: &mut Value) -> Result<&mut Map<String, Value>> {
    value
        .as_object_mut()
        .ok_or_else(|| GovError::InvalidConfig("expected JSON object".into()))
}

fn array_mut(value: &mut Value) -> Result<&mut Vec<Value>> {
    value
        .as_array_mut()
        .ok_or_else(|| GovError::InvalidConfig("expected JSON array".into()))
}

fn quote_posix(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn user_home() -> Result<PathBuf> {
    env::var_os("AGENT_GOV_TEST_USER_HOME")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| GovError::Runtime("HOME is not defined".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_is_idempotent_and_preserves_unrelated_hooks() {
        let original = json!({"hooks":{"PreToolUse":[{"matcher":"Read","hooks":[]}]}});
        let once =
            patch_claude(original, Path::new("/opt/agent gov/agent-gov"), None).expect("patch");
        let twice = patch_claude(once, Path::new("/opt/agent gov/agent-gov"), None).expect("patch");
        let groups = twice
            .pointer("/hooks/PreToolUse")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["matcher"], "Read");
    }

    #[test]
    fn cursor_path_is_posix_quoted() {
        let value =
            patch_cursor(json!({}), Path::new("/opt/it's here/agent-gov"), None).expect("patch");
        assert_eq!(
            value["hooks"]["preToolUse"][0]["command"],
            "'/opt/it'\\''s here/agent-gov' hook cursor"
        );
    }
}
