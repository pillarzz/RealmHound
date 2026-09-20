//! TCP/IP packet parsing using etherparse.
//!
//! This module extracts TCP segment information from raw captured packets.

use crate::capture::RawPacket;
use etherparse::{NetSlice, SlicedPacket, TransportSlice};
use std::net::Ipv4Addr;

/// Parsed TCP segment extracted from a raw packet.
#[derive(Debug, Clone)]
pub struct TcpSegment {
    /// Source IPv4 address
    pub src_ip: Ipv4Addr,
    /// Destination IPv4 address
    pub dst_ip: Ipv4Addr,
    /// Source port
    pub src_port: u16,
    /// Destination port
    pub dst_port: u16,
    /// TCP sequence number
    pub sequence: u32,
    /// TCP acknowledgment number
    pub acknowledgment: u32,
    /// TCP flags
    pub flags: TcpFlags,
    /// TCP payload data
    pub payload: Vec<u8>,
    /// Original packet timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// TCP flags from a packet header.
#[derive(Debug, Clone, Copy, Default)]
pub struct TcpFlags {
    /// SYN flag - connection initiation
    pub syn: bool,
    /// ACK flag - acknowledgment
    pub ack: bool,
    /// FIN flag - connection termination
    pub fin: bool,
    /// RST flag - connection reset
    pub rst: bool,
    /// PSH flag - push data
    pub psh: bool,
}

impl TcpFlags {
    /// Create flags from raw TCP flags byte.
    pub fn from_raw(flags: u8) -> Self {
        Self {
            fin: flags & 0x01 != 0,
            syn: flags & 0x02 != 0,
            rst: flags & 0x04 != 0,
            psh: flags & 0x08 != 0,
            ack: flags & 0x10 != 0,
        }
    }

    /// Check if this is a connection start (SYN without ACK).
    pub fn is_syn_only(&self) -> bool {
        self.syn && !self.ack
    }

    /// Check if this is a SYN-ACK (connection acknowledgment).
    pub fn is_syn_ack(&self) -> bool {
        self.syn && self.ack
    }

    /// Check if this packet terminates the connection.
    pub fn is_terminating(&self) -> bool {
        self.fin || self.rst
    }
}

/// Parse a raw captured packet into a [`TcpSegment`], or `None` if it is not a
/// valid IPv4 TCP packet (UDP, IPv6, or malformed).
pub fn parse_tcp_segment(packet: &RawPacket) -> Option<TcpSegment> {
    // Parse the packet using etherparse
    let sliced = SlicedPacket::from_ethernet(&packet.data).ok()?;

    // Extract IPv4 header
    let (src_ip, dst_ip) = match sliced.net {
        Some(NetSlice::Ipv4(ipv4)) => {
            let header = ipv4.header();
            // B2 diagnostic (temporary): record whether IP fragmentation is
            // actually occurring on this link. The `tcp port 2050` BPF filter only
            // delivers FIRST fragments (trailing fragments carry no TCP ports), so
            // this catches the More-Fragments flag on the first fragment. If these
            // logs never appear, no userspace defragmenter is needed.
            record_fragmentation(&header);
            (
                Ipv4Addr::from(header.source()),
                Ipv4Addr::from(header.destination()),
            )
        }
        _ => return None, // Not IPv4
    };

    // Extract TCP header and payload
    let (src_port, dst_port, sequence, acknowledgment, flags, payload) = match sliced.transport {
        Some(TransportSlice::Tcp(tcp)) => {
            let flags = TcpFlags {
                syn: tcp.syn(),
                ack: tcp.ack(),
                fin: tcp.fin(),
                rst: tcp.rst(),
                psh: tcp.psh(),
            };
            // Get payload from the TCP slice
            let payload_slice = tcp.payload();
            (
                tcp.source_port(),
                tcp.destination_port(),
                tcp.sequence_number(),
                tcp.acknowledgment_number(),
                flags,
                payload_slice.to_vec(),
            )
        }
        _ => return None, // Not TCP
    };

    Some(TcpSegment {
        src_ip,
        dst_ip,
        src_port,
        dst_port,
        sequence,
        acknowledgment,
        flags,
        payload,
        timestamp: packet.timestamp,
    })
}

/// B2 diagnostic (temporary): count and log IP-fragmented datagrams.
///
/// A fragmented datagram's first fragment has the More-Fragments flag set (or a
/// non-zero fragment offset). Logging is throttled so a noisy link can't flood
/// the log. If this never fires in practice, the planned userspace defragmenter
/// is unnecessary and the BPF filter can stay as-is.
fn record_fragmentation(header: &etherparse::Ipv4HeaderSlice) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static FRAG_COUNT: AtomicU64 = AtomicU64::new(0);

    if header.is_fragmenting_payload() {
        let n = FRAG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if n == 1 || n % 50 == 0 {
            tracing::warn!(
                "[frag-diag] observed IP-fragmented datagram #{} (more_fragments={}, \
                 frag_offset={}); a userspace defragmenter would be needed if this is frequent",
                n,
                header.more_fragments(),
                header.fragments_offset().value()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tcp_flags_from_raw() {
        // SYN only
        let flags = TcpFlags::from_raw(0x02);
        assert!(flags.syn);
        assert!(!flags.ack);
        assert!(flags.is_syn_only());

        // SYN-ACK
        let flags = TcpFlags::from_raw(0x12);
        assert!(flags.syn);
        assert!(flags.ack);
        assert!(flags.is_syn_ack());

        // FIN-ACK
        let flags = TcpFlags::from_raw(0x11);
        assert!(flags.fin);
        assert!(flags.ack);
        assert!(flags.is_terminating());

        // RST
        let flags = TcpFlags::from_raw(0x04);
        assert!(flags.rst);
        assert!(flags.is_terminating());
    }
}
