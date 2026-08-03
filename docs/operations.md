# Operations and recovery

## Normal checks

Run after installation, an agent update, an RTK update, or a binary replacement:

```bash
agent-gov doctor
agent-gov status
```

`doctor` exits `0` when healthy, `1` for warnings, and `2` when enforcement cannot be trusted.

## Runtime filesystem policy

On macOS, keep the runtime at the default local path under
`~/Library/Application Support/agent-gov`. Before scheduler admission, Agent Governor asks the
kernel whether the containing filesystem has `MNT_LOCAL`. Volumes without that flag—including normal
SMB, NFS, WebDAV, and user-space network mounts—are rejected with exit `69` (`EX_UNAVAILABLE`), and
the workload is not started. Local APFS/HFS volumes and local external disks remain supported when
macOS marks them local.

`agent-gov doctor` reports the filesystem type and locality decision. If it reports a non-local
runtime, move the state back to the default local Application Support path; do not bypass the
governor or place its stable lock files on a share. The policy trusts the macOS kernel's mount flag,
so unusual third-party filesystems should also be validated on a physical Mac before production use.

## Queue pressure

Queue full or wait timeout returns exit 75 and does not start the workload. Agents should wait for
the printed `retry-after` period before retrying. Do not wrap the command in a fallback that bypasses
`agent-gov`.

## Capacity change

```bash
agent-gov config set-capacity 2 --drain
```

The operation blocks new admissions, requires an idle pool, writes the runtime snapshot and config
as one transaction, then resumes admission. A failed config write restores the previous runtime
capacity and the prior drain state. If it fails while busy, wait for active/waiting work to finish and
retry.

## Cancellation

Use the opaque ID from `status`:

```bash
agent-gov cancel a7c2...
```

Cancellation resolves exactly one active slot, validates that its lock is held, and sends the
request through a private per-job Unix socket. It never signals a PID obtained from metadata and
refuses stale, unavailable, or ambiguous control endpoints.

## Crash recovery

Kernel locks close automatically when the supervisor dies. If the child remains alive, active
metadata quarantines that slot. In the public preview, inspect the listed PID before taking manual
action. Never delete stable slot lock files. Identity-verified termination of a live orphan remains
disabled until it is implemented and validated on a physical Mac.

## Uninstall

```bash
agent-gov drain
agent-gov status
agent-gov uninstall --agents claude,cursor
```

If settings still match the installed snapshot, uninstall restores their exact original bytes (or
removes a file that did not previously exist). If settings changed later, it removes only the exact
managed hook and preserves those changes. Backup files remain available for recovery.
