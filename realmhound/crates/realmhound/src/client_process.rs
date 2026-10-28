//! Maps the live main game-client connection to its owning OS process so the
//! forge-fire safeguard can tell whether the game client was **relaunched**
//! after the 00:00 UTC daily reset.
//!
//! Neither the forge-fire value (Sulphur refills, crafting drains, variable
//! daily grant) nor the packet/connection lifecycle (an area change opens a new
//! socket whose Hello is indistinguishable from a cold launch) can detect a
//! relaunch. The only reliable signal is the owning process's creation time,
//! read from the current OS state -- which works regardless of when RealmHound
//! itself was started.
//!
//! Each supported platform answers the same two questions, "which process owns
//! this socket" and "when did that process start", from whatever the kernel
//! exposes: `GetExtendedTcpTable` + `GetProcessTimes` on Windows, `libproc` on
//! macOS, and `/proc` on Linux. Anywhere else the safeguard stays conservative
//! and never unlocks.

use realmhound_core::stream::ConnectionKey;

/// The RotMG game-server TCP port. Every game client -- Steam, standalone, any
/// install -- talks to the world server on this port, and effectively nothing
/// else on the machine connects out to it, so it reliably marks a socket as
/// belonging to a RotMG client without matching any file name.
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
const GAME_SERVER_PORT: u16 = 2050;

/// Convert a Windows `FILETIME` (100-ns intervals since 1601-01-01 UTC, split
/// into low/high 32-bit halves) to a Unix timestamp in seconds.
#[cfg(windows)]
pub(crate) fn filetime_to_unix_secs(low: u32, high: u32) -> i64 {
    const HUNDRED_NS_PER_SEC: u64 = 10_000_000;
    // Whole seconds between 1601-01-01 and 1970-01-01 (Unix epoch).
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    let ticks = ((high as u64) << 32) | (low as u64);
    (ticks / HUNDRED_NS_PER_SEC) as i64 - EPOCH_DIFF_SECS
}

/// Unix timestamp (seconds) of the creation time of the process that owns the
/// given main game-client connection, or `None` if it can't be determined
/// (connection not found in the OS tables, access denied, or the process exited
/// between the two lookups).
#[cfg(windows)]
pub fn connection_process_start_unix(conn: &ConnectionKey) -> Option<i64> {
    let pid = windows_impl::owning_pid(conn)?;
    windows_impl::process_creation_unix(pid)
}

#[cfg(target_os = "macos")]
pub fn connection_process_start_unix(conn: &ConnectionKey) -> Option<i64> {
    let pid = macos_impl::owning_pid(conn)?;
    macos_impl::process_creation_unix(pid)
}

