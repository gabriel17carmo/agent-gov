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
