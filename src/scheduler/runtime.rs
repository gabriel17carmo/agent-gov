use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io::Write,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use fs4::{FileExt, TryLockError};
use nix::{
    fcntl::{OFlag, open},
    sys::{signal, stat::Mode},
    unistd::{Pid, Uid},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{
    config::{Config, runtime_dir},
    error::{GovError, Result},
};

#[derive(Clone, Debug)]
pub struct Runtime {
    root: PathBuf,
    control_dir: PathBuf,
    control_namespace: String,
}

#[derive(Clone, Debug)]
pub struct FilesystemStatus {
    runtime_path: PathBuf,
    filesystem_type: Option<String>,
    enforced: bool,
    supported: bool,
}

impl FilesystemStatus {
    #[must_use]
    pub const fn supported(&self) -> bool {
        self.supported
    }

    #[must_use]
    pub fn detail(&self) -> String {
        if !self.enforced {
            return format!(
                "{}: local-filesystem enforcement applies on macOS",
                self.runtime_path.display()
            );
        }
        let filesystem = self.filesystem_type.as_deref().unwrap_or("unknown");
        if self.supported {
            format!(
                "{} is on local filesystem {filesystem}",
                self.runtime_path.display()
            )
        } else {
            format!(
                "{} is on unsupported non-local filesystem {filesystem}",
                self.runtime_path.display()
            )
        }
    }

    fn require_supported(&self) -> Result<()> {
        if self.supported {
            return Ok(());
        }
        Err(GovError::Runtime(format!(
            "{}; scheduler admission was refused; use a local macOS volume (the default is ~/Library/Application Support/agent-gov)",
            self.detail()
        )))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ActiveMetadata {
    pub schema_version: u8,
    pub state: String,
    pub job_id: String,
    pub owner: String,
    pub supervisor_pid: u32,
    pub child_pid: Option<u32>,
    pub label: String,
}

impl ActiveMetadata {
    #[must_use]
    pub fn starting(job_id: &str, owner: &str, supervisor_pid: u32) -> Self {
        Self {
            schema_version: 1,
            state: "starting".into(),
            job_id: job_id.into(),
            owner: owner.into(),
            supervisor_pid,
            child_pid: None,
            label: "heavy".into(),
        }
    }

    #[must_use]
    pub fn running(
        job_id: &str,
        owner: &str,
        supervisor_pid: u32,
        child_pid: u32,
        label: &str,
    ) -> Self {
        Self {
            schema_version: 1,
            state: "running".into(),
            job_id: job_id.into(),
            owner: owner.into(),
            supervisor_pid,
            child_pid: Some(child_pid),
            label: label.into(),
        }
    }
}

impl Runtime {
    pub fn filesystem_status() -> Result<FilesystemStatus> {
        inspect_runtime_filesystem(&runtime_dir()?)
    }

    pub fn initialize(config: &Config) -> Result<Self> {
        let root = runtime_dir()?;
        inspect_runtime_filesystem(&root)?.require_supported()?;
        let app_dir = root
            .parent()
            .ok_or_else(|| GovError::Internal("runtime root has no parent".into()))?;
        create_private_dir_all(app_dir)?;
        create_private_dir(&root)?;
        inspect_runtime_filesystem(&root)?.require_supported()?;
        for child in ["slots", "active", "waiters", "cooldowns"] {
            create_private_dir(&root.join(child))?;
        }
        for path in [
            root.join("queue.lock"),
            root.join("slots/slot-0.lock"),
            root.join("slots/slot-1.lock"),
        ] {
            open_private_file(&path)?;
        }
        let schema = root.join("schema-version");
        if !schema.exists() {
            write_atomic(&schema, b"1")?;
        }
        let capacity = root.join("capacity");
        if !capacity.exists() {
            write_atomic(&capacity, config.scheduler.capacity.to_string().as_bytes())?;
        }
        let (control_dir, control_namespace) = initialize_control_namespace(&root)?;
        let runtime = Self {
            root,
            control_dir,
            control_namespace,
        };
        runtime.validate()?;
        Ok(runtime)
    }

    pub fn validate(&self) -> Result<()> {
        inspect_runtime_filesystem(&self.root)?.require_supported()?;
        let metadata = fs::symlink_metadata(&self.root)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(GovError::Runtime(
                "runtime root is not a real directory".into(),
            ));
        }
        if metadata.uid() != Uid::current().as_raw() {
            return Err(GovError::Runtime(
                "runtime directory is owned by another user".into(),
            ));
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(GovError::Runtime(
                "runtime directory permissions must be 0700".into(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn waiters_dir(&self) -> PathBuf {
        self.root.join("waiters")
    }

    pub fn open_queue_lock(&self) -> Result<File> {
        open_private_file(&self.root.join("queue.lock"))
    }

    pub fn open_slot(&self, slot: usize) -> Result<File> {
        if slot >= 2 {
            return Err(GovError::Internal(format!("invalid slot {slot}")));
        }
        open_private_file(&self.root.join(format!("slots/slot-{slot}.lock")))
    }

    #[must_use]
    pub fn active_path(&self, slot: usize) -> PathBuf {
        self.root.join(format!("active/slot-{slot}.json"))
    }

    pub fn control_path(&self, job_id: &str) -> Result<PathBuf> {
        if job_id.len() != 16 || !job_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(GovError::Runtime(
                "invalid job id in runtime metadata".into(),
            ));
        }
        Ok(self
            .control_dir
            .join(format!("{}-{job_id}.sock", self.control_namespace)))
    }

    pub fn capacity(&self) -> Result<usize> {
        let value = fs::read_to_string(self.root.join("capacity"))?;
        let capacity: usize = value
            .trim()
            .parse()
            .map_err(|_| GovError::Runtime("invalid runtime capacity".into()))?;
        if !(1..=2).contains(&capacity) {
            return Err(GovError::Runtime("runtime capacity must be 1 or 2".into()));
        }
        Ok(capacity)
    }

    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.root.join("drain.flag").exists()
    }

    pub fn live_waiters(&self) -> Result<Vec<PathBuf>> {
        let mut paths: Vec<PathBuf> = fs::read_dir(self.waiters_dir())?
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "lease")
            })
            .collect();
        paths.sort();
        Ok(paths)
    }

    pub fn prune_stale_waiters(&self) -> Result<()> {
        for path in self.live_waiters()? {
            let file = open_private_file(&path)?;
            if try_lock(&file)? {
                let _ = unlock(&file);
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    pub fn slot_quarantined(&self, slot: usize) -> Result<bool> {
        let path = self.active_path(slot);
        if !path.exists() {
            return Ok(false);
        }
        let metadata: ActiveMetadata = match read_json(&path) {
            Ok(metadata) => metadata,
            Err(_) => return Ok(true),
        };
        if process_alive(metadata.supervisor_pid) || metadata.child_pid.is_some_and(process_alive) {
            return Ok(true);
        }
        if let Ok(control_path) = self.control_path(&metadata.job_id) {
            let _ = fs::remove_file(control_path);
        }
        fs::remove_file(path)?;
        Ok(false)
    }

    pub fn active_count(&self) -> Result<usize> {
        Ok((0..2)
            .filter(|slot| self.active_path(*slot).exists())
            .count())
    }

    pub fn active_metadata(&self, slot: usize) -> Result<Option<ActiveMetadata>> {
        let path = self.active_path(slot);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(read_json(&path)?))
    }

    pub fn slot_locked(&self, slot: usize) -> Result<bool> {
        let file = self.open_slot(slot)?;
        if try_lock(&file)? {
            unlock(&file)?;
            Ok(false)
        } else {
            Ok(true)
        }
    }
}

fn inspect_runtime_filesystem(path: &Path) -> Result<FilesystemStatus> {
    // Hosted integration tests cannot mount remote filesystems. Release builds omit this boundary
    // injection; it is accepted only with the existing isolated test-home override.
    #[cfg(debug_assertions)]
    if std::env::var_os("AGENT_GOV_TEST_HOME").is_some()
        && let Some(value) = std::env::var_os("AGENT_GOV_TEST_FILESYSTEM")
    {
        return match value.to_str() {
            Some("local") => Ok(filesystem_status(path, "test-local", true, true)),
            Some("remote") => Ok(filesystem_status(path, "test-remote", true, false)),
            _ => Err(GovError::Internal(
                "AGENT_GOV_TEST_FILESYSTEM must be local or remote".into(),
            )),
        };
    }

    #[cfg(target_os = "macos")]
    {
        use nix::{mount::MntFlags, sys::statfs::statfs};

        let ancestor = nearest_existing_ancestor(path)?;
        let statistics = statfs(&ancestor).map_err(errno_to_io)?;
        return Ok(filesystem_status(
            path,
            statistics.filesystem_type_name(),
            true,
            statistics.flags().contains(MntFlags::MNT_LOCAL),
        ));
    }

    #[cfg(not(target_os = "macos"))]
    Ok(FilesystemStatus {
        runtime_path: path.to_path_buf(),
        filesystem_type: None,
        enforced: false,
        supported: true,
    })
}

fn filesystem_status(
    path: &Path,
    filesystem_type: &str,
    enforced: bool,
    supported: bool,
) -> FilesystemStatus {
    FilesystemStatus {
        runtime_path: path.to_path_buf(),
        filesystem_type: Some(filesystem_type.to_owned()),
        enforced,
        supported,
    }
}

#[cfg(target_os = "macos")]
fn nearest_existing_ancestor(path: &Path) -> Result<PathBuf> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => return Ok(ancestor.to_path_buf()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(GovError::Runtime(format!(
        "cannot find an existing ancestor for runtime path {}",
        path.display()
    )))
}

#[must_use]
pub fn process_alive(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    signal::kill(Pid::from_raw(raw), None).is_ok()
}

pub fn create_private_file(path: &Path) -> Result<File> {
    let descriptor = open(
        path,
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(errno_to_io)?;
    Ok(File::from(descriptor))
}

pub fn lock(file: &File) -> Result<()> {
    FileExt::lock(file)?;
    Ok(())
}

pub fn try_lock(file: &File) -> Result<bool> {
    match FileExt::try_lock(file) {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

pub fn unlock(file: &File) -> Result<()> {
    FileExt::unlock(file)?;
    Ok(())
}

fn errno_to_io(error: nix::errno::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(error as i32)
}

fn open_private_file(path: &Path) -> Result<File> {
    let descriptor = open(
        path,
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(errno_to_io)?;
    let file = File::from(descriptor);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != Uid::current().as_raw()
    {
        return Err(GovError::Runtime(format!(
            "unsafe runtime file {}",
            path.display()
        )));
    }
    Ok(file)
}

fn create_private_dir(path: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != Uid::current().as_raw()
    {
        return Err(GovError::Runtime(format!(
            "unsafe runtime directory {}",
            path.display()
        )));
    }
    Ok(())
}

fn create_private_dir_all(path: &Path) -> Result<()> {
    if path.exists() {
        return validate_parent_dir(path);
    }
    let parent = path
        .parent()
        .ok_or_else(|| GovError::Internal("private directory has no parent".into()))?;
    if parent.exists() {
        validate_parent_dir(parent)?;
    } else {
        create_private_dir_all(parent)?;
    }
    create_private_dir(path)
}

fn initialize_control_namespace(runtime_root: &Path) -> Result<(PathBuf, String)> {
    let control_dir =
        PathBuf::from("/tmp").join(format!("agent-gov-control-{}", Uid::current().as_raw()));
    create_private_dir(&control_dir)?;

    let canonical_root = fs::canonicalize(runtime_root)?;
    let digest = Sha256::digest(canonical_root.as_os_str().as_bytes());
    let mut namespace = String::with_capacity(16);
    for byte in &digest[..8] {
        use std::fmt::Write as _;
        write!(&mut namespace, "{byte:02x}")
            .map_err(|_| GovError::Internal("cannot derive control namespace".into()))?;
    }
    Ok((control_dir, namespace))
}

fn validate_parent_dir(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != Uid::current().as_raw()
    {
        return Err(GovError::Runtime(format!(
            "unsafe parent directory {}",
            path.display()
        )));
    }
    Ok(())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| GovError::Internal("atomic write has no parent".into()))?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_atomic(path, &serde_json::to_vec(value)?)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path)?;
    if bytes.len() > 64 * 1024 {
        return Err(GovError::Runtime("runtime metadata exceeds 64 KiB".into()));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::filesystem_status;

    #[test]
    fn local_filesystem_policy_accepts_kernel_local_mounts() {
        let status = filesystem_status(Path::new("/runtime"), "apfs", true, true);

        assert!(status.supported());
        assert!(status.require_supported().is_ok());
        assert!(status.detail().contains("local filesystem apfs"));
    }

    #[test]
    fn local_filesystem_policy_rejects_non_local_mounts() {
        let status = filesystem_status(Path::new("/runtime"), "smbfs", true, false);

        assert!(!status.supported());
        let error = status.require_supported().expect_err("remote mount");
        assert!(
            error
                .to_string()
                .contains("unsupported non-local filesystem smbfs")
        );
        assert!(
            error
                .to_string()
                .contains("scheduler admission was refused")
        );
    }
}