#[cfg(target_os = "linux")]
pub fn connection_process_start_unix(conn: &ConnectionKey) -> Option<i64> {
    linux_impl::connection_process_start_unix(conn)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn connection_process_start_unix(_conn: &ConnectionKey) -> Option<i64> {
    None
}

/// Unix timestamp (seconds) of the *earliest* creation time among all running
/// RotMG client processes, identified by an active TCP connection to a game
/// server (remote port 2050).
///
/// This lets the guard detect a relaunch even when RealmHound was started
/// mid-game and hasn't yet observed a Hello (which only arrives on connect or
/// area change), because it reads live OS state rather than our packet capture.
///
/// The *earliest* start is deliberately conservative for multiboxing: if any
/// client has been running since before the reset this stays "pre-reset" and
/// the guard stays locked. It only reports a post-reset time when *every*
/// running client started after the reset -- in which case the tracked main
/// account, whichever client it is, must also have relaunched.
///
/// Windows never reports `Indeterminate`: the table lists TIME_WAIT sockets
/// owned by pid 0, which no process can answer for, so an unreadable pid is not
/// evidence of an unchecked client the way it is on macOS.
#[cfg(windows)]
pub fn earliest_rotmg_client_start_unix() -> ClientStart {
    windows_impl::earliest_client_start_unix()
        .map_or(ClientStart::NoneRunning, ClientStart::Earliest)
}

#[cfg(target_os = "macos")]
pub fn earliest_rotmg_client_start_unix() -> ClientStart {
    macos_impl::earliest_client_start_unix()
}

#[cfg(target_os = "linux")]
pub fn earliest_rotmg_client_start_unix() -> ClientStart {
    linux_impl::earliest_client_start_unix()
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub fn earliest_rotmg_client_start_unix() -> ClientStart {
    ClientStart::NoneRunning
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::{ClientStart, ConnectionKey, GAME_SERVER_PORT};
    use std::collections::{HashMap, HashSet};
    use std::net::Ipv4Addr;
    use std::path::Path;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct TcpEntry {
        local_addr: Ipv4Addr,
        local_port: u16,
        remote_addr: Ipv4Addr,
        remote_port: u16,
        uid: u32,
        inode: u64,
    }

    fn parse_endpoint(value: &str) -> Option<(Ipv4Addr, u16)> {
        let (address, port) = value.split_once(':')?;
        if address.len() != 8 {
            return None;
        }
        let address = u32::from_str_radix(address, 16).ok()?;
        let port = u16::from_str_radix(port, 16).ok()?;
        Some((Ipv4Addr::from(address.to_le_bytes()), port))
    }

    fn parse_tcp_table(text: &str) -> Option<Vec<TcpEntry>> {
        text.lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                let (local_addr, local_port) = parse_endpoint(*fields.get(1)?)?;
                let (remote_addr, remote_port) = parse_endpoint(*fields.get(2)?)?;
                Some(TcpEntry {
                    local_addr,
                    local_port,
                    remote_addr,
                    remote_port,
                    uid: fields.get(7)?.parse().ok()?,
                    inode: fields.get(9)?.parse().ok()?,
                })
            })
            .collect()
    }

    fn tcp_entries(proc_root: &Path) -> Option<Vec<TcpEntry>> {
        parse_tcp_table(&std::fs::read_to_string(proc_root.join("net/tcp")).ok()?)
    }

    fn effective_uid(proc_root: &Path) -> Option<u32> {
        let status = std::fs::read_to_string(proc_root.join("self/status")).ok()?;
        let uid = status.lines().find(|line| line.starts_with("Uid:"))?;
        uid.split_whitespace().nth(2)?.parse().ok()
    }

    fn process_uid(path: &Path) -> Option<u32> {
        let status = std::fs::read_to_string(path.join("status")).ok()?;
        let uid = status.lines().find(|line| line.starts_with("Uid:"))?;
        uid.split_whitespace().nth(2)?.parse().ok()
    }

    fn socket_inode(path: &Path) -> Option<u64> {
        let target = std::fs::read_link(path).ok()?;
        let target = target.to_str()?;
        target
            .strip_prefix("socket:[")?
            .strip_suffix(']')?
            .parse()
            .ok()
    }

    struct ProcessScan {
        owners: HashMap<u64, Vec<u32>>,
        unchecked_process: bool,
        named_clients: Vec<(u32, i64)>,
        named_unreadable: bool,
    }

    /// One pass answers both questions a refresh check asks -- socket owners and
    /// game-client processes -- instead of walking `/proc` once per question.
    fn scan_processes(
        proc_root: &Path,
        uid: u32,
        ticks_per_second: u64,
        inodes: &HashSet<u64>,
    ) -> Option<ProcessScan> {
        let mut scan = ProcessScan {
            owners: HashMap::new(),
            unchecked_process: false,
            named_clients: Vec::new(),
            named_unreadable: false,
        };
        for entry in std::fs::read_dir(proc_root).ok()? {
            let Ok(entry) = entry else {
                scan.unchecked_process = true;
                continue;
            };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok())
            else {
                continue;
            };
            if process_uid(&entry.path()) != Some(uid) {
                continue;
            }
            if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
                if is_rotmg_client_name(&comm) {
                    match process_creation_unix_at(proc_root, pid, ticks_per_second) {
                        Some(start) if start > 0 => scan.named_clients.push((pid, start)),
                        _ => scan.named_unreadable = true,
                    }
                }
            }
            if inodes.is_empty() {
                continue;
            }
            let fds = match std::fs::read_dir(entry.path().join("fd")) {
                Ok(fds) => fds,
                Err(_) => {
                    // A process may have exited between the directory and fd reads.
                    if entry.path().exists() {
                        scan.unchecked_process = true;
                    }
                    continue;
                }
            };
            for fd in fds.flatten() {
                let Some(inode) = socket_inode(&fd.path()) else {
                    continue;
                };
                if inodes.contains(&inode) {
                    scan.owners.entry(inode).or_default().push(pid);
                }
            }
        }
        Some(scan)
    }

    fn named_start(scan: &ProcessScan) -> ClientStart {
        if scan.named_unreadable {
            return ClientStart::Indeterminate;
        }
        scan.named_clients
            .iter()
            .map(|&(_, start)| start)
            .min()
            .map_or(ClientStart::NoneRunning, ClientStart::Earliest)
    }

    fn socket_start(
        proc_root: &Path,
        scan: &ProcessScan,
        inodes: &HashSet<u64>,
        ticks_per_second: u64,
    ) -> ClientStart {
        if scan.unchecked_process || inodes.iter().any(|inode| !scan.owners.contains_key(inode)) {
            return ClientStart::Indeterminate;
        }
        scan.owners
            .values()
            .flatten()
            .filter_map(|&pid| process_creation_unix_at(proc_root, pid, ticks_per_second))
            .filter(|&start| start > 0)
            .min()
            .map_or(ClientStart::Indeterminate, ClientStart::Earliest)
    }

    fn boot_time(proc_root: &Path) -> Option<i64> {
        std::fs::read_to_string(proc_root.join("stat"))
            .ok()?
            .lines()
            .find_map(|line| line.strip_prefix("btime "))?
            .parse()
            .ok()
    }

    fn start_ticks(stat: &str) -> Option<u64> {
        // The command name is parenthesized and may itself contain spaces or ')'.
        // Split after its final closing parenthesis; starttime is field 22, or
        // index 19 in the remaining sequence beginning with field 3 (state).
        stat.rsplit_once(") ")?
            .1
            .split_whitespace()
            .nth(19)?
            .parse()
            .ok()
    }

    fn process_creation_unix_at(proc_root: &Path, pid: u32, ticks_per_second: u64) -> Option<i64> {
        if ticks_per_second == 0 {
            return None;
        }
        let stat = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
        let seconds_after_boot = start_ticks(&stat)? / ticks_per_second;
        boot_time(proc_root)?.checked_add(i64::try_from(seconds_after_boot).ok()?)
    }

    fn clock_ticks_per_second() -> Option<u64> {
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        (ticks > 0).then_some(ticks as u64)
    }

    #[cfg(test)]
    pub(super) fn process_creation_unix(pid: u32) -> Option<i64> {
        process_creation_unix_at(Path::new("/proc"), pid, clock_ticks_per_second()?)
    }

    fn connection_entry(proc_root: &Path, uid: u32, conn: &ConnectionKey) -> Option<TcpEntry> {
        tcp_entries(proc_root)?.into_iter().find(|entry| {
            entry.uid == uid
                && entry.local_addr == conn.client_ip
                && entry.local_port == conn.client_port
                && entry.remote_addr == conn.server_ip
                && entry.remote_port == conn.server_port
        })
    }

    #[cfg(test)]
    pub(super) fn owning_pid(conn: &ConnectionKey) -> Option<u32> {
        let proc_root = Path::new("/proc");
        let uid = effective_uid(proc_root)?;
        let entry = connection_entry(proc_root, uid, conn)?;
        let targets = HashSet::from([entry.inode]);
        let ticks = clock_ticks_per_second().unwrap_or(0);
        scan_processes(proc_root, uid, ticks, &targets)?
            .owners
            .remove(&entry.inode)?
            .into_iter()
            .min()
    }

    pub(super) fn connection_process_start_unix(conn: &ConnectionKey) -> Option<i64> {
        let proc_root = Path::new("/proc");
        let uid = effective_uid(proc_root)?;
        let ticks = clock_ticks_per_second()?;
        let entry = connection_entry(proc_root, uid, conn);
        let inodes: HashSet<u64> = entry.iter().map(|entry| entry.inode).collect();
        let scan = scan_processes(proc_root, uid, ticks, &inodes)?;
        entry
            .and_then(|entry| {
                scan.owners
                    .get(&entry.inode)?
                    .iter()
                    .min()
                    .and_then(|&pid| process_creation_unix_at(proc_root, pid, ticks))
            })
            // Reading another process's fd symlinks is subject to Linux ptrace
            // policy even when both processes have the same uid.  Steam/Proton
            // therefore commonly exposes the socket in /proc/net/tcp but denies
            // the inode -> pid walk.  A verified RealmHound connection still
            // proves that a game is running; use the oldest actual game process
            // (not the launcher) as the conservative multibox-safe fallback.
            .or_else(|| named_start(&scan).confirmed_start())
    }

    fn is_rotmg_client_name(name: &str) -> bool {
        // Linux task names are capped at 15 bytes.  The official client's name
        // is exactly 15 bytes, while "RotMG Exalt Launcher.exe" is deliberately
        // excluded: launching/updating alone must not unlock an API refresh.
        name.trim().eq_ignore_ascii_case("RotMG Exalt.exe")
    }

    #[cfg(test)]
    fn earliest_named_client_start_unix_at(
        proc_root: &Path,
        uid: u32,
        ticks_per_second: u64,
    ) -> ClientStart {
        let Some(scan) = scan_processes(proc_root, uid, ticks_per_second, &HashSet::new()) else {
            return ClientStart::Indeterminate;
        };
        named_start(&scan)
    }

    fn earliest_client_starts_at(
        proc_root: &Path,
        game_port: u16,
        ticks_per_second: u64,
    ) -> (ClientStart, ClientStart) {
        let Some(uid) = effective_uid(proc_root) else {
            return (ClientStart::Indeterminate, ClientStart::Indeterminate);
        };
        let Some(entries) = tcp_entries(proc_root) else {
            return (ClientStart::Indeterminate, ClientStart::Indeterminate);
        };
        let inodes: HashSet<u64> = entries
            .iter()
            .filter(|entry| entry.uid == uid && entry.remote_port == game_port && entry.inode != 0)
            .map(|entry| entry.inode)
            .collect();
        let Some(scan) = scan_processes(proc_root, uid, ticks_per_second, &inodes) else {
            return (ClientStart::Indeterminate, ClientStart::Indeterminate);
        };
        let socket = if inodes.is_empty() {
            ClientStart::NoneRunning
        } else {
            socket_start(proc_root, &scan, &inodes, ticks_per_second)
        };
        (socket, named_start(&scan))
    }

    #[cfg(test)]
    fn earliest_client_start_unix_at(
        proc_root: &Path,
        game_port: u16,
        ticks_per_second: u64,
    ) -> ClientStart {
        earliest_client_starts_at(proc_root, game_port, ticks_per_second).0
    }

    pub(super) fn earliest_client_start_unix() -> ClientStart {
        let Some(ticks) = clock_ticks_per_second() else {
            return ClientStart::Indeterminate;
        };
        let (socket, named) =
            earliest_client_starts_at(Path::new("/proc"), GAME_SERVER_PORT, ticks);
        match socket {
            ClientStart::Earliest(_) => socket,
            // The name fallback also covers the short interval between world
            // sockets during an area change.  Prefer an indeterminate socket
            // result when no named client can be confirmed.
            ClientStart::NoneRunning | ClientStart::Indeterminate => match named {
                named @ ClientStart::Earliest(_) => named,
                _ => socket,
            },
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_linux_tcp_rows() {
            let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:C350 0200007F:0802 01 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000000000000000 100 0 0 10 0\n";
            assert_eq!(
                parse_tcp_table(table),
                Some(vec![TcpEntry {
                    local_addr: Ipv4Addr::new(127, 0, 0, 1),
                    local_port: 50_000,
                    remote_addr: Ipv4Addr::new(127, 0, 0, 2),
                    remote_port: 2050,
                    uid: 1000,
                    inode: 4242,
                }])
            );
        }

        #[test]
        fn resolves_fixture_socket_to_process_start() {
            use std::os::unix::fs::symlink;

            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            std::fs::create_dir_all(root.join("net")).unwrap();
            std::fs::create_dir_all(root.join("self")).unwrap();
            std::fs::create_dir_all(root.join("123/fd")).unwrap();
            std::fs::write(root.join("self/status"), "Uid:\t1000\t1000\t1000\t1000\n").unwrap();
            std::fs::write(root.join("123/status"), "Uid:\t1000\t1000\t1000\t1000\n").unwrap();
            std::fs::write(root.join("stat"), "cpu 0 0 0 0\nbtime 1700000000\n").unwrap();
            let mut fields = vec!["0"; 20];
            fields[0] = "S";
            fields[19] = "500";
            std::fs::write(
                root.join("123/stat"),
                format!("123 (RotMG Exalt.exe) {}\n", fields.join(" ")),
            )
            .unwrap();
            symlink("socket:[4242]", root.join("123/fd/7")).unwrap();
            std::fs::write(
                root.join("net/tcp"),
                "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:C350 0200007F:0802 01 00000000:00000000 00:00000000 00000000  1000        0 4242\n",
            )
            .unwrap();

            assert_eq!(
                earliest_client_start_unix_at(root, GAME_SERVER_PORT, 100),
                ClientStart::Earliest(1_700_000_005)
            );
            // Repeated refresh checks retain the same observable launch time.
            assert_eq!(
                earliest_client_start_unix_at(root, GAME_SERVER_PORT, 100),
                ClientStart::Earliest(1_700_000_005)
            );
            assert_eq!(
                process_creation_unix_at(root, 123, 100),
                Some(1_700_000_005)
            );

            // Once the connection disappears, the gate sees no running client
            // instead of retaining stale state from the previous scan.
            std::fs::write(
                root.join("net/tcp"),
                "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
            )
            .unwrap();
            assert_eq!(
                earliest_client_start_unix_at(root, GAME_SERVER_PORT, 100),
                ClientStart::NoneRunning
            );
        }

        #[test]
        fn named_client_fallback_ignores_launcher_and_uses_oldest_game() {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            std::fs::write(root.join("stat"), "cpu 0 0 0 0\nbtime 1700000000\n").unwrap();

            for (pid, name, ticks) in [
                (100, "RotMG Exalt Launcher.exe", 100),
                (200, "RotMG Exalt.exe", 900),
                (300, "rotmg exalt.exe", 500),
            ] {
                std::fs::create_dir_all(root.join(pid.to_string())).unwrap();
                std::fs::write(
                    root.join(pid.to_string()).join("status"),
                    "Uid:\t1000\t1000\t1000\t1000\n",
                )
                .unwrap();
                std::fs::write(root.join(pid.to_string()).join("comm"), format!("{name}\n"))
                    .unwrap();
                let mut fields = vec!["0".to_owned(); 20];
                fields[0] = "S".to_owned();
                fields[19] = ticks.to_string();
                std::fs::write(
                    root.join(pid.to_string()).join("stat"),
                    format!("{pid} ({name}) {}\n", fields.join(" ")),
                )
                .unwrap();
            }

            assert_eq!(
                earliest_named_client_start_unix_at(root, 1000, 100),
                ClientStart::Earliest(1_700_000_005)
            );
        }
    }
}

