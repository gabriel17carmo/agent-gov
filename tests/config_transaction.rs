use std::{fs, process::Command};

use agent_gov::config::Config;
use assert_cmd::cargo::cargo_bin;
use predicates::prelude::*;
use tempfile::TempDir;

#[test]
fn failed_config_write_rolls_back_runtime_capacity_and_drain_state() {
    let home = TempDir::new().expect("temp home");
    fs::create_dir(home.path().join("config.toml")).expect("blocking config directory");

    let output = Command::new(cargo_bin!("agent-gov"))
        .args(["config", "set-capacity", "2", "--drain"])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .output()
        .expect("configure");

    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(home.path().join("runtime/capacity")).expect("capacity"),
        "1"
    );
    assert!(!home.path().join("runtime/drain.flag").exists());
}

#[test]
fn doctor_reports_desired_and_applied_capacity_divergence() {
    let home = TempDir::new().expect("temp home");
    let binary = cargo_bin!("agent-gov");
    Command::new(binary)
        .args(["status"])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .output()
        .expect("initialize runtime");

    let mut config = Config::default();
    config.scheduler.capacity = 2;
    fs::write(
        home.path().join("config.toml"),
        toml::to_string_pretty(&config).expect("toml"),
    )
    .expect("write config");
    let output = Command::new(binary)
        .args(["doctor"])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .env("AGENT_GOV_TEST_USER_HOME", home.path())
        .output()
        .expect("doctor");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        predicate::str::contains("configured capacity 2 differs from applied capacity 1")
            .eval(&String::from_utf8_lossy(&output.stdout))
    );
}

#[test]
fn doctor_reports_invalid_configuration_as_an_error() {
    let home = TempDir::new().expect("temp home");
    fs::write(home.path().join("config.toml"), "not valid TOML = [").expect("invalid config");
    let output = Command::new(cargo_bin!("agent-gov"))
        .args(["doctor", "--json"])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .env("AGENT_GOV_TEST_USER_HOME", home.path())
        .output()
        .expect("doctor");

    assert_eq!(output.status.code(), Some(2));
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("doctor JSON remains valid");
    assert_eq!(report["checks"][0]["name"], "config");
    assert_eq!(report["checks"][0]["severity"], "error");
}
