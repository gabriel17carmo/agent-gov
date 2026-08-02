//! Transactional hook installation.

use std::{
    env, fs,
    fs::{File, OpenOptions},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use fs4::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    error::{GovError, Result},
    scheduler::write_private_atomic,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Agent {
    Claude,
    Cursor,
}

impl Agent {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor",
        }
    }
}

pub fn install(agents: &[Agent], binary: &Path, rtk: Option<&Path>) -> Result<Vec<PathBuf>> {
    validate_executable_path(binary)?;
    if let Some(path) = rtk {
        validate_executable_path(path)?;
    }
    let home = user_home()?;
    let _lock = install_lock(&home)?;
    let mut prepared = Vec::with_capacity(agents.len());
    for agent in agents {
        let path = settings_path(&home, *agent);
        ensure_parent(&path)?;
        let (original_bytes, original) = read_json_or_empty(&path)?;
        let updated = match agent {
            Agent::Claude => patch_claude(original, binary, rtk)?,
            Agent::Cursor => patch_cursor(original, binary, rtk)?,
        };
        let updated_bytes = serde_json::to_vec_pretty(&updated)?;
        let (original_manifest_bytes, previous_manifest) = read_manifest(&path)?;
        let manifest = install_manifest(
            &path,
            original_bytes.as_deref(),
            previous_manifest.as_ref(),
            &updated_bytes,
            binary,
        )?;
        prepared.push(PreparedSettings {
            path,
            original_bytes,
            updated_bytes: Some(updated_bytes),
            original_manifest_bytes,
            updated_manifest_bytes: Some(serde_json::to_vec_pretty(&manifest)?),
        });
    }
    commit_settings(&prepared, true)
}

pub fn uninstall(agents: &[Agent], binary: &Path) -> Result<Vec<PathBuf>> {
    let home = user_home()?;
    let _lock = install_lock(&home)?;
    let mut prepared = Vec::with_capacity(agents.len());
    for agent in agents {
        let path = settings_path(&home, *agent);
        if !path.exists() {
            continue;
        }
        let (original_bytes, original) = read_json_or_empty(&path)?;
        let (original_manifest_bytes, manifest) = read_manifest(&path)?;
        let installed_binary = manifest
            .as_ref()
            .map_or_else(|| Ok(binary.to_path_buf()), manifest_binary)?;
        let surgical = match agent {
            Agent::Claude => unpatch_claude(original, &installed_binary),
            Agent::Cursor => unpatch_cursor(original, &installed_binary),
        };
        let updated_bytes =
            match uninstall_action(&path, original_bytes.as_deref(), manifest.as_ref())? {
                UninstallAction::Surgical => Some(serde_json::to_vec_pretty(&surgical)?),
                UninstallAction::Restore(bytes) => Some(bytes),
                UninstallAction::Remove => None,
            };
        prepared.push(PreparedSettings {
            path,
            original_bytes,
            updated_bytes,
            original_manifest_bytes,
            updated_manifest_bytes: None,
        });
    }
    commit_settings(&prepared, false)
}

