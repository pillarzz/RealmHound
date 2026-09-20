//! TCP stream reassembly for RotMG packet reconstruction.
//!
//! This module handles reassembling TCP segments into complete
//! RotMG packets, managing multiple concurrent connections.

mod connection;
mod metrics;
mod parser;
mod reassembler;

pub use connection::{Connection, ConnectionKey};
pub use metrics::{CaptureHealth, CaptureMetrics};
pub use parser::{parse_tcp_segment, TcpFlags, TcpSegment};
pub use reassembler::TcpReassembler;
