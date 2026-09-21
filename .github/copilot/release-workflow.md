# Copilot Release Workflow

Use this workflow when the user asks to prepare, create, publish, or ship a
release. `RELEASING.md` is the source of truth for release commands and
verification. This file defines the assistant's approval gates.

## 1. Propose

- Confirm the public repository is on a clean, synchronized `main` branch.
- Review the current version, latest tag, changes since that tag, and CI state.
- Propose a semantic version and concise release notes.
- Explain that prereleases still create a public release and send the Discord
  announcement, including its `@everyone` notification.
- Wait for explicit approval of the version and release notes.

## 2. Prepare

- Complete the preparation and validation steps in `RELEASING.md`.
- Review the complete diff.
- Ask the user to confirm a local release build of the prepared commit. An
  explicit request to skip manual testing is acceptable; never infer it.
- Commit and push the approved release preparation, then wait for `main` CI to
  pass.

## 3. Publish

Show the exact commit, version, annotated tag, and release notes. Wait for a
second explicit approval before creating or pushing the tag.

- Create and push the annotated tag; do not create the GitHub Release manually.
- Monitor the tag-triggered workflow and complete every verification in
  `RELEASING.md`.

If publication fails, inspect the failed step before retrying. The release is
one job, so rerunning it rebuilds and re-uploads assets and can send another
Discord `@everyone` announcement. Explain those effects and wait for explicit
approval before rerunning the original tag-push run. Never use manual workflow
dispatch as a release retry, or move or reuse a published tag. If the source
must change, prepare a new version.
