# Contributing to RealmHound

Contributions are welcome through GitHub issues and pull requests. You do not
need repository access - fork the repository and open a pull request from your
fork.

## Before you start

- Search existing issues and pull requests before opening a duplicate.
- Open an issue before starting a large feature or behavior change.
- Keep pull requests focused on one problem.
- Never post packet captures, logs, tokens, account exports, or other files
  containing player, account, session, or personal data.

## Build requirements

RealmHound is developed for 64-bit Windows. Install:

- The stable [Rust toolchain](https://rustup.rs/)
- [Npcap](https://npcap.com/#download) with WinPcap compatibility mode
- The [Npcap SDK](https://npcap.com/#download) for development

Extract the Npcap SDK and add its 64-bit library directory to the current
PowerShell session before building:

```powershell
$env:LIB = "C:\path\to\npcap-sdk\Lib\x64;$env:LIB"
```

See the [README](README.md#building-from-source) for the basic build command.

## Validate changes

Run these commands from the `realmhound` directory:

```powershell
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
```

Add or update tests when behavior changes. Do not introduce production
abstractions solely to make testing easier.

## Pull requests

Describe:

- What changed and why
- How you tested it
- Any user-visible behavior changes

Maintainers assign priority and decide whether a change fits the project.
Passing checks do not guarantee that a pull request will be merged.
