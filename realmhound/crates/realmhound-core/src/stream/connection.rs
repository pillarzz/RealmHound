//! TCP connection state tracking.

use std::net::Ipv4Addr;

/// Unique identifier for a TCP connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionKey {
    /// Client IP address
    pub client_ip: Ipv4Addr,
    /// Client port
    pub client_port: u16,
    /// Server IP address
    pub server_ip: Ipv4Addr,
    /// Server port
    pub server_port: u16,
}

impl ConnectionKey {
    /// Create a new connection key.
    pub fn new(
        client_ip: Ipv4Addr,
        client_port: u16,
        server_ip: Ipv4Addr,
        server_port: u16,
    ) -> Self {
        Self {
            client_ip,
            client_port,
            server_ip,
            server_port,
        }
    }

    /// Create a connection key from packet addresses, determining direction.
    /// Returns the key and `is_incoming` (true when the packet is
    /// server-to-client). `rotmg_port` is the server port (typically 2050).
    pub fn from_packet(
        src_ip: Ipv4Addr,
        src_port: u16,
        dst_ip: Ipv4Addr,
        dst_port: u16,
        rotmg_port: u16,
    ) -> (Self, bool) {
        // If source port is the RotMG port, this is an incoming packet
        let is_incoming = src_port == rotmg_port;

        let key = if is_incoming {
            Self {
                client_ip: dst_ip,
                client_port: dst_port,
                server_ip: src_ip,
                server_port: src_port,
            }
        } else {
            Self {
                client_ip: src_ip,
                client_port: src_port,
                server_ip: dst_ip,
                server_port: dst_port,
            }
        };

        (key, is_incoming)
    }
}

impl std::fmt::Display for ConnectionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{} <-> {}:{}",
            self.client_ip, self.client_port, self.server_ip, self.server_port
        )
    }
}

/// State of a TCP connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Connection is being established
    Connecting,
    /// Connection is established and active
    Established,
    /// Connection is being closed
    Closing,
    /// Connection is closed
    Closed,
}

/// Represents a single TCP connection for RotMG traffic.
#[derive(Debug)]
pub struct Connection {
    /// Connection identifier
    pub key: ConnectionKey,
    /// Current connection state
    pub state: ConnectionState,
    /// Buffer for incoming data (server → client)
    pub incoming_buffer: Vec<u8>,
    /// Buffer for outgoing data (client → server)
    pub outgoing_buffer: Vec<u8>,
    /// Expected incoming sequence number
    pub incoming_seq: u32,
    /// Expected outgoing sequence number
    pub outgoing_seq: u32,
}

impl Connection {
    /// Create a new connection.
    pub fn new(key: ConnectionKey) -> Self {
        Self {
            key,
            state: ConnectionState::Connecting,
            incoming_buffer: Vec::with_capacity(64 * 1024), // 64KB initial
            outgoing_buffer: Vec::with_capacity(64 * 1024),
            incoming_seq: 0,
            outgoing_seq: 0,
        }
    }

    /// Mark the connection as established with initial sequence numbers.
    pub fn establish(&mut self, client_seq: u32, server_seq: u32) {
        self.state = ConnectionState::Established;
        self.outgoing_seq = client_seq;
        self.incoming_seq = server_seq;
    }

    /// Check if the connection is active.
    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            ConnectionState::Connecting | ConnectionState::Established
        )
    }

    /// Close the connection.
    pub fn close(&mut self) {
        self.state = ConnectionState::Closed;
    }
}
