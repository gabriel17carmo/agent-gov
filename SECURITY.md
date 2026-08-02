# Security policy

## Supported versions

The latest tagged release receives security fixes. During public preview, breaking hardening changes
may be released in a minor version when required to preserve fail-closed behavior.

## Reporting a vulnerability

Please use GitHub's **Report a vulnerability** private reporting flow. Do not open a public issue for
command injection, lock bypass, unsafe cancellation, settings corruption, or sensitive-data leakage.

Include the version, macOS architecture/version, agent and RTK versions, minimal redacted input, and
whether a recognized heavy workload started without a slot. Never attach proprietary commands,
environment variables, transcripts, or corporate security logs.

The project will acknowledge a complete report within five business days and publish a coordinated
fix and advisory when confirmed.
