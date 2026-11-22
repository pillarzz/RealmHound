# Releasing RealmHound

This document is for RealmHound maintainers. Releases are built by GitHub
Actions from version tags.

For an assistant-guided release, see
`.github/copilot/release-workflow.md`.

## Prepare the release

1. Choose a version using semantic versioning:
   - Patch for compatible bug fixes
   - Minor for compatible features
   - Major for breaking changes
2. Update `[workspace.package].version` in `realmhound/Cargo.toml`.
3. Run `cargo check` from the `realmhound` directory to update the tracked
   `Cargo.lock`, then confirm only the workspace package versions changed.
4. Update `realmhound/RELEASE_NOTES.txt` with concise user-facing bullet
   points. Every non-empty line that does not start with `#` becomes an
   in-app changelog entry, so prefix optional section headings with `#`.
   Remove or correct any retained line that does not follow this format.
5. Run from the `realmhound` directory:

```powershell
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
```

6. Commit and push `realmhound/Cargo.toml`, `realmhound/Cargo.lock`, and
   `realmhound/RELEASE_NOTES.txt`.

The tag version without its leading `v` must exactly match the workspace
version.

## Publish

Create and push an annotated tag:

```powershell
git tag -a vX.Y.Z -m "RealmHound vX.Y.Z"
git push origin vX.Y.Z
```

A suffix such as `vX.Y.Z-beta.1` creates a prerelease. Prereleases do not
update the production updater manifest, but they are published and announced
on Discord with the same `@everyone` notification as stable releases. Do not
move, replace, or reuse a published version tag.

The release workflow:

- Downloads and verifies the pinned official sound bundle from the private
  `pillarzz/realmhound-build-assets` repository
- Builds a universal macOS binary, and blocks the release if that build fails: a
  manifest must never advertise a macOS build that does not exist
- Wraps that binary in `RealmHound.app` and publishes it as
  `RealmHound-macos-app.zip`, the only macOS download; the updater replaces the
  whole bundle, so its metadata and signature never outlive the executable
- Builds `RealmHound.exe` with the locked dependency graph
- Builds the Linux GUI and capture helper in a Debian 12 container, the oldest
  supported runtime, and validates the produced binaries there
- Publishes `RealmHound-linux-x86_64` (the bare GUI update artifact and its
  sidecar), `RealmHound-linux-x86_64-helper.sha256`, and the full
  `RealmHound-linux-x86_64.tar.gz` manual-install archive and sidecar
- Generates and publishes `RealmHound.exe.sha256` and
  `RealmHound-macos-app.zip.sha256`
- Creates a GitHub Release in this repository
- Updates the production updater manifest for stable releases only
- Announces public releases on Discord

`DISCORD_WEBHOOK` is required for public release announcements. `GIST_PAT` is
required only for stable public releases, which update the production updater
manifest. The manifest's `linux_min_helper_version` comes from
`packaging/linux/min-helper-version`; keep it equal to
`MIN_SUPPORTED_HELPER_VERSION` in
`realmhound/crates/realmhound-core/src/capture/linux_helper.rs`. Raise it only
when a GUI release genuinely requires a newer helper, because it blocks GUI
updates until users complete the manual helper step.

Distribution packages must be built with the `package-build` Cargo feature so
they never replace package-managed files from upstream artifacts:

```sh
cargo build --release --locked -p RealmHound --features package-build
``` `BUILD_ASSETS_TOKEN` must be a fine-grained, read-only token with
access to `pillarzz/realmhound-build-assets`. The workflow's built-in
`GITHUB_TOKEN` publishes the GitHub Release.

To replace official sounds, publish a new immutable release in the private
asset repository, then update its tag, archive SHA-256, and expected paths in
the release workflow and build script. Never replace an existing asset release.

A manual workflow dispatch builds the executable and checksum without
publishing a release, updating the manifest, or announcing on Discord.
Before retrying a failed release, inspect which step failed. The release is a
single job, so rerunning it rebuilds and re-uploads assets and can send another
Discord `@everyone` announcement. After approval, rerun the original tag-push
run instead of starting a manual workflow dispatch:

```powershell
gh run rerun <RUN_ID> --failed
```

After a rerun, verify the published executable checksum again and confirm a
stable updater manifest contains that checksum.

## Verify

For every release:

- Confirm the workflow completed successfully.
- Confirm the GitHub Release is attached to the intended tag.
- Confirm `RealmHound.exe` and `RealmHound.exe.sha256` are present.
- Confirm `RealmHound-macos-app.zip` and its `.sha256` are present, and that
  unzipping it yields a `RealmHound.app` showing the RealmHound icon in Finder.
- Confirm `RealmHound-linux-x86_64`, its `.sha256`, and
  `RealmHound-linux-x86_64-helper.sha256` are present, and that the full Linux
  tarball and its sidecar are present.
- Download each artifact and verify its SHA-256 against the checksum file.
- Run `./RealmHound-linux-x86_64 --version` and
  `./realmhound-capture --version` from the Linux tarball on Debian 12 (or the
  documented oldest supported runtime) and confirm they start without missing
  libraries.

For a stable version at or above the manifest's current latest version, also
confirm the production updater manifest contains the new version, the
same-repository download URLs and matching SHA-256 for `download_url`,
`macos_download_url`, and `linux_x86_64_download_url`, that
`macos_download_url` points at the `-macos-app.zip` archive rather than a bare
binary, and that `linux_helper_sha256` and `linux_min_helper_version` match the
release. For a prerelease, confirm the production updater manifest remains
unchanged.
