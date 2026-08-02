use std::{fs, os::unix::fs::PermissionsExt, process::Command};

use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

#[test]
fn install_preflight_does_not_partially_update_another_agent() {
    let home = TempDir::new().expect("temp home");
    let claude = home.path().join(".claude/settings.json");
    fs::create_dir_all(claude.parent().expect("parent")).expect("claude directory");
    let original = b"{\n  \"keep\": true\n}\n";
    fs::write(&claude, original).expect("claude settings");

    let cursor = home.path().join(".cursor/hooks.json");
    fs::create_dir_all(&cursor).expect("invalid cursor settings directory");
    let output = Command::new(cargo_bin!("agent-gov"))
        .args(["install", "--agents", "claude,cursor"])
        .env("AGENT_GOV_TEST_USER_HOME", home.path())
        .output()
        .expect("install");

    assert!(!output.status.success());
    assert_eq!(fs::read(&claude).expect("read claude"), original);
}

#[test]
fn unchanged_install_restores_the_exact_original_on_uninstall() {
    let home = TempDir::new().expect("temp home");
    let claude = home.path().join(".claude/settings.json");
    fs::create_dir_all(claude.parent().expect("parent")).expect("claude directory");
    let original = b"{ \"keep\" : true }\n";
    fs::write(&claude, original).expect("claude settings");
    let binary = cargo_bin!("agent-gov");

    let installed = Command::new(binary)
        .args(["install", "--agents", "claude"])
        .env("AGENT_GOV_TEST_USER_HOME", home.path())
        .status()
        .expect("install");
    assert!(installed.success());
    let uninstalled = Command::new(binary)
        .args(["uninstall", "--agents", "claude"])
        .env("AGENT_GOV_TEST_USER_HOME", home.path())
        .status()
        .expect("uninstall");

    assert!(uninstalled.success());
    assert_eq!(fs::read(&claude).expect("read restored settings"), original);
    assert!(!claude.with_extension("json.agent-gov-manifest").exists());
}

#[test]
fn modified_install_is_surgically_unpatched() {
    let home = TempDir::new().expect("temp home");
    let binary = cargo_bin!("agent-gov");
    let installed = Command::new(binary)
        .args(["install", "--agents", "cursor"])
        .env("AGENT_GOV_TEST_USER_HOME", home.path())
        .status()
        .expect("install");
    assert!(installed.success());
    let cursor = home.path().join(".cursor/hooks.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&cursor).expect("cursor settings")).expect("json");
    value["keep"] = serde_json::json!(true);
    fs::write(
        &cursor,
        serde_json::to_vec_pretty(&value).expect("modified json"),
    )
    .expect("modify settings");

    let uninstalled = Command::new(binary)
        .args(["uninstall", "--agents", "cursor"])
        .env("AGENT_GOV_TEST_USER_HOME", home.path())
        .status()
        .expect("uninstall");
    assert!(uninstalled.success());
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(&cursor).expect("cursor settings")).expect("json");
    assert_eq!(value["keep"], true);
    assert_eq!(
        value["hooks"]["preToolUse"]
            .as_array()
            .expect("hooks")
            .len(),
        0
    );
}

#[test]
fn doctor_recognizes_rtk_path_composed_into_installed_hooks() {
    let home = TempDir::new().expect("temp home");
    let rtk = home.path().join("rtk");
    fs::write(&rtk, "#!/bin/sh\nexit 1\n").expect("fake RTK");
    fs::set_permissions(&rtk, fs::Permissions::from_mode(0o700)).expect("chmod RTK");
    let binary = cargo_bin!("agent-gov");
    let installed = Command::new(binary)
        .args([
            "install",
            "--agents",
            "claude,cursor",
            "--with-rtk",
            "--rtk",
        ])
        .arg(&rtk)
        .env("AGENT_GOV_TEST_USER_HOME", home.path())
        .status()
        .expect("install");
    assert!(installed.success());

    let output = Command::new(binary)
        .args(["doctor"])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .env("AGENT_GOV_TEST_USER_HOME", home.path())
        .output()
        .expect("doctor");
    assert!(
        output.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("configured at"));
}