struct PreparedSettings {
    path: PathBuf,
    original_bytes: Option<Vec<u8>>,
    updated_bytes: Option<Vec<u8>>,
    original_manifest_bytes: Option<Vec<u8>>,
    updated_manifest_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InstallManifest {
    schema_version: u8,
    original_existed: bool,
    original_sha256: String,
    installed_sha256: String,
    binary_path: String,
}

enum UninstallAction {
    Surgical,
    Restore(Vec<u8>),
    Remove,
}

fn commit_settings(prepared: &[PreparedSettings], create_backup: bool) -> Result<Vec<PathBuf>> {
    let mut committed = Vec::new();
    for settings in prepared {
        let result = (|| {
            if create_backup {
                backup_once(&settings.path, settings.original_bytes.as_deref())?;
            }
            apply_optional(&settings.path, settings.updated_bytes.as_deref())?;
            apply_optional(
                &manifest_path(&settings.path),
                settings.updated_manifest_bytes.as_deref(),
            )
        })();
        if let Err(error) = result {
            let mut rollback_paths = committed.clone();
            rollback_paths.push(settings.path.clone());
            if let Err(rollback_error) = rollback_settings(prepared, &rollback_paths) {
                return Err(GovError::Internal(format!(
                    "settings update failed ({error}); rollback also failed ({rollback_error})"
                )));
            }
            return Err(error);
        }
        committed.push(settings.path.clone());
    }
    Ok(committed)
}

fn rollback_settings(prepared: &[PreparedSettings], committed: &[PathBuf]) -> Result<()> {
    for path in committed.iter().rev() {
        let settings = prepared
            .iter()
            .find(|settings| settings.path == *path)
            .ok_or_else(|| GovError::Internal("missing rollback snapshot".into()))?;
        match &settings.original_bytes {
            Some(bytes) => write_private_atomic(path, bytes)?,
            None => remove_if_exists(path)?,
        }
        apply_optional(
            &manifest_path(path),
            settings.original_manifest_bytes.as_deref(),
        )?;
    }
    Ok(())
}

fn apply_optional(path: &Path, bytes: Option<&[u8]>) -> Result<()> {
    match bytes {
        Some(bytes) => write_private_atomic(path, bytes),
        None => remove_if_exists(path),
    }
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn install_manifest(
    path: &Path,
    current: Option<&[u8]>,
    previous: Option<&InstallManifest>,
    updated: &[u8],
    binary: &Path,
) -> Result<InstallManifest> {
    let (original_existed, original_sha256) = if let Some(previous) = previous {
        verify_backup(path, previous)?;
        (previous.original_existed, previous.original_sha256.clone())
    } else {
        let expected = hash_hex(current.unwrap_or_default());
        let backup = backup_path(path);
        if backup.exists() && hash_hex(&fs::read(&backup)?) != expected {
            return Err(GovError::InvalidConfig(format!(
                "install backup {} has no matching manifest",
                backup.display()
            )));
        }
        (current.is_some(), expected)
    };
    Ok(InstallManifest {
        schema_version: 1,
        original_existed,
        original_sha256,
        installed_sha256: hash_hex(updated),
        binary_path: path_string(binary)?,
    })
}

fn uninstall_action(
    path: &Path,
    current: Option<&[u8]>,
    manifest: Option<&InstallManifest>,
) -> Result<UninstallAction> {
    let Some(manifest) = manifest else {
        return Ok(UninstallAction::Surgical);
    };
    if hash_hex(current.unwrap_or_default()) != manifest.installed_sha256 {
        return Ok(UninstallAction::Surgical);
    }
    let backup = verify_backup(path, manifest)?;
    if manifest.original_existed {
        Ok(UninstallAction::Restore(backup))
    } else {
        Ok(UninstallAction::Remove)
    }
}

fn verify_backup(path: &Path, manifest: &InstallManifest) -> Result<Vec<u8>> {
    let backup_path = backup_path(path);
    let backup = fs::read(&backup_path).map_err(|error| {
        GovError::InvalidConfig(format!(
            "cannot read install backup {}: {error}",
            backup_path.display()
        ))
    })?;
    if hash_hex(&backup) != manifest.original_sha256 {
        return Err(GovError::InvalidConfig(format!(
            "install backup {} does not match its manifest",
            backup_path.display()
        )));
    }
    Ok(backup)
}

fn read_manifest(path: &Path) -> Result<(Option<Vec<u8>>, Option<InstallManifest>)> {
    let path = manifest_path(path);
    if !path.exists() {
        return Ok((None, None));
    }
    let bytes = fs::read(&path)?;
    if bytes.len() > 64 * 1024 {
        return Err(GovError::InvalidConfig(format!(
            "install manifest {} exceeds 64 KiB",
            path.display()
        )));
    }
    let manifest: InstallManifest = serde_json::from_slice(&bytes)?;
    if manifest.schema_version != 1 {
        return Err(GovError::InvalidConfig(format!(
            "unsupported install manifest at {}",
            path.display()
        )));
    }
    Ok((Some(bytes), Some(manifest)))
}

fn manifest_binary(manifest: &InstallManifest) -> Result<PathBuf> {
    let path = PathBuf::from(&manifest.binary_path);
    if !path.is_absolute() {
        return Err(GovError::InvalidConfig(
            "install manifest contains a non-absolute binary path".into(),
        ));
    }
    Ok(path)
}

pub fn installed_binary_path(settings: &Path) -> Result<Option<PathBuf>> {
    read_manifest(settings)?
        .1
        .as_ref()
        .map(manifest_binary)
        .transpose()
}

fn hash_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.agent-gov-backup")
}

fn manifest_path(path: &Path) -> PathBuf {
    path.with_extension("json.agent-gov-manifest")
}

#[must_use]
pub fn settings_path(home: &Path, agent: Agent) -> PathBuf {
    match agent {
        Agent::Claude => home.join(".claude/settings.json"),
        Agent::Cursor => home.join(".cursor/hooks.json"),
    }
}

fn patch_claude(mut root: Value, binary: &Path, rtk: Option<&Path>) -> Result<Value> {
    let object = object_mut(&mut root)?;
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = object_mut(hooks)?;
    let groups = hooks
        .entry("PreToolUse")
        .or_insert_with(|| Value::Array(Vec::new()));
    let groups = array_mut(groups)?;
    retain_claude_hooks(groups, |hook| {
        !is_agent_gov_claude_hook(hook, binary)
            && (rtk.is_none() || !is_known_rtk_claude_hook(hook))
    });

    let mut args = vec![json!("hook"), json!("claude")];
    if let Some(path) = rtk {
        args.push(json!("--rtk"));
        args.push(json!(path_string(path)?));
    }
    groups.push(json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": path_string(binary)?,
            "args": args,
            "timeout": 5
        }]
    }));
    Ok(root)
}

