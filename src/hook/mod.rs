//! Claude Code and Cursor hook adapters.

mod rtk;

use std::{env, fmt::Write as _, path::Path, time::Duration};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    config::Config,
    error::{GovError, Result},
    shell::{analyze, rewrite},
};

pub use rtk::{RtkDecision, invoke as invoke_rtk};

const MAX_HOOK_INPUT: usize = 128 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Host {
    Claude,
    Cursor,
}

#[derive(Clone, Debug)]
pub struct HookOptions<'a> {
    pub host: Host,
    pub binary_path: &'a Path,
    pub rtk_path: Option<&'a Path>,
    pub config: &'a Config,
}

pub fn handle(input: &[u8], options: &HookOptions<'_>) -> Result<Vec<u8>> {
    if input.len() > MAX_HOOK_INPUT {
        return Ok(pass_through(options.host));
    }
    let Ok(mut payload) = serde_json::from_slice::<Value>(input) else {
        return Ok(pass_through(options.host));
    };
    let Some(root) = payload.as_object_mut() else {
        return Ok(pass_through(options.host));
    };
    if !matches_tool(root, options.host) {
        return Ok(pass_through(options.host));
    }
    let Some(tool_input) = root.get("tool_input").and_then(Value::as_object).cloned() else {
        return Ok(pass_through(options.host));
    };
    let Some(original) = tool_input.get("command").and_then(Value::as_str) else {
        return Ok(pass_through(options.host));
    };
    if original.len() > 64 * 1024 || original.contains('\0') {
        return Ok(pass_through(options.host));
    }

    let owner = owner_hash(root);
    let original_analysis = analyze(original, &options.config.classification.rules)?;
    let original_heavy = original_analysis.heavy_count();
    let mut permission = RtkDecision::Preserve;
    let mut candidate = original.to_owned();

    let rtk_enabled = options.config.rtk.enabled
        && env::var_os("RTK_DISABLED").as_deref() != Some(std::ffi::OsStr::new("1"));
    if rtk_enabled && let Some(path) = options.rtk_path.or(options.config.rtk.path.as_deref()) {
        match rtk::invoke(path, original, options.config.rtk.timeout) {
            RtkDecision::Rewrite { command, ask } => {
                candidate = command;
                if ask {
                    permission = RtkDecision::Ask;
                }
            }
            RtkDecision::Deny => {
                return serialize_deny(options.host, "RTK denied this command");
            }
            RtkDecision::Preserve | RtkDecision::Ask => {}
        }
    }

    let binary = options
        .binary_path
        .to_str()
        .ok_or_else(|| GovError::InvalidInput("binary path is not UTF-8".into()))?;
    let mut outcome = rewrite(
        &candidate,
        binary,
        &owner,
        &options.config.classification.rules,
        options.config.classification.deny_background_heavy,
    )?;

    if original_heavy > 0 && outcome.heavy_segments == 0 && !outcome.denied {
        outcome = rewrite(
            original,
            binary,
            &owner,
            &options.config.classification.rules,
            options.config.classification.deny_background_heavy,
        )?;
        candidate.clone_from(&outcome.command);
    }
    if outcome.denied {
        return serialize_deny(
            options.host,
            outcome.reason.as_deref().unwrap_or("unsafe heavy command"),
        );
    }

    let final_command = if outcome.changed {
        outcome.command
    } else {
        candidate
    };
    if final_command == original && permission == RtkDecision::Preserve {
        return Ok(pass_through(options.host));
    }

    let mut updated = tool_input;
    updated.insert("command".into(), Value::String(final_command));
    if options.host == Host::Claude && outcome.heavy_segments > 0 {
        increase_claude_timeout(&mut updated);
    }
    serialize_update(options.host, updated, &permission)
}

fn matches_tool(root: &Map<String, Value>, host: Host) -> bool {
    let expected = match host {
        Host::Claude => "Bash",
        Host::Cursor => "Shell",
    };
    root.get("tool_name").and_then(Value::as_str) == Some(expected)
}

fn increase_claude_timeout(tool_input: &mut Map<String, Value>) {
    const FLOOR_MS: u64 = 600_000;
    let current = tool_input
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    tool_input.insert("timeout".into(), Value::from(current.max(FLOOR_MS)));
}

fn owner_hash(root: &Map<String, Value>) -> String {
    let owner = ["session_id", "conversation_id", "tool_use_id"]
        .iter()
        .find_map(|key| root.get(*key).and_then(Value::as_str))
        .unwrap_or("anonymous");
    let digest = Sha256::digest(owner.as_bytes());
    digest[..8].iter().fold(String::new(), |mut output, byte| {
        let _ = write!(output, "{byte:02x}");
        output
    })
}

fn serialize_update(
    host: Host,
    updated: Map<String, Value>,
    permission: &RtkDecision,
) -> Result<Vec<u8>> {
    let value = match host {
        Host::Claude => {
            let mut output = Map::new();
            output.insert("hookEventName".into(), Value::String("PreToolUse".into()));
            output.insert("updatedInput".into(), Value::Object(updated));
            if *permission == RtkDecision::Ask {
                output.insert("permissionDecision".into(), Value::String("ask".into()));
                output.insert(
                    "permissionDecisionReason".into(),
                    Value::String("RTK requested confirmation".into()),
                );
            }
            json!({"hookSpecificOutput": output})
        }
        Host::Cursor => json!({"updated_input": updated}),
    };
    Ok(serde_json::to_vec(&value)?)
}

fn serialize_deny(host: Host, reason: &str) -> Result<Vec<u8>> {
    let value = match host {
        Host::Claude => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason
            }
        }),
        Host::Cursor => json!({"permission": "deny", "message": reason}),
    };
    Ok(serde_json::to_vec(&value)?)
}

fn pass_through(host: Host) -> Vec<u8> {
    match host {
        Host::Claude => Vec::new(),
        Host::Cursor => b"{}".to_vec(),
    }
}

#[must_use]
pub const fn default_hook_deadline() -> Duration {
    Duration::from_secs(1)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::Value;

    use super::*;

    #[test]
    fn claude_preserves_unknown_tool_input_fields() {
        let config = Config::default();
        let input = br#"{
            "session_id":"s1",
            "tool_name":"Bash",
            "tool_input":{"command":"npm test","description":"tests","future":42}
        }"#;
        let output = handle(
            input,
            &HookOptions {
                host: Host::Claude,
                binary_path: Path::new("/usr/local/bin/agent-gov"),
                rtk_path: None,
                config: &config,
            },
        )
        .expect("handle");
        let value: Value = serde_json::from_slice(&output).expect("json");
        let updated = &value["hookSpecificOutput"]["updatedInput"];
        assert_eq!(updated["description"], "tests");
        assert_eq!(updated["future"], 42);
        assert_eq!(updated["timeout"], 600_000);
        assert!(updated["command"].as_str().unwrap().contains("agent-gov"));
        assert!(
            value["hookSpecificOutput"]
                .get("permissionDecision")
                .is_none()
        );
    }

    #[test]
    fn cursor_always_emits_json() {
        let config = Config::default();
        let output = handle(
            br#"{"tool_name":"Other","tool_input":{"command":"echo ok"}}"#,
            &HookOptions {
                host: Host::Cursor,
                binary_path: Path::new("/bin/agent-gov"),
                rtk_path: None,
                config: &config,
            },
        )
        .expect("handle");
        assert_eq!(output, b"{}");
    }
}
