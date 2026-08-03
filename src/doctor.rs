//! Runtime and integration diagnostics.

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use serde::Serialize;
use serde_json::Value;

use crate::{
    VERSION,
    config::Config,
    error::Result,
    install::{
        Agent, installed_binary_path, managed_hook_count, managed_rtk_paths,
        separate_rtk_hook_count, settings_path,
    },
    scheduler::{FilesystemStatus, Runtime},
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
    pub fn run(
        config: &Config,
        runtime: Option<&Runtime>,
        filesystem: &FilesystemStatus,
        binary: &Path,
    ) -> Self {
        let mut checks = Vec::new();
        checks.push(check(
            "binary",
            Severity::Ok,
            format!("agent-gov {VERSION} ({})", std::env::consts::ARCH),
        ));
        checks.push(check(
            "filesystem",
            if filesystem.supported() {
                Severity::Ok
            } else {
                Severity::Error
            },
            filesystem.detail(),
        ));
        if let Some(runtime) = runtime {
            checks.push(match runtime.validate() {
                Ok(()) => check(
                    "runtime",
                    Severity::Ok,
                    format!("{} has private permissions", runtime.root().display()),
                ),
                Err(error) => check("runtime", Severity::Error, error.to_string()),
            });
            checks.push(match runtime.capacity() {
                Ok(capacity @ 1..=2) if capacity == usize::from(config.scheduler.capacity) => check(
                    "capacity",
                    Severity::Ok,
                    format!("configured and applied capacity is {capacity}"),
                ),
                Ok(capacity @ 1..=2) => check(
                    "capacity",
                    Severity::Error,
                    format!(
                        "configured capacity {} differs from applied capacity {capacity}; drain and reapply configuration",
                        config.scheduler.capacity
                    ),
                ),
                Ok(capacity) => check(
                    "capacity",
                    Severity::Error,
                    format!("invalid capacity {capacity}"),
                ),
                Err(error) => check("capacity", Severity::Error, error.to_string()),
            });
        } else {
            checks.push(check(
                "runtime",
                Severity::Error,
                "not initialized because the filesystem policy failed".into(),
            ));
        }
        checks.push(check_rtk(config, binary));
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

    pub fn record_config_error(&mut self, detail: String) {
        self.checks
            .insert(0, check("config", Severity::Error, detail));
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

fn check_rtk(config: &Config, binary: &Path) -> Check {
    if !config.rtk.enabled {
        return check("rtk", Severity::Warning, "integration disabled".into());
    }
    let mut hook_paths = composed_rtk_paths(binary);
    hook_paths.sort();
    hook_paths.dedup();
    if hook_paths.len() > 1 {
        return check(
            "rtk",
            Severity::Error,
            "composed hooks use different RTK paths".into(),
        );
    }
    let hook_path = hook_paths.first().map(PathBuf::as_path);
    if let (Some(configured), Some(composed)) = (config.rtk.path.as_deref(), hook_path)
        && configured != composed
    {
        return check(
            "rtk",
            Severity::Error,
            format!(
                "configured path {} differs from composed hook path {}",
                configured.display(),
                composed.display()
            ),
        );
    }
    match hook_path.or(config.rtk.path.as_deref()) {
        Some(path) if is_executable(path) => check(
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

fn composed_rtk_paths(binary: &Path) -> Vec<PathBuf> {
    let Some(home) = env::var_os("AGENT_GOV_TEST_USER_HOME").or_else(|| env::var_os("HOME")) else {
        return Vec::new();
    };
    [Agent::Claude, Agent::Cursor]
        .into_iter()
        .filter_map(|agent| {
            fs::read(settings_path(Path::new(&home), agent))
                .ok()
                .map(|bytes| (agent, bytes))
        })
        .filter_map(|(agent, bytes)| {
            serde_json::from_slice::<Value>(&bytes)
                .ok()
                .map(|value| (agent, value))
        })
        .flat_map(|(agent, value)| managed_rtk_paths(&value, agent, binary))
        .collect()
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
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
    if agent == Agent::Cursor && value.get("version").and_then(Value::as_u64) != Some(1) {
        return check(
            agent.name(),
            Severity::Error,
            "Cursor hook schema version must be 1".into(),
        );
    }
    let count = managed_hook_count(&value, agent, binary);
    let recorded_binary = match installed_binary_path(&path) {
        Ok(value) => value,
        Err(error) => {
            return check(
                agent.name(),
                Severity::Error,
                format!("install manifest is invalid: {error}"),
            );
        }
    };
    if count == 0
        && let Some(installed_binary) = recorded_binary
        && installed_binary != binary
        && managed_hook_count(&value, agent, &installed_binary) == 1
    {
        return check(
            agent.name(),
            Severity::Error,
            format!(
                "hook points to replaced binary {}; rerun install",
                installed_binary.display()
            ),
        );
    }
    let rtk_count = separate_rtk_hook_count(&value, agent);
    if rtk_count > 0 {
        return check(
            agent.name(),
            Severity::Error,
            format!(
                "found {rtk_count} separate RTK hook(s); rerun install --with-rtk to compose them"
            ),
        );
    }
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
