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
| Linux | `RealmHound-linux-x86_64.tar.gz` | Extract, install Debian dependencies, then run `./packaging/linux/install.sh`. |

macOS quarantines anything downloaded through a browser, so the first launch is
blocked with "cannot be opened because the developer cannot be verified". Clear
it once with `xattr -dr com.apple.quarantine RealmHound.app`, or right-click in
Finder and choose **Open**. Updates applied from inside the app are not
quarantined and need neither step.

The in-app updater replaces the whole `RealmHound.app`, so the icon, `Info.plist`
and signature always match the version you are running. It updates only a copy
running from `RealmHound.app`; a binary built from source has to be rebuilt.

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

### Linux (Debian trixie, Steam/Proton)

Install the stable Rust toolchain from [rustup.rs](https://rustup.rs/), then
install the native build prerequisites once:

```sh
sudo apt install build-essential pkg-config libpcap-dev libasound2-dev libssl-dev libudev-dev libxkbcommon-dev libwayland-dev libx11-dev libxcb1-dev libgtk-3-dev libcap2-bin
cd realmhound
cargo build --workspace --release
cd ..
./packaging/linux/install.sh
```

The installer copies the GUI and a separate root-owned capture helper into
`/usr/local/bin`, grants only `CAP_NET_RAW` to the helper, and installs a
desktop launcher. It is the explicit one-time privileged setup. Check it with
`/usr/sbin/getcap /usr/local/bin/realmhound-capture` (some user shells omit
`/usr/sbin` from `PATH`); the expected output includes
`cap_net_raw=ep`. Never grant capabilities to `RealmHound` itself. Re-run the
installer after rebuilding because replacing the helper file clears its file
capability. Ordinary launches need no sudo:

```sh
RealmHound
```

Steam/Proton traffic normally crosses the host network stack. The helper
listens passively on libpcap's Linux `any` device with a fixed `tcp port 2050`
filter, so starting either program first and changing network interfaces do
not require configuration. For debugging, set
`REALMHOUND_CAPTURE_INTERFACE=eth0` before launch to select one interface.
The helper sends packets only to the unprivileged GUI through a private pipe;
it does not modify traffic or run as a persistent service.

RealmHound reads Steam's `libraryfolders.vdf` and RotMG's app manifest in the
usual Steam locations, including additional libraries and Flatpak Steam. It
checks the Steam install and matching Proton prefix for Unity's
`resources.assets`. Override discovery with `REALMHOUND_GAME_DIR=/path/to/game`
if necessary. Game files are read only. Extracted assets and account data live
under `$XDG_DATA_HOME/RealmHound` (normally `~/.local/share/RealmHound`),
settings under `$XDG_CONFIG_HOME/RealmHound`, and logs under
`$XDG_STATE_HOME/RealmHound/logs` (normally `~/.local/state/RealmHound/logs`).

If capture fails, verify the helper and its capability with `/usr/sbin/getcap`, and read
`~/.local/state/RealmHound/logs` for the capture error. If the window reports
waiting for a connection, enter a realm or change maps to create a fresh game
connection; the cipher cannot be reconstructed from the middle of an existing
stream. If sprites or names are missing, check that Steam has downloaded the
Unity client and inspect `REALMHOUND_GAME_DIR`. Linux automatic self-update is
disabled; rebuild and rerun the installer for updates. The existing protocol
pipeline decodes IPv4 game connections; IPv6 game connections are a current
limitation. Live Steam/Proton packet decoding still requires a game-session
smoke test on the target machine.

### Prerequisites

All platforms need the stable [Rust](https://rustup.rs/) toolchain, plus:

| Platform | Additional prerequisites |
| --- | --- |
| Windows | Visual Studio Build Tools with **Desktop development with C++**, MSVC tools, and a Windows SDK; use Rust's MSVC toolchain |
| macOS | Xcode Command Line Tools (`xcode-select --install`) - these supply `libpcap` |

Windows builds generate the `wpcap.lib` import library automatically; the Npcap
SDK is not required. Running the app or tests still requires
[Npcap](https://npcap.com/#download) with WinPcap compatibility mode.

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

- **Automatic updates** fetch the build for the platform they are running on and
  verify its SHA-256 before swapping it in. A release that publishes no binary
  for a platform reports that instead of offering one it cannot run.
- **Game assets** are read from the RotMG install: `%LOCALAPPDATA%` on Windows
  and `~/.local/share/RealmOfTheMadGod` on macOS. See
  [Game assets](#game-assets).
- **RealmHound's own data** lives under `%LOCALAPPDATA%\RealmHound` on Windows
  and `~/Library/Application Support/RealmHound` on macOS.

## Game assets

RealmHound ships no game art. On first launch it finds your RotMG install and
extracts sprites and the object-name table from the client's own
`resources.assets`. This is automatic and needs no configuration - the Deca
launcher's install location is known for each platform:

| Platform | Install location |
| --- | --- |
| Windows | `%LOCALAPPDATA%\RealmOfTheMadGod\Production\` |
| macOS | `~/.local/share/RealmOfTheMadGod/Production/` |

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

This tool is for educational and personal use only. It passively captures network traffic and does not modify game data or inject packets. Use at your own risk and in accordance with the game's Terms of Service.

## License

MIT License - see [LICENSE](LICENSE) for details.

## Acknowledgments

- Built with [Rust](https://www.rust-lang.org/), [egui](https://github.com/emilk/egui), and [pcap](https://crates.io/crates/pcap)
- Protocol definitions and packet structures were ported from [RealmShark](https://github.com/X-com/RealmShark) (MIT License) - see [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md)
- The Characters panel and parts of the sprite/API logic were inspired by [Muledump](https://github.com/jakcodex/muledump) (BSD-3-Clause) - see [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md)