/// What a scan of the running game clients could establish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientStart {
    /// No running process holds a game-server connection.
    NoneRunning,
    /// Earliest creation time (Unix seconds) across every running client, all
    /// of which could be read.
    Earliest(i64),
    /// A candidate could not be inspected. Its start time may well predate the
    /// reset, so the scan proves nothing and must not stand in for a relaunch.
    Indeterminate,
}

impl ClientStart {
    /// The start time only when every running client was accounted for.
    pub fn confirmed_start(self) -> Option<i64> {
        match self {
            ClientStart::Earliest(start) if start > 0 => Some(start),
            _ => None,
        }
    }
}

/// The MIB and `libproc` port fields both store the port in network byte order
/// in the low 16 bits; decode to a host-order `u16`.
#[cfg(any(windows, target_os = "macos"))]
fn decode_port(raw: u32) -> u16 {
    u16::from_be((raw & 0xFFFF) as u16)
}

/// The kernel address fields store the IPv4 address as network-order octets in
/// a `u32`; the native-endian bytes are already those octets in order.
#[cfg(any(windows, target_os = "macos"))]
fn decode_addr(raw: u32) -> std::net::Ipv4Addr {
    std::net::Ipv4Addr::from(raw.to_ne_bytes())
}

#[cfg(windows)]
mod windows_impl {
    use super::{decode_addr, decode_port, filetime_to_unix_secs, GAME_SERVER_PORT};
    use realmhound_core::stream::ConnectionKey;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// Fetch the IPv4 TCP table (with owning PIDs) from the OS and hand the row
    /// slice to `f`. Returns `None` on any failure. Centralizes the sized-query
    /// + aligned-allocation + parse dance so callers can't get it subtly wrong.
    fn with_tcp_rows<R>(f: impl FnOnce(&[MIB_TCPROW_OWNER_PID]) -> R) -> Option<R> {
        // Sized query first (returns ERROR_INSUFFICIENT_BUFFER), then fetch.
        let mut size: u32 = 0;
        unsafe {
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                &mut size,
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            );
        }
        if size == 0 {
            return None;
        }

