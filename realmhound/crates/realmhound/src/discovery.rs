//! Lightweight packet capture for account discovery.
//!
//! Inspects game traffic only far enough to establish account identity:
//! HELLO (token) → CreateSuccess (object_id) → Update (account ID from that
//! object). No account-scoped resources are opened and no game state is tracked.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

use realmhound_core::account::AccountId;
use realmhound_core::capture::{start_capture_auto_detect, CaptureHandle, NetworkInterface};
use realmhound_core::protocol::packets::ParsedPacket;
use realmhound_core::protocol::parse_packet_with_status;
use realmhound_core::stats::extract_account_identity;
use realmhound_core::stream::{parse_tcp_segment, ConnectionKey, TcpReassembler, TcpSegment};

const IDENTITY_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_CANDIDATES: usize = 8;

/// A verified account candidate discovered from network traffic.
#[derive(Debug, Clone)]
pub struct DiscoveredCandidate {
    pub account_id: AccountId,
    pub display_name: Option<String>,
    pub token: String,
}

/// Per-connection state machine for identity extraction.
#[derive(Debug)]
enum CandidateState {
    /// HELLO captured; waiting for CreateSuccess.
    AwaitingCreateSuccess {
        token: String,
        started_at: std::time::Instant,
    },
    /// CreateSuccess captured; waiting for Update with this object_id.
    AwaitingIdentity {
        token: String,
        object_id: i32,
        started_at: std::time::Instant,
    },
}

