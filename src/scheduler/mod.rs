//! Bounded local scheduler.

mod runtime;

use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    error::{GovError, Result},
};

pub use runtime::{ActiveMetadata, Runtime, process_alive};

pub fn write_private_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    runtime::write_atomic(path, bytes)
}

#[derive(Debug)]
pub struct Permit {
    slot: usize,
    lock: File,
    active_path: PathBuf,
    job_id: String,
    owner: String,
}

impl Permit {
    #[must_use]
    pub const fn slot(&self) -> usize {
        self.slot
    }

    #[must_use]
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn mark_starting(&self) -> Result<()> {
        let metadata = ActiveMetadata::starting(&self.job_id, &self.owner, std::process::id());
        runtime::write_json_atomic(&self.active_path, &metadata)
    }

    pub fn mark_running(&self, child_pid: u32, label: &str) -> Result<()> {
        let metadata = ActiveMetadata::running(
            &self.job_id,
            &self.owner,
            std::process::id(),
            child_pid,
            label,
        );
        runtime::write_json_atomic(&self.active_path, &metadata)
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.active_path);
        let _ = runtime::unlock(&self.lock);
    }
}

pub struct Scheduler {
    runtime: Runtime,
    config: Config,
}

impl Scheduler {
    pub fn new(config: Config) -> Result<Self> {
        config.validate()?;
        let runtime = Runtime::initialize(&config)?;
        Ok(Self { runtime, config })
    }

    pub fn acquire(&self, owner: &str) -> Result<Permit> {
        let started = Instant::now();
        let mut announced = false;
        let mut backoff = Duration::from_millis(25);
        let mut lease: Option<WaiterLease> = None;

        loop {
            let queue_lock = self.runtime.open_queue_lock()?;
            runtime::lock(&queue_lock)?;
            self.runtime.prune_stale_waiters()?;
            if self.runtime.is_draining() {
                runtime::unlock(&queue_lock)?;
                return Err(GovError::Temporary(
                    "governor is draining; retry later".into(),
                ));
            }

            let waiters = self.runtime.live_waiters()?;
            let may_try = lease.as_ref().map_or(waiters.is_empty(), |current| {
                waiters.first().is_some_and(|path| path == &current.path)
            });
            if may_try && let Some(permit) = self.try_slots(owner)? {
                if let Some(current) = lease.take() {
                    current.remove();
                }
                permit.mark_starting()?;
                runtime::unlock(&queue_lock)?;
                if announced {
                    eprintln!(
                        "agent-gov: slot acquired after {:.1}s; starting governed workload",
                        started.elapsed().as_secs_f64()
                    );
                }
                return Ok(permit);
            }

            if lease.is_none() {
                self.enforce_admission(owner, &waiters)?;
                let new_lease = WaiterLease::create(&self.runtime, owner)?;
                let position = waiters.len() + 1;
                eprintln!(
                    "agent-gov: heavy slot busy; queued ({}/{}), wait limit {}s",
                    position,
                    self.config.scheduler.max_queue,
                    self.config.scheduler.max_wait.as_secs()
                );
                announced = true;
                lease = Some(new_lease);
            }
            runtime::unlock(&queue_lock)?;

            if started.elapsed() >= self.config.scheduler.max_wait {
                drop(lease.take());
                return Err(GovError::Temporary(format!(
                    "heavy slot unavailable after {}s; command was not started; retry after {}s",
                    self.config.scheduler.max_wait.as_secs(),
                    self.config.scheduler.retry_after.as_secs()
                )));
            }
            let backoff_millis = u64::try_from(backoff.as_millis().max(1)).unwrap_or(500);
            let jitter = rand::rng().random_range(0..=backoff_millis / 4);
            thread::sleep(backoff + Duration::from_millis(jitter));
            backoff = (backoff * 2).min(Duration::from_millis(500));
        }
    }

