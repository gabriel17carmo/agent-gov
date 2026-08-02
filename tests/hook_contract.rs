use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};

use agent_gov::{
    config::Config,
    hook::{HookOptions, Host, handle},
};
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn rtk_rewrite_remains_inside_the_governed_slot() {
    let temp = TempDir::new().expect("temp");
    let rtk = fake_rtk(temp.path(), "printf 'rtk npm test\\n'; exit 0");
    let mut config = Config::default();
    config.rtk.timeout = Duration::from_millis(200);
    let output = handle(
        br#"{"session_id":"abc","tool_name":"Bash","tool_input":{"command":"npm test"}}"#,
        &HookOptions {
            host: Host::Claude,
            binary_path: Path::new("/opt/agent-gov"),
            rtk_path: Some(&rtk),
            config: &config,
        },
    )
    .expect("hook");
    let value: Value = serde_json::from_slice(&output).expect("JSON");
    let command = value["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .expect("command");
    assert!(command.contains("-- rtk npm test"));
    assert!(command.starts_with("'/opt/agent-gov' run"));
}

#[test]
fn rtk_timeout_does_not_bypass_governance() {
    let temp = TempDir::new().expect("temp");
    let rtk = fake_rtk(temp.path(), "sleep 2; exit 0");
    let mut config = Config::default();
    config.rtk.timeout = Duration::from_millis(100);
    let output = handle(
        br#"{"session_id":"abc","tool_name":"Bash","tool_input":{"command":"npm test"}}"#,
        &HookOptions {
            host: Host::Claude,
            binary_path: Path::new("/opt/agent-gov"),
            rtk_path: Some(&rtk),
            config: &config,
        },
    )
    .expect("hook");
    let value: Value = serde_json::from_slice(&output).expect("JSON");
    let command = value["hookSpecificOutput"]["updatedInput"]["command"]
        .as_str()
        .expect("command");
    assert!(command.ends_with("-- npm test"));
}

fn fake_rtk(root: &Path, body: &str) -> std::path::PathBuf {
    let path = root.join("rtk");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake RTK");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("chmod");
    path
}
