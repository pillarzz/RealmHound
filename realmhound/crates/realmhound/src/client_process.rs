//! Maps the live main game-client connection to its owning OS process so the
//! forge-fire safeguard can tell whether the game client was **relaunched**
//! after the 00:00 UTC daily reset.
//!
//! Neither the forge-fire value (Sulphur refills, crafting drains, variable
//! daily grant) nor the packet/connection lifecycle (an area change opens a new
//! socket whose Hello is indistinguishable from a cold launch) can detect a
//! relaunch. The only reliable signal is the owning process's creation time,
//! read from the current OS state via `GetExtendedTcpTable` + `GetProcessTimes`
//! -- which works regardless of when RealmHound itself was started.

use realmhound_core::stream::ConnectionKey;

/// Convert a Windows `FILETIME` (100-ns intervals since 1601-01-01 UTC, split
/// into low/high 32-bit halves) to a Unix timestamp in seconds.
pub(crate) fn filetime_to_unix_secs(low: u32, high: u32) -> i64 {
    const HUNDRED_NS_PER_SEC: u64 = 10_000_000;
    // Whole seconds between 1601-01-01 and 1970-01-01 (Unix epoch).
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    let ticks = ((high as u64) << 32) | (low as u64);
    (ticks / HUNDRED_NS_PER_SEC) as i64 - EPOCH_DIFF_SECS
}

/// Unix timestamp (seconds) of the creation time of the process that owns the
/// given main game-client connection, or `None` if it can't be determined
/// (connection not found in the OS TCP table, access denied, or non-Windows).
#[cfg(windows)]
pub fn connection_process_start_unix(conn: &ConnectionKey) -> Option<i64> {
    let pid = windows_impl::owning_pid(conn)?;
    windows_impl::process_creation_unix(pid)
}

#[cfg(not(windows))]
pub fn connection_process_start_unix(_conn: &ConnectionKey) -> Option<i64> {
    None
}

/// Unix timestamp (seconds) of the *earliest* creation time among all running
/// RotMG client processes, identified by an active TCP connection to a game
/// server (remote port 2050). `None` if no such connection/process is found.
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
#[cfg(windows)]
pub fn earliest_rotmg_client_start_unix() -> Option<i64> {
    windows_impl::earliest_client_start_unix()
}

#[cfg(not(windows))]
pub fn earliest_rotmg_client_start_unix() -> Option<i64> {
    None
}

#[cfg(windows)]
mod windows_impl {
    use super::filetime_to_unix_secs;
    use realmhound_core::stream::ConnectionKey;
    use std::net::Ipv4Addr;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// The RotMG game-server TCP port. Every game client -- Steam, standalone,
    /// any install -- talks to the world server on this port, and effectively
    /// nothing else on the machine connects out to it, so it reliably marks a
    /// socket as belonging to a RotMG client without matching any file name.
    const GAME_SERVER_PORT: u16 = 2050;

    /// The MIB port fields store the port in network byte order in the low 16
    /// bits; decode to a host-order `u16`.
    fn decode_port(raw: u32) -> u16 {
        u16::from_be((raw & 0xFFFF) as u16)
    }

    /// The MIB address fields store the IPv4 address as network-order octets in
    /// a `u32`; the native-endian bytes are already those octets in order.
    fn decode_addr(raw: u32) -> Ipv4Addr {
        Ipv4Addr::from(raw.to_ne_bytes())
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_epoch_is_unix_zero() {
        // 11644473600 seconds after 1601-01-01, in 100-ns ticks, is 1970-01-01.
        let ticks: u64 = 11_644_473_600 * 10_000_000;
        let low = (ticks & 0xFFFF_FFFF) as u32;
        let high = (ticks >> 32) as u32;
        assert_eq!(filetime_to_unix_secs(low, high), 0);
    }

    #[test]
    fn filetime_known_value() {
        // 2021-01-01T00:00:00Z == Unix 1609459200.
        let secs_since_1601: u64 = 1_609_459_200 + 11_644_473_600;
        let ticks = secs_since_1601 * 10_000_000;
        let low = (ticks & 0xFFFF_FFFF) as u32;
        let high = (ticks >> 32) as u32;
        assert_eq!(filetime_to_unix_secs(low, high), 1_609_459_200);
    }

    // The following exercise the real Windows FFI (table parse, port/address
    // decoding, OpenProcess/GetProcessTimes) against a live loopback socket and
    // this test process, so struct-layout or byte-order bugs surface at runtime.
    #[cfg(windows)]
    #[test]
    fn resolves_own_process_creation_time() {
        let start = super::windows_impl::process_creation_unix(std::process::id())
            .expect("own process creation time");
        // Plausible: after 2020-01-01 and not in the future.
        assert!(start > 1_577_836_800, "start too old: {start}");
        assert!(
            start <= chrono::Utc::now().timestamp() + 60,
            "start in future: {start}"
        );
    }

    #[cfg(windows)]
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
            super::windows_impl::owning_pid(&conn),
            Some(std::process::id()),
            "loopback connection should be owned by this test process"
        );
        let start = super::connection_process_start_unix(&conn).expect("start via connection");
        assert!(start > 1_577_836_800, "start too old: {start}");
    }

    #[cfg(windows)]
    #[test]
    fn earliest_client_scan_does_not_panic() {
        // No RotMG client is expected in the test environment, so this is
        // normally None; if some process happens to hold a :2050 connection the
        // time must at least be plausible. Either way it must not panic or
        // return a bogus value -- this exercises the shared table read + scan.
        if let Some(t) = super::earliest_rotmg_client_start_unix() {
            assert!(t > 1_577_836_800, "scan returned implausible time: {t}");
        }
    }
}
