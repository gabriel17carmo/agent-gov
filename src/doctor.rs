//! Runtime and integration diagnostics.

use std::{env, fs, path::Path};

use serde::Serialize;
use serde_json::Value;

use crate::{
    VERSION,
    config::Config,
    error::Result,
    install::{Agent, settings_path},
    scheduler::Runtime,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub severity: Severity,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u8,
    pub version: String,
    pub checks: Vec<Check>,
}

impl Report {
    #[must_use]
    pub fn run(config: &Config, runtime: &Runtime, binary: &Path) -> Self {
        let mut checks = Vec::new();
        checks.push(check(
            "binary",
            Severity::Ok,
            format!("agent-gov {VERSION} ({})", std::env::consts::ARCH),
        ));
        checks.push(match runtime.validate() {
            Ok(()) => check(
                "runtime",
                Severity::Ok,
                format!("{} has private permissions", runtime.root().display()),
            ),
            Err(error) => check("runtime", Severity::Error, error.to_string()),
        });
        checks.push(match runtime.capacity() {
            Ok(capacity @ 1..=2) => check(
                "capacity",
                Severity::Ok,
                format!("capacity snapshot is {capacity}"),
            ),
            Ok(capacity) => check(
                "capacity",
                Severity::Error,
                format!("invalid capacity {capacity}"),
            ),
            Err(error) => check("capacity", Severity::Error, error.to_string()),
        });
        checks.push(check_rtk(config));
        for agent in [Agent::Claude, Agent::Cursor] {
            checks.push(check_hook(agent, binary));
        }
        Self {
            schema_version: 1,
            version: VERSION.into(),
            checks,
        }
    }

    #[must_use]
    pub fn exit_code(&self) -> i32 {
        if self
            .checks
            .iter()
            .any(|check| check.severity == Severity::Error)
        {
            2
        } else {
            i32::from(
                self.checks
                    .iter()
                    .any(|check| check.severity == Severity::Warning),
            )
        }
    }

    #[must_use]
    pub fn human(&self) -> String {
        self.checks
            .iter()
            .map(|check| {
                let icon = match check.severity {
                    Severity::Ok => "ok",
                    Severity::Warning => "warn",
                    Severity::Error => "error",
                };
                format!("[{icon}] {}: {}", check.name, check.detail)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn check_rtk(config: &Config) -> Check {
    if !config.rtk.enabled {
        return check("rtk", Severity::Warning, "integration disabled".into());
    }
    match config.rtk.path.as_deref() {
        Some(path) if path.is_file() => check(
            "rtk",
            Severity::Ok,
            format!("configured at {}", path.display()),
        ),
        Some(path) => check(
            "rtk",
            Severity::Warning,
            format!("configured path {} is unavailable", path.display()),
        ),
        None => check(
            "rtk",
            Severity::Warning,
            "no absolute RTK path configured; governance remains active".into(),
        ),
    }
}

fn check_hook(agent: Agent, binary: &Path) -> Check {
    let Some(home) = env::var_os("AGENT_GOV_TEST_USER_HOME").or_else(|| env::var_os("HOME")) else {
        return check(agent.name(), Severity::Error, "HOME is not defined".into());
    };
    let path = settings_path(Path::new(&home), agent);
    let Ok(bytes) = fs::read(&path) else {
        return check(
            agent.name(),
            Severity::Warning,
            format!("hook is not installed at {}", path.display()),
        );
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return check(
            agent.name(),
            Severity::Error,
            "settings JSON is invalid".into(),
        );
    };
    let binary = binary.to_string_lossy();
    let serialized = value.to_string();
    let count = serialized.matches(binary.as_ref()).count();
    if count == 1 {
        check(
            agent.name(),
            Severity::Ok,
            "one composed hook is installed".into(),
        )
    } else if count == 0 {
        check(
            agent.name(),
            Severity::Warning,
            "agent-gov hook not found".into(),
        )
    } else {
        check(
            agent.name(),
            Severity::Error,
            format!("found {count} agent-gov references; expected exactly one"),
        )
    }
}

fn check(name: impl Into<String>, severity: Severity, detail: String) -> Check {
    Check {
        name: name.into(),
        severity,
        detail,
    }
}

pub fn to_json(report: &Report) -> Result<String> {
    Ok(serde_json::to_string_pretty(report)?)
}
