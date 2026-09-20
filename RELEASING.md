# Releasing RealmHound

This document is for RealmHound maintainers. Releases are built by GitHub
Actions from version tags.

## Prepare the release

1. Choose a version using semantic versioning:
   - Patch for compatible bug fixes
   - Minor for compatible features
   - Major for breaking changes
2. Update `[workspace.package].version` in `realmhound/Cargo.toml`.
3. Update `realmhound/RELEASE_NOTES.txt` with concise user-facing changes.
4. Run from the `realmhound` directory:

```powershell
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
```

5. Commit and push the version and release notes.

The tag version without its leading `v` must exactly match the workspace
version.

## Publish

Create and push an annotated tag:

```powershell
git tag -a vX.Y.Z -m "RealmHound vX.Y.Z"
git push origin vX.Y.Z
```

A suffix such as `vX.Y.Z-beta.1` creates a prerelease. Do not move, replace, or
reuse a published version tag.

The release workflow:

- Downloads and verifies the pinned official sound bundle from the private
  `pillarzz/realmhound-build-assets` repository
- Builds `RealmHound.exe` with the locked dependency graph
- Generates and publishes `RealmHound.exe.sha256`
- Creates a GitHub Release in this repository
- Updates the production updater manifest for stable releases only
- Announces public releases on Discord

`DISCORD_WEBHOOK` is required for public release announcements. `GIST_PAT` is
required only for stable public releases, which update the production updater
manifest. `BUILD_ASSETS_TOKEN` must be a fine-grained, read-only token with
access to `pillarzz/realmhound-build-assets`. The workflow's built-in
`GITHUB_TOKEN` publishes the GitHub Release.

To replace official sounds, publish a new immutable release in the private
asset repository, then update its tag, archive SHA-256, and expected paths in
the release workflow and build script. Never replace an existing asset release.

A manual workflow dispatch builds the executable and checksum without
publishing a release, updating the manifest, or announcing on Discord.

## Verify

For every release:

- Confirm the workflow completed successfully.
- Confirm the GitHub Release is attached to the intended tag.
- Confirm `RealmHound.exe` and `RealmHound.exe.sha256` are present.
- Download the executable and verify its SHA-256 against the checksum file.

For a stable release, also confirm the production updater manifest contains
the new version, same-repository download URL, and matching SHA-256. For a
prerelease, confirm the production updater manifest remains unchanged.