fn patch_cursor(mut root: Value, binary: &Path, rtk: Option<&Path>) -> Result<Value> {
    let object = object_mut(&mut root)?;
    match object.get("version") {
        Some(version) if version.as_u64() != Some(1) => {
            return Err(GovError::InvalidConfig(
                "unsupported Cursor hook schema version; expected 1".into(),
            ));
        }
        Some(_) => {}
        None => {
            object.insert("version".into(), json!(1));
        }
    }
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = object_mut(hooks)?;
    let groups = hooks
        .entry("preToolUse")
        .or_insert_with(|| Value::Array(Vec::new()));
    let groups = array_mut(groups)?;
    let binary_text = path_string(binary)?;
    groups.retain(|group| {
        let command = group
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        !is_agent_gov_cursor(command, binary) && (rtk.is_none() || !is_known_rtk_cursor(command))
    });
    let mut command = format!("{} hook cursor", quote_posix(&binary_text));
    if let Some(path) = rtk {
        command.push_str(" --rtk ");
        command.push_str(&quote_posix(&path_string(path)?));
    }
    groups.push(json!({"command": command, "matcher": "Shell"}));
    Ok(root)
}

fn unpatch_claude(mut root: Value, binary: &Path) -> Value {
    if let Some(groups) = root
        .pointer_mut("/hooks/PreToolUse")
        .and_then(Value::as_array_mut)
    {
        retain_claude_hooks(groups, |hook| !is_agent_gov_claude_hook(hook, binary));
    }
    root
}

fn unpatch_cursor(mut root: Value, binary: &Path) -> Value {
    if let Some(groups) = root
        .pointer_mut("/hooks/preToolUse")
        .and_then(Value::as_array_mut)
    {
        groups.retain(|group| {
            !group
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| is_agent_gov_cursor(command, binary))
        });
    }
    root
}

fn retain_claude_hooks(groups: &mut Vec<Value>, mut keep: impl FnMut(&Value) -> bool) {
    groups.retain_mut(|group| {
        let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let was_nonempty = !hooks.is_empty();
        hooks.retain(&mut keep);
        !was_nonempty || !hooks.is_empty()
    });
}

fn is_agent_gov_claude_hook(hook: &Value, binary: &Path) -> bool {
    hook.get("command").and_then(Value::as_str) == binary.to_str()
        && hook
            .get("args")
            .and_then(Value::as_array)
            .is_some_and(|args| {
                args.first().and_then(Value::as_str) == Some("hook")
                    && args.get(1).and_then(Value::as_str) == Some("claude")
            })
}

fn is_valid_agent_gov_claude_hook(hook: &Value, binary: &Path) -> bool {
    is_agent_gov_claude_hook(hook, binary)
        && hook.get("type").and_then(Value::as_str) == Some("command")
        && hook.get("timeout").and_then(Value::as_u64) == Some(5)
}

fn is_known_rtk_claude_hook(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| is_known_rtk_command(command, "claude"))
}

fn is_known_rtk_cursor(command: &str) -> bool {
    is_known_rtk_command(command, "cursor")
}

fn is_known_rtk_command(command: &str, host: &str) -> bool {
    shlex::split(command).is_some_and(|argv| {
        let executable = argv
            .first()
            .and_then(|value| Path::new(value).file_name())
            .and_then(|value| value.to_str());
        executable == Some("rtk-rewrite.sh")
            || executable == Some("rtk")
                && argv.len() == 3
                && argv.get(1).map(String::as_str) == Some("hook")
                && argv.get(2).map(String::as_str) == Some(host)
    })
}

fn is_agent_gov_cursor(command: &str, binary: &Path) -> bool {
    shlex::split(command).is_some_and(|argv| {
        argv.first().map(String::as_str) == binary.to_str()
            && argv.get(1).map(String::as_str) == Some("hook")
            && argv.get(2).map(String::as_str) == Some("cursor")
    })
}

