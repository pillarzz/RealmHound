# RealmHound 🐕

A high-performance packet capture and analysis tool for **Realm of the Mad
God** (RotMG). RealmHound organizes data received by your game client and
presents your account data and observed activity in a clearer, more usable
way. It does not access server-side information unavailable to your client.

![License](https://img.shields.io/badge/license-MIT-blue.svg)
![Platform](https://img.shields.io/badge/platform-Windows-lightgrey.svg)
![Language](https://img.shields.io/badge/language-Rust-orange.svg)

## Features

- **Widget Bar** - Customizable key currencies, seasonal timers, dust, forge materials, and account information available on every page. <img width="1810" height="36" alt="image" src="https://github.com/user-attachments/assets/01e153a9-0aff-4c1a-9d69-de67728eca76" />
- **Taskbar** - Customizable mission and quest tracking on every page, with merged objectives, suggested choices, and progress tips. <img width="1810" height="36" alt="image" src="https://github.com/user-attachments/assets/b07f7aaa-c67d-417f-8850-cdf3a6185628" />
- **Live Feed** - Rolling log of realm events, dungeon entries, realm and score details, direct messages, alerts, and clipboard callouts. <img width="1605" height="787" alt="image" src="https://github.com/user-attachments/assets/41bd94dd-e83d-483d-93f8-2cf6d674d673" />
- **Quests** - Daily and event Tinkerer quests with progress, categories, Taskbar tracking, filtering, and claimable-reward reminders. <img width="1603" height="1279" alt="image" src="https://github.com/user-attachments/assets/3c68f46e-dcba-49c0-9058-e942d21588d9" />
- **Missions** - Seasonal mission progress with status categories, custom ordering, Taskbar tracking, eligibility checks, and reward reminders.<img width="1608" height="1313" alt="image" src="https://github.com/user-attachments/assets/56c6eb07-e200-42ed-8ed3-fa02d6eb7ff1" />
- **Loot History** - Searchable loot records by item, enemy, dungeon, character, date, and special item type. <img width="1605" height="257" alt="image" src="https://github.com/user-attachments/assets/1880ef93-8689-462d-bcd7-56082960e09e" />
- **Combat History** - Boss encounter records with player performance, team damage dealt and taken, search, and filters. <img width="1607" height="625" alt="image" src="https://github.com/user-attachments/assets/4647f3f3-99cb-48fb-b60b-4e2a4de42778" /> <img width="889" height="1116" alt="image" src="https://github.com/user-attachments/assets/e5022670-ddc3-4d5c-b259-1227cab19d77" />
- **Trophy Hall** - Rare item collections by dungeon, with run summaries and discovery history in detailed tooltips. <img width="1607" height="1185" alt="image" src="https://github.com/user-attachments/assets/4ba98eaa-7868-4bcd-b0eb-32f07dad45e3" /> <img width="1601" height="1199" alt="image" src="https://github.com/user-attachments/assets/96b41a8c-0c80-4712-b2c2-25579e519c37" />
- **Treasury** - Search and count items across observed account storage, with categories and aggregate totals. <img width="1608" height="1231" alt="image" src="https://github.com/user-attachments/assets/2d2c1ec4-e65f-4d18-bebd-4f35467ceccb" />
 <img width="1023" height="696" alt="image" src="https://github.com/user-attachments/assets/ad698f10-a451-4933-9a55-b970d3e8a336" /> <img width="1608" height="406" alt="image" src="https://github.com/user-attachments/assets/3562cf51-5379-4052-b140-0c439c7ff569" />
- **Vault** - Last-observed replicas of vault and storage containers for browsing and planning without loading them in game. <img width="452" height="405" alt="image" src="https://github.com/user-attachments/assets/2c372785-1fc6-4ab8-a2d1-489552dfe0c5" /> <img width="575" height="788" alt="image" src="https://github.com/user-attachments/assets/b6e6816a-9c25-414f-8027-3826e8f07c31" />
- **Characters** - Character overview with equipment, inventories, stats, maxing progress, fame calculations, and milestone tracking. <img width="1598" height="660" alt="image" src="https://github.com/user-attachments/assets/3581f4c9-5e83-4a11-b916-66f4030baea5" /> <img width="1459" height="1127" alt="image" src="https://github.com/user-attachments/assets/433de390-9a0f-4cdf-91f8-a13f682bfd8e" />
- **Exaltations** - Account-wide exaltation progress by class and stat on one page. <img width="1607" height="616" alt="image" src="https://github.com/user-attachments/assets/7a105ed3-79ba-440b-a620-ab48658c1673" />
- **Party** - Party overview, watchlists, and copy-ready moderation commands without reopening the in-game Party UI. <img width="1607" height="781" alt="image" src="https://github.com/user-attachments/assets/94196443-26df-4b09-b2ac-b989aa35286f" />
- **Chat** - Searchable message history for browsing conversations and finding previous contacts. <img width="911" height="357" alt="image" src="https://github.com/user-attachments/assets/661a22f9-de8f-401e-ba3a-b3d63c3fddcb" />
- **Flexible settings** - Customize what you see and hear. <img width="1052" height="619" alt="image" src="https://github.com/user-attachments/assets/bbffd134-0105-451d-9131-57d57df2ffe0" /> <img width="1053" height="618" alt="image" src="https://github.com/user-attachments/assets/8fed0e73-23fb-4838-97d3-cde78e56d243" />




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
