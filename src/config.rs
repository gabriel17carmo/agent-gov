use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::error::{GovError, Result};

pub const DEFAULT_CAPACITY: u8 = 1;
pub const MAX_CAPACITY: u8 = 2;
pub const DEFAULT_MAX_QUEUE: usize = 8;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema_version: u8,
    pub scheduler: SchedulerConfig,
    pub rtk: RtkConfig,
    pub classification: ClassificationConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SchedulerConfig {
    pub capacity: u8,
    pub max_queue: usize,
    pub max_queued_per_owner: usize,
    #[serde(with = "duration_serde")]
    pub max_wait: Duration,
    #[serde(with = "duration_serde")]
    pub retry_after: Duration,
    #[serde(with = "duration_serde")]
    pub max_run: Duration,
    #[serde(with = "duration_serde")]
    pub termination_grace: Duration,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RtkConfig {
    pub enabled: bool,
    pub path: Option<PathBuf>,
    #[serde(with = "duration_serde")]
    pub timeout: Duration,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClassificationConfig {
    pub deny_background_heavy: bool,
    pub rules: Vec<CustomRule>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CustomRule {
    pub id: String,
    pub argv_prefix: Vec<String>,
    pub class: CustomClass,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomClass {
    Heavy,
    Light,
    Service,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            scheduler: SchedulerConfig::default(),
            rtk: RtkConfig::default(),
            classification: ClassificationConfig::default(),
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
            max_queue: DEFAULT_MAX_QUEUE,
            max_queued_per_owner: 1,
            max_wait: Duration::from_secs(300),
            retry_after: Duration::from_secs(30),
            max_run: Duration::from_secs(1_800),
            termination_grace: Duration::from_secs(5),
        }
    }
}

impl Default for RtkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: None,
            timeout: Duration::from_millis(750),
        }
    }
}

impl Default for ClassificationConfig {
    fn default() -> Self {
        Self {
            deny_background_heavy: true,
            rules: Vec::new(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(GovError::InvalidConfig(format!(
                "unsupported schema version {}",
                self.schema_version
            )));
        }
        if !(1..=MAX_CAPACITY).contains(&self.scheduler.capacity) {
            return Err(GovError::InvalidConfig(
                "scheduler.capacity must be 1 or 2".into(),
            ));
        }
        if !(1..=64).contains(&self.scheduler.max_queue) {
            return Err(GovError::InvalidConfig(
                "scheduler.max_queue must be between 1 and 64".into(),
            ));
        }
        if !(Duration::from_secs(5)..=Duration::from_secs(900)).contains(&self.scheduler.max_wait) {
            return Err(GovError::InvalidConfig(
                "scheduler.max_wait must be between 5s and 15m".into(),
            ));
        }
        if !(Duration::from_millis(100)..=Duration::from_secs(2)).contains(&self.rtk.timeout) {
            return Err(GovError::InvalidConfig(
                "rtk.timeout must be between 100ms and 2s".into(),
            ));
        }
        if self
            .rtk
            .path
            .as_deref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err(GovError::InvalidConfig(
                "rtk.path must be absolute when configured".into(),
            ));
        }
        for rule in &self.classification.rules {
            if rule.id.is_empty() || rule.argv_prefix.is_empty() {
                return Err(GovError::InvalidConfig(
                    "custom rules require a non-empty id and argv_prefix".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&config_path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(path)?;
        if bytes.len() > 64 * 1024 {
            return Err(GovError::InvalidConfig(
                "configuration exceeds 64 KiB".into(),
            ));
        }
        let config: Self = toml::from_str(
            std::str::from_utf8(&bytes)
                .map_err(|_| GovError::InvalidConfig("configuration is not UTF-8".into()))?,
        )?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        self.validate()?;
        let path = config_path()?;
        let parent = path
            .parent()
            .ok_or_else(|| GovError::Internal("configuration path has no parent".into()))?;
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        crate::scheduler::write_private_atomic(&path, toml::to_string_pretty(self)?.as_bytes())
    }
}

pub fn app_support_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("AGENT_GOV_TEST_HOME") {
        return Ok(PathBuf::from(path));
    }
    let home =
        env::var_os("HOME").ok_or_else(|| GovError::Runtime("HOME is not defined".into()))?;
    #[cfg(target_os = "macos")]
    return Ok(PathBuf::from(home).join("Library/Application Support/agent-gov"));
    #[cfg(not(target_os = "macos"))]
    Ok(env::var_os("XDG_STATE_HOME").map_or_else(
        || PathBuf::from(home).join(".local/state/agent-gov"),
        |base| PathBuf::from(base).join("agent-gov"),
    ))
}

pub fn runtime_dir() -> Result<PathBuf> {
    Ok(app_support_dir()?.join("runtime"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(app_support_dir()?.join("config.toml"))
}

mod duration_serde {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let millis = value.as_millis();
        if millis.is_multiple_of(1_000) {
            serializer.serialize_str(&format!("{}s", millis / 1_000))
        } else {
            serializer.serialize_str(&format!("{millis}ms"))
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse(&value).ok_or_else(|| D::Error::custom("expected a duration like 750ms, 5s, or 5m"))
    }

    fn parse(value: &str) -> Option<Duration> {
        let split = value.find(|c: char| !c.is_ascii_digit())?;
        let amount: u64 = value[..split].parse().ok()?;
        match &value[split..] {
            "ms" => Some(Duration::from_millis(amount)),
            "s" => Some(Duration::from_secs(amount)),
            "m" => Some(Duration::from_secs(amount.checked_mul(60)?)),
            "h" => Some(Duration::from_secs(amount.checked_mul(3_600)?)),
            _ => None,
        }
    }
}
