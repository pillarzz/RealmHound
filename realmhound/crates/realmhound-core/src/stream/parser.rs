//! TCP/IP packet parsing using etherparse.
//!
//! This module extracts TCP segment information from raw captured packets.

use crate::capture::{PacketFormat, RawPacket};
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
    let sliced = match packet.packet_format {
        PacketFormat::Ethernet => SlicedPacket::from_ethernet(&packet.data).ok()?,
        PacketFormat::RawIp => SlicedPacket::from_ip(&packet.data).ok()?,
        PacketFormat::Loopback | PacketFormat::LoopbackNetwork => {
            let family: [u8; 4] = packet.data.get(..4)?.try_into().ok()?;
            let ipv4 = u32::from_be_bytes(family) == 2
                || (packet.packet_format == PacketFormat::Loopback
                    && u32::from_le_bytes(family) == 2);
            if !ipv4 {
                return None;
            }
            SlicedPacket::from_ip(&packet.data[4..]).ok()?
        }
        PacketFormat::LinuxSll => {
            // SLL v1: protocol (EtherType) is at bytes 14..16.
            if packet.data.get(14..16)? != [0x08, 0x00] {
                return None;
            }
            SlicedPacket::from_ip(packet.data.get(16..)?).ok()?
        }
        PacketFormat::LinuxSll2 => {
            // SLL v2: protocol is at bytes 0..2, header is 20 bytes.
            if packet.data.get(0..2)? != [0x08, 0x00] {
                return None;
            }
            SlicedPacket::from_ip(packet.data.get(20..)?).ok()?
        }
    };

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
    use etherparse::PacketBuilder;

    fn ipv4_tcp() -> Vec<u8> {
        let mut data = Vec::new();
        PacketBuilder::ipv4([192, 0, 2, 1], [198, 51, 100, 2], 64)
            .tcp(45678, 2050, 123, 32768)
            .syn()
            .ack(456)
            .psh()
            .write(&mut data, &[1, 2, 3, 4])
            .unwrap();
        data
    }

    fn framed(ip: &[u8], packet_format: PacketFormat) -> RawPacket {
        let mut data = match packet_format {
            PacketFormat::Ethernet => {
                let mut header = vec![0; 12];
                header.extend_from_slice(&[0x08, 0x00]);
                header
            }
            PacketFormat::RawIp => Vec::new(),
            PacketFormat::Loopback => 2u32.to_ne_bytes().to_vec(),
            PacketFormat::LoopbackNetwork => 2u32.to_be_bytes().to_vec(),
            PacketFormat::LinuxSll => {
                let mut header = vec![0; 16];
                header[14..16].copy_from_slice(&[0x08, 0x00]);
                header
            }
            PacketFormat::LinuxSll2 => {
                let mut header = vec![0; 20];
                header[0..2].copy_from_slice(&[0x08, 0x00]);
                header
            }
        };
        data.extend_from_slice(ip);
        RawPacket {
            data,
            packet_format,
            timestamp: chrono::DateTime::from_timestamp_nanos(1_700_000_000_123_456_789),
            #[cfg(feature = "latency-diagnostics")]
            enqueued_at: None,
        }
    }

    fn assert_tcp_packet(packet: &RawPacket) {
        let segment = parse_tcp_segment(packet).expect("valid IPv4 TCP packet");
        assert_eq!(segment.src_ip, Ipv4Addr::new(192, 0, 2, 1));
        assert_eq!(segment.dst_ip, Ipv4Addr::new(198, 51, 100, 2));
        assert_eq!(segment.src_port, 45678);
        assert_eq!(segment.dst_port, 2050);
        assert_eq!(segment.sequence, 123);
        assert_eq!(segment.acknowledgment, 456);
        assert!(segment.flags.syn);
        assert!(segment.flags.ack);
        assert!(segment.flags.psh);
        assert!(!segment.flags.fin);
        assert!(!segment.flags.rst);
        assert_eq!(segment.payload, [1, 2, 3, 4]);
        assert_eq!(segment.timestamp, packet.timestamp);
    }

    #[test]
    fn parses_ethernet_tcp() {
        assert_tcp_packet(&framed(&ipv4_tcp(), PacketFormat::Ethernet));
    }

    #[test]
    fn parses_raw_ip_tcp() {
        assert_tcp_packet(&framed(&ipv4_tcp(), PacketFormat::RawIp));
    }

    #[test]
    fn parses_linux_cooked_tcp() {
        assert_tcp_packet(&framed(&ipv4_tcp(), PacketFormat::LinuxSll));
        assert_tcp_packet(&framed(&ipv4_tcp(), PacketFormat::LinuxSll2));
    }

    #[test]
    fn parses_native_loopback_tcp() {
        assert_tcp_packet(&framed(&ipv4_tcp(), PacketFormat::Loopback));
    }

    #[test]
    fn parses_loopback_recorded_in_either_byte_order() {
        for family in [2u32.to_le_bytes(), 2u32.to_be_bytes()] {
            let mut packet = framed(&ipv4_tcp(), PacketFormat::Loopback);
            packet.data[..4].copy_from_slice(&family);
            assert_tcp_packet(&packet);
        }
    }

    #[test]
    fn parses_network_loopback_tcp() {
        assert_tcp_packet(&framed(&ipv4_tcp(), PacketFormat::LoopbackNetwork));
    }

    #[test]
    fn parses_vlan_tagged_ethernet_tcp() {
        let mut packet = framed(&ipv4_tcp(), PacketFormat::Ethernet);
        packet
            .data
            .splice(12..14, [0x81, 0x00, 0x00, 0x01, 0x08, 0x00]);
        assert_tcp_packet(&packet);
    }

    #[test]
    fn rejects_non_ipv4_loopback_families() {
        for packet_format in [PacketFormat::Loopback, PacketFormat::LoopbackNetwork] {
            for family in [0u32, 24, 28, 30] {
                let mut packet = framed(&ipv4_tcp(), packet_format);
                packet.data[..4].copy_from_slice(&family.to_be_bytes());
                assert!(parse_tcp_segment(&packet).is_none());
            }
        }
    }

    #[test]
    fn rejects_little_endian_network_loopback_header() {
        let mut packet = framed(&ipv4_tcp(), PacketFormat::LoopbackNetwork);
        packet.data[..4].copy_from_slice(&2u32.to_le_bytes());
        assert!(parse_tcp_segment(&packet).is_none());
    }

    #[test]
    fn rejects_udp_and_ipv6() {
        let mut udp = Vec::new();
        PacketBuilder::ipv4([192, 0, 2, 1], [198, 51, 100, 2], 64)
            .udp(45678, 2050)
            .write(&mut udp, &[1, 2, 3, 4])
            .unwrap();
        let mut ipv6 = Vec::new();
        PacketBuilder::ipv6([0; 16], [1; 16], 64)
            .tcp(45678, 2050, 123, 32768)
            .write(&mut ipv6, &[1, 2, 3, 4])
            .unwrap();

        for packet_format in [
            PacketFormat::Ethernet,
            PacketFormat::RawIp,
            PacketFormat::Loopback,
            PacketFormat::LoopbackNetwork,
        ] {
            assert!(parse_tcp_segment(&framed(&udp, packet_format)).is_none());
            assert!(parse_tcp_segment(&framed(&ipv6, packet_format)).is_none());
        }
    }

    #[test]
    fn rejects_truncated_frames() {
        for packet_format in [
            PacketFormat::Ethernet,
            PacketFormat::RawIp,
            PacketFormat::Loopback,
            PacketFormat::LoopbackNetwork,
        ] {
            let original = framed(&ipv4_tcp(), packet_format);
            for length in 0..original.data.len() {
                let mut packet = original.clone();
                packet.data.truncate(length);
                assert!(
                    parse_tcp_segment(&packet).is_none(),
                    "{packet_format:?} truncated at byte {length}"
                );
            }
        }
    }

    #[test]
    fn does_not_guess_packet_format() {
        let mut raw_ip = framed(&ipv4_tcp(), PacketFormat::RawIp);
        raw_ip.packet_format = PacketFormat::Ethernet;
        assert!(parse_tcp_segment(&raw_ip).is_none());

        let mut ethernet = framed(&ipv4_tcp(), PacketFormat::Ethernet);
        ethernet.packet_format = PacketFormat::RawIp;
        assert!(parse_tcp_segment(&ethernet).is_none());
    }

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
