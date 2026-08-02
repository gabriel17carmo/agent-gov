use std::{
    fs::{self, OpenOptions},
    os::unix::fs::PermissionsExt,
    os::unix::process::ExitStatusExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use agent_gov::{config::Config, scheduler::ActiveMetadata};
use assert_cmd::cargo::cargo_bin;
use nix::{
    sys::signal::{self, Signal},
    unistd::Pid,
};
use tempfile::TempDir;

#[test]
fn first_run_creates_a_missing_application_state_directory() {
    let home = TempDir::new().expect("temp home");
    let state = home.path().join("missing/state");
    let status = Command::new(cargo_bin!("agent-gov"))
        .args(["run", "--owner", "first-run", "--", "/usr/bin/true"])
        .env("AGENT_GOV_TEST_HOME", &state)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("first run status");

    assert!(status.success());
    assert!(state.join("runtime/slots/slot-0.lock").is_file());
}

#[test]
fn full_queue_returns_75_without_starting_the_workload() {
    let home = configured_home(1);
    let binary = cargo_bin!("agent-gov");
    let mut active = governed_sleep(binary, &home, "active", "1");
    wait_for_count(&home.path().join("runtime/active"), "json", 1);
    let mut waiter = governed_sleep(binary, &home, "waiter", "0.1");
    wait_for_count(&home.path().join("runtime/waiters"), "lease", 1);

    let marker = home.path().join("escaped");
    let spy = executable(home.path(), "spy", &format!("touch '{}'", marker.display()));
    let status = Command::new(binary)
        .args(["run", "--owner", "overflow", "--"])
        .arg(&spy)
        .env("AGENT_GOV_TEST_HOME", home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("overflow status");
    assert_eq!(status.code(), Some(75));
    assert!(!marker.exists(), "overflow workload escaped the governor");
    assert!(active.wait().expect("active").success());
    assert!(waiter.wait().expect("waiter").success());
}

#[test]
fn killed_supervisor_quarantines_its_live_child() {
    let home = configured_home(8);
    let binary = cargo_bin!("agent-gov");
    let mut supervisor = governed_sleep(binary, &home, "orphan", "5");
    let metadata_path = home.path().join("runtime/active/slot-0.json");
    wait_for_path(&metadata_path);
    let metadata = wait_for_running_metadata(&metadata_path);
    let child_pid = metadata.child_pid.expect("child pid");

    signal::kill(
        Pid::from_raw(i32::try_from(supervisor.id()).expect("pid")),
        Signal::SIGKILL,
    )
    .expect("kill supervisor");
    let _ = supervisor.wait();

    let marker = home.path().join("must-not-start");
    let spy = executable(
        home.path(),
        "orphan-spy",
        &format!("touch '{}'", marker.display()),
    );
    let mut blocked = Command::new(binary)
        .args(["run", "--owner", "blocked", "--"])
        .arg(&spy)
        .env("AGENT_GOV_TEST_HOME", home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("blocked workload");
    thread::sleep(Duration::from_millis(250));
    assert!(blocked.try_wait().expect("poll").is_none());
    assert!(!marker.exists(), "quarantined slot admitted a new workload");
    signal::kill(
        Pid::from_raw(i32::try_from(blocked.id()).expect("pid")),
        Signal::SIGTERM,
    )
    .expect("stop blocked waiter");
    let _ = blocked.wait();

    let child_group = Pid::from_raw(i32::try_from(child_pid).expect("child pid"));
    let _ = signal::killpg(child_group, Signal::SIGKILL);
    thread::sleep(Duration::from_millis(100));
    let recovered = Command::new(binary)
        .args(["run", "--owner", "recovered", "--", "/usr/bin/true"])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("recovered workload");
    assert!(recovered.success());
}

#[test]
fn termination_signal_is_propagated_after_runtime_cleanup() {
    let home = configured_home(8);
    let binary = cargo_bin!("agent-gov");
    let mut supervisor = governed_sleep(binary, &home, "signal", "5");
    let metadata_path = home.path().join("runtime/active/slot-0.json");
    let _ = wait_for_running_metadata(&metadata_path);

    signal::kill(
        Pid::from_raw(i32::try_from(supervisor.id()).expect("pid")),
        Signal::SIGTERM,
    )
    .expect("terminate supervisor");
    let status = supervisor.wait().expect("wait supervisor");
    assert_eq!(status.signal(), Some(Signal::SIGTERM as i32));
    assert!(!metadata_path.exists(), "permit metadata must be released");
}

#[test]
fn cancellation_uses_the_private_job_control_endpoint() {
    let home = configured_home(8);
    let binary = cargo_bin!("agent-gov");
    let mut supervisor = governed_sleep(binary, &home, "cancel", "5");
    let metadata_path = home.path().join("runtime/active/slot-0.json");
    let metadata = wait_for_running_metadata(&metadata_path);
    let control_path = home
        .path()
        .join(format!("runtime/control/{}.sock", metadata.job_id));
    wait_for_path(&control_path);

    let cancel = Command::new(binary)
        .args(["cancel", &metadata.job_id])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .output()
        .expect("cancel command");
    assert!(
        cancel.status.success(),
        "cancel failed: {}",
        String::from_utf8_lossy(&cancel.stderr)
    );

    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = supervisor.try_wait().expect("poll supervisor") {
            break status;
        }
        assert!(Instant::now() < deadline, "cancelled job did not stop");
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.signal(), Some(Signal::SIGTERM as i32));
    assert!(!metadata_path.exists());
    assert!(!control_path.exists());
}

#[test]
fn cancellation_never_signals_a_pid_from_stale_metadata() {
    let home = configured_home(8);
    let binary = cargo_bin!("agent-gov");
    let initialized = Command::new(binary)
        .args(["run", "--owner", "init", "--", "/usr/bin/true"])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .status()
        .expect("initialize runtime");
    assert!(initialized.success());

    let slot_path = home.path().join("runtime/slots/slot-0.lock");
    let slot = OpenOptions::new()
        .read(true)
        .write(true)
        .open(slot_path)
        .expect("open slot");
    fs4::FileExt::lock(&slot).expect("lock slot");

    let job_id = "0123456789abcdef";
    let metadata = ActiveMetadata::running(
        job_id,
        "stale",
        std::process::id(),
        std::process::id(),
        "heavy",
    );
    fs::write(
        home.path().join("runtime/active/slot-0.json"),
        serde_json::to_vec(&metadata).expect("serialize metadata"),
    )
    .expect("write metadata");

    let cancel = Command::new(binary)
        .args(["cancel", job_id])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .output()
        .expect("cancel command");
    assert_eq!(cancel.status.code(), Some(75));
    assert!(
        String::from_utf8_lossy(&cancel.stderr).contains("control endpoint is unavailable")
    );
    fs4::FileExt::unlock(&slot).expect("unlock slot");
}

fn configured_home(max_queue: usize) -> TempDir {
    let home = TempDir::new().expect("temp home");
    let mut config = Config::default();
    config.scheduler.max_queue = max_queue;
    config.scheduler.max_wait = Duration::from_secs(5);
    fs::write(
        home.path().join("config.toml"),
        toml::to_string_pretty(&config).expect("TOML"),
    )
    .expect("write config");
    home
}

fn governed_sleep(
    binary: &std::path::Path,
    home: &TempDir,
    owner: &str,
    seconds: &str,
) -> std::process::Child {
    Command::new(binary)
        .args(["run", "--owner", owner, "--", "/bin/sleep", seconds])
        .env("AGENT_GOV_TEST_HOME", home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("governed sleep")
}

fn executable(root: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = root.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write executable");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("chmod");
    path
}

fn wait_for_count(directory: &std::path::Path, extension: &str, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let count = fs::read_dir(directory)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .path()
                            .extension()
                            .is_some_and(|value| value == extension)
                    })
                    .count()
            })
            .unwrap_or_default();
        if count >= expected {
            return;
        }
        assert!(Instant::now() < deadline, "runtime state did not appear");
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_path(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} did not appear",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_running_metadata(path: &std::path::Path) -> ActiveMetadata {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(bytes) = fs::read(path)
            && let Ok(metadata) = serde_json::from_slice::<ActiveMetadata>(&bytes)
            && metadata.child_pid.is_some()
        {
            return metadata;
        }
        assert!(Instant::now() < deadline, "running metadata did not appear");
        thread::sleep(Duration::from_millis(10));
    }
}
