use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};

use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

#[test]
fn non_local_runtime_refuses_admission_before_starting_workload() {
    let home = TempDir::new().expect("temp home");
    let marker = home.path().join("escaped");
    let spy = home.path().join("spy");
    fs::write(&spy, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).expect("spy");
    fs::set_permissions(&spy, fs::Permissions::from_mode(0o700)).expect("chmod spy");

    let output = Command::new(cargo_bin!("agent-gov"))
        .args(["run", "--owner", "remote-test", "--"])
        .arg(&spy)
        .env("AGENT_GOV_TEST_HOME", home.path())
        .env("AGENT_GOV_TEST_FILESYSTEM", "remote")
        .stdout(Stdio::null())
        .output()
        .expect("governed workload");

    assert_eq!(output.status.code(), Some(69));
    assert!(!marker.exists(), "rejected workload must never start");
    assert!(
        !home.path().join("runtime").exists(),
        "rejection must happen before runtime state is created"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unsupported non-local filesystem test-remote"));
    assert!(stderr.contains("scheduler admission was refused"));
}

#[test]
fn doctor_reports_non_local_runtime_without_initializing_it() {
    let home = TempDir::new().expect("temp home");
    let output = Command::new(cargo_bin!("agent-gov"))
        .args(["doctor", "--json"])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .env("AGENT_GOV_TEST_USER_HOME", home.path())
        .env("AGENT_GOV_TEST_FILESYSTEM", "remote")
        .output()
        .expect("doctor");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    assert!(!home.path().join("runtime").exists());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("doctor JSON");
    let filesystem = report["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["name"] == "filesystem")
        .expect("filesystem check");
    assert_eq!(filesystem["severity"], "error");
    assert!(
        filesystem["detail"]
            .as_str()
            .expect("detail")
            .contains("unsupported non-local filesystem test-remote")
    );
}

#[test]
fn local_runtime_preserves_normal_admission() {
    let home = TempDir::new().expect("temp home");
    let status = Command::new(cargo_bin!("agent-gov"))
        .args(["run", "--owner", "local-test", "--", "/usr/bin/true"])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .env("AGENT_GOV_TEST_FILESYSTEM", "local")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("governed workload");

    assert!(status.success());
    assert!(home.path().join("runtime/slots/slot-0.lock").is_file());
}