    fn try_slots(&self, owner: &str) -> Result<Option<Permit>> {
        let capacity = self.runtime.capacity()?;
        let start = (std::process::id() as usize) % capacity;
        for offset in 0..capacity {
            let slot = (start + offset) % capacity;
            if self.runtime.slot_quarantined(slot)? {
                continue;
            }
            let lock = self.runtime.open_slot(slot)?;
            if runtime::try_lock(&lock)? {
                return Ok(Some(Permit {
                    slot,
                    lock,
                    active_path: self.runtime.active_path(slot),
                    job_id: random_id(),
                    owner: owner.to_owned(),
                }));
            }
        }
        Ok(None)
    }

    fn enforce_admission(&self, owner: &str, waiters: &[PathBuf]) -> Result<()> {
        if waiters.len() >= self.config.scheduler.max_queue {
            return Err(GovError::Temporary(format!(
                "heavy queue is full; command was not started; retry after {}s",
                self.config.scheduler.retry_after.as_secs()
            )));
        }
        let owner_count = waiters
            .iter()
            .filter_map(|path| runtime::read_json::<WaiterMetadata>(path).ok())
            .filter(|metadata| metadata.owner == owner)
            .count();
        if owner_count >= self.config.scheduler.max_queued_per_owner {
            return Err(GovError::Temporary(
                "this agent already has a heavy workload queued".into(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub const fn runtime(&self) -> &Runtime {
        &self.runtime
    }
}

#[derive(Debug)]
struct WaiterLease {
    path: PathBuf,
    file: File,
}

impl WaiterLease {
    fn create(runtime: &Runtime, owner: &str) -> Result<Self> {
        let path = runtime.waiters_dir().join(format!(
            "{:020}-{}.lease",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            random_id()
        ));
        let mut file = runtime::create_private_file(&path)?;
        runtime::lock(&file)?;
        let metadata = WaiterMetadata {
            owner: owner.to_owned(),
            pid: std::process::id(),
        };
        serde_json::to_writer(&mut file, &metadata)?;
        file.flush()?;
        Ok(Self { path, file })
    }

    fn remove(self) {
        let _ = runtime::unlock(&self.file);
        let _ = fs::remove_file(&self.path);
    }
}

impl Drop for WaiterLease {
    fn drop(&mut self) {
        let _ = runtime::unlock(&self.file);
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct WaiterMetadata {
    owner: String,
    pid: u32,
}

fn random_id() -> String {
    format!("{:016x}", rand::rng().random::<u64>())
}

pub fn set_drain(runtime: &Runtime, enabled: bool) -> Result<()> {
    let queue = runtime.open_queue_lock()?;
    runtime::lock(&queue)?;
    let result = if enabled {
        let path = runtime.root().join("drain.flag");
        if !path.exists() {
            runtime::create_private_file(&path)?;
        }
        Ok(())
    } else {
        match fs::remove_file(runtime.root().join("drain.flag")) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    };
    runtime::unlock(&queue)?;
    result
}

pub fn set_capacity(runtime: &Runtime, capacity: u8) -> Result<()> {
    set_capacity_transactional(runtime, capacity, || Ok(()))
}

pub fn set_capacity_transactional(
    runtime: &Runtime,
    capacity: u8,
    persist: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if !(1..=2).contains(&capacity) {
        return Err(GovError::InvalidConfig("capacity must be 1 or 2".into()));
    }
    let queue = runtime.open_queue_lock()?;
    runtime::lock(&queue)?;
    let result = (|| {
        if !runtime.is_draining() {
            return Err(GovError::Temporary(
                "capacity changes require drain first".into(),
            ));
        }
        if runtime.active_count()? > 0 || !runtime.live_waiters()?.is_empty() {
            return Err(GovError::Temporary(
                "capacity changes require an idle pool".into(),
            ));
        }
        let previous = runtime.capacity()?;
        runtime::write_atomic(
            &runtime.root().join("capacity"),
            capacity.to_string().as_bytes(),
        )?;
        if let Err(error) = persist() {
            if let Err(rollback_error) = runtime::write_atomic(
                &runtime.root().join("capacity"),
                previous.to_string().as_bytes(),
            ) {
                return Err(GovError::Internal(format!(
                    "configuration write failed ({error}); capacity rollback also failed ({rollback_error})"
                )));
            }
            return Err(error);
        }
        Ok(())
    })();
    let unlock = runtime::unlock(&queue);
    match (result, unlock) {
        (Err(error), _) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}
