# Operations and recovery

## Normal checks

Run after installation, an agent update, an RTK update, or a binary replacement:

```bash
agent-gov doctor
agent-gov status
```

`doctor` exits `0` when healthy, `1` for warnings, and `2` when enforcement cannot be trusted.

## Queue pressure

Queue full or wait timeout returns exit 75 and does not start the workload. Agents should wait for
the printed `retry-after` period before retrying. Do not wrap the command in a fallback that bypasses
`agent-gov`.

## Capacity change

```bash
agent-gov config set-capacity 2 --drain
```

The operation blocks new admissions, requires an idle pool, writes the runtime snapshot and config
atomically, then resumes admission. If it fails while busy, wait for active/waiting work to finish and
retry.

## Cancellation

Use the opaque ID from `status`:

```bash
agent-gov cancel a7c2...
```

Cancellation resolves exactly one active slot and validates that the supervisor still owns its lock.
It refuses stale or ambiguous metadata.

## Crash recovery

Kernel locks close automatically when the supervisor dies. If the child remains alive, active
metadata quarantines that slot. In the public preview, inspect the listed PID before taking manual
action. Never delete stable slot lock files. A v1.0 repair command will automate identity-verified
orphan recovery on macOS.

## Uninstall

```bash
agent-gov drain
agent-gov status
agent-gov uninstall --agents claude,cursor
```

Uninstall removes only entries matching the current absolute binary path and preserves later user
changes and the backup files.