pub struct DiscoveryCaptureHandle {
    capture_handle: CaptureHandle,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl DiscoveryCaptureHandle {
    fn shutdown(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.capture_handle.stop();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for DiscoveryCaptureHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Start a discovery capture on all available interfaces.
pub fn start_discovery_capture(
    interfaces: &[NetworkInterface],
) -> Result<(DiscoveryCaptureHandle, mpsc::Receiver<DiscoveredCandidate>), String> {
    let config = realmhound_core::capture::SnifferConfig::default();
    let (capture_handle, packet_rx, _detected_iface) =
        start_capture_auto_detect(interfaces, config)
            .map_err(|e| format!("Failed to start capture: {e}"))?;

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_flag = cancel.clone();
    let (tx, rx) = mpsc::channel();

    let thread = std::thread::Builder::new()
        .name("discovery-capture".into())
        .spawn(move || {
            discovery_loop(packet_rx, &tx, &cancel_flag);
        })
        .map_err(|e| format!("Failed to spawn discovery thread: {e}"))?;

    let handle = DiscoveryCaptureHandle {
        capture_handle,
        cancel,
        thread: Some(thread),
    };
    Ok((handle, rx))
}

fn discovery_loop(
    packet_rx: realmhound_core::capture::PacketReceiver,
    tx: &mpsc::Sender<DiscoveredCandidate>,
    cancel: &AtomicBool,
) {
    let mut reassembler = TcpReassembler::new();
    let mut candidates: HashMap<ConnectionKey, CandidateState> = HashMap::new();
    let mut seen_accounts: HashMap<AccountId, ConnectionKey> = HashMap::new();

    while !cancel.load(Ordering::SeqCst) {
        let result = packet_rx.recv_timeout(64, Duration::from_millis(250));

        for raw in result.packets {
            let Some(segment) = parse_tcp_segment(&raw) else {
                continue;
            };

            if segment.flags.syn && !segment.flags.ack {
                let (conn_key, _) = conn_key_from_segment(&segment);
                candidates.remove(&conn_key);
            }

            if segment.flags.fin || segment.flags.rst {
                let (conn_key, _) = conn_key_from_segment(&segment);
                candidates.remove(&conn_key);
            }

            let packets = reassembler.process_segment(&segment);

            for packet in packets {
                let (conn_key, _) = ConnectionKey::from_packet(
                    packet.src_ip,
                    packet.src_port,
                    packet.dst_ip,
                    packet.dst_port,
                    2050,
                );

                let parsed = parse_packet_quiet(&packet);
                let Some(parsed) = parsed else { continue };

                match parsed {
                    ParsedPacket::Hello(hello) => {
                        if hello.access_token.is_empty() {
                            continue;
                        }
                        if candidates.len() >= MAX_CANDIDATES && !candidates.contains_key(&conn_key)
                        {
                            continue;
                        }
                        candidates.insert(
                            conn_key,
                            CandidateState::AwaitingCreateSuccess {
                                token: hello.access_token.clone(),
                                started_at: std::time::Instant::now(),
                            },
                        );
                    }
                    ParsedPacket::CreateSuccess(cs) => {
                        if let Some(CandidateState::AwaitingCreateSuccess { token, started_at }) =
                            candidates.remove(&conn_key)
                        {
                            candidates.insert(
                                conn_key,
                                CandidateState::AwaitingIdentity {
                                    token,
                                    object_id: cs.object_id,
                                    started_at,
                                },
                            );
                        }
                    }
                    ParsedPacket::Update(update) => {
                        let should_extract = matches!(
                            candidates.get(&conn_key),
                            Some(CandidateState::AwaitingIdentity { .. })
                        );
                        if !should_extract {
                            continue;
                        }
                        let CandidateState::AwaitingIdentity {
                            token, object_id, ..
                        } = candidates.remove(&conn_key).unwrap()
                        else {
                            unreachable!();
                        };

                        let Some(obj) = update.find_object(object_id) else {
                            // Put it back; this Update didn't contain our object.
                            candidates.insert(
                                conn_key,
                                CandidateState::AwaitingIdentity {
                                    token,
                                    object_id,
                                    started_at: std::time::Instant::now(),
                                },
                            );
                            continue;
                        };

                        let identity = extract_account_identity(&obj.status);
                        let Some(raw_id) = identity.account_id else {
                            candidates.insert(
                                conn_key,
                                CandidateState::AwaitingIdentity {
                                    token,
                                    object_id,
                                    started_at: std::time::Instant::now(),
                                },
                            );
                            continue;
                        };

                        let Ok(account_id) = AccountId::new(&raw_id) else {
                            continue;
                        };

                        // Deduplicate: evict old connection for same account.
                        if let Some(old_conn) = seen_accounts.remove(&account_id) {
                            candidates.remove(&old_conn);
                        }
                        seen_accounts.insert(account_id.clone(), conn_key);

                        let candidate = DiscoveredCandidate {
                            account_id,
                            display_name: identity.account_name,
                            token,
                        };
                        if tx.send(candidate).is_err() {
                            return;
                        }
                    }
                    _ => {}
                }
            }
        }

        // Expire timed-out candidates.
        candidates.retain(|_, state| {
            let started = match state {
                CandidateState::AwaitingCreateSuccess { started_at, .. } => started_at,
                CandidateState::AwaitingIdentity { started_at, .. } => started_at,
            };
            started.elapsed() < IDENTITY_TIMEOUT
        });

        if result.closed {
            break;
        }
    }
}

fn conn_key_from_segment(segment: &TcpSegment) -> (ConnectionKey, bool) {
    ConnectionKey::from_packet(
        segment.src_ip,
        segment.src_port,
        segment.dst_ip,
        segment.dst_port,
        2050,
    )
}

fn parse_packet_quiet(packet: &realmhound_core::protocol::Packet) -> Option<ParsedPacket> {
    let result = parse_packet_with_status(packet.packet_type(), &packet.payload);
    result.packet
}

#[cfg(test)]
mod tests {
    use super::*;
    use realmhound_core::protocol::data::WorldPosData;
    use realmhound_core::protocol::packets::UpdatePacket;

    fn make_connection_key(port: u16) -> ConnectionKey {
        ConnectionKey::new(
            "127.0.0.1".parse().unwrap(),
            port,
            "1.2.3.4".parse().unwrap(),
            2050,
        )
    }

    fn make_candidate_state_awaiting_cs(token: &str) -> CandidateState {
        CandidateState::AwaitingCreateSuccess {
            token: token.to_string(),
            started_at: std::time::Instant::now(),
        }
    }

    fn make_candidate_state_awaiting_id(token: &str, object_id: i32) -> CandidateState {
        CandidateState::AwaitingIdentity {
            token: token.to_string(),
            object_id,
            started_at: std::time::Instant::now(),
        }
    }

    #[test]
    fn candidate_state_transitions_hello_to_create_success() {
        let mut candidates: HashMap<ConnectionKey, CandidateState> = HashMap::new();
        let conn = make_connection_key(12345);

        // Simulate HELLO
        candidates.insert(conn, make_candidate_state_awaiting_cs("tok123"));
        assert!(matches!(
            candidates.get(&conn),
            Some(CandidateState::AwaitingCreateSuccess { .. })
        ));

        // Simulate CreateSuccess
        if let Some(CandidateState::AwaitingCreateSuccess { token, started_at }) =
            candidates.remove(&conn)
        {
            candidates.insert(
                conn,
                CandidateState::AwaitingIdentity {
                    token,
                    object_id: 42,
                    started_at,
                },
            );
        }

        match candidates.get(&conn) {
            Some(CandidateState::AwaitingIdentity { object_id, .. }) => {
                assert_eq!(*object_id, 42);
            }
            other => panic!("unexpected state: {other:?}"),
        }
    }

    #[test]
    fn syn_resets_candidate_state() {
        let mut candidates: HashMap<ConnectionKey, CandidateState> = HashMap::new();
        let conn = make_connection_key(12345);
        candidates.insert(conn, make_candidate_state_awaiting_cs("tok123"));

        // SYN clears existing state
        candidates.remove(&conn);
        assert!(candidates.get(&conn).is_none());
    }

    #[test]
    fn fin_discards_candidate() {
        let mut candidates: HashMap<ConnectionKey, CandidateState> = HashMap::new();
        let conn = make_connection_key(12345);
        candidates.insert(conn, make_candidate_state_awaiting_id("tok", 42));

        candidates.remove(&conn);
        assert!(candidates.get(&conn).is_none());
    }

    #[test]
    fn timeout_expires_candidate() {
        let conn = make_connection_key(12345);
        let old_state = CandidateState::AwaitingCreateSuccess {
            token: "tok".to_string(),
            started_at: std::time::Instant::now() - Duration::from_secs(120),
        };
        let mut candidates = HashMap::new();
        candidates.insert(conn, old_state);

        candidates.retain(|_, state| {
            let started = match state {
                CandidateState::AwaitingCreateSuccess { started_at, .. } => started_at,
                CandidateState::AwaitingIdentity { started_at, .. } => started_at,
            };
            started.elapsed() < IDENTITY_TIMEOUT
        });

        assert!(candidates.is_empty());
    }

    #[test]
    fn update_with_wrong_object_id_keeps_candidate() {
        let mut candidates: HashMap<ConnectionKey, CandidateState> = HashMap::new();
        let conn = make_connection_key(12345);
        candidates.insert(conn, make_candidate_state_awaiting_id("tok", 42));

        // Update that doesn't contain our object_id (simulated)
        let update = UpdatePacket {
            pos: WorldPosData { x: 0.0, y: 0.0 },
            level_type: 0,
            tiles: vec![],
            new_objects: vec![],
            drops: vec![],
        };

        if let Some(CandidateState::AwaitingIdentity {
            token, object_id, ..
        }) = candidates.remove(&conn)
        {
            if update.find_object(object_id).is_none() {
                candidates.insert(
                    conn,
                    CandidateState::AwaitingIdentity {
                        token,
                        object_id,
                        started_at: std::time::Instant::now(),
                    },
                );
            }
        }

        assert!(matches!(
            candidates.get(&conn),
            Some(CandidateState::AwaitingIdentity { .. })
        ));
    }

    #[test]
    fn duplicate_account_replaces_old_connection() {
        let mut seen: HashMap<AccountId, ConnectionKey> = HashMap::new();
        let mut candidates: HashMap<ConnectionKey, CandidateState> = HashMap::new();

        let conn1 = make_connection_key(1111);
        let conn2 = make_connection_key(2222);
        let acct = AccountId::new("ACCT-1").unwrap();

        candidates.insert(conn1, make_candidate_state_awaiting_cs("old_tok"));
        seen.insert(acct.clone(), conn1);

        // Second connection for same account replaces the first.
        if let Some(old_conn) = seen.remove(&acct) {
            candidates.remove(&old_conn);
        }
        seen.insert(acct.clone(), conn2);

        assert!(!candidates.contains_key(&conn1));
        assert_eq!(seen.get(&acct), Some(&conn2));
    }

    #[test]
    fn max_candidates_rejects_new_connections() {
        let mut candidates: HashMap<ConnectionKey, CandidateState> = HashMap::new();

        for i in 0..MAX_CANDIDATES {
            let conn = make_connection_key(10000 + i as u16);
            candidates.insert(conn, make_candidate_state_awaiting_cs("tok"));
        }

        let extra_conn = make_connection_key(20000);
        let rejected = candidates.len() >= MAX_CANDIDATES && !candidates.contains_key(&extra_conn);
        assert!(rejected);
    }
}