#[must_use]
pub fn managed_hook_count(root: &Value, agent: Agent, binary: &Path) -> usize {
    match agent {
        Agent::Claude => root
            .pointer("/hooks/PreToolUse")
            .and_then(Value::as_array)
            .map_or(0, |groups| {
                groups
                    .iter()
                    .filter(|group| group.get("matcher").and_then(Value::as_str) == Some("Bash"))
                    .filter_map(|group| group.get("hooks").and_then(Value::as_array))
                    .flatten()
                    .filter(|hook| is_valid_agent_gov_claude_hook(hook, binary))
                    .count()
            }),
        Agent::Cursor => root
            .pointer("/hooks/preToolUse")
            .and_then(Value::as_array)
            .map_or(0, |groups| {
                groups
                    .iter()
                    .filter(|group| group.get("matcher").and_then(Value::as_str) == Some("Shell"))
                    .filter(|group| {
                        group
                            .get("command")
                            .and_then(Value::as_str)
                            .is_some_and(|command| is_agent_gov_cursor(command, binary))
                    })
                    .count()
            }),
    }
}

#[must_use]
pub fn separate_rtk_hook_count(root: &Value, agent: Agent) -> usize {
    match agent {
        Agent::Claude => root
            .pointer("/hooks/PreToolUse")
            .and_then(Value::as_array)
            .map_or(0, |groups| {
                groups
                    .iter()
                    .filter_map(|group| group.get("hooks").and_then(Value::as_array))
                    .flatten()
                    .filter(|hook| is_known_rtk_claude_hook(hook))
                    .count()
            }),
        Agent::Cursor => root
            .pointer("/hooks/preToolUse")
            .and_then(Value::as_array)
            .map_or(0, |groups| {
                groups
                    .iter()
                    .filter(|group| {
                        group
                            .get("command")
                            .and_then(Value::as_str)
                            .is_some_and(is_known_rtk_cursor)
                    })
                    .count()
            }),
    }
}

#[must_use]
pub fn managed_rtk_paths(root: &Value, agent: Agent, binary: &Path) -> Vec<PathBuf> {
    match agent {
        Agent::Claude => root
            .pointer("/hooks/PreToolUse")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|group| group.get("hooks").and_then(Value::as_array))
            .flatten()
            .filter(|hook| is_agent_gov_claude_hook(hook, binary))
            .filter_map(|hook| hook.get("args").and_then(Value::as_array))
            .filter_map(|args| {
                args.windows(2).find_map(|pair| {
                    (pair[0].as_str() == Some("--rtk"))
                        .then(|| pair[1].as_str().map(PathBuf::from))
                        .flatten()
                })
            })
            .collect(),
        Agent::Cursor => root
            .pointer("/hooks/preToolUse")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|group| group.get("command").and_then(Value::as_str))
            .filter(|command| is_agent_gov_cursor(command, binary))
            .filter_map(shlex::split)
            .filter_map(|args| {
                args.windows(2)
                    .find_map(|pair| (pair[0] == "--rtk").then(|| PathBuf::from(&pair[1])))
            })
            .collect(),
    }
}

fn read_json_or_empty(path: &Path) -> Result<(Option<Vec<u8>>, Value)> {
    if !path.exists() {
        return Ok((None, Value::Object(Map::new())));
    }
    let bytes = fs::read(path)?;
    if bytes.len() > 1024 * 1024 {
        return Err(GovError::InvalidConfig(format!(
            "settings file {} exceeds 1 MiB",
            path.display()
        )));
    }
    let root: Value = serde_json::from_slice(&bytes)?;
    if !root.is_object() {
        return Err(GovError::InvalidConfig(format!(
            "settings file {} must contain a JSON object",
            path.display()
        )));
    }
    Ok((Some(bytes), root))
}

fn backup_once(path: &Path, original: Option<&[u8]>) -> Result<()> {
    let backup = backup_path(path);
    if backup.exists() {
        return Ok(());
    }
    let bytes = original.unwrap_or_default();
    write_private_atomic(&backup, bytes)?;
    let hash = Sha256::digest(bytes);
    write_private_atomic(
        &path.with_extension("json.agent-gov-backup.sha256"),
        format!("{hash:x}\n").as_bytes(),
    )
}

fn ensure_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| GovError::Internal("settings path has no parent".into()))?;
    fs::create_dir_all(parent)?;
    if fs::metadata(parent)?.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn install_lock(home: &Path) -> Result<File> {
    let directory = home.join(".agent-gov");
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(directory.join("install.lock"))?;
    FileExt::lock(&file)?;
    Ok(file)
}

