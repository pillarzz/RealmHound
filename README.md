# RealmHound 

A high-performance packet capture and analysis tool for **Realm of the Mad
God** (RotMG). RealmHound organizes data received by your game client and
presents your account data and observed activity in a clearer, more usable
way. It does not access server-side information unavailable to your client.

Got any questions, suggestions or other feedback? 
Join our Discord!

<img width="64" height="64" alt="discord" src="https://github.com/user-attachments/assets/6ad4035f-4e66-4ee1-be19-54330adc2cde" /> 

**[<img width="18" height="14" alt="discord_logo" src="https://github.com/user-attachments/assets/cab1a267-41db-4727-a91e-27813f284ef9" /> RealmHound](https://discord.gg/4zdy5Z4Sfd)**





![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)
![Language](https://img.shields.io/badge/language-Rust-orange.svg)

## Features

- Combat and DPS tracking for boss fights
- Loot history and trophy collections
- Quest and mission tracking
- Account-wide item search
- Character stats and overview
- Theoretical DPS calculator
- Party and chat logs

See the [full feature tour](https://pillarzz.github.io/RealmHound/) for screenshots and details.




## Download

Get the latest release from the [Releases](../../releases) page.

Releases ship a build for each platform:

| Platform | Download | Notes |
| --- | --- | --- |
| Windows | `RealmHound.exe` | Download and run it. |
| macOS | `RealmHound-macos-app.zip` | Universal (Apple Silicon and Intel). Unzip and drag `RealmHound.app` to Applications. |
| Linux | `RealmHound-linux-x86_64.tar.gz` | Extract and run `./packaging/linux/install.sh` as your normal user. It escalates only for the protected capture helper and retiring old system files. `libpcap` is required at runtime. |

macOS quarantines anything downloaded through a browser, so the first launch is
blocked with "cannot be opened because the developer cannot be verified". Clear
it once with `xattr -dr com.apple.quarantine RealmHound.app`, or right-click in
Finder and choose **Open**. Updates applied from inside the app are not
quarantined and need neither step.

The in-app updater replaces the whole `RealmHound.app`, so the icon, `Info.plist`
and signature always match the version you are running. It updates only a copy
running from `RealmHound.app`; a binary built from source has to be rebuilt.

On Linux, the official installer keeps the GUI under
`~/.local/opt/realmhound` and the capability-bearing helper under
`/usr/local/libexec/realmhound`. In-app updates replace only the user-owned GUI
and need no password. Replacing the helper is a rare manual administrative
step, because Linux file capabilities belong to the file and restoring them
requires privilege the GUI does not have.

## Requirements

### Windows

- **Windows 10/11** (64-bit)
- **[Npcap](https://npcap.com/#download)** - Required for packet capture (install with WinPcap compatibility mode)

### macOS

- **macOS 11 or newer**, Apple Silicon or Intel
- `libpcap` ships with macOS, so there is nothing extra to install - but
  capturing packets needs read access to `/dev/bpf*`, which is root-only by
  default. Grant it once by installing
  [Wireshark](https://www.wireshark.org/download.html) and allowing its
  **ChmodBPF** helper during setup, then log out and back in.

  Running `sudo RealmHound` also works, but ChmodBPF is safer: it grants capture
  access to your user instead of running the whole app as root.

### Linux

- **x86-64** on Steam/Proton. The release artifact is built on Debian 12 and
  requires glibc 2.36 or newer; aarch64 is not published yet.
- `libpcap` at runtime, plus `sudo` and `setcap`/`getcap` from `libcap2-bin`
  (or `libcap` on Arch) for the one-time `CAP_NET_RAW` grant that
  `packaging/linux/install.sh` applies (see
  [Linux build steps](#linux-steamproton))
- For a prebuilt tarball, the linked runtime libraries are `libpcap.so.1`,
  `libasound.so.2`, `libssl.so.3`, `libsystemd.so.0`, and `libdbus-1.so.3`,
  plus the X11/Wayland client libraries your desktop already provides

## Usage

Many features work best fullscreen. If you use two monitors, consider running
RealmHound on the second.

1. Install the packet-capture prerequisite for your platform (see [Requirements](#requirements))
2. Download and run RealmHound
3. The app will automatically detect your network interface and start capturing
4. Launch Realm of the Mad God and play normally
5. RealmHound will display game data in real-time

If RealmHound reports that it may not open a capture device, the capture
permission above has not been granted yet - the message names the fix for your
platform.

## Documentation

- [Player damage sources: implementation status](https://pillarzz.github.io/RealmHound/research/damage-sources-status.html)
  — interactive matrix of every effect that deals player damage, with its
  theoretical-DPS and Combat History attribution status.
  ([Markdown source](docs/research/damage-sources-status.md))

## Building from Source

### Linux (Steam/Proton)

The release artifact is built and validated on Debian 12 (glibc 2.36), the
oldest supported runtime. It is also tested on Debian trixie and Arch-based
systems. Install Rust from [rustup.rs](https://rustup.rs/); distro packages are
often too old.

```sh
sudo apt install build-essential pkg-config libpcap-dev libasound2-dev libssl-dev libudev-dev libxkbcommon-dev libcap2-bin
cd realmhound
cargo build --workspace --release --locked
cd ..
./packaging/linux/install.sh
```

When installing a downloaded release, verify the tarball against its published
`.sha256` first. `install.sh` runs as your normal user. It installs the GUI into
`~/.local/opt/realmhound`, creates the `~/.local/bin/realmhound` launcher
(plus a `RealmHound` alias) and a user desktop entry, and uses sudo only for the
helper and legacy cleanup. The helper is installed at
`/usr/local/libexec/realmhound/realmhound-capture` with exactly
`cap_net_raw=ep`. A fresh helper is `root:root 0755`; an existing root-owned
helper keeps its execution group and safe mode, so a shared-machine restriction
such as `chgrp realmhound` + `chmod 0750` survives an upgrade. The installers
refuse unsafe permissions, symlinked ancestors, and package-owned helper
destinations. Never grant capabilities to `RealmHound` itself. Verify with
`/usr/sbin/getcap /usr/local/libexec/realmhound/realmhound-capture` (some
shells omit `/usr/sbin` from `PATH`); the expected output includes
`cap_net_raw=ep`.

Routine in-app updates replace only the user-owned GUI and need no password.
Desktop entries and icons are installed by `install.sh`; if a release changes
them, rerun the installer from the verified tarball. A helper upgrade is a rare
manual step, because replacing the helper clears its capability.

Download the official release, verify its SHA-256, then run:

```sh
sudo ./packaging/linux/install-helper.sh --sha256 <official helper hash> /path/to/realmhound-capture
```

The helper digest is published as
`RealmHound-linux-x86_64-helper.sha256` with each release.

The helper installer verifies the copied file against the supplied digest,
installs only that helper, preserves an existing root-owned helper's group and
restrictive mode, and restores exactly `cap_net_raw=ep`; it never launches or
modifies the GUI. Re-run `install.sh` after a source rebuild if the helper
itself changed. A fresh helper is world-executable, so any local account on the
machine can use it to capture port-2050 traffic; this assumes a single-user
desktop (see [SECURITY.md](SECURITY.md)). An existing group-scoped restriction
is preserved. Ordinary launches need no sudo:

```sh
~/.local/bin/realmhound
```

Package-managed builds use the `package-build` Cargo feature and never replace
themselves from upstream artifacts; update those through the package manager.
The installers also refuse to overwrite a package-owned helper.

A source build is treated as unmanaged and never replaces itself either. For
development, `cargo run -p RealmHound` uses the sibling
`realmhound-capture` in the same target directory, so build both with
`cargo build --workspace` (or `cargo build -p realmhound-core --bin
realmhound-capture`). The sibling helper needs `CAP_NET_RAW`; grant it once
after building or rebuilding it, because replacing the binary clears the
capability:

```sh
sudo setcap cap_net_raw=ep realmhound/target/debug/realmhound-capture
```

To point a dev build at a helper elsewhere, set
`REALMHOUND_CAPTURE_HELPER=/absolute/path/to/realmhound-capture`. The official
installer's protected helper is preferred only for an official install.

Existing releases cannot bootstrap the new Linux updater from inside the app.
Migrate once by downloading this release and running `install.sh` as your user:
it installs and verifies the protected helper first, then publishes the
user-owned GUI and launcher, and retires `/usr/local/bin/RealmHound` only after
migration succeeds. Account data and settings under `~/.local/share/RealmHound`
are untouched.

Steam/Proton traffic normally crosses the host network stack, so the helper
listens passively on libpcap's `any` device with a fixed `tcp port 2050`
filter: start order and interface changes need no configuration, and
`REALMHOUND_CAPTURE_INTERFACE=eth0` selects a single interface. The helper
sends packets only to the unprivileged GUI through a private pipe; it does not
modify traffic or run as a persistent service.

RealmHound reads Steam's `libraryfolders.vdf` and RotMG's app manifest,
including additional libraries and Flatpak Steam, then checks the Steam install
and matching Proton prefix for Unity's `resources.assets`. Override discovery
with `REALMHOUND_GAME_DIR=/path/to/game`. Game files are read only. Extracted
assets, account data, and settings live under `~/.local/share/RealmHound`
(`$XDG_DATA_HOME`); logs under `~/.local/state/RealmHound/logs`
(`$XDG_STATE_HOME`).

Troubleshooting:

- Capture fails: check `getcap` on `/usr/local/libexec/realmhound/realmhound-capture`, then the log above.
- An update does not start: the launcher checks the replacement and restores
  `RealmHound.old` automatically. If the replacement starts but fails before
  rendering a frame, the next launch restores the retained previous binary. The
  previous binary is kept until the new version has completed a successful
  launch.
- "Waiting for a connection": enter a realm or change maps; the cipher cannot
  be reconstructed from the middle of an existing stream.
- Missing sprites or names: confirm Steam finished downloading the Unity
  client, or set `REALMHOUND_GAME_DIR`.

The Linux updater refuses to replace a GUI when the installed capture helper is
missing or too old for the new release, and explains the manual helper upgrade
instead. It also refuses package-managed and source/unrecognized installations.
The existing protocol pipeline decodes IPv4 game connections; IPv6 is a current
limitation.

### Prerequisites

All platforms need the stable [Rust](https://rustup.rs/) toolchain, plus:

| Platform | Additional prerequisites |
| --- | --- |
| Windows | Visual Studio Build Tools with **Desktop development with C++**, MSVC tools, and a Windows SDK; use Rust's MSVC toolchain |
| macOS | Xcode Command Line Tools (`xcode-select --install`) - these supply `libpcap` |
| Linux | Debian/Ubuntu: `build-essential`, `pkg-config`, `libpcap-dev`, `libasound2-dev`, `libssl-dev` (see [Linux build steps](#linux-steamproton)) |

Windows builds generate the `wpcap.lib` import library automatically; the Npcap
SDK is not required. Running the app or tests still requires
[Npcap](https://npcap.com/#download) with WinPcap compatibility mode.

### Overlay attachment

Enabled Live Combat and Live Loot overlays require a game window linked to the
OS process owning the account-verified main connection. They appear only while
that window is visible and not minimized, and either the linked game or
RealmHound is foreground. Other foreground apps, unresolved attachment and
inspection errors hide them; hovering an overlay does not bypass this rule.

The linked PID is retained across area changes, socket gaps and capture
restarts until the process exits. Switching accounts restarts RealmHound and
clears the link. When starting mid-connection, enter a realm or change maps to
allow account verification. If overlays still do not appear, check the
RealmHound log for process/window inspection errors.

Linux attachment requires X11/XWayland and readable game-process/socket
metadata; native Wayland cannot verify attachment and keeps overlays hidden.
On Wayland, RealmHound selects XWayland only when an overlay is enabled at
startup. Enabling one later offers a restart button when XWayland is available;
`REALMHOUND_WAYLAND=1` continues to opt out.
macOS uses libproc socket ownership, CoreGraphics window visibility and AppKit
foreground-app metadata, without reading window titles or capturing pixels.
Denied or missing metadata hides overlays. Native macOS/Windows interaction
and permission behavior still require validation on those operating systems.

### Build

```sh
cd realmhound
cargo build --release
```

The executable will be at `realmhound/target/release/RealmHound`
(`RealmHound.exe` on Windows).

On macOS, wrap that binary in the icon-carrying bundle the release ships with:

```sh
tools/macos/make-app-bundle.sh realmhound/target/release/RealmHound realmhound/target/release
```

### Platform notes

- **Automatic updates** fetch the build for the platform and architecture they
  are running on and verify its SHA-256 before swapping it in. A release that
  publishes no binary for a target reports that instead of offering one it
  cannot run. Linux updates additionally require the official user-owned
  install layout and a compatible capture helper.
- **Game assets** are read from the RotMG install: `%LOCALAPPDATA%` on Windows,
  `~/.local/share/RealmOfTheMadGod` on macOS, and the Steam/Proton prefix on
  Linux. See [Game assets](#game-assets).
- **RealmHound's own data** lives under `%LOCALAPPDATA%\RealmHound` on Windows,
  `~/Library/Application Support/RealmHound` on macOS, and
  `~/.local/share/RealmHound` on Linux.

## Game assets

RealmHound ships no game art. On first launch it finds your RotMG install and
extracts sprites and the object-name table from the client's own
`resources.assets`. This is automatic and needs no configuration - the Deca
launcher's install location is known for each platform:

| Platform | Install location |
| --- | --- |
| Windows | `%LOCALAPPDATA%\RealmOfTheMadGod\Production\` |
| macOS | `~/.local/share/RealmOfTheMadGod/Production/` |
| Linux | Steam library's Proton prefix (see [Linux build steps](#linux-steamproton)) |

Without a local install the app still captures and records everything, but
items and enemies show as numeric IDs and icons render empty.

## Project Structure

```
RealmHound/
├── realmhound/              # Rust application
│   ├── crates/
│   │   ├── realmhound/      # GUI application (egui/eframe)
│   │   └── realmhound-core/ # Core library (packet parsing, capture)
│   └── Cargo.toml           # Workspace manifest
└── README.md
```

## Legal Disclaimer

This tool is for educational and personal use only. It passively captures network traffic and does not modify game data or inject packets. Use at your own risk and in accordance with the game's Terms of Service. Custom messages configured in RealmHound are authored and sent by you; profanity or insults can get your account suspended regardless of the tools used to prepare them.

## License

MIT License - see [LICENSE](LICENSE) for details.

## Acknowledgments

- Built with [Rust](https://www.rust-lang.org/), [egui](https://github.com/emilk/egui), and [pcap](https://crates.io/crates/pcap)
- Protocol definitions and packet structures were ported from [RealmShark](https://github.com/X-com/RealmShark) (MIT License) - see [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md)
- The Characters panel and parts of the sprite/API logic were inspired by [Muledump](https://github.com/jakcodex/muledump) (BSD-3-Clause) - see [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md)