        // Allocate an aligned buffer: MIB_TCPTABLE_OWNER_PID has u32 fields and
        // requires 4-byte alignment, which a Vec<u8> does not guarantee. A
        // Vec<u32> is 4-byte aligned, so the cast below is sound.
        let dword_len = (size as usize).div_ceil(4);
        let mut buf: Vec<u32> = vec![0u32; dword_len];
        let ret = unsafe {
            GetExtendedTcpTable(
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                &mut size,
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        // NO_ERROR == 0
        if ret != 0 {
            return None;
        }

        let table = unsafe { &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID) };
        let count = table.dwNumEntries as usize;
        let rows: &[MIB_TCPROW_OWNER_PID] =
            unsafe { std::slice::from_raw_parts(table.table.as_ptr(), count) };
        Some(f(rows))
    }

    /// Find the PID owning the TCP connection matching `conn`.
    pub(super) fn owning_pid(conn: &ConnectionKey) -> Option<u32> {
        with_tcp_rows(|rows| {
            rows.iter()
                // Match the full 4-tuple (local addr+port and remote addr+port)
                // so that on multihomed/VPN hosts a different process sharing
                // the same local port to the same remote via a different local
                // address can't resolve to the wrong process.
                .find(|row| {
                    decode_port(row.dwLocalPort) == conn.client_port
                        && decode_addr(row.dwLocalAddr) == conn.client_ip
                        && decode_port(row.dwRemotePort) == conn.server_port
                        && decode_addr(row.dwRemoteAddr) == conn.server_ip
                })
                .map(|row| row.dwOwningPid)
        })
        .flatten()
    }

