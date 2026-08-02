use std::{
    fs,
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use agent_gov::config::Config;
use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

#[test]
fn capacity_one_is_never_exceeded() {
    assert_capacity(1);
}

#[test]
fn capacity_two_is_never_exceeded() {
    assert_capacity(2);
}

fn assert_capacity(capacity: u8) {
    let home = TempDir::new().expect("temp home");
    let mut config = Config::default();
    config.scheduler.capacity = capacity;
    config.scheduler.max_wait = Duration::from_secs(10);
    fs::create_dir_all(home.path()).expect("create home");
    fs::write(
        home.path().join("config.toml"),
        toml::to_string_pretty(&config).expect("config TOML"),
    )
    .expect("write config");

    let binary = cargo_bin!("agent-gov");
    let mut children = Vec::new();
    for index in 0..8 {
        let child = Command::new(binary)
            .args([
                "run",
                "--owner",
                &format!("owner{index:02}"),
                "--",
                "/bin/sleep",
                "0.12",
            ])
            .env("AGENT_GOV_TEST_HOME", home.path())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn governor");
        children.push(child);
    }

    let active_dir = home.path().join("runtime/active");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut observed_max = 0;
    while children
        .iter_mut()
        .any(|child| child.try_wait().unwrap().is_none())
    {
        assert!(Instant::now() < deadline, "governed workloads timed out");
        let active = fs::read_dir(&active_dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .path()
                            .extension()
                            .is_some_and(|extension| extension == "json")
                    })
                    .count()
            })
            .unwrap_or_default();
        observed_max = observed_max.max(active);
        assert!(
            active <= usize::from(capacity),
            "observed {active} active workloads at capacity {capacity}"
        );
        thread::sleep(Duration::from_millis(5));
    }
    for mut child in children {
        let status = child.wait().expect("wait");
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            pipe.read_to_string(&mut stderr).expect("read stderr");
        }
        assert!(status.success(), "governor exited with {status}: {stderr}");
    }
    assert_eq!(observed_max, usize::from(capacity));
}
