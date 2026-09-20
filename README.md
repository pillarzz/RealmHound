# RealmHound 🐕

A high-performance packet capture and analysis tool for **Realm of the Mad
God** (RotMG). RealmHound organizes data received by your game client and
presents your account data and observed activity in a clearer, more usable
way. It does not access server-side information unavailable to your client.

![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Platform](https://img.shields.io/badge/platform-Windows-lightgrey.svg)
![Language](https://img.shields.io/badge/language-Rust-orange.svg)

## Features

- **Widget Bar** - Key currencies, seasonal timers, dust, forge materials, and account information available on every page.
- **Taskbar** - Customizable mission and quest tracking on every page, with merged objectives, suggested choices, and progress tips.
- **Live Feed** - Rolling log of realm events, dungeon entries, realm and score details, direct messages, alerts, and clipboard callouts.
- **Quests** - Daily and event Tinkerer quests with progress, categories, Taskbar tracking, filtering, and claimable-reward reminders.
- **Missions** - Seasonal mission progress with status categories, custom ordering, Taskbar tracking, eligibility checks, and reward reminders.
- **Loot History** - Searchable loot records by item, enemy, dungeon, character, date, and special item type.
- **Combat History** - Boss encounter records with player performance, team damage dealt and taken, search, and filters.
- **Trophy Hall** - Rare item collections by dungeon, with run summaries and discovery history in detailed tooltips.
- **Treasury** - Search and count items across observed account storage, with categories and aggregate totals.
- **Vault** - Last-observed replicas of vault and storage containers for browsing and planning without loading them in game.
- **Characters** - Character overview with equipment, inventories, stats, maxing progress, fame calculations, and milestone tracking.
- **Exaltations** - Account-wide exaltation progress by class and stat on one page.
- **Party** - Party overview, watchlists, and copy-ready moderation commands without reopening the in-game Party UI.
- **Chat** - Searchable message history for browsing conversations and finding previous contacts.

## Download

Get the latest release from the [Releases](../../releases) page.

Just download `RealmHound.exe` and run it!

## Requirements

- **Windows 10/11** (64-bit)
- **[Npcap](https://npcap.com/#download)** - Required for packet capture (install with WinPcap compatibility mode)

## Usage

Many features work best fullscreen. If you use two monitors, consider running
RealmHound on the second.

1. Install [Npcap](https://npcap.com/#download) if you haven't already
2. Download and run `RealmHound.exe`
3. The app will automatically detect your network interface and start capturing
4. Launch Realm of the Mad God and play normally
5. RealmHound will display game data in real-time

## Building from Source

### Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain)
- [Npcap SDK](https://npcap.com/#download) (for development)

### Build

```powershell
cd realmhound
cargo build --release
```

The executable will be at `realmhound/target/release/RealmHound.exe`

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
