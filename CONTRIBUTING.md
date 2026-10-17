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

RealmHound targets 64-bit Windows and macOS. Both need the stable
[Rust toolchain](https://rustup.rs/).

### Windows

- [Npcap](https://npcap.com/#download) with WinPcap compatibility mode
- The [Npcap SDK](https://npcap.com/#download) for development

Extract the Npcap SDK and add its 64-bit library directory to the current
PowerShell session before building:

```powershell
$env:LIB = "C:\path\to\npcap-sdk\Lib\x64;$env:LIB"
```

### macOS

- Xcode Command Line Tools (`xcode-select --install`), which supply `libpcap`

Running the app (not the test suite) also needs read access to `/dev/bpf*`.
Install [Wireshark](https://www.wireshark.org/download.html) and allow its
ChmodBPF helper, then log out and back in.

See the [README](README.md#building-from-source) for the basic build command.

## Validate changes

Run these commands from the `realmhound` directory:

```sh
cargo fmt --all -- --check
cargo check --locked
cargo test --locked
```

Some `realmhound-core` migration tests share process-global state. If they fail
only when run together, re-run that crate serially to confirm:

```sh
cargo test --locked -p realmhound-core -- --test-threads=1
```

Add or update tests when behavior changes. Do not introduce production
abstractions solely to make testing easier.

## Platform-specific code

Keep platform differences behind the narrowest `cfg` that works, next to the
behavior they affect, rather than in a parallel platform tree. Existing examples
to follow:

- `clipboard.rs` and `app.rs`: one `#[cfg(windows)]` / `#[cfg(not(windows))]`
  pair per function.
- `client_process.rs` and `single_instance.rs`: a private module per platform
  behind a shared, platform-neutral public API.

A feature that cannot be supported on a platform should degrade visibly - with a
message naming the fix - rather than silently doing nothing.

## Pull requests

Describe:

- What changed and why
- How you tested it
- Any user-visible behavior changes

Maintainers assign priority and decide whether a change fits the project.
Passing checks do not guarantee that a pull request will be merged.
