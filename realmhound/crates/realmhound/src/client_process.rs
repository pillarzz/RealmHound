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
//! exposes: `GetExtendedTcpTable` + `GetProcessTimes` on Windows, and `libproc`
//! on macOS. Anywhere else the safeguard stays conservative and never unlocks.

use realmhound_core::stream::ConnectionKey;

/// The RotMG game-server TCP port. Every game client -- Steam, standalone, any
/// install -- talks to the world server on this port, and effectively nothing
/// else on the machine connects out to it, so it reliably marks a socket as
/// belonging to a RotMG client without matching any file name.
#[cfg(any(windows, target_os = "macos"))]
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

#[cfg(not(any(windows, target_os = "macos")))]
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

#[cfg(not(any(windows, target_os = "macos")))]
pub fn earliest_rotmg_client_start_unix() -> ClientStart {
    ClientStart::NoneRunning
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

    #[cfg(target_os = "macos")]
    use super::macos_impl::{owning_pid, process_creation_unix};
    /// The remaining tests exercise the real platform lookups (table/descriptor
    /// parse, port and address decoding, process start time) against a live
    /// loopback socket and this test process, so struct-layout, byte-order and
    /// field-offset bugs surface at runtime rather than in the field.
    #[cfg(windows)]
    use super::windows_impl::{owning_pid, process_creation_unix};

    #[cfg(any(windows, target_os = "macos"))]
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

    #[cfg(any(windows, target_os = "macos"))]
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