fn validate_executable_path(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.to_string_lossy().contains(['\0', '\n']) {
        return Err(GovError::InvalidInput(
            "hook executable paths must be absolute and single-line".into(),
        ));
    }
    let metadata = fs::metadata(path).map_err(|error| {
        GovError::InvalidInput(format!(
            "hook executable {} is unavailable: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(GovError::InvalidInput(format!(
            "hook executable {} is not an executable file",
            path.display()
        )));
    }
    Ok(())
}

fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| GovError::InvalidInput("path is not UTF-8".into()))
}

fn object_mut(value: &mut Value) -> Result<&mut Map<String, Value>> {
    value
        .as_object_mut()
        .ok_or_else(|| GovError::InvalidConfig("expected JSON object".into()))
}

fn array_mut(value: &mut Value) -> Result<&mut Vec<Value>> {
    value
        .as_array_mut()
        .ok_or_else(|| GovError::InvalidConfig("expected JSON array".into()))
}

fn quote_posix(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn user_home() -> Result<PathBuf> {
    env::var_os("AGENT_GOV_TEST_USER_HOME")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| GovError::Runtime("HOME is not defined".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_is_idempotent_and_preserves_unrelated_hooks() {
        let original = json!({"hooks":{"PreToolUse":[{"matcher":"Read","hooks":[]}]}});
        let once =
            patch_claude(original, Path::new("/opt/agent gov/agent-gov"), None).expect("patch");
        let twice = patch_claude(once, Path::new("/opt/agent gov/agent-gov"), None).expect("patch");
        let groups = twice
            .pointer("/hooks/PreToolUse")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["matcher"], "Read");
    }

    #[test]
    fn cursor_path_is_posix_quoted() {
        let value =
            patch_cursor(json!({}), Path::new("/opt/it's here/agent-gov"), None).expect("patch");
        assert_eq!(
            value["hooks"]["preToolUse"][0]["command"],
            "'/opt/it'\\''s here/agent-gov' hook cursor"
        );
    }

    #[test]
    fn cursor_unknown_schema_is_rejected_before_editing() {
        let error = patch_cursor(
            json!({"version":2,"hooks":{"preToolUse":[]}}),
            Path::new("/opt/agent-gov"),
            None,
        )
        .expect_err("unsupported schema");
        assert!(error.to_string().contains("schema version"));
    }

    #[test]
    fn install_only_replaces_separate_rtk_hook_when_composition_is_requested() {
        let original = json!({"hooks":{"PreToolUse":[{
            "matcher":"Bash",
            "hooks":[{"type":"command","command":"/opt/homebrew/bin/rtk hook claude"}]
        }]}});
        let preserved = patch_claude(original.clone(), Path::new("/opt/agent-gov"), None)
            .expect("patch without RTK");
        assert_eq!(separate_rtk_hook_count(&preserved, Agent::Claude), 1);

        let composed = patch_claude(
            original,
            Path::new("/opt/agent-gov"),
            Some(Path::new("/opt/rtk")),
        )
        .expect("patch with RTK");
        assert_eq!(separate_rtk_hook_count(&composed, Agent::Claude), 0);
        assert_eq!(
            managed_hook_count(&composed, Agent::Claude, Path::new("/opt/agent-gov")),
            1
        );
    }

    #[test]
    fn cursor_uninstall_matches_tokenized_command_not_substrings() {
        let original = json!({"version":1,"hooks":{"preToolUse":[
            {"matcher":"Shell","command":"'/opt/agent-gov' hook cursor"},
            {"matcher":"Shell","command":"'/opt/agent-gov-helper' hook cursor"}
        ]}});
        let updated = unpatch_cursor(original, Path::new("/opt/agent-gov"));
        let hooks = updated["hooks"]["preToolUse"].as_array().expect("hooks");
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], "'/opt/agent-gov-helper' hook cursor");
    }

    #[test]
    fn claude_surgical_uninstall_preserves_hooks_in_the_same_group() {
        let original = json!({"hooks":{"PreToolUse":[{
            "matcher":"Bash",
            "hooks":[
                {"type":"command","command":"/opt/agent-gov","args":["hook","claude"]},
                {"type":"command","command":"/opt/other-hook"}
            ]
        }]}});
        let updated = unpatch_claude(original, Path::new("/opt/agent-gov"));
        let hooks = updated["hooks"]["PreToolUse"][0]["hooks"]
            .as_array()
            .expect("hooks");
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], "/opt/other-hook");
    }
}
