# Compatibility

## Support policy

| Component | Preview policy |
|---|---|
| macOS | Apple Silicon and Intel are build/test targets; real dogfood is required before v1.0 |
| Claude Code | `PreToolUse`, matcher `Bash`, exec-form `command` + `args`, full `updatedInput` copy |
| Cursor | `preToolUse`, matcher `Shell`, valid JSON on every path; schema must pass `doctor` after update |
| RTK | Direct `rtk rewrite`; exits 0/1/2/3; absolute path recommended |
| Shell | Conservative Bash/POSIX subset parsed with tree-sitter; Zsh-only syntax passes unchanged |

Authoritative contracts:

- [Claude Code hooks reference](https://code.claude.com/docs/en/hooks)
- [Claude Code hooks guide](https://code.claude.com/docs/en/hooks-guide)
- [Cursor hooks documentation](https://cursor.com/docs/hooks)
- [RTK hooks](https://github.com/rtk-ai/rtk/blob/develop/hooks/README.md)
- [RTK technical documentation](https://github.com/rtk-ai/rtk/blob/develop/docs/contributing/TECHNICAL.md)

Claude's `updatedInput` replaces the whole object, so Agent Governor copies every existing field and
changes only `command` and (for governed Bash calls) raises `timeout` to a conservative floor. It does
not emit `permissionDecision: allow` by default.

Only one hook should rewrite a given tool input. Claude executes matching hooks in parallel and the
last completed `updatedInput` wins. The installer therefore removes only recognized RTK rewrite
hooks and installs one composed hook.
