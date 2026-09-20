//! RC4 cipher implementation for RotMG packet decryption.
//!
//! RotMG uses RC4 encryption with known hardcoded keys for
//! client-to-server and server-to-client traffic.

mod cipher;
mod keys;
mod tick_aligner;

pub use cipher::RC4Cipher;
pub use keys::{RotmgKeys, INCOMING_KEY, OUTGOING_KEY};
pub use tick_aligner::TickAligner;
