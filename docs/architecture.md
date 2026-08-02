# Architecture

## Design constraints

Agent Governor optimizes for a small failure surface: one native binary, no daemon, no async
runtime, no privileged component, and no network access. Once a workload is classified as heavy,
every internal error fails closed for that workload. Before that boundary, malformed or unsupported
host/shell input passes through unchanged.

## Components

| Component | Contract |
|---|---|
| `hook` | Bounded JSON input, host-specific output, preserved unknown fields, stdout protocol only |
| `hook::rtk` | Direct argv spawn, bounded output, deadline, explicit exits 0/1/2/3 |
| `shell` | Bash CST with byte spans, conservative rules, insertion-only rewrite |
| `scheduler` | Stable slot inodes, short queue lock, bounded leases, approximate FIFO |
| `supervisor` | Inherited I/O, child process group, signals, execution timeout, exact exit mapping |
| `install` | Absolute paths, install lock, cross-agent rollback, exact backup restore or surgical unpatch |
| `doctor/status` | Read-only, versioned JSON plus concise human output |

## Admission state

```mermaid
stateDiagram-v2
    [*] --> FastPath
    FastPath --> Acquired: healthy slot free and no waiter
    FastPath --> Queued: all healthy slots busy
    Queued --> Acquired: oldest live lease and slot free
    Queued --> TempFail: queue timeout or full
    Acquired --> Starting: metadata persisted
    Starting --> Running: child group spawned
    Running --> Cleanup: exit, cancel, or timeout
    Cleanup --> [*]: metadata removed and lock closed
```

The slot lock is authoritative. Active metadata protects against a supervisor killed while its child
continues. Corrupt metadata quarantines only that slot. The other slot remains usable at capacity 2.

## Shell rewrite invariant

The parser identifies the byte offset of a simple command's executable. The rewriter inserts only a
constant prefix at that offset, applying insertions from the largest offset to the smallest. It never
serializes the shell tree. Removing all inserted prefixes reconstructs the exact candidate input.

RTK is evaluated independently. A candidate is accepted only when each command differs by one added
`rtk` executable token and its command count, control-operator frame, classifications,
heavy-command count, and background placement match the original. Candidates involving redirection
changes are conservatively discarded. A rejected or timed-out candidate never bypasses governance:
the original command is classified and wrapped.

## Runtime layout

```text
~/Library/Application Support/agent-gov/runtime/
  schema-version
  queue.lock
  drain.flag
  capacity
  slots/slot-0.lock
  slots/slot-1.lock
  active/slot-0.json
  active/slot-1.json
  waiters/<timestamp>-<random>.lease
  cooldowns/
```

Directories are `0700`; files are `0600`. Slot files are created once and never rotated during
normal operation.

## Known preview gaps

- TTY foreground process-group transfer still requires real-terminal validation on both Mac
  architectures; agent hooks normally provide non-interactive pipes.
- macOS process-start identity is not yet used by `cancel`; cancellation therefore requires both an
  exact job ID and a currently held slot lock, and refuses ambiguous orphan recovery.
- Strict network-filesystem detection is scheduled before v1.0. Use a local Application Support
  directory in the preview.
- Cursor hook schemas evolve quickly. `doctor` must be run after every Cursor update.