    /// Earliest process creation time (Unix seconds) among all processes that
    /// currently own a TCP connection to a RotMG game server (remote port
    /// 2050).
    pub(super) fn earliest_client_start_unix() -> Option<i64> {
        let pids: Vec<u32> = with_tcp_rows(|rows| {
            let mut pids: Vec<u32> = rows
                .iter()
                .filter(|row| decode_port(row.dwRemotePort) == GAME_SERVER_PORT)
                .map(|row| row.dwOwningPid)
                .collect();
            pids.sort_unstable();
            pids.dedup();
            pids
        })?;

        pids.into_iter()
            .filter_map(process_creation_unix)
            .filter(|&t| t > 0)
            .min()
    }

    /// Read the creation time (Unix seconds) of the process with `pid`.
    pub(super) fn process_creation_unix(pid: u32) -> Option<i64> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle == 0 {
            return None;
        }

        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut kernel = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut user = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };

        let ok =
            unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
        unsafe { CloseHandle(handle) };

        if ok == 0 {
            return None;
        }
        Some(filetime_to_unix_secs(
            creation.dwLowDateTime,
            creation.dwHighDateTime,
        ))
    }
}

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::{decode_addr, decode_port, ClientStart, GAME_SERVER_PORT};
    use libproc::bsd_info::BSDInfo;
    use libproc::file_info::{pidfdinfo, ListFDs, ProcFDType};
    use libproc::net_info::{SocketFDInfo, SocketInfoKind};
    use libproc::proc_pid::{listpidinfo, pidinfo};
    use libproc::processes::{pids_by_type, ProcFilter};
    use realmhound_core::stream::ConnectionKey;

    /// `INI_IPV4` from `sys/proc_info.h`: `insi_vflag` marks which of the v4/v6
    /// address members of the union actually carries the endpoint.
    const INI_IPV4: u8 = 0x1;

    /// The IPv4 TCP endpoints of a process's sockets, as (local, remote) pairs
    /// of `(address, port)`, or `None` when the process cannot be inspected.
    ///
    /// macOS has no global socket table, so each process is asked for its own
    /// descriptors. An individual descriptor that cannot be read is skipped: it
    /// closed between the listing and the query, and a live client never closes
    /// the game-server socket it is playing on.
    fn tcp_endpoints(pid: u32) -> Option<Vec<(std::net::Ipv4Addr, u16, std::net::Ipv4Addr, u16)>> {
        let info = pidinfo::<BSDInfo>(pid as i32, 0).ok()?;
        // `listpidinfo` reports an empty descriptor table as an error, so a
        // process holding nothing open is answered without asking.
        if info.pbi_nfiles == 0 {
            return Some(Vec::new());
        }
        let fds = listpidinfo::<ListFDs>(pid as i32, info.pbi_nfiles as usize).ok()?;

        Some(
            fds.iter()
                .filter(|fd| matches!(ProcFDType::from(fd.proc_fdtype), ProcFDType::Socket))
                .filter_map(|fd| pidfdinfo::<SocketFDInfo>(pid as i32, fd.proc_fd).ok())
                .filter(|socket| {
                    matches!(
                        SocketInfoKind::from(socket.psi.soi_kind),
                        SocketInfoKind::Tcp
                    )
                })
                .filter_map(|socket| {
                    // SAFETY: the union member is selected by `soi_kind`, which the
                    // filter above has already confirmed to be TCP.
                    let tcp = unsafe { socket.psi.soi_proto.pri_tcp };
                    let endpoint = tcp.tcpsi_ini;
                    if endpoint.insi_vflag & INI_IPV4 == 0 {
                        return None;
                    }
                    // SAFETY: `insi_vflag` selects the v4 member of both unions,
                    // checked immediately above.
                    let (local, remote) = unsafe {
                        (
                            endpoint.insi_laddr.ina_46.i46a_addr4.s_addr,
                            endpoint.insi_faddr.ina_46.i46a_addr4.s_addr,
                        )
                    };
                    Some((
                        decode_addr(local),
                        decode_port(endpoint.insi_lport as u32),
                        decode_addr(remote),
                        decode_port(endpoint.insi_fport as u32),
                    ))
                })
                .collect(),
        )
    }

    /// Find the PID owning the TCP connection matching `conn`.
    pub(super) fn owning_pid(conn: &ConnectionKey) -> Option<u32> {
        pids_by_type(ProcFilter::All)
            .ok()?
            .into_iter()
            .find(|&pid| {
                // Match the full 4-tuple for the same reason Windows does: on a
                // multihomed or VPN host a different process may share the local
                // port to the same remote via a different local address.
                tcp_endpoints(pid).is_some_and(|endpoints| {
                    endpoints
                        .iter()
                        .any(|&(local_addr, local_port, remote_addr, remote_port)| {
                            local_port == conn.client_port
                                && local_addr == conn.client_ip
                                && remote_port == conn.server_port
                                && remote_addr == conn.server_ip
                        })
                })
            })
    }

    /// Read the creation time (Unix seconds) of the process with `pid`.
    pub(super) fn process_creation_unix(pid: u32) -> Option<i64> {
        pidinfo::<BSDInfo>(pid as i32, 0)
            .ok()
            .map(|info| info.pbi_start_tvsec as i64)
    }

    /// Earliest process creation time (Unix seconds) among all processes that
    /// currently own a TCP connection to a RotMG game server.
    ///
    /// Only this user's processes are considered: the game client runs as we do,
    /// and another user's processes are not inspectable at all, so including
    /// them would make every scan indeterminate. Within that set an unreadable
    /// process is a candidate we failed to rule out, not one that isn't there.
    pub(super) fn earliest_client_start_unix() -> ClientStart {
        let uid = own_uid();
        let Ok(pids) = pids_by_type(ProcFilter::ByUID { uid }) else {
            return ClientStart::Indeterminate;
        };

        let mut earliest: Option<i64> = None;
        let mut unreadable: Vec<u32> = Vec::new();
        for pid in pids {
            let Some(endpoints) = tcp_endpoints(pid) else {
                unreadable.push(pid);
                continue;
            };
            if !endpoints
                .iter()
                .any(|&(_, _, _, remote_port)| remote_port == GAME_SERVER_PORT)
            {
                continue;
            }
            match process_creation_unix(pid) {
                Some(start) if start > 0 => {
                    earliest = Some(earliest.map_or(start, |best: i64| best.min(start)))
                }
                None | Some(_) => unreadable.push(pid),
            }
        }

        if any_still_running(&unreadable, uid) {
            return ClientStart::Indeterminate;
        }
        resolve(earliest, false)
    }

    /// Decide the scan result from what the pass over the candidates found.
    ///
    /// `earliest` is the oldest start among the clients that could be read, and
    /// `unchecked` whether a process we failed to inspect is still running. The
    /// two are kept apart from the syscalls so the rule itself can be tested.
    pub(super) fn resolve(earliest: Option<i64>, unchecked: bool) -> ClientStart {
        if unchecked {
            return ClientStart::Indeterminate;
        }
        earliest.map_or(ClientStart::NoneRunning, ClientStart::Earliest)
    }

    /// Whether any of `pids` is still listed. Most inspection failures are a
    /// process that exited between the listing and the query, and a process
    /// that is gone is not a client we failed to check.
    pub(super) fn any_still_running(pids: &[u32], uid: u32) -> bool {
        if pids.is_empty() {
            return false;
        }
        match pids_by_type(ProcFilter::ByUID { uid }) {
            Ok(live) => pids.iter().any(|pid| live.contains(pid)),
            // Cannot tell any more; assume the candidates are still there.
            Err(_) => true,
        }
    }

    /// This process's effective user id, read through `libproc` so no separate
    /// `libc` dependency is needed. `u32::MAX` matches no process, which keeps
    /// an unreadable self indeterminate rather than silently "no clients".
    pub(super) fn own_uid() -> u32 {
        pidinfo::<BSDInfo>(std::process::id() as i32, 0)
            .map(|info| info.pbi_uid)
            .unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn filetime_epoch_is_unix_zero() {
        // 11644473600 seconds after 1601-01-01, in 100-ns ticks, is 1970-01-01.
        let ticks: u64 = 11_644_473_600 * 10_000_000;
        let low = (ticks & 0xFFFF_FFFF) as u32;
        let high = (ticks >> 32) as u32;
        assert_eq!(filetime_to_unix_secs(low, high), 0);
    }

    #[cfg(windows)]
    #[test]
    fn filetime_known_value() {
        // 2021-01-01T00:00:00Z == Unix 1609459200.
        let secs_since_1601: u64 = 1_609_459_200 + 11_644_473_600;
        let ticks = secs_since_1601 * 10_000_000;
        let low = (ticks & 0xFFFF_FFFF) as u32;
        let high = (ticks >> 32) as u32;
        assert_eq!(filetime_to_unix_secs(low, high), 1_609_459_200);
    }

    #[cfg(target_os = "linux")]
    use super::linux_impl::{owning_pid, process_creation_unix};
    #[cfg(target_os = "macos")]
    use super::macos_impl::{owning_pid, process_creation_unix};
    /// The remaining tests exercise the real platform lookups (table/descriptor
    /// parse, port and address decoding, process start time) against a live
    /// loopback socket and this test process, so struct-layout, byte-order and
    /// field-offset bugs surface at runtime rather than in the field.
    #[cfg(windows)]
    use super::windows_impl::{owning_pid, process_creation_unix};

    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    #[test]
    fn resolves_own_process_creation_time() {
        let start = process_creation_unix(std::process::id()).expect("own process start time");
        // Plausible: after 2020-01-01 and not in the future.
        assert!(start > 1_577_836_800, "start too old: {start}");
        assert!(
            start <= chrono::Utc::now().timestamp() + 60,
            "start in future: {start}"
        );
    }

    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    #[test]
    fn resolves_own_loopback_connection_to_this_pid() {
        use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let server_addr = listener.local_addr().expect("server addr");
        let client = TcpStream::connect(server_addr).expect("connect");
        let _accepted = listener.accept().expect("accept");
        let local = client.local_addr().expect("client addr");

        let to_v4 = |ip: IpAddr| match ip {
            IpAddr::V4(v) => v,
            IpAddr::V6(_) => Ipv4Addr::UNSPECIFIED,
        };
        let conn = ConnectionKey::new(
            to_v4(local.ip()),
            local.port(),
            to_v4(server_addr.ip()),
            server_addr.port(),
        );

        assert_eq!(
            owning_pid(&conn),
            Some(std::process::id()),
            "loopback connection should be owned by this test process"
        );
        let start = connection_process_start_unix(&conn).expect("start via connection");
        assert!(start > 1_577_836_800, "start too old: {start}");
    }

    #[test]
    fn earliest_client_scan_does_not_panic() {
        // No RotMG client is expected in the test environment, so this is
        // normally NoneRunning; if some process happens to hold a :2050
        // connection the time must at least be plausible, and a candidate we
        // could not read must report Indeterminate rather than NoneRunning.
        // Either way it must not panic or return a bogus value -- this exercises
        // the shared scan on every platform, including the stub.
        if let ClientStart::Earliest(t) = earliest_rotmg_client_start_unix() {
            assert!(t > 1_577_836_800, "scan returned implausible time: {t}");
        }
    }

    /// Only a fully accounted-for scan yields a start time to the caller.
    #[test]
    fn only_a_complete_scan_confirms_a_start_time() {
        assert_eq!(
            ClientStart::Earliest(1_700_000_000).confirmed_start(),
            Some(1_700_000_000)
        );
        assert_eq!(ClientStart::NoneRunning.confirmed_start(), None);
        assert_eq!(ClientStart::Indeterminate.confirmed_start(), None);
        // An implausible timestamp is no better than no answer.
        assert_eq!(ClientStart::Earliest(0).confirmed_start(), None);
        assert_eq!(ClientStart::Earliest(-1).confirmed_start(), None);
    }

    /// an older client that cannot be read must not be masked by a newer one that can.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_unreadable_client_is_not_reported_as_a_relaunch() {
        use super::macos_impl::resolve;

        const YESTERDAY: i64 = 1_700_000_000;
        const TODAY: i64 = 1_700_086_400;

        // Client A started yesterday but is unreadable; client B started today
        // and is readable. The minimum over the readable ones alone says
        // "today", which is exactly what must not be believed.
        assert_eq!(resolve(Some(TODAY), true), ClientStart::Indeterminate);

        // Every candidate accounted for: the earliest start is trustworthy.
        assert_eq!(
            resolve(Some(YESTERDAY), false),
            ClientStart::Earliest(YESTERDAY)
        );

        // Nothing running is a fact, not a gap in what we could check.
        assert_eq!(resolve(None, false), ClientStart::NoneRunning);
    }

    /// only a candidate that is *still there* counts as unchecked,
    /// so a process that exited mid-scan cannot block the guard.
    #[cfg(target_os = "macos")]
    #[test]
    fn only_live_candidates_count_as_unchecked() {
        use super::macos_impl::{any_still_running, own_uid};

        let uid = own_uid();
        assert!(!any_still_running(&[], uid), "nothing unreadable to weigh");
        assert!(
            any_still_running(&[std::process::id()], uid),
            "this test process is running and owned by us"
        );
        assert!(
            !any_still_running(&[u32::MAX], uid),
            "a pid that does not exist is not a client we failed to check"
        );
    }
}
