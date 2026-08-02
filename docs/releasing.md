# Releasing Agent Governor

Releases are derived from the version in `Cargo.toml`. The pipeline creates the Git tag and GitHub
Release only after the `CI` workflow succeeds on `main`.

## Normal release

1. Open a pull request that updates the package version in `Cargo.toml`, refreshes `Cargo.lock`, and
   records the user-visible changes in `CHANGELOG.md`.
2. Merge the pull request after all required checks pass.
3. CI runs on the merge commit. When it succeeds, the Release workflow checks for `v<version>`.
4. If that version is new, the workflow builds and verifies the universal macOS binary, then
   publishes the tag and GitHub Release with generated notes.

The release includes:

- a universal `agent-gov` binary for Apple Silicon and Intel Macs;
- `agent-gov.sha256` for integrity verification;
- a CycloneDX software bill of materials in `agent-gov.cdx.json`;
- `install-agent-gov.sh` for the one-line installation path.

The workflow also creates signed GitHub build-provenance attestations. After downloading an asset,
users with the GitHub CLI can verify its origin:

```bash
shasum -a 256 --check agent-gov.sha256
gh attestation verify agent-gov --repo gabriel17carmo/agent-gov
```

If the version is already published, the automatic run exits without rebuilding it. This makes
ordinary merges safe and ensures that changing the package version is the explicit release signal.
Semantic versions with a pre-release suffix, such as `0.2.0-rc.1`, are published as GitHub
pre-releases and therefore do not replace the stable installer target.

## Recovery and republishing

Use **Actions → Release → Run workflow** when a release must be recovered:

- leave `tag` blank to publish the version currently on `main`;
- provide an existing tag, such as `v0.1.0`, to rebuild that exact tagged commit and replace its
  release assets.

The workflow is idempotent: an existing release receives replacement assets, an existing tag gets a
new release, and a completely new version gets both its tag and release. New releases without an
explicit tag must be dispatched from `main`.

Pushing a matching `v*` tag remains supported as an emergency path. The tag must match the package
version in `Cargo.toml` or the workflow fails before publishing.

Apple Developer ID signing and notarization are intentionally not simulated. They remain a release
gate until the required certificate and Apple credentials are configured as repository secrets.
